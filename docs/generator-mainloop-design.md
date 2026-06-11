# Design: Generators in the Main Loop

Status: **in progress** — Stage 0 implemented on `generator-mainloop-stage0`
(plus the latent yield-in-try fix in BOTH drivers, and tool-suspension
inside `.next()`-pulled bodies arriving early). Stages 1–4 remain.
Author: conformance hardening effort
Scope: drive generator bodies with the main `execute()` loop instead of the
nested drive loop, so tool calls inside generators suspend durably and
async-generator `await`s gain Node tick semantics — without breaking the
snapshot/suspend/resume core.

---

## 1. Problem

Generator bodies run in a **nested drive loop**
(`run_generator_until_yield_or_return`), separate from the main `execute()`
loop. Concretely:

| Behavior | zapcode | Node |
|---|---|---|
| `await tool()` inside a generator body | error: "cannot suspend inside a generator" | works |
| `await p.then(handler-with-tool)` inside a generator | same error | works |
| `await` inside an `async function*` body | runs inline (no tick) | parks the generator |
| `for (const x of infiniteGen()) { break }` | hangs/limits (eager full drain) | lazy pull, breaks |
| `gen.next()` on an async generator | `{value, done}` directly | a *Promise* of `{value, done}` |

Root cause: the nested loop must return a `{value, done}` **synchronously**,
so nothing inside a generator body may suspend the VM (tool calls error) or
yield to the microtask queue (`await` ticks are impossible). The nested loop
also re-implements main-loop responsibilities (helper-frame returns,
ip-overflow, continuation settling) and has historically drifted — the
`SuspendedFrame.boxed` local-corruption bug and the missed
`process_continuation` call both lived here.

## 2. What already exists (and helps)

- **`SuspendedFrame { ip, locals, stack, boxed }`** — serialized detach/resume
  of a single generator frame. This is the very pattern Stage 3 of the
  microtask plan generalized into `AsyncTask`; the direction of reuse now
  reverses.
- **`AsyncTask` (microtask Stage 3)** — proven, durable frame detachment in
  the main loop, *including try-frame migration with depth rebasing*, which
  `SuspendedFrame` does not do today (a `yield` inside `try` leaves the
  try-frame behind — a latent bug family this redesign retires).
- **The continuation driver** (`MicrotaskReaction`, `PromiseExecutor`): "push
  a frame, let the main loop drive it, shape the result when it pops" is now
  a routine, serialized pattern with throw-boundary handling.
- **The microtask queue** for async-generator `await` ticks (`Microtask.task`
  / `ResumeAsync` for parked bodies).

## 3. Target model

### 3.1 `.next(arg)` as a main-loop continuation

`gen.next(arg)` pushes the generator's frame (fresh, or restored from
`SuspendedFrame` — now carrying migrated try-frames) onto the MAIN frame
stack plus a `Continuation::GeneratorNext { gen_id, caller_frame_depth,
callback_frame_index }`.

- **`Yield`** detaches the frame back into the generator object (frame +
  stack slice + try-frames, the `detach_async_task` recipe) and pushes
  `{value, done: false}`; the continuation passes it through to the `.next()`
  call site.
- **`Return` / fall-off-end** runs `finish_generator` and pushes
  `{value, done: true}`.
- **A throw escaping the body** marks the generator done and rethrows at the
  `.next()` site (Node).
- **A tool call inside the body** suspends the whole VM with the generator's
  live frames in the snapshot — durability falls out of the main loop, no
  special machinery.

### 3.2 Lazy iteration

`for…of`, `for await…of`, `yield*`, and spread over generators currently
**drain eagerly** via `drain_generator` (a Vec-collecting Rust loop — wrong
for infinite generators with `break`, and another nested driver). They become
per-pull: each iteration runs one `.next()` through the main loop (compiler
lowering already iterates element-wise for `for…of`; the drain call sites move
to a `GeneratorDrain` continuation that collects pulls for spread).

### 3.3 Async generators

`asyncGen.next()` returns a **promise** of `{value, done}` (today: the raw
object). The body may `await`: it parks exactly like an async function
(AsyncTask-style detach keyed by the generator), and the pending
`next()`-promise settles when the body reaches the next `yield`/return.
`for await` already awaits the `next()` result, so it adopts the promise with
no changes. Reuses `Microtask.task` resumption wholesale.

### 3.4 Serialization

Generator state mid-body is either (a) live frames in the snapshot during a
whole-VM suspension — already serialized, or (b) a detached
`SuspendedFrame`+try-frames between pulls — extending the existing
`SuspendedFrame` (wire bump). No new categories of state.

## 4. Staged rollout

