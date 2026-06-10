//! Conformance: `await` suspends the async function (microtask-design
//! Stage 3).
//!
//! `await` inside an async function body detaches the call into a parked
//! `AsyncTask` and returns the pending result promise to the caller — so
//! `await` always yields a microtask tick, async bodies interleave with
//! synchronous code and with each other in Node's order, and a `throw`
//! escaping a body (before OR after its first await) rejects the result
//! promise with the original reason instead of propagating synchronously.
//!
//! `await tool()` (a host call) still suspends the whole VM — that is the
//! durable-execution boundary — and a parked task serializes with the
//! snapshot (asserted below).
//!
//! Every ordering assertion is ground-truthed against real Node.
//!
//! Residual: a *top-level* `await` of an already-settled promise is still
//! inline (no tick); ordering-sensitive code belongs in `async function
//! main()` — the recommended agent pattern.

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, Value, ZapcodeRun, ZapcodeSnapshot};

/// Run `code` to completion and stringify the result via `to_js_string`.
fn run_str(code: &str) -> String {
    let result = ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        Vec::new(),
        ResourceLimits::default(),
    )
    .unwrap()
    .run(Vec::new())
    .unwrap();
    match result.state {
        VmState::Complete(v) => v.to_js_string(&result.heap),
        other => panic!("expected completion for `{code}`, got {other:?}"),
    }
}

/// Run `code` expecting the program itself to fail; return the error text.
fn run_err(code: &str) -> String {
    let err = ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        Vec::new(),
        ResourceLimits::default(),
    )
    .unwrap()
    .run(Vec::new())
    .expect_err("expected the program to fail");
    err.to_string()
}

// ════════════════════════════════════════════════════════════════════════════
//  Interleaving: await yields to the caller and the queue
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn await_yields_to_caller_before_body_resumes() {
    // Node: f1,sync,f2 — the body parks at `await`, the caller's synchronous
    // code runs, then the resume tick continues the body.
    assert_eq!(
        run_str(
            "const log = []; \
             async function f() { log.push('f1'); await null; log.push('f2'); } \
             const p = f(); \
             log.push('sync'); \
             await p; \
             log.join(',')"
        ),
        "f1,sync,f2"
    );
}

#[test]
fn two_bodies_interleave_tick_by_tick() {
    // Node: a1,b1,sync,a2,b2,a3,b3 — each await is one tick; the two bodies
    // alternate in FIFO order.
    assert_eq!(
        run_str(
            "const log = []; \
             async function a() { log.push('a1'); await null; log.push('a2'); await null; log.push('a3'); } \
             async function b() { log.push('b1'); await null; log.push('b2'); await null; log.push('b3'); } \
             const pa = a(); const pb = b(); \
             log.push('sync'); \
             await Promise.all([pa, pb]); \
             log.join(',')"
        ),
        "a1,b1,sync,a2,b2,a3,b3"
    );
}

#[test]
fn then_and_await_resumptions_share_one_fifo() {
    // Node: then,await — the .then reaction was enqueued before the body's
    // resume tick.
    assert_eq!(
        run_str(
            "const log = []; \
             async function f() { await null; log.push('await'); } \
             Promise.resolve().then(() => log.push('then')); \
             const p = f(); \
             await p; \
             log.join(',')"
        ),
        "then,await"
    );
}

#[test]
fn classic_async_ordering_kata() {
    // The well-known interview snippet, byte-for-byte Node order.
    assert_eq!(
        run_str(
            "const log = []; \
             async function async2() { log.push('async2'); } \
             async function async1() { log.push('async1 start'); await async2(); log.push('async1 end'); } \
             log.push('script start'); \
             const p = async1(); \
             Promise.resolve().then(() => log.push('promise1')).then(() => log.push('promise2')); \
             log.push('script end'); \
             await p.then(() => {}).then(() => {}); \
             log.join(',')"
        ),
        "script start,async1 start,async2,script end,async1 end,promise1,promise2"
    );
}

#[test]
fn nested_async_calls_chain_across_parks() {
    assert_eq!(
        run_str(
            "async function inner() { await null; return 1 } \
             async function outer() { const x = await inner(); return x + 1 } \
             await outer()"
        ),
        "2"
    );
}

