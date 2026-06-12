# Zapcode improvement axes — standing direction

> The four axes every improvement cycle pushes on, with current state, the
> bar, and the prioritized backlog per axis. Maintained by the improvement
> loop; update "current state" numbers when they move.
>
> Global policy: **memory ranks above small speed wins** (snapshots are the
> persisted artifact; many VMs run concurrently). Every perf change states
> its memory delta. Correctness beats both: the differential gate
> (`packages/zapcode-ai/tests/differential.mjs`) and the 104-binary suite
> must stay green, and any serialized-layout change bumps the wire version.

## 1. Speed

**Current state** (apple silicon, release): `ZapcodeProgram` prepare-once API ships (dump/load with wire framing; realistic agent program 30→22 µs/run, ~28% saved). Simple expression ~4–5 µs
end-to-end (parse 0.3 µs, compile 0.15 µs, VM construct+dispatch ~4 µs);
loop iterations ~50 µs/100; calls ~0.8 µs each. Dispatch profile is flat —
per-instruction clock reads amortized, `Instruction` is 40 bytes.

**Direction**: stop micro-optimizing dispatch (flat profile, targets met
with headroom). The remaining structural wins are *avoiding repeated work*:

- **Compiled-program caching** (monty's `dump()`/`load()` of parsed code):
  `ZapcodeRun` re-parses and re-compiles the same agent code on every run.
  Add a `CompiledProgram` dump/load (bytecode + version guard) and a
  prepare-once/run-many API through the bindings. Also kills the remaining
  parse cost in multi-run hosts (sessions re-compile every chunk).
- Template-clone cost is the residual first-run floor (~3 µs of clone);
  only worth touching with a copy-on-write heap design — do not take this
  on for less than a 2× win.
- Threaded/computed-goto dispatch: last resort, big unsafe surface. Not
  unless a workload demands it.

## 2. Memory

**Current state**: typical agent snapshot 472 B (template-elided,
mark-compacted); per-hop snapshot growth is live-state-only; `Instruction`
40 B; builtin template shared per-process via `OnceLock`. Measured (cycle
1): parked-as-bytes ~2 KiB RSS/VM vs ~63 KiB live (park as bytes!); object
keys have ZERO sharing today (3,069 occurrences → 3,069 Arc allocations) —
interning is a GO; Arc-sharing `CompiledProgram` is the next density lead.

**Direction**: the arena still never frees *during* a run — compaction
happens only at snapshot capture. In-run pressure is bounded by
`memory_limit_bytes`, but long synchronous loops over big temporaries peak
high.

- **In-run heap reuse**: a free-list or periodic in-place compaction at
  safe points (e.g. piggyback on the amortized 1024-instruction tick when
  allocation pressure is high). Must preserve handle stability — likely a
  remap at loop-safe points only; design doc first.
- **Key interning**: object keys are `Arc<str>` per-insert; a per-VM (or
  template-level) intern pool for hot keys (`status`, `value`, `id`, …)
  would dedupe thousands of tiny allocations. Measure first.
- **Many-VM density benchmark**: add a bench that holds N=1000 suspended
  VMs and reports RSS/VM — this is the real production metric; we do not
  currently measure it.

## 3. Conformance

**Current state**: differential gate = 334 snippets + a seeded fuzzer
(`test:fuzz`, auto-minimizing; its first run found a host-aborting slice
panic and three negative-zero bugs) + an evaluation-count suite (the RMW
double-eval class). Previously: 330 snippets (whole realistic
programs + a by-name stdlib sweep), 1 deliberate pin (`Symbol.toPrimitive`).
Known residuals: tagged-template `strings.raw` companion,
deeper-than-top-level param field defaults, `Promise.race([])` hang-vs-pend,
generator-body await tick interleaving, eager spread over generators.

**Direction**: the per-method sweep is done; the next class of bugs hides in
*combinations*.

- **Property-based differential fuzzing**: a small generator that composes
  random programs from the known-supported grammar (expressions, control
  flow, async, collections) and diff-tests them against Node. Run N=500 per
  cycle; minimize failures into corpus snippets.
- Keep ground-truthing every new builtin against Node before merging
  (the round-13/14/15 discipline).
- Residuals stay deprioritized unless agent code hits them; `strings.raw`
  is the most likely to surface (custom tag functions).

## 4. Ergonomics (the agent-DSL surface) — monty-informed

What `pydantic/monty` ships that zapcode should match or beat
(`zapcode-ai` already has: tool declaration → `declare function` signatures
in the system prompt, named-args validation, suspend/resume bridging,
session chunks):

- **Typecheck agent code before running it** (monty bundles Astral's `ty`).
  Zapcode runs *TypeScript* — generate a `.d.ts` from the registered tools
  (we already generate the textual signatures) and typecheck the agent's
  code against it host-side in `zapcode-ai` (a `typescript` peer-dep hook
  exists already). A type error returned to the model **before** execution
  is the cheapest self-correction signal there is.
- **Error fidelity for self-correction**: runtime errors should carry
  line/column into the agent's code and a short code frame. Spans exist in
  the IR; thread them through `ZapcodeError` → bindings → the tool-result
  the model sees.
- **Prepare-once API**: expose compiled-program caching (axis 1) through
  the bindings: `prepare(code) -> Program`, `program.run(inputs, tools)`.
- **stdout/stderr split**: `console.error`/`console.warn` currently mix
  into stdout; agents (and hosts) want them separated.
- **Tool-call introspection/trace**: a structured per-run trace (which
  tools, what args, how long suspended) the host can log or feed back.
- Watch monty's roadmap: dataclass-style typed returns, pydantic-ai
  "code mode" toolset packaging — the `zapcode({tools})` wrapper is the
  equivalent; keep its system-prompt contract tight as tools grow.

## Loop protocol

Each improvement cycle: (1) re-assess all four axes against this doc,
(2) pick the highest-leverage non-conflicting item per axis, (3) implement
behind the full gate (suite + e2e + differential), (4) update this doc's
"current state" lines and re-prioritize. Small, verified, compounding.
