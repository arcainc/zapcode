//! Tests for the versioned, integrity-checked snapshot wire format.

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, Value, ZapcodeRun, ZapcodeSessionSnapshot, ZapcodeSnapshot};

fn suspended_snapshot() -> ZapcodeSnapshot {
    let runner = ZapcodeRun::new(
        r#"const r = await fetch("https://example.com");"#.to_string(),
        Vec::new(),
        vec!["fetch".to_string()],
        ResourceLimits::default(),
    )
    .unwrap();
    match runner.start(Vec::new()).unwrap() {
        VmState::Suspended { snapshot, .. } => snapshot,
        VmState::Complete(_) => panic!("expected suspension"),
        VmState::SuspendedMany { .. } => panic!("unexpected batch suspension"),
    }
}

#[test]
fn dump_emits_magic_header() {
    let bytes = suspended_snapshot().dump().unwrap();
    assert_eq!(&bytes[0..4], b"ZPC1", "frame should start with magic bytes");
    // 4 magic + 2 version + 1 kind + 1 compression + 32 sha256 = 40-byte header.
    assert!(bytes.len() > 40);
}

#[test]
fn load_roundtrips_a_real_snapshot() {
    let bytes = suspended_snapshot().dump().unwrap();
    let loaded = ZapcodeSnapshot::load(&bytes).unwrap();
    // Resuming the reloaded snapshot completes the program.
    match loaded.resume(Value::Int(7)).unwrap().state {
        VmState::Complete(_) => {}
        VmState::Suspended { .. } => panic!("expected completion after resume"),
        VmState::SuspendedMany { .. } => panic!("unexpected batch suspension"),
    }
}

#[test]
fn load_rejects_bad_magic() {
    let mut bytes = suspended_snapshot().dump().unwrap();
    bytes[0] ^= 0xFF;
    let err = ZapcodeSnapshot::load(&bytes).unwrap_err().to_string();
    assert!(err.contains("magic"), "unexpected error: {err}");
}

#[test]
fn load_rejects_truncated_blob() {
    let err = ZapcodeSnapshot::load(&[0u8; 8]).unwrap_err().to_string();
    assert!(err.contains("too short"), "unexpected error: {err}");
}

#[test]
fn load_rejects_version_mismatch() {
    let mut bytes = suspended_snapshot().dump().unwrap();
    // Bump the format version (bytes 4..6, little-endian u16) to a future value.
    bytes[4] = bytes[4].wrapping_add(1);
    let err = ZapcodeSnapshot::load(&bytes).unwrap_err().to_string();
    assert!(err.contains("format version"), "unexpected error: {err}");
}

#[test]
fn load_rejects_tampered_payload() {
    let mut bytes = suspended_snapshot().dump().unwrap();
    // Flip the last byte of the stored payload — the sha256 must fail.
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    let err = ZapcodeSnapshot::load(&bytes).unwrap_err().to_string();
    assert!(err.contains("integrity"), "unexpected error: {err}");
}

#[test]
fn load_rejects_wrong_kind() {
    // A session blob must not load as a plain snapshot.
    let session = ZapcodeSessionSnapshot::new(Vec::new(), ResourceLimits::default()).unwrap();
    let session_bytes = session.dump().unwrap();
    let err = ZapcodeSnapshot::load(&session_bytes)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("expected a snapshot blob but got a session blob"),
        "unexpected error: {err}"
    );
}

#[test]
fn large_compressible_state_is_compressed_on_the_wire() {
    // A session holding a big, highly-repetitive string global should dump to
    // far fewer bytes than the raw payload — proving DEFLATE engaged.
    let session = ZapcodeSessionSnapshot::new(Vec::new(), ResourceLimits::default()).unwrap();
    let state = session
        .run_chunk(
            r#"const big = "a".repeat(50000); big.length"#.to_string(),
            Vec::new(),
        )
        .unwrap();
    let bytes = match state {
        zapcode_core::ZapcodeSessionState::Complete { session, .. } => session.dump().unwrap(),
        zapcode_core::ZapcodeSessionState::Suspended { .. } => panic!("expected completion"),
        zapcode_core::ZapcodeSessionState::SuspendedMany { .. } => {
            panic!("unexpected batch suspension")
        }
    };
    // The global alone is 50 KB; a compressed dump of a run of 'a' is tiny.
    assert!(
        bytes.len() < 5000,
        "expected compressed dump well under 5KB, got {} bytes",
        bytes.len()
    );
    // And it still round-trips.
    ZapcodeSessionSnapshot::load(&bytes).unwrap();
}

