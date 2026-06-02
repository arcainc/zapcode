//! `Promise.all([extCall(), ...])` batches its external calls into a single
//! suspension so the host can run them in parallel.

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, Value, ZapcodeRun, ZapcodeSnapshot};

fn start(code: &str) -> VmState {
    ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        vec!["fetch".to_string()],
        ResourceLimits::default(),
    )
    .unwrap()
    .start(Vec::new())
    .unwrap()
}

#[test]
fn promise_all_of_external_calls_suspends_once_with_all_calls() {
    let state = start(
        r#"
        const results = await Promise.all([fetch("a"), fetch("b"), fetch("c")]);
        results.join(",")
    "#,
    );

    let (calls, snapshot) = match state {
        VmState::SuspendedMany { calls, snapshot } => (calls, snapshot),
        other => panic!("expected SuspendedMany, got {other:?}"),
    };

    // All three calls surface at once, in order — the host can run them in parallel.
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0].name, "fetch");
    assert_eq!(calls[0].args, vec![Value::String("a".into())]);
    assert_eq!(calls[1].args, vec![Value::String("b".into())]);
    assert_eq!(calls[2].args, vec![Value::String("c".into())]);

    let done = snapshot
        .resume_many(vec![
            Value::String("A".into()),
            Value::String("B".into()),
            Value::String("C".into()),
        ])
        .unwrap();
    match done.state {
        VmState::Complete(v) => assert_eq!(v, Value::String("A,B,C".into())),
        other => panic!("expected completion, got {other:?}"),
    }
}

#[test]
fn batch_survives_a_dump_load_boundary() {
    let state = start(r#"const r = await Promise.all([fetch("x"), fetch("y")]); r"#);
    let snapshot = match state {
        VmState::SuspendedMany { snapshot, .. } => snapshot,
        other => panic!("expected SuspendedMany, got {other:?}"),
    };

    // Ship the suspended batch across a boundary, then resume it elsewhere.
    let bytes = snapshot.dump().unwrap();
    let resumed = ZapcodeSnapshot::load(&bytes)
        .unwrap()
        .resume_many(vec![Value::Int(1), Value::Int(2)])
        .unwrap();
    match resumed.state {
        VmState::Complete(Value::Array(items)) => {
            assert_eq!(
                resumed.heap.array_vec(items),
                vec![Value::Int(1), Value::Int(2)]
            );
        }
        other => panic!("expected completed array, got {other:?}"),
    }
}

#[test]
fn promise_all_mixes_external_calls_and_plain_values() {
    let state = start(r#"const r = await Promise.all([fetch("a"), 42]); r"#);
    let snapshot = match state {
        VmState::SuspendedMany { calls, snapshot } => {
            assert_eq!(calls.len(), 1, "only the external call is batched");
            snapshot
        }
        other => panic!("expected SuspendedMany, got {other:?}"),
    };
    let resumed = snapshot
        .resume_many(vec![Value::String("A".into())])
        .unwrap();
    match resumed.state {
        VmState::Complete(Value::Array(items)) => {
            assert_eq!(
                resumed.heap.array_vec(items),
                vec![Value::String("A".into()), Value::Int(42)]
            );
        }
        other => panic!("expected completed array, got {other:?}"),
    }
}

#[test]
fn single_await_still_uses_the_simple_suspension() {
    // A bare `await fetch(...)` is unchanged: one call via VmState::Suspended.
    let state = start(r#"const r = await fetch("a"); r"#);
    match state {
        VmState::Suspended { function_name, .. } => assert_eq!(function_name, "fetch"),
        other => panic!("expected single Suspended, got {other:?}"),
    }
}
