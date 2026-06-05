# Design: Microtask / Event-Loop Ordering

Status: **proposal** (no core code changed yet)
Author: conformance hardening effort
Scope: make Promise/`async`/`await` ordering match Node, without breaking the
durable-execution (snapshot/suspend/resume) core.

---

## 1. Problem

zapcode runs async code **eagerly and synchronously**. There is no event loop
and no microtask queue. Concretely (all measured vs Node):

| Snippet | zapcode | Node |
|---|---|---|
| `Promise.resolve().then(()=>'A'); 'B'` order | `A,B` | `B,A` |
| `async f(){…await…}` interleaving with sync code | runs inline | yields |
| `async function f(){return 5}; f().then(cb)` | **throws** (`5` has no `.then`) | `5` |
| nested `.then` scheduling | depth-first inline | breadth-first by tick |

Root causes:

1. **`.then`/`.catch`/`.finally` run their callback immediately**
   (`start_promise_callback` → a `PromiseCallback` continuation that the main
   loop drives *now*), instead of enqueuing a microtask.
2. **`await` continues inline.** The `Await` instruction unwraps a settled
   promise and falls straight through to the next instruction; it never yields
   so that pending synchronous code can run first.
3. **`async` functions return their unwrapped value, not a Promise.** Because
   `await` is synchronous, an `async` function just runs to completion and
   returns the bare value. So `.then` on the result fails.

The only place zapcode "yields" today is a **host/external call**
(`CallExternalDeferred` / `Await` on a `pending_call` promise), which suspends
the *entire VM* and returns `VmState::Suspended` to the host. That is a
coarse-grained, whole-program suspension — not the fine-grained,
within-one-`execute()`-call interleaving that microtasks require.

---

## 2. What already exists (and helps)

The codebase already has most of the machinery a microtask model needs — it is
just wired for eager execution:

- **`continuations: Vec<Continuation>`** (serialized): drives `.then` callbacks
  and async array callbacks one frame at a time, via `process_continuation()`
  called from the `execute()` loop whenever a frame pops. A `PromiseCallback`
  continuation already knows how to shape a callback's return value into the
  chain's result and can survive a mid-callback suspension.
- **`Continuation::PromiseCallback { mode, original_promise, caller_frame_depth,
  callback_frame_index }`** and **`PromiseCallbackMode { WrapResult, PassThrough }`**
  — exactly the "reaction" payload a microtask needs.
- **`resume_action: Option<ResumeAction>`** (serialized): already threads a
  promise-method/await across a host suspension and resume.
- **Promise objects** are heap objects `{__promise__: true, status, value|reason}`.
- **`CompiledFunction.is_async`** flags async functions.
- The snapshot (`VmSnapshot`) already serializes `continuations`,
  `pending_calls`, `resolved`, `pending_batch`, `resume_action`, the full
  `stack`/`frames`/`heap`/`cells`. Adding a microtask queue + pending-reaction
  lists is an incremental extension of an already-rich serialized async state.

**Implication:** we are not inventing suspension from scratch. We are changing
*when* reactions run (enqueue vs inline) and adding a *drain* step, reusing the
existing continuation driver to actually run each reaction.

---

## 3. Target model

### 3.1 Promise representation

Extend the promise object to a proper state machine with reaction lists:

```
{
  __promise__: true,
  status: "pending" | "fulfilled" | "rejected",
  value | reason,                 // set when settled
  fulfill_reactions: [Reaction],  // run when fulfilled
  reject_reactions:  [Reaction],  // run when rejected
}
```

A **Reaction** is `{ handler: Value|undefined, result_promise: Handle, mode }`
— i.e. the existing `PromiseCallback` payload plus the dependent promise to
settle. Reactions live on the promise object in the heap, so they serialize for
free with the heap.

`status: "pending"` is new for *guest-created* promises (`new Promise(executor)`,
an `async` function that has awaited but not finished). Today only host-call
promises are effectively pending.

### 3.2 Microtask queue

Add to `Vm` (and `VmSnapshot`):

```
microtasks: VecDeque<Microtask>,
```

where `Microtask` is a serializable job. Two job kinds:

- **`RunReaction { reaction, settle_value, is_rejection }`** — run a `.then`
  reaction handler with the settled value, then settle its `result_promise`.
- **`ResumeAsync { task: AsyncTaskId }`** — resume a suspended `async` function
  body whose `await`ed promise just settled.