#[test]
fn async_then_handler_parks_and_chain_adopts() {
    // An async `.then` handler detaches at its await; the chain adopts the
    // handler's result promise and settles when the task finishes.
    assert_eq!(
        run_str("await Promise.resolve(1).then(async x => { await null; return x + 1 })"),
        "2"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  Throws reject the result promise (completing the Stage-2 residual)
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn body_throw_before_any_await_rejects_the_call() {
    assert_eq!(
        run_str(
            "async function f() { throw new Error('x') } \
             await f().catch(e => e.message)"
        ),
        "x"
    );
}

#[test]
fn body_throw_after_await_rejects_the_call() {
    assert_eq!(
        run_str(
            "async function f() { await null; throw 'late' } \
             await f().catch(e => 'c:' + e)"
        ),
        "c:late"
    );
}

#[test]
fn callers_try_does_not_catch_an_unawaited_call() {
    // Node: the call evaluates to a rejected promise; only consumers of that
    // promise see the throw.
    assert_eq!(
        run_str(
            "let c = 'no'; \
             async function f() { throw 'k' } \
             try { f().catch(() => {}) } catch (e) { c = 'yes'; } \
             c"
        ),
        "no"
    );
}

#[test]
fn awaiting_a_throwing_body_rethrows_the_original_error() {
    // The original Error object (identity + message) arrives in the catch.
    assert_eq!(
        run_str(
            "let r = 'none'; \
             async function f() { throw new Error('orig') } \
             try { await f(); } catch (e) { r = 'caught:' + e.message; } \
             r"
        ),
        "caught:orig"
    );
}

#[test]
fn orphaned_async_rejections_fail_the_run() {
    // Thrown before any await…
    let err = run_err("async function g() { throw 'k2' } g(); 'done'");
    assert!(
        err.contains("Unhandled promise rejection") && err.contains("k2"),
        "unexpected error: {err}"
    );
    // …and thrown after a park, during the drain.
    let err = run_err("async function f() { await null; throw 'k' } f(); 'done'");
    assert!(
        err.contains("Unhandled promise rejection") && err.contains("k"),
        "unexpected error: {err}"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  try/catch/finally travel with the parked body
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn try_catch_inside_body_catches_awaited_rejection_after_park() {
    // The try-frame migrates into the AsyncTask and rebases on resume; the
    // catch receives the ORIGINAL reason.
    assert_eq!(
        run_str(
            "async function f() { \
                 try { await Promise.reject('r') } catch (e) { return 'in:' + e } \
             } \
             await f()"
        ),
        "in:r"
    );
}

#[test]
fn try_finally_runs_across_a_park() {
    assert_eq!(
        run_str(
            "const log = []; \
             async function f() { try { await null; return 'v' } finally { log.push('fin') } } \
             const r = await f(); \
             `${r}|${log.join('')}`"
        ),
        "v|fin"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  Durable execution: host calls and parked tasks
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn tool_call_after_a_park_suspends_and_resumes() {
    // The body parks at `await null`, resumes in the drain, then hits a host
    // call — the whole VM suspends with the task's frame live. The snapshot
    // round-trips (parked awaiting frame included) and the body finishes.
    let runner = ZapcodeRun::new(
        "async function f() { await null; const r = await callTool('t'); return r + '!' } \
         const p = f(); \
         await p"
            .to_string(),
        Vec::new(),
        vec!["callTool".to_string()],
        ResourceLimits::default(),
    )
    .unwrap();

    let state = runner.start(Vec::new()).unwrap();
    let snapshot = match state {
        VmState::Suspended {
            function_name,
            args,
            snapshot,
        } => {
            assert_eq!(function_name, "callTool");
            assert_eq!(args.first(), Some(&Value::String("t".into())));
            snapshot
        }
        other => panic!("expected suspension on callTool, got {other:?}"),
    };
    let bytes = snapshot.dump().unwrap();
    let restored = ZapcodeSnapshot::load(&bytes).unwrap();
    let final_state = restored.resume(Value::String("X".into())).unwrap().state;
    match final_state {
        VmState::Complete(Value::String(s)) => assert_eq!(s.to_string(), "X!"),
        other => panic!("expected completion, got {other:?}"),
    }
}

#[test]
fn snapshot_with_a_parked_task_round_trips() {
    // Body A parks awaiting body B's promise; B makes the host call. At the
    // suspension, A exists ONLY as a serialized AsyncTask. After dump/load,
    // B finishes, its promise settles, and the ResumeAsync reaction revives A.
    let runner = ZapcodeRun::new(
        "async function b() { const r = await callTool('for-b'); return r; } \
         async function a(pb) { const v = await pb; return 'a saw ' + v; } \
         const pb = b(); \
         const pa = a(pb); \
         await pa"
            .to_string(),
        Vec::new(),
        vec!["callTool".to_string()],
        ResourceLimits::default(),
    )
    .unwrap();

    let state = runner.start(Vec::new()).unwrap();
    let snapshot = match state {
        VmState::Suspended { snapshot, .. } => snapshot,
        other => panic!("expected suspension, got {other:?}"),
    };
    let bytes = snapshot.dump().unwrap();
    let restored = ZapcodeSnapshot::load(&bytes).unwrap();
    let final_state = restored.resume(Value::String("B".into())).unwrap().state;
    match final_state {
        VmState::Complete(Value::String(s)) => assert_eq!(s.to_string(), "a saw B"),
        other => panic!("expected completion, got {other:?}"),
    }
}

#[test]
fn rejected_tool_call_after_park_rejects_through_catch() {
    let runner = ZapcodeRun::new(
        "async function f() { \
             await null; \
             try { await callTool('t'); return 'no-throw'; } \
             catch (e) { return 'recovered:' + e; } \
         } \
         await f()"
            .to_string(),
        Vec::new(),
        vec!["callTool".to_string()],
        ResourceLimits::default(),
    )
    .unwrap();

    let state = runner.start(Vec::new()).unwrap();
    let snapshot = match state {
        VmState::Suspended { snapshot, .. } => snapshot,
        other => panic!("expected suspension, got {other:?}"),
    };
    let final_state = snapshot
        .resume_with_error(Value::String("tool down".into()))
        .unwrap()
        .state;
    match final_state {
        VmState::Complete(Value::String(s)) => assert_eq!(s.to_string(), "recovered:tool down"),
        other => panic!("expected completion, got {other:?}"),
    }
}