// ── Decompression-bomb / forged-limit hardening ─────────────────────

use miniz_oxide::deflate::compress_to_vec;
use sha2::{Digest, Sha256};

/// Hand-build a wire frame over arbitrary stored bytes (mirrors `encode_frame`),
/// recomputing the SHA-256 — exactly what an attacker who controls the durable
/// blob can do, since the integrity hash is keyless.
fn forge_frame(kind: u8, compression: u8, stored: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"ZPC1");
    out.extend_from_slice(&8u16.to_le_bytes()); // FORMAT_VERSION
    out.push(kind); // 1 = snapshot, 2 = session
    out.push(compression); // 0 = none, 1 = deflate
    out.extend_from_slice(&Sha256::digest(stored));
    out.extend_from_slice(stored);
    out
}

#[test]
fn load_rejects_decompression_bomb() {
    // A tiny stored payload that inflates to far past the load cap must be
    // rejected during inflation, not allocated in full.
    let huge = vec![0u8; 400 * 1024 * 1024]; // 400MB of zeros -> tiny when DEFLATEd
    let stored = compress_to_vec(&huge, 9);
    assert!(
        stored.len() < 1024 * 1024,
        "the bomb payload should be small (high ratio): {} bytes",
        stored.len()
    );
    let frame = forge_frame(2 /* session */, 1 /* deflate */, &stored);
    let err = ZapcodeSessionSnapshot::load(&frame).unwrap_err().to_string();
    assert!(
        err.contains("decompression bomb") || err.contains("load limit"),
        "expected a decompression-bomb rejection, got: {err}"
    );
}

#[test]
fn load_rejects_oversized_uncompressed_payload() {
    // An uncompressed payload bigger than the load cap is rejected too.
    let stored = vec![0u8; 300 * 1024 * 1024];
    let frame = forge_frame(1 /* snapshot */, 0 /* none */, &stored);
    let err = ZapcodeSnapshot::load(&frame).unwrap_err().to_string();
    assert!(
        err.contains("load limit"),
        "expected an oversized-payload rejection, got: {err}"
    );
}

#[test]
fn loaded_session_limits_are_clamped_to_default() {
    // A session persisted with looser-than-default limits must NOT keep them on
    // reload (a forged blob could otherwise raise its own limits). After reload,
    // the default allocation budget (100k) must be enforced — a 1.5M-iteration
    // allocating loop that the inflated limit would permit must now be rejected.
    let loose = ResourceLimits {
        memory_limit_bytes: 2 * 1024 * 1024 * 1024, // 2GB
        max_allocations: 5_000_000_000,
        max_stack_depth: 1_000_000,
        ..ResourceLimits::default()
    };
    let session = ZapcodeSessionSnapshot::new(Vec::new(), loose).unwrap();
    let bytes = session.dump().unwrap();

    let reloaded = ZapcodeSessionSnapshot::load(&bytes).unwrap();
    let loop_code =
        "let n = 0; for (let i = 0; i < 1500000; i++) { const x = [i]; n = n + x[0]; } n";
    match reloaded.run_chunk(loop_code.to_string(), Vec::new()) {
        Err(zapcode_core::ZapcodeError::AllocationLimitExceeded) => {} // clamped — good
        Err(zapcode_core::ZapcodeError::MemoryLimitExceeded(_)) => {}  // also acceptable
        other => panic!(
            "expected the loaded session's limits to be clamped to default and fire \
             a resource limit, got: {other:?}"
        ),
    }
}