`VecDeque` (FIFO) gives spec microtask order. It must serialize deterministically
(use a `Vec` on the wire, like `resolved`'s `BTreeMap` pattern).

### 3.3 `.then` / `.catch` / `.finally`

Change from *run-now* to *enqueue*:

- Create the dependent `result_promise` (pending).
- If the receiver is already settled → push a `RunReaction` microtask now.
- If the receiver is pending → append the reaction to the receiver's
  `fulfill_reactions`/`reject_reactions`; it is enqueued when the receiver
  settles (see `resolve_promise`).
- Return `result_promise` synchronously.

`finally` keeps `PassThrough` mode (already implemented).

### 3.4 `async` functions return a Promise; `await` suspends the task

This is the heart of the change.

- Calling an `async` function creates a **pending result promise** and an
  **AsyncTask** record, runs the body until the first `await` (or completion).
- **`await p`**:
  - If `p` is not a thenable → still schedule a microtask to resume *after* the
    current synchronous run (await always yields at least one tick, even on a
    non-promise).
  - If `p` is a promise → register a reaction on `p` that, when `p` settles,
    enqueues `ResumeAsync` for this task with the value (or throws the reason
    into the task).
  - Either way, the async function **suspends its frames** and control returns
    to the caller. When the body finally returns/throws, settle the task's
    result promise (which enqueues that promise's own reactions).

The mechanism for "suspend an async function mid-body and resume it later" is
the same shape as the existing generator suspend/resume and the
`PromiseCallback` continuation driver — an **AsyncTask** captures the
suspended frames + stack slice for that call, keyed by id, stored in the VM
(and snapshot). `ResumeAsync` restores them and continues at the instruction
after `Await`.

> Note: today `Await` lives in the same frame as surrounding code and just
> unwraps. The new `Await` must be able to *detach* the async call's frames into
> an `AsyncTask` and return control upward. This is the single most invasive
> part and the main risk (see §6).

### 3.5 The drain: where the loop changes

In `execute()`, the top-level completion branch (frames ≤ 1, ip overflow)
currently returns `VmState::Complete` immediately. Instead:

```
on top-level completion:
    loop:
        if a microtask is queued: pop it and run it (may push frames; may
            itself enqueue more microtasks; may hit a host call → return
            VmState::Suspended with the queue intact in the snapshot)
        else: return VmState::Complete(result)
```

The same drain runs **after each host-call resume** completes its synchronous
continuation, so microtasks scheduled before/around a tool call run in the right
order relative to the resumed code.

Because each microtask runs the existing instruction loop and the existing
`process_continuation()` driver, a tool call inside a `.then`/`await`
continuation suspends exactly as it does today — the queue is already in the
serialized state, so resume picks up mid-drain.

### 3.6 `resolve_promise(promise, value)`

Settling a pending promise (executor `resolve`, async body return, host-call
resume):

1. set `status`/`value`.
2. for each queued reaction: enqueue a `RunReaction` microtask.
3. clear the reaction lists.

Rejection symmetric. Thenable adoption (resolving with a promise) chains via a
reaction, reusing existing `make_resolved_promise` unwrap logic.

---

## 4. Interaction with durable execution (the critical part)

Everything new is **serializable and already-modeled in spirit**:

| New state | Serialized where | Notes |
|---|---|---|
| `microtasks` queue | `VmSnapshot` (new field, `#[serde(default)]`) | FIFO of `Microtask` (Values + ids) |
| pending-promise reaction lists | in the heap (on the promise object) | travels with `heap` already |
| `AsyncTask` records (suspended async frames) | `VmSnapshot` (new field) | mirrors how frames are already serialized |
| `next_async_task_id` | `VmSnapshot` (new u64) | like `next_call_id` |

Bump `wire::FORMAT_VERSION` 4 → 5; add `forge_frame` version in
`tests/wire_format.rs`; document in `wire.rs`.

**Host-call suspension still works** because: a host call inside a microtask
returns `VmState::Suspended` with the *entire* VM state — including the
remaining `microtasks` and any `AsyncTask`s — captured in the snapshot. Resume
restores them and the top-level drain continues. The existing `resume_action` /
`PromiseCallback` path is the template.

**Determinism for Temporal replay** is preserved: microtask order is FIFO and
fully determined by program order; no wall-clock or real timers are introduced
(there is no `setTimeout`/macrotask layer — only microtasks, which is all
Promise/await ordering needs).

---

## 5. Staged rollout

Each stage is independently testable and green-gated on the full
`cargo test -p zapcode-core` + `npm run test:e2e-full` (esp. the
durable-session suite).

1. **Stage 0 — Promise state machine + `resolve_promise` + reaction lists**, but
   keep eager drain (reactions still run inline). No behavior change; pure
   refactor that introduces the data model. Lowest risk; lands first.
2. **Stage 1 — Microtask queue + drain at top-level completion.** Switch
   `.then`/`.catch`/`.finally` from run-now to enqueue. Fixes `.then`
   ordering and nested-`.then` order. `await` still inline for now.
   - Test: `then_order`, `nested_then`, `finally_order`.
3. **Stage 2 — `async` returns a Promise.** Fixes `async function(){return 5}`
   `.then` throwing, and `f().then(...)`. `await` of that promise still settles
   synchronously within the body.
   - Test: `async_return`; ensure `const x = await f()` unchanged.
4. **Stage 3 — `await` suspends the task** (AsyncTask detach + `ResumeAsync`).
   Fixes interleaving (`await_order`, `two_awaits`, `microtask_vs_sync`). The
   invasive stage; do last, behind the now-proven queue/drain.
5. **Stage 4 — durable hardening.** Snapshot/resume tests with microtasks and
   suspended async tasks in flight across a host call; wire-version bump; fuzz
   the drain across suspensions.

If Stage 3 proves too risky, Stages 0–2 alone already remove the *throwing* bug
and fix `.then`/`.finally` ordering — a shippable partial win — with `await`
interleaving documented as a residual.

---

## 6. Risks & mitigations

- **R1 — `await` frame detachment (Stage 3).** Splitting an async call's frames
  into a resumable `AsyncTask` mid-`execute()` is the deepest change. *Mitigation:*
  model it on the existing generator suspend/resume (which already detaches and
  resumes a frame), and gate behind Stages 0–2.
- **R2 — Snapshot of in-flight async tasks.** A dump taken while an async task is
  parked on a pending promise must resume correctly. *Mitigation:* reuse the
  `PromiseCallback` continuation serialization pattern; add explicit
  dump/load/resume tests (Stage 4) before enabling Stage 3 by default.
- **R3 — Unhandled rejection semantics.** Today a rejected `await` throws a
  `RuntimeError` to the host. With queued reactions, an unhandled rejection must
  surface deterministically (e.g. at end-of-drain) rather than being lost.
  *Mitigation:* track promises with no reject reaction at settle time; report at
  drain end.
- **R4 — Re-entrancy / infinite microtask loops** (`function loop(){ Promise.resolve().then(loop) }`).
  *Mitigation:* count microtasks against the existing allocation/step budget so a
  runaway drain hits a resource limit instead of hanging.
- **R5 — Existing eager-order tests.** Some current tests likely assert the eager
  order. *Mitigation:* expect to rewrite them to Node order (same pattern used
  for the scoping/`const` work); ground-truth each against Node.

---

## 7. Test matrix (assert real-Node answers)

Ordering: `then_order`, `nested_then`, `await_order`, `two_awaits`,
`microtask_vs_sync`, `finally_order`, `async_return`, chained `.then().then()`,
`Promise.resolve().then` vs `queueMicrotask`-equivalent.
Settlement: `new Promise((res)=>res(v))`, executor that throws, thenable
adoption, double-settle is a no-op.
Combinators: `Promise.all`/`race`/`any`/`allSettled` ordering relative to
microtasks (must still interop with the existing host-batch path).
Errors: unhandled rejection surfacing; `await` of a rejected promise in
try/catch.
Durable: dump/load/resume with (a) a non-empty microtask queue and (b) a
suspended async task parked on a host-call promise; Temporal-style replay
determinism.

---

## 8. Estimate

- Stages 0–2 (model + queue + async-returns-promise): medium, self-contained,
  removes the throwing bug and fixes `.then`/`.finally` ordering.
- Stage 3 (`await` suspension): large, the real risk; the bulk of the work.
- Stage 4 (durable hardening): medium but essential before default-on.

Recommend implementing Stage 0–2 behind the existing behavior first, proving the
queue/drain on `.then` ordering, then committing to Stage 3 with the durable
test suite as the gate.
