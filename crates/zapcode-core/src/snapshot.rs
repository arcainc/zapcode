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
    /// Compiled programs, shared behind `Arc` so capture clones a refcount
    /// rather than the whole bytecode (a live snapshot used to own a full deep
    /// copy of every program). `Arc<T>` serializes identically to `T`, so the
    /// wire format is unchanged.
    pub(crate) programs: Vec<std::sync::Arc<crate::compiler::CompiledProgram>>,
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
    /// `console.error` / `console.warn` output, carried alongside `stdout` so a
    /// suspension mid-run preserves both streams across resume. Added at wire
    /// v16; v15 blobs are rejected by the version guard, so no positional
    /// `#[serde(default)]` reconstruction is needed.
    pub(crate) stderr: String,
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
    /// `Vm::builtin_base` — when non-zero, the (reconstructed) heap's first
    /// `builtin_base` slots are this build's builtin template and the
    /// restore reuses the template globals (no re-registration appends).
    #[serde(default)]
    pub(crate) builtin_base: u64,
    /// When true, `heap` holds only the slots PAST the builtin template
    /// (the prefix was byte-identical to the template at capture and is
    /// reconstructed from it on restore). `template_fingerprint` guards
    /// against the template having changed between dump and load.
    #[serde(default)]
    pub(crate) heap_template_elided: bool,
    #[serde(default)]
    pub(crate) template_fingerprint: u64,
    /// Pending `setTimeout` callbacks — carried so a suspension with timers
    /// in flight fires them on resume (deterministic macrotask ordering).
    #[serde(default)]
    pub(crate) timers: Vec<crate::vm::TimerEntry>,
    #[serde(default)]
    pub(crate) next_timer_id: u64,
}

/// Postcard bytes + FNV-1a fingerprint of the builtin-template heap, built
/// once. Capture compares a snapshot heap's prefix against the bytes before
/// eliding it; restore verifies the fingerprint before splicing the template
/// back in.
fn template_heap_bytes() -> &'static (Vec<u8>, u64) {
    static BYTES: std::sync::OnceLock<(Vec<u8>, u64)> = std::sync::OnceLock::new();
    BYTES.get_or_init(|| {
        let bytes = postcard::to_allocvec(&crate::vm::builtin_template().1)
            .expect("builtin template heap serializes");
        let fp = fnv1a(&bytes);
        (bytes, fp)
    })
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

impl VmSnapshot {
    pub(crate) fn capture(vm: &Vm) -> Self {
        Self::capture_with(vm, &mut |_| {})
    }

    /// Like [`Self::capture`], but `extra` walks host-facing values that ride
    /// OUTSIDE the snapshot (the `args`/`calls` of a `VmState::Suspended*`).
    /// They are added as compaction roots and remapped to the compacted
    /// layout — without this, the host would read them against a heap whose
    /// slots moved (or were dropped: a pending call's args are removed from
    /// `pending_calls` before capture, so nothing else roots them).
    pub(crate) fn capture_with(
        vm: &Vm,
        extra: &mut dyn FnMut(&mut dyn FnMut(&mut crate::heap::Handle)),
    ) -> Self {
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

        let mut snapshot = Self {
            programs: vm.programs.clone(),
            stack: vm.stack.clone(),
            frames: vm.frames.clone(),
            cells: vm.cells.clone(),
            globals: user_globals,
            try_stack: vm.try_stack.clone(),
            continuations: vm.continuations.clone(),
            stdout: vm.stdout.clone(),
            stderr: vm.stderr.clone(),
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
            builtin_base: vm.builtin_base as u64,
            heap_template_elided: false,
            template_fingerprint: 0,
            timers: vm.timers.clone(),
            next_timer_id: vm.next_timer_id,
        };
        // Drop dead heap slots first (the arena never frees during
        // execution, so a churning agent otherwise persists every dead
        // temporary in every snapshot, forever), THEN elide the template
        // prefix the compactor deliberately retained.
        snapshot.compact_heap(extra);
        snapshot.elide_template();
        snapshot
    }

