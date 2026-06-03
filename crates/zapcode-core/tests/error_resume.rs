//! Resuming a suspended external call with an *error* (a failed host tool /
//! Temporal activity) instead of a value.

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, Value, ZapcodeError, ZapcodeRun};

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

fn snapshot(state: VmState) -> zapcode_core::ZapcodeSnapshot {
    match state {
        VmState::Suspended { snapshot, .. } => snapshot,
        VmState::Complete(_) => panic!("expected suspension"),
        VmState::SuspendedMany { .. } => panic!("unexpected batch suspension"),
    }
}

#[test]
fn error_is_catchable_in_guest_try_catch() {
    let state = start(
        r#"
        let outcome;
        try {
            const value = await callTool("x");
            outcome = "ok:" + value;
        } catch (e) {
            outcome = "caught:" + e;
        }
        outcome
    "#,
    );

    let resumed = snapshot(state)
        .resume_with_error(Value::String("upstream 500".into()))
        .unwrap()
        .state;

    match resumed {
        VmState::Complete(v) => assert_eq!(v, Value::String("caught:upstream 500".into())),
        VmState::Suspended { .. } => panic!("expected completion"),
        VmState::SuspendedMany { .. } => panic!("unexpected batch suspension"),
    }
}

#[test]
fn uncaught_error_propagates_to_host() {
    let state = start(r#"const value = await callTool("x"); value"#);

    let err = snapshot(state)
        .resume_with_error(Value::String("upstream 500".into()))
        .unwrap_err();

    match err {
        ZapcodeError::ExternalError(msg) => assert_eq!(msg, "upstream 500"),
        other => panic!("expected ExternalError, got {other:?}"),
    }
}

#[test]
fn execution_continues_normally_after_a_caught_error() {
    // After catching a failed call, the guest can call the tool again and the
    // VM keeps running — proving the resume left a clean VM state.
    let state = start(
        r#"
        let result;
        try {
            await callTool("first");
        } catch (e) {
            result = await callTool("retry");
        }
        result
    "#,
    );

    // First call fails...
    let after_error = snapshot(state)
        .resume_with_error(Value::String("boom".into()))
        .unwrap()
        .state;
    // ...which suspends again on the retry call inside catch.
    let retry = match after_error {
        VmState::Suspended {
            function_name,
            args,
            snapshot,
        } => {
            assert_eq!(function_name, "callTool");
            assert_eq!(args, vec![Value::String("retry".into())]);
            snapshot
        }
        VmState::Complete(_) => panic!("expected a second suspension on retry"),
        VmState::SuspendedMany { .. } => panic!("unexpected batch suspension"),
    };

    match retry.resume(Value::String("recovered".into())).unwrap().state {
        VmState::Complete(v) => assert_eq!(v, Value::String("recovered".into())),
        VmState::Suspended { .. } => panic!("expected completion"),
        VmState::SuspendedMany { .. } => panic!("unexpected batch suspension"),
    }
}
