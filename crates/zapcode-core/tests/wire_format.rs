//! Tests for the versioned, integrity-checked snapshot wire format.

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, Value, ZapcodeRun, ZapcodeSessionSnapshot, ZapcodeSnapshot};

fn suspended_snapshot() -> ZapcodeSnapshot {
    let runner = ZapcodeRun::new(
        r#"const r = fetch("https://example.com");"#.to_string(),
        Vec::new(),
        vec!["fetch".to_string()],
        ResourceLimits::default(),
    )
    .unwrap();
    match runner.start(Vec::new()).unwrap() {
        VmState::Suspended { snapshot, .. } => snapshot,
        VmState::Complete(_) => panic!("expected suspension"),
    }
}

#[test]
fn dump_emits_magic_header() {
    let bytes = suspended_snapshot().dump().unwrap();
    assert_eq!(&bytes[0..4], b"ZPC1", "frame should start with magic bytes");
    // 4 magic + 2 version + 1 kind + 32 sha256 = 39-byte header.
    assert!(bytes.len() > 39);
}

#[test]
fn load_roundtrips_a_real_snapshot() {
    let bytes = suspended_snapshot().dump().unwrap();
    let loaded = ZapcodeSnapshot::load(&bytes).unwrap();
    // Resuming the reloaded snapshot completes the program.
    match loaded.resume(Value::Int(7)).unwrap() {
        VmState::Complete(_) => {}
        VmState::Suspended { .. } => panic!("expected completion after resume"),
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
    // Flip a byte in the payload (after the 39-byte header) — hash must fail.
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
