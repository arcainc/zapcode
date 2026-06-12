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
2): the per-VM compiled-program clone is gone — `programs` is now
`Vec<Arc<CompiledProgram>>`, so a prepare-once fleet shares ONE bytecode
allocation (~19 KiB/VM → ~0 KiB/VM in `profile_density`; wire unchanged).
Object keys are interned per-VM (3,069 → 96 distinct allocations for ~49
unique keys; pure runtime accelerator, off the wire). Remaining lead:
in-run arena reuse (still no free during a run).

**Direction**: in-run heap reuse is now DONE (see
`docs/in-run-memory-design.md`): the top-level dispatch loop mark-compacts
the live heap at safe instruction boundaries on a growth / byte-high-water
trigger, and resets `memory_bytes` to the surviving live estimate — so
`memory_limit_bytes` is a LIVE ceiling (24 MB churned through an 8 MiB limit
in `profile_inrun`), not cumulative-allocation. `max_allocations` (count)
stays the cumulative DoS guard. Validated by `gc_stress.rs` (force-compact
every instruction across diverse program shapes). No wire change. The
remaining memory ideas are lower-value:

- **Key interning** — already shipped (cycle 2).
- **Key interning**: object keys are `Arc<str>` per-insert; a per-VM (or
  template-level) intern pool for hot keys (`status`, `value`, `id`, …)
  would dedupe thousands of tiny allocations. Measure first.
- **Many-VM density benchmark**: add a bench that holds N=1000 suspended
  VMs and reports RSS/VM — this is the real production metric; we do not
  currently measure it.

## 3. Conformance

**Current state**: differential gate = 339 snippets + a seeded fuzzer
(`test:fuzz`, auto-minimizing; its first run found a host-aborting slice
panic and three negative-zero bugs) + an evaluation-count suite (RMW
double-eval) + a side-effect suite (`conformance_side_effects.rs`: order,
laziness, conversion/iterator/callback/getter counts — the classes
output-only tests cannot see). That suite's first run caught two real
divergences, now fixed: plain assignment to a member/index target
evaluated the VALUE before the object/key (`obj[f()] = g()` ran g before
f), and `for…of` over a custom `[Symbol.iterator]()` drained the iterator
eagerly (over-pulling past an early `break`; now a lazy `__custom__`
marker pulls one `next()` per iteration, calling `[Symbol.iterator]()`
exactly once per loop). One documented structural residual: array
DESTRUCTURING still drains eagerly (`const [a,b]=iter` pulls to done, not
k) — `iterator_destructure_pulls_exactly_k` is `#[ignore]`. Previously:
330 snippets (whole realistic programs + a by-name stdlib sweep), 1
deliberate pin (`Symbol.toPrimitive`).
Known residuals: tagged-template `strings.raw` companion,
deeper-than-top-level param field defaults, `Promise.race([])` hang-vs-pend,
generator-body await tick interleaving, eager spread over generators.

**Direction**: the per-method sweep is done; the next class of bugs hides in
*combinations*. The fuzzer now threads effect logs (order/laziness/counts
diffed against Node) and generates classes/Map/Set/switch/optional-chaining/
custom-iterables — 4,500 programs clean; Symbol.toPrimitive now dispatched, so the differential gate holds ZERO pins. Next: tagged-template custom tags
(probe the `strings.raw` residual), deeper combinatorial nesting, a nightly
multi-seed sweep.

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
- **stdout/stderr split** — DONE (cycle 2, wire v16): `console.error`/`warn`
  write to a separate `stderr` stream through core, snapshots, sessions, and
  the JS, Python, and WASM bindings.
- **Prepare-once API** — DONE (cycle 2): `ZapcodeProgramHandle` napi class
  exposes compile-once / run-many + dump/load through the binding (a
  zapcode-ai `prepare()` wrapper around it is the remaining follow-up).
- **Tool-call trace + run report** — DONE (cycle 3): `ExecutionResult.toolCalls`
  carry per-call timing; `ExecutionResult.report` is a structured receipt
  (completed / durationMs / toolCallCount / error{message,line,column}).
- **dryRun** — DONE (cycle 3): `dryRun(code, tools)` typechecks then
  smoke-executes against side-effect-free stub tools under a tight budget,
  reporting reached-completion / error-site / the tool-call sequence it would
  make. The "does agent code instantly error" pre-flight.
- **prepare()** — DONE (cycle 3.5): `prepare(code, tools) -> PreparedProgram`
  compiles once and `run()`s many times through the same execute loop (tool
  validation + trace + report), starting from the cycle-1 program cache
  instead of recompiling — the package-level prepare-once surface.
- **Forking** — DONE (cycle 3): `forkSnapshot(bytes)` names the proven
  snapshot-fork primitive (load the same suspension bytes N times → N
  independent, deterministic, program-sharing continuations) for agent
  checkpoint / rollback / speculative branching.
- **`console.assert`** — DONE (cycle 3): falsy condition writes
  "Assertion failed[: msg]" to stderr without throwing — in-sandbox
  self-verification, pairing with the stdout/stderr split.
- **Agent-DX e2e suite** — DONE (cycle 3): `tests/agent-dx.mjs` asserts the
  QUALITY of feedback (clear error locations, type errors caught pre-run, no
  silent hangs, partial output on failure, deterministic fork/rollback),
  wired into `test:e2e-full`.
- Watch monty's roadmap: dataclass-style typed returns, pydantic-ai
  "code mode" toolset packaging — the `zapcode({tools})` wrapper is the
  equivalent; keep its system-prompt contract tight as tools grow.

## Loop protocol

Each improvement cycle: (1) re-assess all four axes against this doc,
(2) pick the highest-leverage non-conflicting item per axis, (3) implement
behind the full gate (suite + e2e + differential), (4) update this doc's
"current state" lines and re-prioritize. Small, verified, compounding.