    /// Visit every heap handle the snapshot serializes OUTSIDE the heap
    /// itself — the compactor's roots and rewrite targets. Every Value- or
    /// Handle-bearing field must be walked here; missing one corrupts that
    /// field on restore (its handle would dangle or point at the wrong
    /// slot), so additions to `VmSnapshot` carrying values belong here too.
    fn for_each_handle_mut(&mut self, f: &mut dyn FnMut(&mut crate::heap::Handle)) {
        fn walk_frame(fr: &mut CallFrame, f: &mut dyn FnMut(&mut crate::heap::Handle)) {
            for v in &mut fr.locals {
                v.for_each_handle_mut(f);
            }
            if let Some(v) = &mut fr.this_value {
                v.for_each_handle_mut(f);
            }
            if let Some(v) = &mut fr.async_result {
                v.for_each_handle_mut(f);
            }
        }
        for v in &mut self.stack {
            v.for_each_handle_mut(f);
        }
        for fr in &mut self.frames {
            walk_frame(fr, f);
        }
        for v in &mut self.cells {
            v.for_each_handle_mut(f);
        }
        for (_, v) in &mut self.globals {
            v.for_each_handle_mut(f);
        }
        for c in &mut self.continuations {
            match c {
                Continuation::ArrayMap {
                    callback,
                    source,
                    results,
                    ..
                } => {
                    callback.for_each_handle_mut(f);
                    for v in source.iter_mut().chain(results.iter_mut()) {
                        v.for_each_handle_mut(f);
                    }
                }
                Continuation::ArrayForEach {
                    callback, source, ..
                } => {
                    callback.for_each_handle_mut(f);
                    for v in source.iter_mut() {
                        v.for_each_handle_mut(f);
                    }
                }
                Continuation::MicrotaskReaction {
                    result_promise,
                    original_value,
                    ..
                } => {
                    result_promise.for_each_handle_mut(f);
                    original_value.for_each_handle_mut(f);
                }
                Continuation::GeneratorNext { gen_obj, .. } => {
                    gen_obj.for_each_handle_mut(f);
                }
                Continuation::PromiseExecutor { promise, .. } => {
                    promise.for_each_handle_mut(f);
                }
            }
        }
        if let Some(v) = &mut self.last_receiver {
            v.for_each_handle_mut(f);
        }
        for pc in &mut self.pending_calls {
            for v in &mut pc.args {
                v.for_each_handle_mut(f);
            }
        }
        for v in self.resolved.values_mut() {
            v.for_each_handle_mut(f);
        }
        if let Some(b) = &mut self.pending_batch {
            for v in &mut b.items {
                v.for_each_handle_mut(f);
            }
        }
        match &mut self.resume_action {
            Some(ResumeAction::PromiseMethod { args, .. }) => {
                for v in args {
                    v.for_each_handle_mut(f);
                }
            }
            Some(ResumeAction::SettleResult {
                result_promise,
                pass_original,
            }) => {
                result_promise.for_each_handle_mut(f);
                if let Some((v, _)) = pass_original {
                    v.for_each_handle_mut(f);
                }
            }
            Some(ResumeAction::CacheValue { .. }) | None => {}
        }
        for m in &mut self.microtasks {
            m.handler.for_each_handle_mut(f);
            m.value.for_each_handle_mut(f);
            m.result_promise.for_each_handle_mut(f);
        }
        for h in &mut self.unhandled_rejections {
            f(h);
        }
        for t in self.async_tasks.values_mut() {
            walk_frame(&mut t.frame, f);
            for v in &mut t.stack {
                v.for_each_handle_mut(f);
            }
        }
        for t in &mut self.timers {
            t.callback.for_each_handle_mut(f);
        }
    }

    /// Snapshot-time GC: mark from every serialized handle, compact the heap
    /// (template prefix always retained), rewrite all handles to the new
    /// layout. Order-preserving, so bytes stay deterministic.
    fn compact_heap(&mut self, extra: &mut dyn FnMut(&mut dyn FnMut(&mut crate::heap::Handle))) {
        let mut roots: Vec<crate::heap::Handle> = Vec::new();
        self.for_each_handle_mut(&mut |h| roots.push(*h));
        extra(&mut |h| roots.push(*h));
        let remap = self
            .heap
            .compact_retaining(self.builtin_base as usize, &roots);
        let mut rewrite = |h: &mut crate::heap::Handle| {
            let new = remap[*h as usize];
            debug_assert_ne!(new, crate::heap::Handle::MAX, "compactor dropped a rooted slot");
            *h = new;
        };
        self.for_each_handle_mut(&mut rewrite);
        extra(&mut rewrite);
    }

