//! Identical logical state must serialize to identical bytes. Globals,
//! external-function names, and top-level bindings all originate from
//! hash containers whose iteration order is randomized per instance, so this
//! guards against ordering leaking into the wire format (which would break
//! content-addressing and dedup).

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, Value, ZapcodeRun, ZapcodeSessionSnapshot, ZapcodeSnapshot};

// Enough distinct names that a randomized hash order would almost certainly
// differ between two independent runs if we weren't sorting.
const MANY_GLOBALS: &str = r#"
    const alpha = 1;
    const bravo = 2;
    const charlie = 3;
    const delta = 4;
    const echo = 5;
    const foxtrot = 6;
    const golf = 7;
    const hotel = 8;
    const india = 9;
    const juliet = 10;
    const kilo = 11;
    const lima = 12;
    alpha + lima
"#;

fn idle_session_dump() -> Vec<u8> {
    let session = ZapcodeSessionSnapshot::new(
        vec![
            "tool_a".to_string(),
            "tool_b".to_string(),
            "tool_c".to_string(),
        ],
        ResourceLimits::default(),
    )
    .unwrap();
    let state = session
        .run_chunk(MANY_GLOBALS.to_string(), Vec::new())
        .unwrap();
    match state {
        zapcode_core::ZapcodeSessionState::Complete { session, .. } => session.dump().unwrap(),
        zapcode_core::ZapcodeSessionState::Suspended { .. } => panic!("expected completion"),
        zapcode_core::ZapcodeSessionState::SuspendedMany { .. } => {
            panic!("unexpected batch suspension")
        }
    }
}

#[test]
fn idle_session_bytes_are_deterministic_across_runs() {
    let first = idle_session_dump();
    let second = idle_session_dump();
    assert_eq!(
        first, second,
        "identical session state produced different bytes"
    );
}

fn suspended_snapshot_dump() -> Vec<u8> {
    let runner = ZapcodeRun::new(
        format!("{MANY_GLOBALS}\nconst pending = await fetch(\"x\");\npending"),
        Vec::new(),
        vec!["fetch".to_string()],
        ResourceLimits::default(),
    )
    .unwrap();
    match runner.start(Vec::new()).unwrap() {
        VmState::Suspended { snapshot, .. } => snapshot.dump().unwrap(),
        VmState::Complete(_) => panic!("expected suspension"),
        VmState::SuspendedMany { .. } => panic!("unexpected batch suspension"),
    }
}

#[test]
fn suspended_snapshot_bytes_are_deterministic_across_runs() {
    let first = suspended_snapshot_dump();
    let second = suspended_snapshot_dump();
    assert_eq!(
        first, second,
        "identical suspended state produced different bytes"
    );
}

#[test]
fn resume_still_works_after_sorting() {
    let bytes = suspended_snapshot_dump();
    let resumed = ZapcodeSnapshot::load(&bytes)
        .unwrap()
        .resume(Value::Int(99))
        .unwrap();
    match resumed {
        VmState::Complete(v) => assert_eq!(v, Value::Int(99)),
        VmState::Suspended { .. } => panic!("expected completion"),
        VmState::SuspendedMany { .. } => panic!("unexpected batch suspension"),
    }
}
