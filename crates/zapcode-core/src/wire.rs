//! Versioned, integrity-checked envelope for serialized VM state.
//!
//! Every snapshot/session that crosses a process or storage boundary (Temporal
//! activities, durable queues, disk) is wrapped in a self-describing frame:
//!
//! ```text
//! [ MAGIC "ZPC1" (4) ][ format_version u16 LE (2) ][ kind u8 (1) ][ compression u8 (1) ][ sha256 (32) ][ stored payload ]
//! ```
//!
//! The `stored payload` is the postcard bytes, optionally DEFLATE-compressed
//! (the `compression` byte says which). The sha256 covers the stored bytes, so
//! integrity is verified *before* decompression. Compression is only applied
//! when it actually shrinks the payload, so tiny snapshots never grow.
//!
//! This buys three things we need for durable execution:
//!
//! 1. **Version safety.** A snapshot persisted by one build can be resumed by a
//!    later build that may have changed the `Value`/bytecode layout. postcard is
//!    not self-describing, so without a version tag a layout change silently
//!    misinterprets bytes. We hard-reject mismatched versions instead.
//! 2. **Integrity / tamper detection.** Loading a snapshot is untrusted-input
//!    deserialization. The SHA-256 over the payload lets us reject corrupted or
//!    tampered bytes before handing them to postcard. (Borrowed from Monty's
//!    `[version][sha256][payload]` wire format.)
//! 3. **Type discrimination.** The `kind` byte means loading a *session* blob as
//!    a plain *snapshot* (or vice versa) fails with a clear error rather than a
//!    confusing postcard decode error deep in the wrong type.

use miniz_oxide::deflate::compress_to_vec;
use miniz_oxide::inflate::decompress_to_vec;
use sha2::{Digest, Sha256};

use crate::error::{Result, ZapcodeError};

const MAGIC: &[u8; 4] = b"ZPC1";

/// Bump on any breaking change to the serialized layout of `Value`,
/// `CompiledProgram`, the VM frame/continuation types, or the snapshot structs.
/// v2: `Value::Array`/`Value::Object` now carry a `Handle` into the heap, and
/// the snapshot carries the heap itself — incompatible with v1 byte layouts.
pub(crate) const FORMAT_VERSION: u16 = 2;

const HEADER_LEN: usize = 4 + 2 + 1 + 1 + 32;

const COMPRESSION_NONE: u8 = 0;
const COMPRESSION_DEFLATE: u8 = 1;
// DEFLATE level 6 — solid ratio without the cost of max compression.
const DEFLATE_LEVEL: u8 = 6;

/// Distinguishes the kind of payload carried in a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameKind {
    Snapshot = 1,
    Session = 2,
}

impl FrameKind {
    fn label(self) -> &'static str {
        match self {
            FrameKind::Snapshot => "snapshot",
            FrameKind::Session => "session",
        }
    }

    fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(FrameKind::Snapshot),
            2 => Some(FrameKind::Session),
            _ => None,
        }
    }
}

/// Reject serialized VM state that exceeds the memory budget — a backstop
/// against a runaway session producing a snapshot too big to move between
/// processes / Temporal activities.
pub(crate) fn check_state_size(payload_len: usize, max_bytes: usize) -> Result<()> {
    if payload_len > max_bytes {
        return Err(ZapcodeError::SnapshotError(format!(
            "serialized state is {} bytes, exceeding the {}-byte limit; \
             the session state has grown too large to persist",
            payload_len, max_bytes
        )));
    }
    Ok(())
}

/// Wrap a postcard payload in a versioned, hashed, optionally-compressed frame.
pub(crate) fn encode_frame(kind: FrameKind, payload: &[u8]) -> Vec<u8> {
    // Compress, but only keep it if it actually shrinks the payload — small
    // snapshots shouldn't pay DEFLATE overhead.
    let compressed = compress_to_vec(payload, DEFLATE_LEVEL);
    let (compression, stored): (u8, &[u8]) = if compressed.len() < payload.len() {
        (COMPRESSION_DEFLATE, &compressed)
    } else {
        (COMPRESSION_NONE, payload)
    };

    let mut out = Vec::with_capacity(HEADER_LEN + stored.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.push(kind as u8);
    out.push(compression);
    out.extend_from_slice(&Sha256::digest(stored));
    out.extend_from_slice(stored);
    out
}

/// Validate a frame and return the inner (decompressed) postcard payload.
///
/// Rejects (with actionable errors) a bad magic, a format-version mismatch, a
/// wrong payload kind, a payload whose SHA-256 doesn't match the header, or an
/// undecompressable payload. The hash is checked before decompression.
pub(crate) fn decode_frame(expected: FrameKind, bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.len() < HEADER_LEN {
        return Err(ZapcodeError::SnapshotError(format!(
            "{} blob is too short to contain a header ({} bytes)",
            expected.label(),
            bytes.len()
        )));
    }

    let (magic, rest) = bytes.split_at(4);
    if magic != MAGIC {
        return Err(ZapcodeError::SnapshotError(
            "not a Zapcode snapshot (bad magic bytes)".to_string(),
        ));
    }

    let (version_bytes, rest) = rest.split_at(2);
    let version = u16::from_le_bytes([version_bytes[0], version_bytes[1]]);
    if version != FORMAT_VERSION {
        return Err(ZapcodeError::SnapshotError(format!(
            "snapshot format version {} is not supported by this build (expected {}); \
             it was produced by an incompatible version of Zapcode",
            version, FORMAT_VERSION
        )));
    }

    let (kind_byte, rest) = rest.split_at(1);
    let kind = FrameKind::from_byte(kind_byte[0]).ok_or_else(|| {
        ZapcodeError::SnapshotError(format!("unknown snapshot kind byte {}", kind_byte[0]))
    })?;
    if kind != expected {
        return Err(ZapcodeError::SnapshotError(format!(
            "expected a {} blob but got a {} blob",
            expected.label(),
            kind.label()
        )));
    }

    let (compression_byte, rest) = rest.split_at(1);
    let (expected_hash, stored) = rest.split_at(32);
    let actual_hash = Sha256::digest(stored);
    if actual_hash.as_slice() != expected_hash {
        return Err(ZapcodeError::SnapshotError(
            "snapshot integrity check failed (sha256 mismatch); the bytes are corrupted or tampered"
                .to_string(),
        ));
    }

    match compression_byte[0] {
        COMPRESSION_NONE => Ok(stored.to_vec()),
        COMPRESSION_DEFLATE => decompress_to_vec(stored).map_err(|e| {
            ZapcodeError::SnapshotError(format!("snapshot decompression failed: {}", e))
        }),
        other => Err(ZapcodeError::SnapshotError(format!(
            "unknown snapshot compression byte {}",
            other
        ))),
    }
}
