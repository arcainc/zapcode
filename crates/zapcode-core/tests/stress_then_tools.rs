//! N4: external (tool) calls inside `.then`/`.catch`/`.finally` callbacks.
//!
//! Previously a tool call inside one of these callbacks tripped the
//! "cannot call an external function inside an array-callback method" guard,
//! because the callback ran synchronously via `call_function_internal` (which
//! cannot suspend). The callbacks now run through the continuation machinery
//! (a `Continuation::PromiseCallback`) driven by the main `execute()` loop, so
//! a `CallExternal` inside them suspends the VM and resumes with the tool
//! result — making the common `primary().catch(() => fallbackTool())` retry
//! pattern work.
//!
//! Harness copied from `error_resume.rs` (note: `run_str` ends with
//! `v.to_js_string(&result.heap)`).

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, Value, ZapcodeRun, ZapcodeSnapshot};

fn start(code: &str) -> VmState {
    ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        vec!["callTool".to_string()],
        ResourceLimits::default(),
    )
    .unwrap()
    .start(Vec::new())
    .unwrap()
}

/// Unwrap a single-tool suspension, asserting the function name and first arg.
fn expect_suspend(state: VmState, expect_arg: &str) -> ZapcodeSnapshot {
    match state {
        VmState::Suspended {
            function_name,
            args,
            snapshot,
        } => {
            assert_eq!(function_name, "callTool");
            assert_eq!(args.first(), Some(&Value::String(expect_arg.into())));
            snapshot
        }
        VmState::Complete(_) => panic!("expected suspension on callTool({expect_arg})"),
        VmState::SuspendedMany { .. } => panic!("unexpected batch suspension"),
    }
}

fn expect_complete_string(state: VmState) -> String {
    match state {
        VmState::Complete(Value::String(s)) => s.to_string(),
        VmState::Complete(other) => panic!("expected string completion, got {other:?}"),
        VmState::Suspended { .. } => panic!("expected completion, got suspension"),
        VmState::SuspendedMany { .. } => panic!("unexpected batch suspension"),
    }
}

fn expect_complete_int(state: VmState) -> i64 {
    match state {
        VmState::Complete(Value::Int(n)) => n,
        VmState::Complete(other) => panic!("expected int completion, got {other:?}"),
        VmState::Suspended { .. } => panic!("expected completion, got suspension"),
        VmState::SuspendedMany { .. } => panic!("unexpected batch suspension"),
    }
}

/// The headline pattern: `primary().catch(() => fallbackTool())`. The primary
/// promise rejects; the catch callback makes a tool call (which suspends),
/// resumes with the fallback value, and the chain completes with it.
#[test]
fn catch_callback_can_call_a_tool_and_suspend() {
    let state = start(
        r#"
        const result = await Promise.reject("primary down")
            .catch(() => callTool("fallback"));
        "got:" + result
    "#,
    );

    let snap = expect_suspend(state, "fallback");
    let done = snap.resume(Value::String("from-fallback".into())).unwrap().state;
    assert_eq!(expect_complete_string(done), "got:from-fallback");
}

/// A `.then` onFulfilled callback that makes a tool call, threading the upstream
/// resolved value into the tool argument.
#[test]
fn then_callback_can_call_a_tool_and_suspend() {
    let state = start(
        r#"
        const result = await Promise.resolve("ctx")
            .then((v) => callTool("with:" + v));
        result
    "#,
    );

    let snap = expect_suspend(state, "with:ctx");
    let done = snap.resume(Value::String("tool-out".into())).unwrap().state;
    assert_eq!(expect_complete_string(done), "tool-out");
}

/// `.then(_, onRejected)` where the onRejected handler makes a tool call.
#[test]
fn then_on_rejected_callback_can_call_a_tool() {
    let state = start(
        r#"
        const result = await Promise.reject("err")
            .then(null, (e) => callTool("rejected:" + e));
        result
    "#,
    );

    let snap = expect_suspend(state, "rejected:err");
    let done = snap.resume(Value::String("recovered".into())).unwrap().state;
    assert_eq!(expect_complete_string(done), "recovered");
}

/// `.finally` runs the tool but its return value is discarded — the original
/// resolved value passes through unchanged.
#[test]
fn finally_callback_can_call_a_tool_and_passes_value_through() {
    let state = start(
        r#"
        const result = await Promise.resolve(7)
            .finally(() => callTool("cleanup"));
        result
    "#,
    );

    let snap = expect_suspend(state, "cleanup");
    // Even though the tool returns 999, finally discards it.
    let done = snap.resume(Value::Int(999)).unwrap().state;
    assert_eq!(expect_complete_int(done), 7);
}

/// The continuation must survive snapshot serialization: dump the suspended
/// snapshot to bytes, load it back, and resume — the `.catch` chain still
/// completes. This proves `Continuation::PromiseCallback` round-trips.
#[test]
fn promise_callback_continuation_survives_snapshot_roundtrip() {
    let state = start(
        r#"
        const result = await Promise.reject("down")
            .catch(() => callTool("retry"));
        "final:" + result
    "#,
    );

    let snap = expect_suspend(state, "retry");
    let bytes = snap.dump().unwrap();
    let reloaded = ZapcodeSnapshot::load(&bytes).unwrap();
    let done = reloaded
        .resume(Value::String("after-reload".into()))
        .unwrap()
        .state;
    assert_eq!(expect_complete_string(done), "final:after-reload");
}

/// A failing tool inside `.catch` whose error is caught by an inner try/catch.
/// The tool call is `await`-ed inside the try so its rejection surfaces there
/// (post-N5 a bare `return tool()` returns a deferred promise that settles
/// *after* the try exits — matching JS — so the catchable form must await).
#[test]
fn tool_error_inside_catch_callback_is_catchable() {
    let state = start(
        r#"
        const result = await Promise.reject("primary")
            .catch(async () => {
                try {
                    return await callTool("flaky");
                } catch (e) {
                    return "handled:" + e;
                }
            });
        result
    "#,
    );

    let snap = expect_suspend(state, "flaky");
    let done = snap
        .resume_with_error(Value::String("boom".into()))
        .unwrap()
        .state;
    assert_eq!(expect_complete_string(done), "handled:boom");
}

/// Chained callbacks where a later `.then` (after a recovering `.catch`) makes
/// the tool call — exercises multiple promise-callback continuations in a row.
#[test]
fn then_after_catch_chain_with_tool_call() {
    let state = start(
        r#"
        const result = await Promise.reject("e1")
            .catch(() => "recovered")
            .then((v) => callTool("next:" + v));
        result
    "#,
    );

    let snap = expect_suspend(state, "next:recovered");
    let done = snap.resume(Value::String("done".into())).unwrap().state;
    assert_eq!(expect_complete_string(done), "done");
}

/// A resolved `.catch` does NOT run its callback, so no tool call happens — the
/// value passes through. Guards against accidentally always starting a
/// continuation.
#[test]
fn resolved_promise_skips_catch_callback_no_suspend() {
    let state = start(
        r#"
        const result = await Promise.resolve(123)
            .catch(() => callTool("should-not-run"));
        result
    "#,
    );

    assert_eq!(expect_complete_int(state), 123);
}