Each stage green-gated on `cargo test -p zapcode-core` + `npm run
test:e2e-full`.

1. **Stage 0 — `.next()` through the main loop, behavior-identical.**
   `GeneratorNext` continuation + Yield-as-detach + try-frame migration.
   Delete the nested drive loop. Pure re-plumbing; the suite must not move.
   **✅ Implemented** (`generator-mainloop-stage0`): `gen.next(arg)` pushes
   the body frame plus `Continuation::GeneratorNext` (the in-flight
   generator object rides on the continuation); the main loop's `Yield` arm
   detaches frame + stack + try-frames (stashed in a new
   `Vm.generator_try_frames` map keyed by generator id — `TryInfo` stays
   out of `value.rs`); return/fall-off answers `{value, done: true}` via
   `process_continuation`; a throw escaping the body propagates to the
   `.next()` caller and marks the generator done; re-entrant pulls raise
   Node's "Generator is already running" TypeError. The try-frame
   stash/restore also went into the legacy nested driver, fixing the latent
   yield-in-try leak for `for…of`/spread paths too. The nested driver's
   Yield interception is depth-gated so a main-loop pull of another
   generator inside a nested-driven body coexists. Suite unchanged (99
   binaries green); wire v9 → v10. EARLY Stage-2 capability: tool calls and
   pending-chain awaits inside `.next()`-pulled bodies suspend/drain
   durably (`tests/generator_mainloop.rs`, incl. dump/load mid-body). The
   nested driver still serves `for…of`/spread/`yield*` until Stage 1.
2. **Stage 1 — lazy `for…of`/`yield*`/spread.** Retire `drain_generator`;
   infinite generators with `break` start working.
   **✅ Implemented** (`generator-mainloop-stage1`): `IteratorNext`'s
   generator pull goes through `start_generator_pull` with a `for_of` shape
   on `GeneratorNext` (answers the protocol pair `[triple, value]` instead
   of an iterator-result object) — covering `for…of`, `for await`, and
   `yield*` (which compiles to an IteratorNext loop) in one move. Tool
   calls inside loop-driven generator bodies now suspend durably (tested
   with dump/load hops at every suspension; the old pinned
   "cannot suspend inside a generator" stress test is rewritten as a
   capability test). Re-entrant loop pulls raise the running-generator
   TypeError. Wire v10 → v11. NOTE: `for…of` was already *lazy* via
   IteratorNext (the design doc's eager-drain concern applies only to
   spread / `Array.from` / array destructuring, which consume the whole
   sequence by definition and keep the nested driver — tool calls there
   remain unsupported; documented).
3. **Stage 2 — tool calls inside generator bodies.** Lift the
   "cannot suspend inside a generator" error; durable tests (dump/load with
   a generator frame live mid-body, and detached between pulls).
   **✅ Arrived with Stages 0–1** for every `.next()`/loop-driven pull (the
   capability is inherent to main-loop frames); covered by
   `generator_mainloop.rs` and `generator_lazy_pulls.rs`. The remaining
   carve-out is the eager spread/destructure path.
4. **Stage 3 — async generators.** `next()` returns a promise; body `await`s
   park with full tick order.
5. **Stage 4 — durable hardening.** Replay-determinism sweep (the
   `durable_async_state.rs` harness) over generator-heavy programs.

## 5. Risks & mitigations

- **R1 — `.next()` call sites that expect a synchronous value** (drain,
  spread, `yield*`, Map/Set constructors over generators). *Mitigation:*
  Stage 0 converts only the `.next()` method path; remaining sync consumers
  keep a bounded fallback until Stage 1 converts them.
- **R2 — yield-in-try.** Migrating try-frames changes behavior where the old
  code silently leaked them. Expect to FIX latent bugs, not preserve them;
  ground-truth each against Node (the R5 pattern).
- **R3 — generator methods `.return(v)` / `.throw(e)`** interact with the
  detach/restore cycle (running `finally` blocks in a detached frame requires
  a restore-then-unwind pass). Model on `resume_async_task`'s rejection path.
- **R4 — re-entrancy**: `gen.next()` called from inside the same generator's
  body must error (Node: TypeError "already running"). Track a running flag
  on the generator object.
- **R5 — wire format**: `SuspendedFrame` gains try-frames; `Continuation`
  gains variants. One version bump at Stage 0.

## 6. Estimate

- Stage 0: large-ish but mechanical (the AsyncTask recipe, applied).
- Stage 1: medium (compiler + call-site sweep).
- Stage 2: small (mostly tests — the capability falls out of Stage 0).
- Stage 3: medium (promise-wrapped next + parked bodies).
- Stage 4: small (reuse the existing harness).
