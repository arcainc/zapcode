//! Conformance: `new Promise(executor)`.
//!
//! The executor runs synchronously with serializable `resolve`/`reject`
//! capability objects that settle the new pending promise through the
//! microtask machinery — so `.then` chains, `await`, combinators, unhandled
//! rejection tracking, and durable suspension all work on constructed
//! promises with no special cases. A throw escaping the executor rejects
//! the promise (spec'd constructor catch); the executor's return value is
//! discarded.
//!
//! All assertions ground-truthed against real Node.
//!
//! The capabilities are marker objects internally (not function values — a
//! Value-enum variant would ripple through every binding crate), but they
//! join the callable-marker set in `TypeOf`, so `typeof resolve` reports
//! `"function"` like Node. Calling them works.

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, Value, ZapcodeRun};

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
//  Basics
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn resolve_and_reject() {
    assert_eq!(run_str("await new Promise((resolve) => resolve(42))"), "42");
    assert_eq!(
        run_str("await new Promise((_, reject) => reject('r')).catch(e => 'c:' + e)"),
        "c:r"
    );
    assert_eq!(run_str("typeof new Promise(r => r(1))"), "object");
}

#[test]
fn executor_runs_synchronously() {
    // Node: exec,after-new,then:1 — executor body runs during `new`; the
    // .then handler defers to the queue.
    assert_eq!(
        run_str(
            "const log = []; \
             const p = new Promise(r => { log.push('exec'); r(1); }); \
             log.push('after-new'); \
             await p.then(v => log.push('then:' + v)); \
             log.join(',')"
        ),
        "exec,after-new,then:1"
    );
}

#[test]
fn deferred_pattern_settles_later() {
    // The capability escapes the executor and settles the promise later —
    // reactions registered while pending fire on settle.
    assert_eq!(
        run_str(
            "let res; \
             const p = new Promise(r => { res = r; }); \
             const chained = p.then(v => v + 1); \
             res(7); \
             await chained"
        ),
        "8"
    );
}

#[test]
fn first_settle_wins() {
    assert_eq!(
        run_str("await new Promise((r, j) => { r(1); j('x'); r(2); })"),
        "1"
    );
}

#[test]
fn non_function_executor_is_a_type_error() {
    let err = run_err("new Promise(5)");
    assert!(
        err.contains("Promise resolver") && err.contains("not a function"),
        "unexpected error: {err}"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  Thenable adoption via resolve
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn resolve_adopts_settled_promise() {
    assert_eq!(
        run_str("await new Promise(r => r(Promise.resolve(9)))"),
        "9"
    );
    assert_eq!(
        run_str(
            "await new Promise(r => r(Promise.reject('bad'))) \
                 .catch(e => 'caught:' + e)"
        ),
        "caught:bad"
    );
}

#[test]
fn resolve_adopts_pending_chain() {
    assert_eq!(
        run_str("await new Promise(r => r(Promise.resolve(1).then(x => x + 1)))"),
        "2"
    );
}

#[test]
fn resolve_adopts_async_call() {
    assert_eq!(
        run_str(
            "async function f() { await null; return 5 } \
             await new Promise(r => r(f()))"
        ),
        "5"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  Throws in the executor
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn executor_throw_rejects_the_promise() {
    assert_eq!(
        run_str(
            "await new Promise(() => { throw new Error('boom') }) \
                 .catch(e => e.message)"
        ),
        "boom"
    );
    // The constructor catch is internal — the caller's try does NOT see it.
    assert_eq!(
        run_str(
            "let out = 'no'; \
             let p; \
             try { p = new Promise(() => { throw 'inner' }); } catch (e) { out = 'outer'; } \
             const r = await p.catch(e => 'chain:' + e); \
             `${r}|${out}`"
        ),
        "chain:inner|no"
    );
}

#[test]
fn throw_after_settle_is_swallowed() {
    assert_eq!(
        run_str("await new Promise(r => { r('kept'); throw 'late' })"),
        "kept"
    );
}

#[test]
fn unsettled_rejection_via_executor_fails_the_run() {
    // An orphaned constructed rejection reports at end-of-drain like any
    // other unhandled rejection.
    let err = run_err("new Promise((_, j) => j('orphan')).then(x => x); 'done'");
    assert!(
        err.contains("Unhandled promise rejection") && err.contains("orphan"),
        "unexpected error: {err}"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  Interop: combinators, async fns, durability
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn constructed_promises_work_in_combinators() {
    assert_eq!(
        run_str(
            "JSON.stringify(await Promise.all([ \
                 new Promise(r => r(1)), \
                 new Promise(r => r(2)).then(x => x * 10), \
                 3, \
             ]))"
        ),
        "[1,20,3]"
    );
}

#[test]
fn await_inside_async_body_parks_on_constructed_promise() {
    assert_eq!(
        run_str(
            "let res; \
             const gate = new Promise(r => { res = r; }); \
             const log = []; \
             async function waiter() { log.push('start'); const v = await gate; log.push('got:' + v); } \
             const w = waiter(); \
             log.push('sync'); \
             res('open'); \
             await w; \
             log.join(',')"
        ),
        "start,sync,got:open"
    );
}

#[test]
fn tool_call_inside_executor_suspends_and_resumes() {
    // The executor frame runs in the main loop, so a host call inside it
    // suspends the whole VM; the capability settles after resume.
    let runner = ZapcodeRun::new(
        "const p = new Promise(async (resolve) => { \
             const v = await callTool('t'); \
             resolve(v + '!'); \
         }); \
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
            snapshot,
            ..
        } => {
            assert_eq!(function_name, "callTool");
            snapshot
        }
        other => panic!("expected suspension, got {other:?}"),
    };
    match snapshot.resume(Value::String("T".into())).unwrap().state {
        VmState::Complete(Value::String(s)) => assert_eq!(s.to_string(), "T!"),
        other => panic!("expected completion, got {other:?}"),
    }
}

#[test]
fn resolve_with_deferred_tool_promise_forces_the_call() {
    // resolve(toolPromise) adopts the deferred host call: it is forced, the
    // VM suspends, and the constructed promise settles with the host value.
    let runner = ZapcodeRun::new(
        "const p = new Promise(r => r(callTool('t'))); \
         await p"
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
    match snapshot.resume(Value::String("V".into())).unwrap().state {
        VmState::Complete(Value::String(s)) => assert_eq!(s.to_string(), "V"),
        other => panic!("expected completion, got {other:?}"),
    }
}
