use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::error::{Result, ZapcodeError};
use crate::sandbox::ResourceLimits;
use crate::value::Value;
use crate::vm::{CallFrame, Continuation, ReceiverSource, TryInfo, Vm, VmState};
use crate::wire::FrameKind;

/// Internal serializable representation of VM state at a suspension point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct VmSnapshot {
    pub(crate) programs: Vec<crate::compiler::CompiledProgram>,
    pub(crate) stack: Vec<Value>,
    pub(crate) frames: Vec<CallFrame>,
    /// User-defined globals only — builtins are re-registered on resume.
    pub(crate) globals: Vec<(String, Value)>,
    pub(crate) try_stack: Vec<TryInfo>,
    pub(crate) continuations: Vec<Continuation>,
    pub(crate) stdout: String,
    pub(crate) limits: ResourceLimits,
    pub(crate) external_functions: Vec<String>,
    pub(crate) next_generator_id: u64,
    pub(crate) last_receiver: Option<Value>,
    pub(crate) last_receiver_source: Option<ReceiverSource>,
    pub(crate) last_global_name: Option<String>,
    pub(crate) last_load_source: Option<ReceiverSource>,
}

impl VmSnapshot {
    pub(crate) fn capture(vm: &Vm) -> Self {
        // Filter out builtin globals — they'll be re-registered on resume.
        // Sort by name: globals live in a HashMap whose iteration order is
        // randomized per-instance, so without sorting two captures of the same
        // logical state would produce different bytes. Deterministic bytes are
        // required for content-addressing, dedup, and snapshot-equality tests.
        let builtin_names: HashSet<&str> = Vm::BUILTIN_GLOBAL_NAMES.iter().copied().collect();
        let mut user_globals: Vec<(String, Value)> = vm
            .globals
            .iter()
            .filter(|(k, _)| !builtin_names.contains(k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        user_globals.sort_by(|a, b| a.0.cmp(&b.0));

        let mut external_functions: Vec<String> = vm.external_functions.iter().cloned().collect();
        external_functions.sort();

        Self {
            programs: vm.programs.clone(),
            stack: vm.stack.clone(),
            frames: vm.frames.clone(),
            globals: user_globals,
            try_stack: vm.try_stack.clone(),
            continuations: vm.continuations.clone(),
            stdout: vm.stdout.clone(),
            limits: vm.limits.clone(),
            external_functions,
            next_generator_id: vm.next_generator_id,
            last_receiver: vm.last_receiver.clone(),
            last_receiver_source: vm.last_receiver_source.clone(),
            last_global_name: vm.last_global_name.clone(),
            last_load_source: vm.last_load_source.clone(),
        }
    }

    pub(crate) fn restore_vm(self) -> Vm {
        let user_globals: HashMap<String, Value> = self.globals.into_iter().collect();
        let ext_set: HashSet<String> = self.external_functions.into_iter().collect();

        Vm::from_snapshot(
            self.programs,
            self.stack,
            self.frames,
            user_globals,
            self.try_stack,
            self.continuations,
            self.stdout,
            self.limits,
            ext_set,
            self.next_generator_id,
            self.last_receiver,
            self.last_receiver_source,
            self.last_global_name,
            self.last_load_source,
        )
    }
}

/// A snapshot of VM state at a suspension point.
/// Can be serialized to bytes and resumed later in any process.
#[derive(Debug, Clone)]
pub struct ZapcodeSnapshot {
    // Boxed so `VmState::Suspended` stays small relative to `Complete` — the
    // captured VM state is large (programs, stack, frames).
    snapshot: Box<VmSnapshot>,
}

impl ZapcodeSnapshot {
    /// Capture the current VM state as a snapshot.
    pub(crate) fn capture(vm: &Vm) -> Result<Self> {
        Ok(Self {
            snapshot: Box::new(VmSnapshot::capture(vm)),
        })
    }

    /// Serialize the snapshot to bytes for storage / transport.
    ///
    /// The bytes are wrapped in a versioned, integrity-checked, compressed frame
    /// (see [`crate::wire`]) so a snapshot persisted by one build is safely
    /// rejected by an incompatible build instead of being silently
    /// misinterpreted.
    pub fn dump(&self) -> Result<Vec<u8>> {
        let payload = postcard::to_allocvec(&self.snapshot)
            .map_err(|e| ZapcodeError::SnapshotError(format!("dump failed: {}", e)))?;
        Ok(crate::wire::encode_frame(FrameKind::Snapshot, &payload))
    }

    /// Deserialize a snapshot from bytes produced by [`Self::dump`].
    pub fn load(bytes: &[u8]) -> Result<Self> {
        let payload = crate::wire::decode_frame(FrameKind::Snapshot, bytes)?;
        let snapshot: VmSnapshot = postcard::from_bytes(&payload)
            .map_err(|e| ZapcodeError::SnapshotError(format!("load failed: {}", e)))?;
        Ok(Self {
            snapshot: Box::new(snapshot),
        })
    }

    /// Resume execution with a return value from the external function.
    /// Returns a `VmState` which may be `Complete` or another `Suspended`.
    pub fn resume(self, return_value: Value) -> Result<VmState> {
        let mut vm = self.snapshot.restore_vm();

        // Push the return value onto the stack — this is the result the
        // `CallExternal` instruction was waiting for.
        vm.stack.push(return_value);

        vm.resume_execution()
    }

    /// Resume execution by raising `error` at the suspended external call,
    /// instead of returning a value. The error is catchable by a surrounding
    /// `try`/`catch` in the guest; if uncaught it propagates to the host. Use
    /// this when a host tool / Temporal activity failed.
    pub fn resume_with_error(self, error: Value) -> Result<VmState> {
        let mut vm = self.snapshot.restore_vm();
        vm.resume_with_error(error)
    }
}
