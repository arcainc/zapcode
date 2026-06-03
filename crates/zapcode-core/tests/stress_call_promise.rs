//! N5: a bare (un-awaited) tool-call expression evaluates to a real *deferred*
//! Promise object, not an eagerly-resolved value.
//!
//! Before N5, `const p = tool()` suspended immediately and `p` became the
//! resolved value. Now `tool()` (when not directly awaited) compiles to a
//! deferred single-call promise (`status: "pending_call"`) that:
//!   * is a genuine object — `typeof p === "object"`, `p instanceof Promise`-ish
//!     (it carries `__promise__`), and stringifies as `[object Promise]`;
//!   * does NOT make the host call until it is awaited or driven by
//!     `.then`/`.catch`/`.finally`;
//!   * when awaited, suspends once on its host call and resumes with the result;
//!   * when `.then`/`.catch`/`.finally`-chained, forces the call and runs the
//!     callbacks (which may themselves make tool calls, per N4).
//!
//! The *directly awaited* form (`await tool()`) still uses the eager-suspend
//! path, unchanged — covered by `async_await.rs` / `stress_then_tools.rs`.
//!
//! Harness mirrors `stress_then_tools.rs` (note: completion values are read via
//! `to_js_string` against the result heap).

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

/// Assert the program completed without ever suspending, returning the JS-string
/// rendering of the result value (resolved against its heap).
fn expect_complete_str(state: VmState) -> String {
    match state {
        VmState::Complete(v) => {
            // Build a throwaway result via a no-op: the value's heap-dependent
            // rendering needs the VM heap, which `Complete` already resolved into
            // the snapshot heap exposed below. We reconstruct via RunResult-style
            // access by matching primitives directly; compound values are rendered
            // through an empty heap only for primitives in these tests.
            v.to_js_string(&zapcode_core::heap::Heap::new())
        }
        VmState::Suspended { function_name, .. } => {
            panic!("expected completion, got suspension on {function_name}")
        }
        VmState::SuspendedMany { .. } => panic!("unexpected batch suspension"),
    }
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
        VmState::Suspended { function_name, .. } => {
            panic!("expected completion, got suspension on {function_name}")
        }
        VmState::SuspendedMany { .. } => panic!("unexpected batch suspension"),
    }
}

fn expect_complete_int(state: VmState) -> i64 {
    match state {
        VmState::Complete(Value::Int(n)) => n,
        VmState::Complete(other) => panic!("expected int completion, got {other:?}"),
        VmState::Suspended { function_name, .. } => {
            panic!("expected completion, got suspension on {function_name}")
        }
        VmState::SuspendedMany { .. } => panic!("unexpected batch suspension"),
    }
}

/// A bare tool-call is a deferred Promise object: `typeof` is `"object"` and it
/// does NOT make the host call (the program completes without suspending).
#[test]
fn bare_tool_call_is_an_object_and_defers() {
    let state = start(
        r#"
        const p = callTool("a");
        typeof p
    "#,
    );
    assert_eq!(expect_complete_str(state), "object");
}

/// The deferred promise is a truthy object that string-coerces to the spec's
/// `[object Promise]` (the `__promise__`-tagged heap object is recognized by
/// stringification — shared with batch / resolved promises).
#[test]
fn bare_tool_call_is_truthy_object() {
    let state = start(
        r#"
        const p = callTool("a");
        (p ? "truthy:" : "falsy:") + ("" + p)
    "#,
    );
    assert_eq!(expect_complete_str(state), "truthy:[object Promise]");
}

/// Building a bare tool-call promise but never consuming it makes no host call.
#[test]
fn unconsumed_promise_makes_no_host_call() {
    let state = start(
        r#"
        const p = callTool("never");
        const q = callTool("alsonever");
        "done"
    "#,
    );
    assert_eq!(expect_complete_string(state), "done");
}

/// Awaiting a stored deferred promise triggers the host call and yields its result.
#[test]
fn await_stored_promise_triggers_the_call() {
    let state = start(
        r#"
        const p = callTool("ping");
        const r = await p;
        "got:" + r
    "#,
    );
    let snap = expect_suspend(state, "ping");
    let done = snap.resume(Value::String("pong".into())).unwrap().state;
    assert_eq!(expect_complete_string(done), "got:pong");
}

