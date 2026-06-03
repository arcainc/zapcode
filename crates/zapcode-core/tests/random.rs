//! Math.random is deterministic across replay (same program → same sequence)
//! but varies call to call, and its state survives suspend/resume.

use zapcode_core::heap::Heap;
use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, Value, ZapcodeRun};

/// Run `code` and return the completion value together with the heap it
/// references, so array/object handles can be resolved.
fn run(code: &str) -> (Value, Heap) {
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
        VmState::Complete(v) => (v, result.heap),
        other => panic!("expected completion, got {other:?}"),
    }
}

#[test]
fn random_is_in_range_and_varies() {
    let (out, heap) = run("[Math.random(), Math.random(), Math.random()]");
    let arr = match out {
        Value::Array(h) => heap.array_vec(h),
        other => panic!("expected array, got {other:?}"),
    };
    assert_eq!(arr.len(), 3);
    let nums: Vec<f64> = arr.iter().map(|v| v.to_number()).collect();
    for n in &nums {
        assert!(*n >= 0.0 && *n < 1.0, "value out of range: {n}");
    }
    // Successive draws differ — not the old constant 0.5.
    assert_ne!(nums[0], nums[1]);
    assert_ne!(nums[1], nums[2]);
    assert!(nums.iter().all(|n| *n != 0.5));
}

fn nums((value, heap): (Value, Heap)) -> Vec<f64> {
    match value {
        Value::Array(h) => heap.array_vec(h).iter().map(|v| v.to_number()).collect(),
        other => panic!("expected array, got {other:?}"),
    }
}

#[test]
fn random_sequence_is_deterministic_across_runs() {
    // Value::Array uses reference equality, so compare the extracted numbers.
    let a = nums(run("[Math.random(), Math.random()]"));
    let b = nums(run("[Math.random(), Math.random()]"));
    assert_eq!(a, b, "same program must produce the same random sequence");
}

#[test]
fn random_state_survives_suspend_resume() {
    // Draw one before an external call and one after; the post-resume draw must
    // continue the sequence (i.e. differ from the first), proving rng_state was
    // carried through the snapshot.
    let state = ZapcodeRun::new(
        r#"
        const a = Math.random();
        const ignored = await fetch("x");
        const b = Math.random();
        [a, b]
        "#
        .to_string(),
        Vec::new(),
        vec!["fetch".to_string()],
        ResourceLimits::default(),
    )
    .unwrap()
    .start(Vec::new())
    .unwrap();

    let snapshot = match state {
        VmState::Suspended { snapshot, .. } => snapshot,
        other => panic!("expected suspension, got {other:?}"),
    };
    let resumed = snapshot.resume(Value::Null).unwrap();
    let out = match resumed.state {
        VmState::Complete(v) => (v, resumed.heap),
        other => panic!("expected completion, got {other:?}"),
    };
    let drawn = nums(out);
    assert_ne!(
        drawn[0], drawn[1],
        "post-resume draw should continue the sequence"
    );
    // And it matches a non-suspending run of the same two draws.
    assert_eq!(drawn, nums(run("[Math.random(), Math.random()]")));
}
