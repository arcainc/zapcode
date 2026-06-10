use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::error::{Result, ZapcodeError};
use crate::heap::Heap;
use crate::sandbox::ResourceLimits;
use crate::value::Value;
use crate::vm::{
    CallFrame, Continuation, PendingBatch, PendingExternalCall, ReceiverSource, ResumeAction,
    RunResult, TryInfo, Vm,
};
use crate::wire::FrameKind;

/// Internal serializable representation of VM state at a suspension point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct VmSnapshot {
    pub(crate) programs: Vec<crate::compiler::CompiledProgram>,
    pub(crate) stack: Vec<Value>,
    pub(crate) frames: Vec<CallFrame>,
    /// Shared upvalue cells (captured variables). Ids are indices; sharing is
    /// preserved because closures/frames reference the same id.
    #[serde(default)]
    pub(crate) cells: Vec<Value>,
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
    /// Deferred external calls awaiting batch resolution (`Promise.all`).
    pub(crate) pending_calls: Vec<PendingExternalCall>,
    pub(crate) resolved: BTreeMap<u64, Value>,
    pub(crate) next_call_id: u64,
    pub(crate) pending_batch: Option<PendingBatch>,
    /// Deferred action for resuming a single-call-promise suspension (N5). New
    /// field — defaults to `None` so snapshots written before N5 still load.
    #[serde(default)]
    pub(crate) resume_action: Option<ResumeAction>,
    /// Cumulative allocation count, carried across resumes so a long-running
    /// suspend/resume chain can't evade `max_allocations` by resetting it.
    pub(crate) allocations: usize,
    /// Deterministic PRNG state for `Math.random`, carried so the random
    /// sequence is stable across a dump/load/resume.
    #[serde(default)]
    pub(crate) rng_state: u64,
    /// The object heap: backing store for all array/object values referenced by
    /// handles in `stack`/`frames`/`globals`/`cells`. Must travel with the
    /// snapshot or those handles dangle on resume.
    #[serde(default)]
    pub(crate) heap: crate::heap::Heap,
    /// The microtask (Promise-reaction) queue, carried so a suspension that
    /// happens mid-drain (a host call inside a `.then`/`await` continuation)
    /// resumes with the remaining reactions intact.
    #[serde(default)]
    pub(crate) microtasks: std::collections::VecDeque<crate::vm::Microtask>,
    /// Rejected promises with no handler at settle time (see
    /// `Vm::unhandled_rejections`) — carried so a suspension mid-drain still
    /// reports the rejection deterministically at end-of-drain after resume.
    #[serde(default)]
    pub(crate) unhandled_rejections: Vec<crate::heap::Handle>,
    /// Async function calls parked at an `await` (Stage 3) — restored so a
    /// host-call suspension with tasks in flight resumes them when their
    /// awaited promises settle.
    #[serde(default)]
    pub(crate) async_tasks: BTreeMap<u64, crate::vm::AsyncTask>,
    #[serde(default)]
    pub(crate) next_async_task_id: u64,
    /// Try-frames covering suspended generator bodies (see
    /// `Vm::generator_try_frames`).
    #[serde(default)]
    pub(crate) generator_try_frames: BTreeMap<u64, Vec<TryInfo>>,
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
            cells: vm.cells.clone(),
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
            pending_calls: vm.pending_calls.clone(),
            resolved: vm.resolved.clone(),
            next_call_id: vm.next_call_id,
            pending_batch: vm.pending_batch.clone(),
            resume_action: vm.resume_action.clone(),
            allocations: vm.tracker.allocations,
            rng_state: vm.rng_state,
            heap: vm.heap.clone(),
            microtasks: vm.microtasks.clone(),
            unhandled_rejections: vm.unhandled_rejections.clone(),
            async_tasks: vm.async_tasks.clone(),
            next_async_task_id: vm.next_async_task_id,
            generator_try_frames: vm.generator_try_frames.clone(),
        }
    }

    pub(crate) fn restore_vm(self) -> Vm {
        let user_globals: HashMap<String, Value> = self.globals.into_iter().collect();
        let ext_set: HashSet<String> = self.external_functions.into_iter().collect();

        let mut vm = Vm::from_snapshot(
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
            self.heap,
        );
        // Restore batched-call state (kept off the long from_snapshot signature).
        vm.pending_calls = self.pending_calls;
        vm.resolved = self.resolved;
        vm.next_call_id = self.next_call_id;
        vm.pending_batch = self.pending_batch;
        vm.resume_action = self.resume_action;
        vm.tracker.allocations = self.allocations;
        vm.rng_state = self.rng_state;
        vm.cells = self.cells;
        vm.microtasks = self.microtasks;
        vm.unhandled_rejections = self.unhandled_rejections;
        vm.async_tasks = self.async_tasks;
        vm.next_async_task_id = self.next_async_task_id;
        vm.generator_try_frames = self.generator_try_frames;
        vm
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
        // Guard against unbounded serialized state — a runaway session shouldn't
        // produce a snapshot too large to pass between activities.
        crate::wire::check_state_size(payload.len(), self.snapshot.limits.max_snapshot_bytes)?;
        Ok(crate::wire::encode_frame(FrameKind::Snapshot, &payload))
    }

    /// Deserialize a snapshot from bytes produced by [`Self::dump`].
    pub fn load(bytes: &[u8]) -> Result<Self> {
        let payload = crate::wire::decode_frame(
            FrameKind::Snapshot,
            bytes,
            crate::wire::MAX_LOAD_DECOMPRESSED_BYTES,
        )?;
        let mut snapshot: VmSnapshot = postcard::from_bytes(&payload)
            .map_err(|e| ZapcodeError::SnapshotError(format!("load failed: {}", e)))?;
        // Clamp untrusted, snapshot-embedded resource limits down to safe
        // defaults so a forged/tampered blob can't raise its own limits to
        // bypass sandbox enforcement on resume (the wire SHA is keyless — it
        // detects corruption, not forgery).
        snapshot.limits.clamp_to_default();
        Ok(Self {
            snapshot: Box::new(snapshot),
        })
    }

    /// Borrow the snapshot's object heap. A `Value::Array`/`Value::Object`
    /// returned in a suspension's `args` carries a `Handle` into this heap;
    /// use it to read array elements or object fields of those arguments.
    pub fn heap(&self) -> &Heap {
        &self.snapshot.heap
    }

    /// Mutably borrow the snapshot's object heap so a host can allocate a
    /// compound return value (array/object) into the same heap before passing
    /// the resulting `Value` (and its valid `Handle`) to one of the `resume`
    /// methods. Primitive return values need no allocation.
    pub fn heap_mut(&mut self) -> &mut Heap {
        &mut self.snapshot.heap
    }

    /// Resume execution with a return value from the external function.
    /// Returns a [`RunResult`] whose `state` may be `Complete` or another
    /// `Suspended`, and whose `heap` resolves any handles in that state.
    pub fn resume(self, return_value: Value) -> Result<RunResult> {
        let mut vm = self.snapshot.restore_vm();

        // Push the return value onto the stack — this is the result the
        // `CallExternal` instruction was waiting for.
        vm.stack.push(return_value);

        let state = vm.resume_execution()?;
        Ok(RunResult::from_resume(state, vm))
    }

    /// Resume execution by raising `error` at the suspended external call,
    /// instead of returning a value. The error is catchable by a surrounding
    /// `try`/`catch` in the guest; if uncaught it propagates to the host. Use
    /// this when a host tool / Temporal activity failed.
    pub fn resume_with_error(self, error: Value) -> Result<RunResult> {
        let mut vm = self.snapshot.restore_vm();
        let state = vm.resume_with_error(error)?;
        Ok(RunResult::from_resume(state, vm))
    }

    /// Resume a batch suspension (`VmState::SuspendedMany`) with one result per
    /// call, in the order the calls were presented. The host can run those
    /// calls in parallel and pass back all results at once.
    pub fn resume_many(self, results: Vec<Value>) -> Result<RunResult> {
        let mut vm = self.snapshot.restore_vm();
        let state = vm.resume_many(results)?;
        Ok(RunResult::from_resume(state, vm))
    }
}