    /// Splice the template back in front of an elided heap so handles in the
    /// serialized state resolve against `self.heap` directly. The inverse of
    /// [`Self::elide_template`]; idempotent.
    pub(crate) fn materialize_heap(&mut self) {
        if self.heap_template_elided {
            let tail = std::mem::take(&mut self.heap);
            self.heap = Heap::with_template_prefix(&crate::vm::builtin_template().1, tail);
            self.heap_template_elided = false;
            self.template_fingerprint = 0;
        }
    }

    /// Elide the builtin-template prefix when it is still byte-identical to
    /// the template (the overwhelmingly common case — guest code rarely
    /// mutates builtin objects). A mutated prefix, or a heap seeded with
    /// caller data (`builtin_base == 0`), serializes in full.
    fn elide_template(&mut self) {
        let template = template_heap_bytes();
        if self.builtin_base > 0
            && postcard::to_allocvec(&self.heap.prefix(self.builtin_base as usize)).as_deref()
                == Ok(template.0.as_slice())
        {
            self.heap = self.heap.tail_from(self.builtin_base as usize);
            self.heap_template_elided = true;
            self.template_fingerprint = template.1;
        }
    }

    pub(crate) fn restore_vm(self) -> Result<Vm> {
        let user_globals: HashMap<String, Value> = self.globals.into_iter().collect();
        let ext_set: HashSet<String> = self.external_functions.into_iter().collect();

        // A template-elided heap splices this build's template back in front
        // of the serialized tail. The fingerprint catches a builtin set that
        // changed without a wire-format bump — restoring against a different
        // template would silently corrupt every handle in the prefix.
        let heap = if self.heap_template_elided {
            let template = template_heap_bytes();
            if self.template_fingerprint != template.1 {
                return Err(ZapcodeError::SnapshotError(
                    "snapshot was captured against a different builtin set \
                     (template fingerprint mismatch)"
                        .to_string(),
                ));
            }
            Heap::with_template_prefix(&crate::vm::builtin_template().1, self.heap)
        } else {
            self.heap
        };

        let mut vm = Vm::from_snapshot(
            self.programs,
            self.stack,
            self.frames,
            user_globals,
            self.try_stack,
            self.continuations,
            self.stdout,
            self.stderr,
            self.limits,
            ext_set,
            self.next_generator_id,
            self.last_receiver,
            self.last_receiver_source,
            self.last_global_name,
            self.last_load_source,
            heap,
            self.builtin_base as usize,
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
        vm.timers = self.timers;
        vm.next_timer_id = self.next_timer_id;
        Ok(vm)
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

    /// Unwrap into the inner [`VmSnapshot`] — used by the session layer to
    /// REUSE the capture taken at suspension time (whose compaction the
    /// suspension's `args`/`calls` are already remapped against) instead of
    /// re-capturing and double-remapping.
    pub(crate) fn into_vm_snapshot(self) -> VmSnapshot {
        *self.snapshot
    }

    /// Capture with a caller-supplied walk over host-facing values that must
    /// survive compaction (see [`VmSnapshot::capture_with`]).
    pub(crate) fn capture_with(
        vm: &Vm,
        extra: &mut dyn FnMut(&mut dyn FnMut(&mut crate::heap::Handle)),
    ) -> Result<Self> {
        Ok(Self {
            snapshot: Box::new(VmSnapshot::capture_with(vm, extra)),
        })
    }

    /// Capture, treating `values` (the host-facing suspension args) as extra
    /// compaction roots and remapping them to the compacted heap layout.
    pub(crate) fn capture_with_values(vm: &Vm, values: &mut [Value]) -> Result<Self> {
        Ok(Self {
            snapshot: Box::new(VmSnapshot::capture_with(vm, &mut |f| {
                for v in values.iter_mut() {
                    v.for_each_handle_mut(f);
                }
            })),
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

    /// Splice the builtin template back in front of a template-elided heap
    /// so the public accessors hand out a heap whose handles match what the
    /// restored VM will see. Host-allocated handles (see [`Self::heap_mut`])
    /// would otherwise be tail-relative and dangle after reconstruction.
    /// Capture re-elides the (unchanged) prefix on the next dump.
    fn materialize_heap(&mut self) {
        self.snapshot.materialize_heap();
    }

    /// Borrow the snapshot's object heap. A `Value::Array`/`Value::Object`
    /// returned in a suspension's `args` carries a `Handle` into this heap;
    /// use it to read array elements or object fields of those arguments.
    pub fn heap(&mut self) -> &Heap {
        self.materialize_heap();
        &self.snapshot.heap
    }

    /// Mutably borrow the snapshot's object heap so a host can allocate a
    /// compound return value (array/object) into the same heap before passing
    /// the resulting `Value` (and its valid `Handle`) to one of the `resume`
    /// methods. Primitive return values need no allocation.
    pub fn heap_mut(&mut self) -> &mut Heap {
        self.materialize_heap();
        &mut self.snapshot.heap
    }

    /// Resume execution with a return value from the external function.
    /// Returns a [`RunResult`] whose `state` may be `Complete` or another
    /// `Suspended`, and whose `heap` resolves any handles in that state.
    pub fn resume(self, return_value: Value) -> Result<RunResult> {
        let mut vm = self.snapshot.restore_vm()?;

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
        let mut vm = self.snapshot.restore_vm()?;
        let state = vm.resume_with_error(error)?;
        Ok(RunResult::from_resume(state, vm))
    }

    /// Resume a batch suspension (`VmState::SuspendedMany`) with one result per
    /// call, in the order the calls were presented. The host can run those
    /// calls in parallel and pass back all results at once.
    pub fn resume_many(self, results: Vec<Value>) -> Result<RunResult> {
        let mut vm = self.snapshot.restore_vm()?;
        let state = vm.resume_many(results)?;
        Ok(RunResult::from_resume(state, vm))
    }
}

#[cfg(test)]
mod template_elision {
    use super::*;
    use crate::vm::VmState;
    use crate::ZapcodeRun;

    fn suspended() -> ZapcodeSnapshot {
        let runner = ZapcodeRun::new(
            "async function main() { return await callTool('x'); } main();".to_string(),
            Vec::new(),
            vec!["callTool".to_string()],
            ResourceLimits::default(),
        )
        .unwrap();
        match runner.start(Vec::new()).unwrap() {
            VmState::Suspended { snapshot, .. } => snapshot,
            other => panic!("expected suspension, got {other:?}"),
        }
    }

    #[test]
    fn fresh_heap_snapshot_elides_the_template() {
        let snap = suspended();
        assert!(snap.snapshot.heap_template_elided);
        assert_eq!(snap.snapshot.template_fingerprint, template_heap_bytes().1);
        // The serialized heap is only the guest tail — far smaller than the
        // template it elides.
        let tail_bytes = postcard::to_allocvec(&snap.snapshot.heap).unwrap();
        assert!(
            tail_bytes.len() < template_heap_bytes().0.len() / 4,
            "tail {} bytes vs template {}",
            tail_bytes.len(),
            template_heap_bytes().0.len()
        );
    }

    #[test]
    fn tampered_template_fingerprint_refuses_to_restore() {
        // A fingerprint mismatch means the snapshot's elided prefix came from
        // a DIFFERENT builtin set; splicing this build's template in would
        // silently corrupt every handle into the prefix. It must error.
        let mut snap = suspended();
        assert!(snap.snapshot.heap_template_elided);
        snap.snapshot.template_fingerprint ^= 0xdead_beef;
        let err = snap.resume(Value::Int(1)).unwrap_err();
        assert!(
            err.to_string().contains("different builtin set"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn heap_mut_materializes_so_host_handles_stay_valid() {
        let mut snap = suspended();
        assert!(snap.snapshot.heap_template_elided);
        let template_len = crate::vm::builtin_template().1.len();
        let handle = snap.heap_mut().alloc_object(indexmap::IndexMap::new());
        // The handle is full-heap-relative (past the template), not
        // tail-relative — the restored VM sees the same heap.
        assert!(snap.snapshot.heap.len() > template_len);
        assert!((handle as usize) >= template_len);
        assert!(!snap.snapshot.heap_template_elided);
    }
}