/// A deferred promise settles once: awaiting the same stored promise twice
/// reuses the cached value and makes only one host call.
#[test]
fn deferred_promise_settles_once_across_two_awaits() {
    let state = start(
        r#"
        const p = callTool("once");
        const a = await p;
        const b = await p;
        a + "/" + b
    "#,
    );
    let snap = expect_suspend(state, "once");
    // Only one suspension occurs; the second await reuses the cached value.
    let done = snap.resume(Value::String("R".into())).unwrap().state;
    assert_eq!(expect_complete_string(done), "R/R");
}

/// After awaiting a deferred promise, `.then` on the same promise runs
/// synchronously against the cached value (no second host call).
#[test]
fn then_after_await_uses_cached_value() {
    let state = start(
        r#"
        const p = callTool("cache");
        const a = await p;
        const b = await p.then((v) => v + "!");
        a + "/" + b
    "#,
    );
    let snap = expect_suspend(state, "cache");
    let done = snap.resume(Value::String("Z".into())).unwrap().state;
    assert_eq!(expect_complete_string(done), "Z/Z!");
}

/// `p.then(cb)` forces the call, runs the callback with the resolved value, and
/// the awaited chain completes with the callback's return value.
#[test]
fn then_on_deferred_promise_runs_callback() {
    let state = start(
        r#"
        const p = callTool("ctx");
        const r = await p.then((v) => "then:" + v);
        r
    "#,
    );
    let snap = expect_suspend(state, "ctx");
    let done = snap.resume(Value::String("X".into())).unwrap().state;
    assert_eq!(expect_complete_string(done), "then:X");
}

/// `.then` chaining works: two chained callbacks transform the resolved value.
#[test]
fn then_chain_on_deferred_promise() {
    let state = start(
        r#"
        const r = await callTool("seed")
            .then((v) => v + "-1")
            .then((v) => v + "-2");
        r
    "#,
    );
    let snap = expect_suspend(state, "seed");
    let done = snap.resume(Value::String("s".into())).unwrap().state;
    assert_eq!(expect_complete_string(done), "s-1-2");
}

/// A `.then` callback may itself make a tool call (N4 inside N5): forcing the
/// first call suspends, and running the callback suspends again on the inner
/// tool call.
#[test]
fn then_callback_can_make_a_tool_call() {
    let state = start(
        r#"
        const r = await callTool("first")
            .then((v) => callTool("second:" + v));
        r
    "#,
    );
    // First suspension: the deferred promise's own call.
    let snap1 = expect_suspend(state, "first");
    let state2 = snap1.resume(Value::String("A".into())).unwrap().state;
    // Second suspension: the tool call made inside the .then callback.
    let snap2 = expect_suspend(state2, "second:A");
    let done = snap2.resume(Value::String("B".into())).unwrap().state;
    assert_eq!(expect_complete_string(done), "B");
}

/// `.catch` on a deferred promise whose call rejects runs the handler with the
/// rejection reason (the host rejects via `resume_with_error`).
#[test]
fn catch_on_deferred_promise_handles_rejection() {
    let state = start(
        r#"
        const r = await callTool("flaky").catch((e) => "recovered:" + e);
        r
    "#,
    );
    let snap = expect_suspend(state, "flaky");
    let done = snap
        .resume_with_error(Value::String("boom".into()))
        .unwrap()
        .state;
    assert_eq!(expect_complete_string(done), "recovered:boom");
}

/// `.catch` is a no-op when the deferred call succeeds — the resolved value
/// passes through.
#[test]
fn catch_on_resolved_deferred_promise_passes_value_through() {
    let state = start(
        r#"
        const r = await callTool("ok").catch((e) => "should-not-run");
        r
    "#,
    );
    let snap = expect_suspend(state, "ok");
    let done = snap.resume(Value::String("value".into())).unwrap().state;
    assert_eq!(expect_complete_string(done), "value");
}

