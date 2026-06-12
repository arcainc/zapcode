//! Compile-once / run-many programs.
//!
//! [`crate::vm::ZapcodeRun`] re-parses and re-compiles its source on every
//! `run()` / `start()`. For hosts that execute the same agent program many
//! times (different inputs, retries, fan-out workers), [`ZapcodeProgram`]
//! front-loads parse + compile once and reuses the compiled bytecode for every
//! run, skipping the dominant fixed cost of the pipeline.
//!
//! The compiled form can also be persisted with [`ZapcodeProgram::dump`] /
//! [`ZapcodeProgram::load`], wrapped in the same versioned, integrity-checked
//! wire frame as snapshots (see [`crate::wire`]). A build that changed the
//! bytecode layout rejects a stale cached program by format version instead of
//! misinterpreting its bytes, and a corrupted blob fails the SHA-256 check.
//!
//! Security note: a program blob is *code*, not untrusted data — loading and
//! running a blob executes its bytecode, exactly as running the original
//! source would. The sandbox invariants still hold (the VM grants no host
//! access, and the `ResourceLimits` are supplied by the host at run time, not
//! embedded in the blob), but hosts should treat cached program blobs with the
//! same trust level as the source they were compiled from.

use std::collections::HashSet;

use crate::compiler::CompiledProgram;
use crate::error::{Result, ZapcodeError};
use crate::heap::Heap;
use crate::sandbox::ResourceLimits;
use crate::trace::SpanBuilder;
use crate::value::Value;
use crate::vm::{execute_compiled, RunResult, VmState};
use crate::wire::FrameKind;

/// A parsed + compiled Zapcode program, ready to run any number of times.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ZapcodeProgram {
    pub(crate) compiled: CompiledProgram,
    /// External function names registered at compile time. Compilation decides
    /// which identifiers lower to external (suspending) calls, so the set is
    /// fixed per compiled program and travels with the bytecode.
    pub(crate) external_functions: Vec<String>,
    /// The original source, kept (and serialized in [`Self::dump`] blobs) so
    /// uncaught runtime errors can render their one-line code frame even when
    /// running from a cached program. Program blobs already carry the same
    /// trust level as source.
    #[serde(default)]
    pub(crate) source: String,
}

impl ZapcodeProgram {
    /// Parse and compile `source` once. The resulting program can be run many
    /// times (and dumped / loaded) without re-paying parse + compile.
    pub fn compile(source: &str, external_functions: Vec<String>) -> Result<Self> {
        let program = crate::parser::parse(source)?;
        let ext_set: HashSet<String> = external_functions.iter().cloned().collect();
        let compiled = crate::compiler::compile_with_externals(&program, ext_set)?;
        Ok(Self {
            compiled,
            external_functions,
            source: source.to_string(),
        })
    }

    /// External function names this program was compiled against.
    pub fn external_functions(&self) -> &[String] {
        &self.external_functions
    }

    /// Serialize the compiled program for storage / transport, wrapped in the
    /// versioned, integrity-checked wire frame (kind: program).
    pub fn dump(&self) -> Result<Vec<u8>> {
        let payload = postcard::to_allocvec(self)
            .map_err(|e| ZapcodeError::SnapshotError(format!("program dump failed: {}", e)))?;
        // Same backstop as snapshots: don't emit a blob too large to move
        // between processes / activities.
        crate::wire::check_state_size(
            payload.len(),
            ResourceLimits::default().max_snapshot_bytes,
        )?;
        Ok(crate::wire::encode_frame(FrameKind::Program, &payload))
    }

    /// Deserialize a program from bytes produced by [`Self::dump`]. Rejects
    /// blobs from an incompatible format version, of the wrong kind (snapshot /
    /// session), or with a failing integrity hash.
    pub fn load(bytes: &[u8]) -> Result<Self> {
        let payload = crate::wire::decode_frame(
            FrameKind::Program,
            bytes,
            crate::wire::MAX_LOAD_DECOMPRESSED_BYTES,
        )?;
        postcard::from_bytes(&payload)
            .map_err(|e| ZapcodeError::SnapshotError(format!("program load failed: {}", e)))
    }

    /// Run the pre-compiled program. Mirrors [`crate::vm::ZapcodeRun::run`] but
    /// skips parse + compile; `limits` are per-run, supplied by the host.
    pub fn run(
        &self,
        input_values: Vec<(String, Value)>,
        limits: ResourceLimits,
    ) -> Result<RunResult> {
        self.run_with_input_heap(input_values, Heap::new(), limits)
    }

    /// Like [`Self::run`], but seeds the VM with `input_heap` — the heap that
    /// backs any array/object `Value`s in `input_values` (see
    /// [`crate::vm::ZapcodeRun::run_with_input_heap`]).
    pub fn run_with_input_heap(
        &self,
        input_values: Vec<(String, Value)>,
        input_heap: Heap,
        limits: ResourceLimits,
    ) -> Result<RunResult> {
        // One clone of the cached bytecode per run — far cheaper than the
        // parse + compile it replaces, and the VM needs an owned program.
        self.clone()
            .run_consuming(input_values, input_heap, limits, SpanBuilder::new("zapcode.run"))
    }

    /// Start execution, returning the raw `VmState` (the suspension /
    /// snapshot / resume entry point). Mirrors [`crate::vm::ZapcodeRun::start`].
    pub fn start(
        &self,
        input_values: Vec<(String, Value)>,
        limits: ResourceLimits,
    ) -> Result<VmState> {
        Ok(self.run(input_values, limits)?.state)
    }

    /// Execute, consuming `self` to hand the VM an owned program without a
    /// clone. `root_span` may already carry parse/compile child spans
    /// (`ZapcodeRun` path) or be fresh (pre-compiled path).
    pub(crate) fn run_consuming(
        self,
        input_values: Vec<(String, Value)>,
        input_heap: Heap,
        limits: ResourceLimits,
        root_span: SpanBuilder,
    ) -> Result<RunResult> {
        let ext_set: HashSet<String> = self.external_functions.iter().cloned().collect();
        let source = if self.source.is_empty() {
            None
        } else {
            Some(self.source.clone())
        };
        execute_compiled(
            self.compiled,
            ext_set,
            limits,
            input_values,
            input_heap,
            source,
            root_span,
        )
    }
}
