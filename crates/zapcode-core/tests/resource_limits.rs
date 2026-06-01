//! Resource budgets must be cumulative across a session's lifetime, and a
//! runaway session must not produce an unbounded snapshot.

use zapcode_core::{ResourceLimits, ZapcodeError, ZapcodeSessionSnapshot, ZapcodeSessionState};

#[test]
fn allocation_budget_accumulates_across_chunks() {
    // A single chunk stays well under the budget, but many chunks together
    // must eventually exceed it — proving the count isn't reset per chunk.
    let limits = ResourceLimits {
        max_allocations: 200,
        ..ResourceLimits::default()
    };
    let mut session = ZapcodeSessionSnapshot::new(Vec::new(), limits).unwrap();

    // No top-level binding (sessions reject redeclaration) — just allocate.
    let chunk = "[1, 2, 3, 4, 5, 6, 7, 8, 9, 10].map(x => x * 2).length";

    // The first chunk must succeed (it's under the per-chunk cost).
    let mut chunks_run = 0;
    let mut hit_limit = false;
    for _ in 0..100 {
        match session.run_chunk(chunk.to_string(), Vec::new()) {
            Ok(ZapcodeSessionState::Complete { session: next, .. }) => {
                session = next;
                chunks_run += 1;
            }
            Ok(_) => panic!("unexpected suspension"),
            Err(ZapcodeError::AllocationLimitExceeded) => {
                hit_limit = true;
                break;
            }
            Err(e) => panic!("unexpected error: {e:?}"),
        }
    }

    assert!(hit_limit, "cumulative allocation budget was never enforced");
    assert!(
        chunks_run >= 1,
        "the first chunk should fit under the budget, but it tripped immediately"
    );
}

#[test]
fn dump_rejects_oversized_state() {
    // A tiny snapshot cap makes even normal state too large to persist.
    let limits = ResourceLimits {
        max_snapshot_bytes: 16,
        ..ResourceLimits::default()
    };
    let session = ZapcodeSessionSnapshot::new(Vec::new(), limits).unwrap();
    let state = session
        .run_chunk("const x = 1; x".to_string(), Vec::new())
        .unwrap();
    let session = match state {
        ZapcodeSessionState::Complete { session, .. } => session,
        _ => panic!("expected completion"),
    };

    let err = session.dump().unwrap_err();
    match err {
        ZapcodeError::SnapshotError(msg) => {
            assert!(msg.contains("too large"), "unexpected message: {msg}");
        }
        other => panic!("expected SnapshotError, got {other:?}"),
    }
}