/// `.finally` forces the call, runs cleanup (return value discarded), and passes
/// the original resolved value through.
#[test]
fn finally_on_deferred_promise_passes_value_through() {
    let state = start(
        r#"
        const r = await callTool("ok").finally(() => "ignored");
        r
    "#,
    );
    let snap = expect_suspend(state, "ok");
    let done = snap.resume(Value::Int(42)).unwrap().state;
    assert_eq!(expect_complete_int(done), 42);
}

/// The directly-awaited form is unchanged: `await tool()` suspends eagerly on the
/// call (no intermediate promise object) and resumes with the value.
#[test]
fn directly_awaited_tool_call_still_eager() {
    let state = start(
        r#"
        const r = await callTool("direct");
        "v:" + r
    "#,
    );
    let snap = expect_suspend(state, "direct");
    let done = snap.resume(Value::String("ok".into())).unwrap().state;
    assert_eq!(expect_complete_string(done), "v:ok");
}

/// The deferred-promise suspension survives a snapshot dump/load round-trip
/// (resume_action + pending_call are serialized): a `.then` chain reloaded
/// mid-flight still completes.
#[test]
fn then_chain_survives_snapshot_roundtrip() {
    let state = start(
        r#"
        const r = await callTool("seed").then((v) => "reload:" + v);
        r
    "#,
    );
    let snap = expect_suspend(state, "seed");
    let bytes = snap.dump().unwrap();
    let reloaded = ZapcodeSnapshot::load(&bytes).unwrap();
    let done = reloaded
        .resume(Value::String("V".into()))
        .unwrap()
        .state;
    assert_eq!(expect_complete_string(done), "reload:V");
}

/// `Promise.all` over a *dynamic* array of deferred single-call promises (built
/// with `.map`, not a literal) lowers to a batch suspension so the host runs all
/// the calls — `Promise.all(items.map(f))` works when `f` is a bare tool call.
#[test]
fn promise_all_over_mapped_deferred_calls_batches() {
    let state = start(
        r#"
        const out = await Promise.all([1, 2, 3].map((n) => callTool("k" + n)));
        out.join(",")
    "#,
    );
    let (combinator, snapshot, calls) = match state {
        VmState::SuspendedMany {
            combinator,
            snapshot,
            calls,
        } => (combinator, snapshot, calls),
        other => panic!("expected batch suspension, got {other:?}"),
    };
    assert_eq!(combinator, zapcode_core::BatchKind::All);
    // One call per mapped element, in order.
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0].args.first(), Some(&Value::String("k1".into())));
    assert_eq!(calls[2].args.first(), Some(&Value::String("k3".into())));
    let done = snapshot
        .resume_many(vec![
            Value::String("a".into()),
            Value::String("b".into()),
            Value::String("c".into()),
        ])
        .unwrap()
        .state;
    assert_eq!(expect_complete_string(done), "a,b,c");
}

/// `Promise.race` over mapped deferred calls lowers to a race batch suspension.
#[test]
fn promise_race_over_mapped_deferred_calls_batches() {
    let state = start(
        r#"
        const r = await Promise.race([1, 2].map((n) => callTool("r" + n)));
        "won:" + r
    "#,
    );
    let snapshot = match state {
        VmState::SuspendedMany {
            combinator,
            snapshot,
            ..
        } => {
            assert_eq!(combinator, zapcode_core::BatchKind::Race);
            snapshot
        }
        other => panic!("expected batch suspension, got {other:?}"),
    };
    // race resumes with the single winning value.
    let done = snapshot.resume_many(vec![Value::String("fast".into())]).unwrap().state;
    assert_eq!(expect_complete_string(done), "won:fast");
}

/// Returning a bare tool-call promise from an async function and awaiting it at
/// the call site triggers the call (deferral threads through a function return).
#[test]
fn returned_deferred_promise_is_awaitable_at_call_site() {
    let state = start(
        r#"
        async function make() {
            return callTool("via-return");
        }
        const r = await make();
        "out:" + r
    "#,
    );
    let snap = expect_suspend(state, "via-return");
    let done = snap.resume(Value::String("R".into())).unwrap().state;
    assert_eq!(expect_complete_string(done), "out:R");
}
