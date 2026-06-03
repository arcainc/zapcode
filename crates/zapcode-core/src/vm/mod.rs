use std::collections::{BTreeMap, HashMap, HashSet};
use std::mem::size_of;
use std::sync::Arc;

use indexmap::IndexMap;

use crate::compiler::instruction::{BatchKind, Constant, Instruction};
use crate::compiler::CompiledProgram;
use crate::error::{Result, ZapcodeError};
use crate::heap::{Handle, Heap};
use crate::sandbox::{ResourceLimits, ResourceTracker};
use crate::snapshot::ZapcodeSnapshot;
use crate::trace::{ExecutionTrace, SpanBuilder, TraceStatus};
use crate::value::{
    Closure, FunctionRef, GeneratorObject, Place, PlaceRoot, PlaceSeg, SuspendedFrame, Value,
};

mod builtins;

/// The result of VM execution.
#[derive(Debug)]
pub enum VmState {
    Complete(Value),
    Suspended {
        function_name: String,
        args: Vec<Value>,
        snapshot: ZapcodeSnapshot,
    },
    /// Suspended on a batch of external calls (`Promise.{all,race,any,
    /// allSettled}([...])`) that the host can run in parallel. `combinator`
    /// tells the host which `Promise.*` settle semantics to apply. Resume with
    /// `resume_many` passing the settled values the combinator produced (for
    /// `all`/`allSettled` one entry per call in order; for `race`/`any` a single
    /// entry), or `resume_with_error` on rejection.
    SuspendedMany {
        calls: Vec<ExternalCall>,
        combinator: BatchKind,
        snapshot: ZapcodeSnapshot,
    },
}

/// One pending external call exposed to the host in a batch suspension.
#[derive(Debug, Clone)]
pub struct ExternalCall {
    pub name: String,
    pub args: Vec<Value>,
}

/// A deferred external call recorded by `CallExternalDeferred`, awaiting batch
/// resolution.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct PendingExternalCall {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) args: Vec<Value>,
}

/// The structure of an in-flight `Promise.*` batch, captured at the await so
/// resume can assemble the final promise value per the combinator.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct PendingBatch {
    /// Which combinator produced this batch (`all`/`race`/`any`/`allSettled`).
    /// Defaults to `All` so snapshots written before this field existed still
    /// load with the historical behavior.
    #[serde(default = "batch_kind_all")]
    pub(crate) kind: BatchKind,
    /// Original array elements (some are `Value::Pending`).
    pub(crate) items: Vec<Value>,
    /// Call ids being awaited, in the order presented to the host.
    pub(crate) call_ids: Vec<u64>,
}

fn batch_kind_all() -> BatchKind {
    BatchKind::All
}

/// The result of classifying an already-settled batch element.
enum SettledOutcome {
    Fulfilled(Value),
    Rejected(Value),
}

/// Tracks where a method receiver originated so that mutations to `this`
/// inside the method can be written back to the source variable.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) enum ReceiverSource {
    /// The receiver was loaded from a global variable with the given name.
    Global(String),
    /// The receiver was loaded from a local variable at the given slot index
    /// in the frame at the given depth (index into `self.frames`).
    Local { frame_index: usize, slot: usize },
    /// The receiver was loaded from a captured variable held in a shared cell.
    Cell(u64),
}

/// A call frame in the VM stack.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct CallFrame {
    pub(crate) program_index: usize,
    pub(crate) func_index: Option<usize>,
    pub(crate) ip: usize,
    pub(crate) locals: Vec<Value>,
    pub(crate) stack_base: usize,
    /// The `this` value for method/constructor calls.
    pub(crate) this_value: Option<Value>,
    /// Where the method receiver came from, so we can write back mutations.
    pub(crate) receiver_source: Option<ReceiverSource>,
    /// Local slots that have been promoted to shared upvalue cells (captured by
    /// a nested closure): slot -> cell id. Reads/writes of these slots route
    /// through the cell arena so the closure and this frame stay in sync.
    #[serde(default)]
    pub(crate) boxed: HashMap<usize, u64>,
    /// Free-variable bindings for a closure frame: name -> cell id. A name found
    /// here shadows the global of the same name (LoadGlobal/StoreGlobal consult
    /// it first), connecting captured names to their shared cells.
    #[serde(default)]
    pub(crate) env: HashMap<String, u64>,
}

/// A continuation for array callback methods that may suspend (e.g., `.map()` with async callbacks).
/// Instead of running callbacks in a Rust for-loop (which can't be suspended), the continuation
/// tracks progress so the main `execute()` loop can drive iteration one callback at a time.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) enum Continuation {
    /// Collecting `.map()` results element-by-element.
    ArrayMap {
        callback: Value,
        source: Vec<Value>,
        results: Vec<Value>,
        next_index: usize,
        /// Frame depth of the caller — the continuation fires when
        /// we return to this depth AND the callback's frame has been popped.
        caller_frame_depth: usize,
        /// The frame index of the currently-executing callback. Only when
        /// this specific frame is popped does the continuation advance.
        callback_frame_index: usize,
    },
    /// Collecting `.forEach()` calls element-by-element.
    ArrayForEach {
        callback: Value,
        source: Vec<Value>,
        next_index: usize,
        caller_frame_depth: usize,
        callback_frame_index: usize,
    },
    /// Running a `.then()`/`.catch()`/`.finally()` callback that may itself make
    /// an external (tool) call and therefore must be able to suspend. The main
    /// `execute()` loop drives the callback the same way it drives array
    /// callbacks; when the callback's frame pops, the continuation shapes the
    /// result into the promise the chain expects and pushes it. Because the
    /// continuation is part of the serialized VM state, a suspension mid-callback
    /// resumes cleanly and finishes the chain.
    PromiseCallback {
        /// How to turn the callback's return value into the chain's result.
        mode: PromiseCallbackMode,
        /// The original promise (used by `finally`, which passes it through).
        original_promise: Value,
        caller_frame_depth: usize,
        callback_frame_index: usize,
    },
}

/// What a [`Continuation::PromiseCallback`] does with the callback's return value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum PromiseCallbackMode {
    /// `.then(onFulfilled)` / `.then(_, onRejected)` / `.catch(onRejected)`:
    /// wrap the callback's return value in a resolved promise (thenables pass
    /// through unwrapped via `make_resolved_promise`).
    WrapResult,
    /// `.finally(onFinally)`: discard the callback's return value and pass the
    /// original promise through unchanged.
    PassThrough,
}

/// Outcome of dispatching a `.then`/`.catch`/`.finally` method. See
/// [`Vm::execute_promise_method`].
enum PromiseMethodOutcome {
    /// Completed synchronously (no callback ran); the value should be pushed.
    Value(Value),
    /// A callback frame + [`Continuation::PromiseCallback`] were pushed; the
    /// dispatch caller must return `Ok(None)` so the main loop drives it.
    ContinuationStarted,
    /// The method was called on a deferred single-call promise (N5), forcing its
    /// host call: the VM must suspend on it. The dispatch caller returns this
    /// `VmState` up to the host; on resume, `resume_action` finishes the method.
    Suspend(VmState),
    /// The receiver was not a promise, or the method is unknown.
    NotAPromise,
}

/// The Zapcode VM.
pub struct Vm {
    pub(crate) programs: Vec<CompiledProgram>,
    pub(crate) stack: Vec<Value>,
    pub(crate) frames: Vec<CallFrame>,
    pub(crate) globals: HashMap<String, Value>,
    /// Arena of shared upvalue cells, indexed by id. A captured variable lives
    /// here once boxed; every closure and frame referencing it shares the id, so
    /// the sharing survives serialization (ids are reconstructed on load).
    pub(crate) cells: Vec<Value>,
    /// The object heap: backing store for all array/object values. Handles in
    /// `Value::Array`/`Object` index into it; shared handles give reference
    /// semantics. Serialized with the snapshot so identity survives resume.
    pub(crate) heap: Heap,
    pub(crate) stdout: String,
    pub(crate) limits: ResourceLimits,
    pub(crate) tracker: ResourceTracker,
    pub(crate) external_functions: HashSet<String>,
    pub(crate) try_stack: Vec<TryInfo>,
    /// Active continuations for array callback methods that may suspend.
    pub(crate) continuations: Vec<Continuation>,
    /// The last object a property was accessed on — used for method dispatch.
    pub(crate) last_receiver: Option<Value>,
    /// Where the last receiver came from — used to write back `this` mutations.
    pub(crate) last_receiver_source: Option<ReceiverSource>,
    /// The name of the last global loaded — used to identify known globals.
    pub(crate) last_global_name: Option<String>,
    /// Tracks the source of the most recent Load instruction for receiver tracking.
    pub(crate) last_load_source: Option<ReceiverSource>,
    /// Write-back place (root + path) of the most recently loaded/accessed value,
    /// captured onto a builtin method so mutating calls persist to the right
    /// location (incl. nested `obj.items.push(...)`). Transient; not serialized.
    pub(crate) last_place: Option<Place>,
    /// Counter for assigning unique generator IDs.
    pub(crate) next_generator_id: u64,
    /// External calls deferred by `CallExternalDeferred`, pending batch resolution.
    pub(crate) pending_calls: Vec<PendingExternalCall>,
    /// Results delivered by `resume_many`, keyed by call id (BTreeMap for
    /// deterministic snapshot bytes).
    pub(crate) resolved: BTreeMap<u64, Value>,
    /// Counter for assigning unique deferred-call IDs.
    pub(crate) next_call_id: u64,
    /// The batch currently awaiting host resolution (set at the batch await).
    pub(crate) pending_batch: Option<PendingBatch>,
    /// Deterministic PRNG state for `Math.random`. Seeded to a fixed value and
    /// carried in the snapshot, so a replayed program produces the same random
    /// sequence (required for durable/Temporal replay) while still varying call
    /// to call.
    pub(crate) rng_state: u64,
    /// The value of an in-flight guest `throw`, so `catch` receives the original
    /// value (string, object, …) rather than a stringified error. Transient —
    /// set by `Throw`, consumed by the catch handler; never crosses a suspension.
    pub(crate) pending_throw: Option<Value>,
    /// Re-entrancy depth of the ToPrimitive machinery. A user `valueOf`/`toString`
    /// hook can itself trigger another coercion (e.g. by returning/operating on
    /// another object); this bounds that recursion so a pathological hook can't
    /// loop forever. Transient — never serialized.
    pub(crate) to_primitive_depth: u32,
    /// What to do with the value the host pushes when resuming a suspension that
    /// was triggered by *consuming a deferred single-call promise* (N5). For a
    /// plain `await p` this is `None` (the pushed value is exactly the await
    /// result). For `p.then(cb)`/`.catch`/`.finally` it carries the promise
    /// method + callbacks so the resumed value is wrapped in a settled promise
    /// and the callback chain runs. Serialized so it survives dump/load/resume.
    pub(crate) resume_action: Option<ResumeAction>,
}

/// Deferred action applied to the value the host delivers when resuming a
/// single-call-promise suspension. See [`Vm::resume_action`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) enum ResumeAction {
    /// The suspension was driven by a `.then`/`.catch`/`.finally` on a deferred
    /// single-call promise. On resume, wrap the host value in a settled promise
    /// (`resolved` on success; the error path uses `resume_with_error` instead)
    /// and run `method` with `args`, threading through the normal promise-method
    /// machinery (which supports tool calls inside the callback, per N4).
    PromiseMethod { method: String, args: Vec<Value> },
    /// The suspension was driven by a promise callback *returning* a deferred
    /// single-call promise (thenable adoption): the chain forced that call. On
    /// resume, the settled promise becomes (or is folded into) the chain's
    /// result per `mode`. See the `PromiseCallback` arm of `process_continuation`.
    ChainResult {
        mode: PromiseCallbackMode,
        original_promise: Value,
    },
    /// A plain `await p` on a deferred single-call promise forced its call. The
    /// resumed value is the await result (pushed by the host) — but we also cache
    /// it under the call id so a *second* `await p` / `p.then(...)` on the same
    /// promise reuses the settled value instead of re-invoking the host (matching
    /// JS, where a promise settles once).
    CacheValue { id: u64 },
}

/// The preferred type passed to [`Vm::to_primitive`], mirroring the JS
/// `ToPrimitive` hint. Determines whether `valueOf` or `toString` is tried first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToPrimitiveHint {
    /// `Number(x)`, arithmetic, relational comparisons.
    Number,
    /// `String(x)`, template literals.
    String,
    /// `+` (string-or-number) — same method order as Number per spec.
    Default,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct TryInfo {
    pub(crate) catch_ip: usize,
    pub(crate) frame_depth: usize,
    pub(crate) stack_depth: usize,
    /// Start ip of the `finally` body, if this try statement has one. Abrupt
    /// completions (`return`/`break`/`continue`) and uncaught throws that leave
    /// the protected region run this finally before continuing.
    #[serde(default)]
    pub(crate) finally_ip: Option<usize>,
    /// Whether this try statement has a `catch` handler. When it does not, a
    /// throw routes straight to the finally (carrying the exception) instead of
    /// being treated as caught.
    #[serde(default)]
    pub(crate) has_catch: bool,
    /// The completion to resume once the finally body finishes (`EndFinally`).
    /// `None` while executing the protected body; set when control enters the
    /// finally. A `Normal` completion simply falls through.
    #[serde(default)]
    pub(crate) pending: Option<Completion>,
    /// Set once control has entered this try's finally body, so a further abrupt
    /// completion raised *inside* the finally does not re-run the same finally
    /// (it supersedes the pending completion and propagates outward).
    #[serde(default)]
    pub(crate) in_finally: bool,
    /// ip just past the whole try/catch/finally statement. A `break`/`continue`
    /// whose target lies inside `[setup_ip, region_end)` stays within this try
    /// (its finally is skipped); a target outside escapes (the finally runs).
    #[serde(default)]
    pub(crate) region_start: usize,
    #[serde(default)]
    pub(crate) region_end: usize,
}

/// An abrupt or normal completion threaded through `finally` blocks. JS requires
/// a `finally` to run on every way of leaving its `try`/`catch`, and an abrupt
/// completion *inside* the finally to supersede whatever the body was doing.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) enum Completion {
    /// Fall through to the code after the try statement.
    Normal,
    /// Resume a pending `return value;`.
    Return(Value),
    /// Re-raise a pending exception once the finally finishes.
    Throw(Value),
    /// Resume a pending `break`/`continue`, transferring control to `target`.
    Break(usize),
    Continue(usize),
}

/// Whether `completion` escapes the try statement described by `info` (i.e.
/// transferring control out of it must run its `finally`). A `return` always
/// escapes the enclosing trys in its frame; a `break`/`continue` escapes a try
/// only when its target lands outside that try statement's ip range.
fn completion_escapes(completion: &Completion, info: &TryInfo) -> bool {
    match completion {
        Completion::Return(_) | Completion::Throw(_) | Completion::Normal => true,
        Completion::Break(target) | Completion::Continue(target) => {
            !(info.region_start..info.region_end).contains(target)
        }
    }
}

impl Vm {
    fn new(
        program: CompiledProgram,
        limits: ResourceLimits,
        external_functions: HashSet<String>,
    ) -> Self {
        Self::with_programs(vec![program], limits, external_functions)
    }

    pub(crate) fn with_programs(
        programs: Vec<CompiledProgram>,
        limits: ResourceLimits,
        external_functions: HashSet<String>,
    ) -> Self {
        Self::with_programs_and_heap(programs, limits, external_functions, Heap::new())
    }

    /// Like [`with_programs`] but seeds the VM with an existing heap (e.g. the
    /// heap carried forward between session chunks, which backs the persisted
    /// array/object globals). Builtin globals are re-registered, appending fresh
    /// slots to the supplied heap — the same approach as [`from_snapshot`] — so
    /// user handles in the restored heap remain valid.
    pub(crate) fn with_programs_and_heap(
        programs: Vec<CompiledProgram>,
        limits: ResourceLimits,
        external_functions: HashSet<String>,
        heap: Heap,
    ) -> Self {
        let mut globals = HashMap::new();
        let mut heap = heap;

        // Register built-in globals
        builtins::register_globals(&mut globals, &mut heap);

        Self {
            programs,
            stack: Vec::new(),
            frames: Vec::new(),
            globals,
            cells: Vec::new(),
            heap,
            stdout: String::new(),
            limits,
            tracker: ResourceTracker::default(),
            external_functions,
            try_stack: Vec::new(),
            continuations: Vec::new(),
            last_receiver: None,
            last_receiver_source: None,
            last_global_name: None,
            last_load_source: None,
            last_place: None,
            next_generator_id: 0,
            pending_calls: Vec::new(),
            resolved: BTreeMap::new(),
            next_call_id: 0,
            pending_batch: None,
            rng_state: 0,
            pending_throw: None,
            to_primitive_depth: 0,
            resume_action: None,
        }
    }

    /// Names of all builtin globals registered by `register_globals`.
    pub(crate) const BUILTIN_GLOBAL_NAMES: &'static [&'static str] = &[
        "console",
        "JSON",
        "Object",
        "Array",
        "Math",
        "Promise",
        "Map",
        "Date",
        "String",
        "Number",
        "Boolean",
        "parseInt",
        "parseFloat",
        "isNaN",
        "isFinite",
        "Set",
        "Error",
        "TypeError",
        "RangeError",
        "SyntaxError",
        "ReferenceError",
        "structuredClone",
    ];

    /// Restore a VM from snapshot state and continue execution.
    /// Builtins are re-registered after restoring user globals.
    /// The return_value is pushed onto the stack (result of the external call).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_snapshot(
        programs: Vec<CompiledProgram>,
        stack: Vec<Value>,
        frames: Vec<CallFrame>,
        user_globals: HashMap<String, Value>,
        try_stack: Vec<TryInfo>,
        continuations: Vec<Continuation>,
        stdout: String,
        limits: ResourceLimits,
        external_functions: HashSet<String>,
        next_generator_id: u64,
        last_receiver: Option<Value>,
        last_receiver_source: Option<ReceiverSource>,
        last_global_name: Option<String>,
        last_load_source: Option<ReceiverSource>,
        heap: Heap,
    ) -> Self {
        let mut globals = HashMap::new();
        let mut heap = heap;
        // Re-register builtins first (appended to the restored heap).
        builtins::register_globals(&mut globals, &mut heap);
        // Then overlay user globals (user globals take precedence if names collide)
        for (k, v) in user_globals {
            globals.insert(k, v);
        }

        Self {
            programs,
            stack,
            frames,
            globals,
            cells: Vec::new(),
            heap,
            stdout,
            limits,
            tracker: ResourceTracker::default(),
            external_functions,
            try_stack,
            continuations,
            last_receiver,
            last_receiver_source,
            last_global_name,
            last_load_source,
            last_place: None,
            next_generator_id,
            pending_calls: Vec::new(),
            resolved: BTreeMap::new(),
            next_call_id: 0,
            pending_batch: None,
            rng_state: 0,
            pending_throw: None,
            to_primitive_depth: 0,
            resume_action: None,
        }
    }

    /// Advance the deterministic PRNG (splitmix64) and return a float in [0, 1).
    pub(crate) fn next_random(&mut self) -> f64 {
        self.rng_state = self.rng_state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.rng_state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        // Use the top 53 bits for a uniform double in [0, 1).
        (z >> 11) as f64 / ((1u64 << 53) as f64)
    }

    /// Resume execution after a snapshot restore. The return value from
    /// the external function should already be pushed onto the stack.
    pub(crate) fn resume_execution(&mut self) -> Result<VmState> {
        self.tracker.start();
        // If this suspension was a `.then`/`.catch`/`.finally` forcing a deferred
        // single-call promise (N5), the host value on the stack is the call's
        // *fulfilled* result. Shape it into a resolved promise and run the
        // method, so the callback chain proceeds (possibly suspending again for
        // a tool call inside the callback). A plain `await` has no resume_action,
        // so the value is already the await result — fall through to `execute`.
        if let Some(action) = self.resume_action.take() {
            match action {
                // Plain `await p`: the host value on the stack IS the await result.
                // Cache it under the call id (so a later await/then on the same
                // promise settles once) and leave it on the stack.
                ResumeAction::CacheValue { id } => {
                    let value = self.peek()?.clone();
                    self.resolved.insert(id, value);
                }
                other => {
                    let value = self.pop()?;
                    let settled = builtins::make_resolved_promise(value, &mut self.heap);
                    if let Some(state) = self.run_resume_action(other, settled)? {
                        return Ok(state);
                    }
                }
            }
        }
        self.execute()
    }

    /// Apply a [`ResumeAction`] to the settled (resolved/rejected) promise that a
    /// resumed single-call promise produced, dispatching the deferred promise
    /// method. Returns `Some(state)` if running the method itself suspends (a
    /// tool call inside the callback) and `None` once the method's result has
    /// been pushed / a callback continuation has been started (in which case the
    /// caller drives the main loop). Used by both the success and error resume
    /// paths.
    fn run_resume_action(
        &mut self,
        action: ResumeAction,
        settled: Value,
    ) -> Result<Option<VmState>> {
        match action {
            ResumeAction::PromiseMethod { method, args } => {
                match self.execute_promise_method(settled.clone(), &method, args)? {
                    // The method completed synchronously — push its promise and
                    // let the following instructions (typically `Await`) consume.
                    PromiseMethodOutcome::Value(v) => {
                        self.push(v)?;
                        Ok(None)
                    }
                    // A callback frame + continuation were started; the main loop
                    // drives them.
                    PromiseMethodOutcome::ContinuationStarted => Ok(None),
                    // The callback itself was a tool call that suspended again.
                    PromiseMethodOutcome::Suspend(state) => Ok(Some(state)),
                    PromiseMethodOutcome::NotAPromise => {
                        // Should not happen — `make_resolved_promise` always yields
                        // a promise. Push the settled value defensively.
                        self.push(settled)?;
                        Ok(None)
                    }
                }
            }
            ResumeAction::ChainResult {
                mode,
                original_promise,
            } => {
                // A promise callback returned a deferred single-call promise; its
                // host call has now settled into `settled`. Fold it into the chain
                // result per the callback mode, then continue the execute loop.
                let is_rejected = matches!(
                    &settled,
                    Value::Object(h) if matches!(
                        self.heap.object(*h).and_then(|m| m.get("status")),
                        Some(Value::String(s)) if s.as_ref() == "rejected"
                    )
                );
                let chain_result = match mode {
                    // `.then`/`.catch`: adopt the settled promise as the chain's
                    // next promise (resolved value flows on; a rejection rejects).
                    PromiseCallbackMode::WrapResult => settled,
                    // `.finally`: the returned promise is awaited but its *value*
                    // is discarded — the original promise passes through on
                    // success; a rejection from the cleanup promise wins.
                    PromiseCallbackMode::PassThrough => {
                        if is_rejected {
                            settled
                        } else {
                            original_promise
                        }
                    }
                };
                self.push(chain_result)?;
                Ok(None)
            }
            // `CacheValue` is handled inline by `resume_execution` /
            // `resume_with_error` (it needs the raw value / throw path), never via
            // this helper. Push the settled promise defensively if it ever arrives.
            ResumeAction::CacheValue { .. } => {
                self.push(settled)?;
                Ok(None)
            }
        }
    }

    /// Resume a suspended external call by making it *throw* `error` instead of
    /// returning a value. The error surfaces inside guest code at the await/call
    /// site: if it is inside a `try`, the `catch` block runs (receiving `error`);
    /// otherwise it propagates out to the host as `ExternalError`. This mirrors
    /// how a real failing tool/activity should look to agent-written code.
    pub(crate) fn resume_with_error(&mut self, error: Value) -> Result<VmState> {
        self.tracker.start();
        // If this suspension was a `.then`/`.catch`/`.finally` forcing a deferred
        // single-call promise (N5), a rejection becomes a *rejected* promise and
        // the method runs so a `.catch`/onRejected can handle it — rather than
        // propagating the error out of the chain.
        if let Some(action) = self.resume_action.take() {
            // A plain `await p` whose deferred call rejected: fall through to the
            // normal throw-at-await-site path below (the error surfaces in guest
            // try/catch or propagates to the host), exactly like `await tool()`.
            if matches!(action, ResumeAction::CacheValue { .. }) {
                // (no shaping needed — the error is raised at the await site)
            } else {
                let rejected = builtins::make_rejected_promise(error.clone(), &mut self.heap);
                return match self.run_resume_action(action, rejected)? {
                    Some(state) => Ok(state),
                    None => self.execute(),
                };
            }
        }
        // Route the rejection to the nearest catch or finally (running finallys
        // on the way), exactly as the execute loop does for a runtime error.
        if self.route_thrown(error.clone(), 0)? {
            self.execute()
        } else {
            // No handler — the failure is the program's failure.
            Err(ZapcodeError::ExternalError(error.to_js_string(&self.heap)))
        }
    }

    /// Route a thrown value to the nearest enclosing handler (a `catch` or a
    /// `finally`). Returns `true` if a handler was engaged (control was
    /// transferred and execution should continue), or `false` if the throw is
    /// uncaught (no remaining handler) and must propagate to the host.
    ///
    /// `min_frame_depth` limits how far unwinding may go: only try frames at
    /// depth `> min_frame_depth` are considered, so an internal call (which sets
    /// it to the caller's depth) does not steal the caller's handlers.
    fn route_thrown(&mut self, error_val: Value, min_frame_depth: usize) -> Result<bool> {
        loop {
            let Some(try_info) = self
                .try_stack
                .last()
                .filter(|t| t.frame_depth > min_frame_depth)
                .cloned()
            else {
                // No handler at this level. Preserve the thrown value so a
                // caller that re-raises (e.g. a generator surfacing the error at
                // its driving `.next()`) reconstructs the original error object /
                // value rather than a stringified one.
                self.pending_throw = Some(error_val);
                return Ok(false);
            };
            self.try_stack.pop();

            // Unwind to the protected region's frame/stack depth.
            while self.frames.len() > try_info.frame_depth {
                self.frames.pop();
                self.tracker.pop_frame();
            }
            self.stack.truncate(try_info.stack_depth);

            if try_info.in_finally {
                // The exception was raised *inside* this try's finally body. It
                // supersedes whatever the finally was resuming; drop this frame
                // and keep unwinding to an outer handler.
                continue;
            }

            if try_info.has_catch {
                // Enter the catch block. The catch protection is consumed: a
                // throw inside the catch body must route to this try's finally
                // (if any) or propagate, never back to the same catch.
                self.push(error_val)?;
                if try_info.finally_ip.is_some() {
                    let mut info = try_info;
                    info.has_catch = false;
                    let catch_ip = info.catch_ip;
                    self.try_stack.push(info);
                    self.current_frame_mut().ip = catch_ip;
                } else {
                    self.current_frame_mut().ip = try_info.catch_ip;
                }
                return Ok(true);
            }

            if let Some(finally_ip) = try_info.finally_ip {
                // No catch (or catch already consumed): run the finally with the
                // exception pending, then re-raise it via EndFinally.
                let mut info = try_info;
                info.pending = Some(Completion::Throw(error_val));
                info.in_finally = true;
                info.has_catch = false;
                self.try_stack.push(info);
                self.current_frame_mut().ip = finally_ip;
                return Ok(true);
            }

            // This try frame neither catches nor has a finally for this throw;
            // keep unwinding to an outer handler.
        }
    }

    /// Run any `finally` blocks in the current frame that an abrupt completion
    /// (`return`/`break`/`continue`) is escaping, then perform the completion.
    /// Returns `Ok(true)` if a finally was entered (execution continues at the
    /// finally body); the caller should resume the main loop. Returns `Ok(false)`
    /// if no intervening finally needs to run and the caller should carry out the
    /// completion itself.
    ///
    /// `escapes` decides, per candidate try frame in the current call frame,
    /// whether the completion leaves it (always true for return; for break/
    /// continue, true unless the jump target stays within the try statement).
    fn route_abrupt(&mut self, completion: Completion) -> Result<bool> {
        let frame_depth = self.frames.len();
        // Find the innermost try frame in the *current* call frame that still has
        // an un-run finally and that this completion escapes.
        let idx = self.try_stack.iter().rposition(|t| {
            t.frame_depth == frame_depth
                && t.finally_ip.is_some()
                && !t.in_finally
                && completion_escapes(&completion, t)
        });
        let Some(idx) = idx else {
            return Ok(false);
        };

        // Any inner try frames (above idx) in this call frame are being escaped
        // too; drop them (their finallys, if any, were handled by recursion when
        // their own bodies completed — here they are inner-to-idx and already
        // resolved, so simply discard).
        self.try_stack.truncate(idx + 1);

        let try_info = &mut self.try_stack[idx];
        let finally_ip = try_info.finally_ip.unwrap();
        let stack_depth = try_info.stack_depth;
        try_info.pending = Some(completion);
        try_info.in_finally = true;
        try_info.has_catch = false;
        self.stack.truncate(stack_depth);
        self.current_frame_mut().ip = finally_ip;
        Ok(true)
    }

    /// Resume a completion saved by a `finally` (via `EndFinally`). A `Normal`
    /// completion falls through; the abrupt ones re-perform their transfer,
    /// routing through any *further* enclosing finallys first. Returns a
    /// `VmState` only when the resumed completion ends the program/suspends.
    fn resume_completion(&mut self, completion: Completion) -> Result<Option<VmState>> {
        match completion {
            Completion::Normal => Ok(None),
            Completion::Return(v) => {
                if self.route_abrupt(Completion::Return(v.clone()))? {
                    return Ok(None);
                }
                self.perform_return(v)
            }
            Completion::Throw(v) => {
                // Re-raise the pending exception. Routing to the next handler is
                // done uniformly by the execute loop's error path, which reads
                // `pending_throw` so the original value/identity is preserved.
                let msg = v.to_js_string(&self.heap);
                self.pending_throw = Some(v);
                Err(ZapcodeError::RuntimeError(msg))
            }
            Completion::Break(target) => {
                if self.route_abrupt(Completion::Break(target))? {
                    return Ok(None);
                }
                self.current_frame_mut().ip = target;
                Ok(None)
            }
            Completion::Continue(target) => {
                if self.route_abrupt(Completion::Continue(target))? {
                    return Ok(None);
                }
                self.current_frame_mut().ip = target;
                Ok(None)
            }
        }
    }

    /// Pop the current call frame and deliver `return_val` to the caller (the
    /// body of the old `Return` instruction). Returns `VmState::Complete` at the
    /// top level. The caller must already have run any escaped `finally` blocks.
    fn perform_return(&mut self, return_val: Value) -> Result<Option<VmState>> {
        if self.frames.len() <= 1 {
            return Ok(Some(VmState::Complete(return_val)));
        }

        let frame = self.frames.pop().unwrap();
        self.tracker.pop_frame();

        // If this was a constructor frame (has this_value), return the updated
        // `this` instead of the explicit return value (unless the constructor
        // explicitly returns an object).
        let actual_return = if let Some(ref this_val) = frame.this_value {
            if let Some(parent) = self.frames.last_mut() {
                if parent.this_value.is_some() {
                    parent.this_value = Some(this_val.clone());
                }
            }
            if let Some(ref source) = frame.receiver_source {
                self.write_receiver_source(source, this_val.clone());
            }
            if matches!(return_val, Value::Undefined) {
                this_val.clone()
            } else {
                return_val
            }
        } else {
            return_val
        };

        self.stack.truncate(frame.stack_base);
        self.push(actual_return)?;
        Ok(None)
    }

    /// Resume a batch suspension with the values the host's combinator produced.
    ///
    /// The shape of `results` depends on the batch's combinator (carried in
    /// `pending_batch.kind`):
    /// - `all`: one resolved value per pending call, in the order the calls were
    ///   presented. The VM rebuilds the full result array in element order
    ///   (deferred calls substituted, plain-promise/value elements unwrapped).
    /// - `allSettled`: one settled object (`{status,value}` / `{status,reason}`)
    ///   per pending call, in order; merged into the element-order array.
    /// - `race`/`any`: a single entry — the one settled value the host chose. It
    ///   becomes the promise value directly.
    ///
    /// A rejection is delivered via `resume_with_error` instead.
    pub(crate) fn resume_many(&mut self, results: Vec<Value>) -> Result<VmState> {
        self.tracker.start();
        let batch = self.pending_batch.take().ok_or_else(|| {
            ZapcodeError::RuntimeError(
                "resume_many called but the VM is not suspended on a batch".to_string(),
            )
        })?;

        // Drop the now-resolved pending calls regardless of combinator.
        let resolved_ids: HashSet<u64> = batch.call_ids.iter().copied().collect();
        self.pending_calls.retain(|c| !resolved_ids.contains(&c.id));

        match batch.kind {
            // race/any settle to a single value supplied by the host's real JS
            // combinator; push it directly (no element-order reassembly).
            BatchKind::Race | BatchKind::Any => {
                if results.len() != 1 {
                    return Err(ZapcodeError::RuntimeError(format!(
                        "resume_many for {} expected 1 result but got {}",
                        batch.kind.as_str(),
                        results.len()
                    )));
                }
                let value = results.into_iter().next().unwrap_or(Value::Undefined);
                self.push(value)?;
                self.execute()
            }
            // all/allSettled deliver one settled entry per pending call, in the
            // order the calls were presented; reassemble in element order.
            BatchKind::All | BatchKind::AllSettled => {
                if results.len() != batch.call_ids.len() {
                    return Err(ZapcodeError::RuntimeError(format!(
                        "resume_many expected {} results but got {}",
                        batch.call_ids.len(),
                        results.len()
                    )));
                }
                for (id, value) in batch.call_ids.iter().zip(results) {
                    self.resolved.insert(*id, value);
                }
                let array = match batch.kind {
                    BatchKind::AllSettled => self.build_settled_array(batch.items)?,
                    _ => self.build_batch_array(batch.items)?,
                };
                let h = self.heap.alloc_array(array);
                self.push(Value::Array(h))?;
                self.execute()
            }
        }
    }

    /// Build a `Promise.allSettled` result array from its elements: deferred
    /// calls are replaced by the host-supplied settled object as-is; plain
    /// promise/value elements are coerced to `{status,value}` /
    /// `{status,reason}` settled objects. Never throws.
    fn build_settled_array(&mut self, items: Vec<Value>) -> Result<Vec<Value>> {
        let mut array = Vec::with_capacity(items.len());
        for item in items {
            match item {
                Value::Pending(id) => {
                    // Host already produced the settled object for this call.
                    let value = self.resolved.remove(&id).ok_or_else(|| {
                        ZapcodeError::RuntimeError(format!("missing result for call {}", id))
                    })?;
                    array.push(value);
                }
                Value::Object(h) if builtins::is_promise(&item, &self.heap) => {
                    let map = self.heap.object_map(h);
                    let mut entry = IndexMap::new();
                    match map.get("status") {
                        Some(Value::String(s)) if s.as_ref() == "rejected" => {
                            entry.insert(Arc::from("status"), Value::String(Arc::from("rejected")));
                            entry.insert(
                                Arc::from("reason"),
                                map.get("reason").cloned().unwrap_or(Value::Undefined),
                            );
                        }
                        _ => {
                            entry.insert(
                                Arc::from("status"),
                                Value::String(Arc::from("fulfilled")),
                            );
                            entry.insert(
                                Arc::from("value"),
                                map.get("value").cloned().unwrap_or(Value::Undefined),
                            );
                        }
                    }
                    array.push(Value::Object(self.heap.alloc_object(entry)));
                }
                other => {
                    // Plain value: fulfilled.
                    let mut entry = IndexMap::new();
                    entry.insert(Arc::from("status"), Value::String(Arc::from("fulfilled")));
                    entry.insert(Arc::from("value"), other);
                    array.push(Value::Object(self.heap.alloc_object(entry)));
                }
            }
        }
        Ok(array)
    }

    /// Build a `Promise.all` result array from its elements: deferred calls are
    /// replaced by their resolved value, and any plain promise element is
    /// unwrapped (resolved → value, rejected → propagate) exactly like the
    /// non-batched `Promise.all` builtin. Plain values pass through unchanged.
    fn build_batch_array(&mut self, items: Vec<Value>) -> Result<Vec<Value>> {
        let mut array = Vec::with_capacity(items.len());
        for item in items {
            match item {
                Value::Pending(id) => {
                    let value = self.resolved.remove(&id).ok_or_else(|| {
                        ZapcodeError::RuntimeError(format!("missing result for call {}", id))
                    })?;
                    array.push(value);
                }
                Value::Object(h) if builtins::is_promise(&item, &self.heap) => {
                    let map = self.heap.object_map(h);
                    match map.get("status") {
                        Some(Value::String(s)) if s.as_ref() == "resolved" => {
                            array.push(map.get("value").cloned().unwrap_or(Value::Undefined));
                        }
                        Some(Value::String(s)) if s.as_ref() == "rejected" => {
                            let reason = map.get("reason").cloned().unwrap_or(Value::Undefined);
                            let reason = reason.to_js_string(&self.heap);
                            return Err(ZapcodeError::RuntimeError(format!(
                                "Unhandled promise rejection: {}",
                                reason
                            )));
                        }
                        _ => array.push(item),
                    }
                }
                other => array.push(other),
            }
        }
        Ok(array)
    }

    /// Await a `Promise.{all,race,any,allSettled}` batch. If any element is an
    /// unresolved deferred call, suspend once with the whole batch (tagged with
    /// `kind`) so the host can run the calls in parallel and settle them with
    /// the real JS combinator; otherwise assemble and push the value inline.
    fn await_batch(&mut self, kind: BatchKind, items: Vec<Value>) -> Result<Option<VmState>> {
        let mut call_ids = Vec::new();
        for item in &items {
            if let Value::Pending(id) = item {
                if !self.resolved.contains_key(id) {
                    call_ids.push(*id);
                }
            }
        }

        if call_ids.is_empty() {
            // Every element already settled — assemble inline without the host.
            self.assemble_batch_inline(kind, items)?;
            return Ok(None);
        }

        // Present the calls to the host in element order.
        let mut calls = Vec::with_capacity(call_ids.len());
        for id in &call_ids {
            let pc = self
                .pending_calls
                .iter()
                .find(|c| c.id == *id)
                .ok_or_else(|| {
                    ZapcodeError::RuntimeError(format!("unknown pending call {}", id))
                })?;
            calls.push(ExternalCall {
                name: pc.name.clone(),
                args: pc.args.clone(),
            });
        }

        self.pending_batch = Some(PendingBatch { kind, items, call_ids });
        let snapshot = ZapcodeSnapshot::capture(self)?;
        Ok(Some(VmState::SuspendedMany {
            calls,
            combinator: kind,
            snapshot,
        }))
    }

    /// Suspend on a deferred single-call promise's host call (N5). The call was
    /// registered by `CallExternalDeferred` and is held in `pending_calls`. We
    /// present it to the host as an ordinary single suspension (`name`/`args`),
    /// so the existing host bridge resolves it exactly like `await tool()`. The
    /// pending-call record is removed so it can't be invoked twice. The resumed
    /// value is pushed onto the stack; any post-resume shaping (for
    /// `.then`/`.catch`/`.finally`) is governed by `self.resume_action`.
    fn suspend_on_pending_call(&mut self, id: u64) -> Result<Option<VmState>> {
        let pos = self
            .pending_calls
            .iter()
            .position(|c| c.id == id)
            .ok_or_else(|| {
                ZapcodeError::RuntimeError(format!("unknown pending call {}", id))
            })?;
        let pc = self.pending_calls.remove(pos);
        let snapshot = ZapcodeSnapshot::capture(self)?;
        Ok(Some(VmState::Suspended {
            function_name: pc.name,
            args: pc.args,
            snapshot,
        }))
    }

    /// Assemble and push the batch value when every element is already settled
    /// (no host round-trip needed). Mirrors the per-combinator builtin
    /// semantics in `builtins::call_promise_method`.
    fn assemble_batch_inline(&mut self, kind: BatchKind, items: Vec<Value>) -> Result<()> {
        match kind {
            BatchKind::All => {
                let array = self.build_batch_array(items)?;
                let h = self.heap.alloc_array(array);
                self.push(Value::Array(h))?;
            }
            BatchKind::AllSettled => {
                let array = self.build_settled_array(items)?;
                let h = self.heap.alloc_array(array);
                self.push(Value::Array(h))?;
            }
            BatchKind::Race => {
                // First element wins (already settled). Resolve → push value;
                // reject → propagate as an unhandled rejection.
                match items.into_iter().next() {
                    Some(first) => {
                        let value = self.unwrap_settled_or_propagate(first)?;
                        self.push(value)?;
                    }
                    None => {
                        // Empty race never settles in real JS; surface undefined
                        // rather than hang the deterministic VM.
                        self.push(Value::Undefined)?;
                    }
                }
            }
            BatchKind::Any => {
                // First fulfilled value wins; if all reject, reject with an
                // AggregateError-shaped value.
                let mut errors = Vec::new();
                let mut chosen: Option<Value> = None;
                for item in items {
                    match self.settled_outcome(&item) {
                        SettledOutcome::Fulfilled(v) => {
                            chosen = Some(v);
                            break;
                        }
                        SettledOutcome::Rejected(reason) => errors.push(reason),
                    }
                }
                match chosen {
                    Some(v) => self.push(v)?,
                    None => {
                        let errors_arr = Value::Array(self.heap.alloc_array(errors));
                        let mut agg = IndexMap::new();
                        agg.insert(
                            Arc::from("name"),
                            Value::String(Arc::from("AggregateError")),
                        );
                        agg.insert(
                            Arc::from("message"),
                            Value::String(Arc::from("All promises were rejected")),
                        );
                        agg.insert(Arc::from("errors"), errors_arr);
                        let reason = Value::Object(self.heap.alloc_object(agg));
                        let reason = reason.to_js_string(&self.heap);
                        return Err(ZapcodeError::RuntimeError(format!(
                            "Unhandled promise rejection: {}",
                            reason
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    /// Classify an already-settled element (plain value or promise object) as
    /// fulfilled or rejected, cloning out the relevant payload.
    fn settled_outcome(&self, item: &Value) -> SettledOutcome {
        if let Value::Object(h) = item {
            if builtins::is_promise(item, &self.heap) {
                let map = self.heap.object_map(*h);
                if matches!(map.get("status"), Some(Value::String(s)) if s.as_ref() == "rejected") {
                    return SettledOutcome::Rejected(
                        map.get("reason").cloned().unwrap_or(Value::Undefined),
                    );
                }
                return SettledOutcome::Fulfilled(
                    map.get("value").cloned().unwrap_or(Value::Undefined),
                );
            }
        }
        SettledOutcome::Fulfilled(item.clone())
    }

    /// Unwrap an already-settled element: resolved promise → its value, plain
    /// value → itself, rejected promise → propagate as an unhandled rejection.
    fn unwrap_settled_or_propagate(&mut self, item: Value) -> Result<Value> {
        match self.settled_outcome(&item) {
            SettledOutcome::Fulfilled(v) => Ok(v),
            SettledOutcome::Rejected(reason) => {
                let reason = reason.to_js_string(&self.heap);
                Err(ZapcodeError::RuntimeError(format!(
                    "Unhandled promise rejection: {}",
                    reason
                )))
            }
        }
    }

    fn push(&mut self, value: Value) -> Result<()> {
        self.tracker.track_allocation(&self.limits)?;
        self.stack.push(value);
        Ok(())
    }

    fn track_array_capacity(&mut self, len: usize) -> Result<()> {
        self.tracker
            .track_memory(len.saturating_mul(size_of::<Value>()), &self.limits)
    }

    /// The value a `catch` block should bind: the original guest-thrown value if
    /// this came from `throw`, otherwise a real Error object built from the
    /// runtime error (so `e.name`/`e.message` and `e instanceof Error` work).
    fn caught_error_value(&mut self, err: &ZapcodeError) -> Value {
        if let Some(v) = self.pending_throw.take() {
            return v;
        }
        let (name, message) = match err {
            ZapcodeError::TypeError(s) => ("TypeError", s.clone()),
            ZapcodeError::ReferenceError(s) => {
                ("ReferenceError", format!("{} is not defined", s))
            }
            ZapcodeError::UnknownExternalFunction(s) => {
                ("ReferenceError", format!("{} is not defined", s))
            }
            ZapcodeError::RuntimeError(s) | ZapcodeError::ExternalError(s) => ("Error", s.clone()),
            other => ("Error", other.to_string()),
        };
        make_error_object(name, &message, &mut self.heap)
    }

    fn write_receiver_source(&mut self, source: &ReceiverSource, value: Value) {
        match source {
            ReceiverSource::Global(name) => {
                self.globals.insert(name.clone(), value);
            }
            ReceiverSource::Local { frame_index, slot } => {
                self.write_local(*frame_index, *slot, value);
            }
            ReceiverSource::Cell(id) => {
                if let Some(slot_ref) = self.cells.get_mut(*id as usize) {
                    *slot_ref = value;
                }
            }
        }
    }

    fn pop(&mut self) -> Result<Value> {
        self.stack
            .pop()
            .ok_or_else(|| ZapcodeError::RuntimeError("stack underflow".to_string()))
    }

    fn peek(&self) -> Result<&Value> {
        self.stack
            .last()
            .ok_or_else(|| ZapcodeError::RuntimeError("stack underflow".to_string()))
    }

    /// Promote frame `frame_index`'s local `slot` to a shared cell, returning its
    /// id. Idempotent: a slot already boxed returns its existing cell.
    fn box_local(&mut self, frame_index: usize, slot: usize) -> u64 {
        if let Some(&cell) = self
            .frames
            .get(frame_index)
            .and_then(|f| f.boxed.get(&slot))
        {
            return cell;
        }
        let val = self
            .frames
            .get(frame_index)
            .and_then(|f| f.locals.get(slot).cloned())
            .unwrap_or(Value::Undefined);
        let cell_id = self.cells.len() as u64;
        self.cells.push(val);
        if let Some(f) = self.frames.get_mut(frame_index) {
            f.boxed.insert(slot, cell_id);
        }
        cell_id
    }

    /// Build a closure over the current scope. Free variables that name a frame
    /// local (or an enclosing closure's captured variable) are bound by reference
    /// through shared cells (`env`); user globals are captured by value. The
    /// innermost binding of a name wins, matching lexical scoping.
    fn create_closure(&mut self, func_idx: usize) -> Closure {
        let mut seen: HashSet<String> = HashSet::new();
        // Names to box (frame_index, slot, name) and cells inherited from
        // enclosing closure frames, gathered innermost -> outermost.
        let mut to_box: Vec<(usize, usize, String)> = Vec::new();
        let mut inherited: Vec<(String, u64)> = Vec::new();
        for fi in (0..self.frames.len()).rev() {
            let frame = &self.frames[fi];
            // Cells this frame already references (it is itself a closure body).
            for (name, cell) in &frame.env {
                if seen.insert(name.clone()) {
                    inherited.push((name.clone(), *cell));
                }
            }
            let local_names = if let Some(fidx) = frame.func_index {
                &self.program(frame.program_index).functions[fidx].local_names
            } else {
                &self.program(frame.program_index).local_names
            };
            for (slot, name) in local_names.iter().enumerate() {
                if seen.insert(name.clone()) {
                    to_box.push((fi, slot, name.clone()));
                }
            }
        }
        let mut env: Vec<(String, u64)> = Vec::with_capacity(to_box.len() + inherited.len());
        for (fi, slot, name) in to_box {
            let cell = self.box_local(fi, slot);
            env.push((name, cell));
        }
        env.extend(inherited);
        // User globals (functions, top-level consts) are captured by value.
        let mut captured = Vec::new();
        for (name, val) in &self.globals {
            if !Self::BUILTIN_GLOBAL_NAMES.contains(&name.as_str()) && !seen.contains(name) {
                captured.push((name.clone(), val.clone()));
            }
        }
        Closure {
            func_ref: FunctionRef {
                program_id: self.current_frame().program_index,
                function_id: func_idx,
            },
            captured,
            env,
        }
    }

    /// Read a local, routing through its cell if the slot has been boxed.
    fn read_local(&self, frame_index: usize, slot: usize) -> Value {
        let Some(frame) = self.frames.get(frame_index) else {
            return Value::Undefined;
        };
        if let Some(&cell) = frame.boxed.get(&slot) {
            self.cells
                .get(cell as usize)
                .cloned()
                .unwrap_or(Value::Undefined)
        } else {
            frame.locals.get(slot).cloned().unwrap_or(Value::Undefined)
        }
    }

    /// Write a local, routing through its cell if the slot has been boxed.
    fn write_local(&mut self, frame_index: usize, slot: usize, value: Value) {
        let cell = self
            .frames
            .get(frame_index)
            .and_then(|f| f.boxed.get(&slot).copied());
        if let Some(cell) = cell {
            if let Some(slot_ref) = self.cells.get_mut(cell as usize) {
                *slot_ref = value;
            }
            return;
        }
        if let Some(f) = self.frames.get_mut(frame_index) {
            while f.locals.len() <= slot {
                f.locals.push(Value::Undefined);
            }
            f.locals[slot] = value;
        }
    }

    fn current_frame(&self) -> &CallFrame {
        // Frames are always non-empty during execution (run() pushes the initial frame).
        // This is an internal invariant, not reachable by guest code.
        self.frames.last().expect("internal error: no active frame")
    }

    fn current_frame_mut(&mut self) -> &mut CallFrame {
        self.frames
            .last_mut()
            .expect("internal error: no active frame")
    }

    fn program(&self, program_index: usize) -> &CompiledProgram {
        self.programs
            .get(program_index)
            .expect("internal error: invalid program index")
    }

    fn current_program(&self) -> &CompiledProgram {
        self.program(self.current_frame().program_index)
    }

    fn current_function(&self, func_ref: FunctionRef) -> &crate::compiler::CompiledFunction {
        self.program(func_ref.program_id)
            .functions
            .get(func_ref.function_id)
            .expect("internal error: invalid function reference")
    }

    #[allow(dead_code)]
    fn instructions(&self) -> &[Instruction] {
        match self.current_frame().func_index {
            Some(idx) => &self.current_program().functions[idx].instructions,
            None => &self.current_program().instructions,
        }
    }

    /// Build the locals vec by binding `args` to the function's declared `params`.
    /// Handles positional, rest, and default-value patterns.
    fn bind_params(
        params: &[ParamPattern],
        args: &[Value],
        local_count: usize,
        needs_arguments: bool,
        heap: &mut Heap,
    ) -> Vec<Value> {
        let mut locals = Vec::with_capacity(local_count);
        for (i, param) in params.iter().enumerate() {
            match param {
                ParamPattern::Ident(_) => {
                    locals.push(args.get(i).cloned().unwrap_or(Value::Undefined));
                }
                ParamPattern::Rest(_) => {
                    let rest: Vec<Value> = args.get(i..).map(|s| s.to_vec()).unwrap_or_default();
                    locals.push(Value::Array(heap.alloc_array(rest)));
                }
                ParamPattern::DefaultValue { pattern, .. } => {
                    let val = args.get(i).cloned().unwrap_or(Value::Undefined);
                    match pattern.as_ref() {
                        // `function f(p = …)`: one local holds the (possibly
                        // undefined) argument; the compiler-emitted default fires.
                        ParamPattern::Ident(_) | ParamPattern::Rest(_) => locals.push(val),
                        // `function f({a} = …)` / `function f([a] = …)`: push the
                        // raw argument as a hidden temp FIRST, then the extracted
                        // leaves. The compiler re-destructures the temp's default
                        // into the leaves when the argument is undefined.
                        nested => {
                            locals.push(val.clone());
                            extract_pattern(nested, &val, &mut locals, heap);
                        }
                    }
                }
                // Destructuring params bind multiple locals in declaration order;
                // extract the fields from the argument into those slots.
                ParamPattern::ObjectDestructure(_) | ParamPattern::ArrayDestructure(_) => {
                    let arg = args.get(i).cloned().unwrap_or(Value::Undefined);
                    extract_pattern(param, &arg, &mut locals, heap);
                }
            }
        }
        // `arguments`: an array-like of ALL passed arguments, bound right after
        // the param-derived locals (its slot was reserved by the compiler).
        if needs_arguments {
            locals.push(Value::Array(heap.alloc_array(args.to_vec())));
        }
        locals
    }

    /// Common setup for calling a closure: inject captures, bind params, push frame.
    fn push_call_frame(
        &mut self,
        closure: &Closure,
        args: &[Value],
        this_value: Option<Value>,
    ) -> Result<()> {
        self.tracker.push_frame();
        self.tracker.check_stack(&self.limits)?;

        // Inject captured-by-value variables as globals
        for (name, val) in &closure.captured {
            if !self.globals.contains_key(name) {
                self.globals.insert(name.clone(), val.clone());
            }
        }

        let func = self.current_function(closure.func_ref);
        let params = func.params.clone();
        let local_count = func.local_count;
        let needs_arguments = func.needs_arguments;
        let locals = Self::bind_params(&params, args, local_count, needs_arguments, &mut self.heap);

        // If this is a method call (has this_value from a receiver), transfer
        // the receiver source so we can write back mutations on return.
        let receiver_source = if this_value.is_some() {
            self.last_receiver_source.take()
        } else {
            self.last_receiver_source = None;
            None
        };

        // Captured-by-reference variables: the frame's env maps each name to its
        // shared cell so LoadGlobal/StoreGlobal in the body see and mutate it.
        let env: HashMap<String, u64> = closure.env.iter().cloned().collect();

        self.frames.push(CallFrame {
            program_index: closure.func_ref.program_id,
            func_index: Some(closure.func_ref.function_id),
            ip: 0,
            locals,
            stack_base: self.stack.len(),
            this_value,
            receiver_source,
            boxed: HashMap::new(),
            env,
        });
        Ok(())
    }

    fn run(&mut self) -> Result<VmState> {
        self.run_program(0)
    }

    pub(crate) fn run_program(&mut self, program_index: usize) -> Result<VmState> {
        self.tracker.start();

        // Set up top-level frame
        self.frames.push(CallFrame {
            program_index,
            func_index: None,
            ip: 0,
            locals: Vec::new(),
            stack_base: 0,
            this_value: None,
            receiver_source: None,
            boxed: HashMap::new(),
            env: HashMap::new(),
        });

        self.execute()
    }

    fn execute(&mut self) -> Result<VmState> {
        loop {
            // Resource checks
            self.tracker.check_time(&self.limits)?;

            let frame = self.frames.last().unwrap();
            let instructions = match frame.func_index {
                Some(idx) => &self.program(frame.program_index).functions[idx].instructions,
                None => &self.program(frame.program_index).instructions,
            };

            if frame.ip >= instructions.len() {
                // End of function/program
                if self.frames.len() <= 1 {
                    // Top-level: return last value on stack or undefined
                    let result = if self.stack.is_empty() {
                        Value::Undefined
                    } else {
                        self.stack.pop().unwrap_or(Value::Undefined)
                    };
                    return Ok(VmState::Complete(result));
                } else {
                    // Return from function
                    let frame = self.frames.pop().unwrap();
                    self.tracker.pop_frame();
                    // If this was a constructor, return `this`
                    if let Some(this_val) = frame.this_value {
                        self.stack.truncate(frame.stack_base);
                        self.push(this_val)?;
                    } else {
                        self.push(Value::Undefined)?;
                    }
                    // Check if a continuation callback just completed
                    if let Some(state) = self.process_continuation()? {
                        return Ok(state);
                    }
                    continue;
                }
            }

            let instr = instructions[frame.ip].clone();
            let result = self.dispatch(instr);

            match result {
                Ok(Some(state)) => return Ok(state),
                Ok(None) => {
                    // After dispatch, check if a continuation callback returned
                    // (via Return instruction or ip overflow). A continuation may
                    // itself suspend — e.g. a promise callback that returned a
                    // deferred single-call promise, which must be forced (N5).
                    if let Some(state) = self.process_continuation()? {
                        return Ok(state);
                    }
                }
                Err(err) => {
                    // Route the error to the nearest catch or finally. If no
                    // handler remains it propagates to the host.
                    let error_val = self.caught_error_value(&err);
                    if !self.route_thrown(error_val, 0)? {
                        return Err(err);
                    }
                }
            }
        }
    }

    /// Process the top continuation if the current frame depth indicates a callback
    /// has returned. Returns `Ok(Some(state))` if processing the continuation must
    /// suspend the VM (e.g. a promise callback returned a deferred single-call
    /// promise that has to be forced), otherwise `Ok(None)` (the caller simply
    /// proceeds to the next execute-loop iteration, whether or not a continuation
    /// was advanced).
    fn process_continuation(&mut self) -> Result<Option<VmState>> {
        let cont = match self.continuations.last() {
            Some(c) => c,
            None => return Ok(None),
        };

        // Check if the callback's specific frame has been popped — only then
        // has the callback returned. This avoids false triggers when inner
        // helper functions return to the same depth.
        let (callback_frame_index, caller_frame_depth) = match cont {
            Continuation::ArrayMap {
                callback_frame_index,
                caller_frame_depth,
                ..
            } => (*callback_frame_index, *caller_frame_depth),
            Continuation::ArrayForEach {
                callback_frame_index,
                caller_frame_depth,
                ..
            } => (*callback_frame_index, *caller_frame_depth),
            Continuation::PromiseCallback {
                callback_frame_index,
                caller_frame_depth,
                ..
            } => (*callback_frame_index, *caller_frame_depth),
        };

        // The callback frame is still active — not done yet
        if self.frames.len() > callback_frame_index {
            return Ok(None);
        }

        // Guard against stale continuations on stack unwinds — we must be
        // back at the original caller's frame depth.
        if self.frames.len() != caller_frame_depth {
            return Ok(None);
        }

        // The callback just returned — collect its result from the stack.
        // The compiler always emits PushUndefined+Return for implicit returns,
        // so an empty stack here indicates a VM bug.
        let callback_result = self.pop()?;

        // `PromiseCallback` continuations re-wrap the callback's return value
        // into the chain's next promise (see `make_resolved_promise`), so we
        // must NOT eagerly unwrap/error on internal promises here — that is the
        // promise-arm's job. Array continuations, by contrast, expect a plain
        // value per element, so they unwrap a resolved promise and treat a
        // rejected one as an unhandled rejection.
        let is_promise_cont = matches!(
            self.continuations.last(),
            Some(Continuation::PromiseCallback { .. })
        );

        // Unwrap internal promise values: async callbacks return
        // {__promise__: true, status: "resolved", value: X} or {status: "rejected", ...}.
        // Only unwrap objects with the __promise__ marker to avoid mangling user objects.
        let callback_result = if is_promise_cont {
            callback_result
        } else if let Value::Object(h) = &callback_result {
            let map = self.heap.object_map(*h);
            if !matches!(map.get("__promise__"), Some(Value::Bool(true))) {
                // Not an internal promise — leave untouched
                callback_result
            } else {
                match map.get("status") {
                    Some(Value::String(s)) if s.as_ref() == "resolved" => {
                        map.get("value").cloned().unwrap_or(Value::Undefined)
                    }
                    Some(Value::String(s)) if s.as_ref() == "rejected" => {
                        let reason = map.get("reason").cloned().unwrap_or(Value::Undefined);
                        let reason = reason.to_js_string(&self.heap);
                        // Clean up the continuation before returning error
                        self.continuations.pop();
                        return Err(ZapcodeError::RuntimeError(format!(
                            "Unhandled promise rejection: {}",
                            reason
                        )));
                    }
                    _ => callback_result,
                }
            }
        } else {
            callback_result
        };

        // Pop the continuation, take ownership to avoid cloning results
        let cont = self.continuations.pop().unwrap();

        match cont {
            Continuation::ArrayMap {
                callback,
                source,
                mut results,
                next_index,
                caller_frame_depth,
                ..
            } => {
                results.push(callback_result);
                let next = next_index + 1;

                if next < source.len() {
                    // Set up next callback call
                    let item = source[next].clone();
                    let closure = match &callback {
                        Value::Function(c) => c.clone(),
                        _ => unreachable!("callback validated at start"),
                    };
                    self.push_call_frame(&closure, &[item, Value::Int(next as i64)], None)?;
                    let new_frame_index = self.frames.len() - 1;
                    // Push updated continuation back
                    self.continuations.push(Continuation::ArrayMap {
                        callback,
                        source,
                        results,
                        next_index: next,
                        caller_frame_depth,
                        callback_frame_index: new_frame_index,
                    });
                    Ok(None)
                } else {
                    // All done — push final array.
                    let h = self.heap.alloc_array(results);
                    self.push(Value::Array(h))?;
                    Ok(None)
                }
            }
            Continuation::ArrayForEach {
                callback,
                source,
                next_index,
                caller_frame_depth,
                ..
            } => {
                let next = next_index + 1;

                if next < source.len() {
                    let item = source[next].clone();
                    let closure = match &callback {
                        Value::Function(c) => c.clone(),
                        _ => unreachable!("callback validated at start"),
                    };
                    self.push_call_frame(&closure, &[item, Value::Int(next as i64)], None)?;
                    let new_frame_index = self.frames.len() - 1;
                    self.continuations.push(Continuation::ArrayForEach {
                        callback,
                        source,
                        next_index: next,
                        caller_frame_depth,
                        callback_frame_index: new_frame_index,
                    });
                    Ok(None)
                } else {
                    self.push(Value::Undefined)?;
                    Ok(None)
                }
            }
            Continuation::PromiseCallback {
                mode,
                original_promise,
                ..
            } => {
                // If the callback returned a *deferred single-call promise* (a bare
                // tool call, N5), the chain must adopt it: force its host call now
                // and let the settled value flow into the chain. This makes
                // `.then(() => tool())`, `.catch(() => tool())`, and
                // `.finally(() => tool())` all drive the deferred call (matching JS
                // thenable adoption), and preserves the pre-N5 eager behavior where
                // a bare tool call inside a callback ran the tool.
                if let Value::Object(h) = &callback_result {
                    let is_pending_call = matches!(
                        self.heap.object(*h).and_then(|m| m.get("status")),
                        Some(Value::String(s)) if s.as_ref() == "pending_call"
                    );
                    if is_pending_call {
                        let id = match self.heap.object(*h).and_then(|m| m.get("__call_id__")) {
                            Some(Value::Int(n)) => *n as u64,
                            _ => {
                                return Err(ZapcodeError::RuntimeError(
                                    "internal error: pending_call promise missing __call_id__"
                                        .to_string(),
                                ))
                            }
                        };
                        self.resume_action = Some(ResumeAction::ChainResult {
                            mode,
                            original_promise,
                        });
                        return self.suspend_on_pending_call(id);
                    }
                }
                // The promise callback finished (possibly after suspending for a
                // tool call). Shape its result into the chain's next promise.
                let chain_result = match mode {
                    // `.then`/`.catch`: the callback's return value becomes the
                    // resolved value of the next promise. `make_resolved_promise`
                    // unwraps a returned promise (thenable) so chaining works.
                    PromiseCallbackMode::WrapResult => {
                        builtins::make_resolved_promise(callback_result, &mut self.heap)
                    }
                    // `.finally`: ignore the callback's return value and pass the
                    // original promise through unchanged.
                    PromiseCallbackMode::PassThrough => original_promise,
                };
                self.push(chain_result)?;
                Ok(None)
            }
        }
    }

    /// JS ToPrimitive ([Symbol.toPrimitive] / valueOf / toString) for a heap
    /// Object that defines a callable hook. Returns the primitive a user hook
    /// produced, or the original value unchanged when no usable hook exists (so
    /// the caller's built-in `to_js_string` / `to_number_heap` still applies —
    /// e.g. a plain `{}` becomes `"[object Object]"` / `NaN`).
    ///
    /// Only `Value::Object` carries user hooks; everything else (primitives,
    /// arrays, functions, dates) is returned as-is so existing coercion behavior
    /// is untouched. Per spec the method order is valueOf-then-toString for the
    /// "number"/"default" hints and toString-then-valueOf for the "string" hint;
    /// a method whose result is itself an object is skipped.
    ///
    /// Symbol.toPrimitive is not dispatched: this crate's Symbol support is a
    /// stub (no well-known symbols, object property keys are strings), so a
    /// `[Symbol.toPrimitive]` computed key can't be reliably matched. See
    /// STRESS-PASS-BUGS.md.
    fn to_primitive(&mut self, value: &Value, hint: ToPrimitiveHint) -> Result<Value> {
        let handle = match value {
            Value::Object(h) => *h,
            // No user hooks on non-objects; let the built-in coercion handle it.
            _ => return Ok(value.clone()),
        };

        // Order the method names per the requested hint.
        let methods: [&str; 2] = match hint {
            ToPrimitiveHint::String => ["toString", "valueOf"],
            ToPrimitiveHint::Number | ToPrimitiveHint::Default => ["valueOf", "toString"],
        };

        // Bound the re-entrancy so a hook that itself coerces objects can't loop
        // forever (and so a tool-call-suspending hook is caught at a shallow depth).
        // Each level nests a full guest-call interpreter loop on the *native* stack,
        // so the cap must be low enough that a cyclic hook (e.g.
        // `toString(){ return "" + this }`) is caught with a clean RuntimeError
        // rather than overflowing the Rust stack and aborting the host process.
        // 8 is far above any legitimate valueOf->toString fallback nesting (which
        // is at most a couple of levels) yet well below native-stack exhaustion.
        if self.to_primitive_depth >= 8 {
            return Err(ZapcodeError::RuntimeError(
                "ToPrimitive recursion limit exceeded (cyclic valueOf/toString?)".to_string(),
            ));
        }

        for name in methods {
            // Read the hook out of the object (clone-out to avoid borrowing the
            // heap across the guest call). Only an own callable field counts.
            let hook = match self.heap.object(handle).and_then(|m| m.get(name)) {
                Some(Value::Function(c)) => Value::Function(c.clone()),
                _ => continue,
            };

            self.to_primitive_depth += 1;
            let called = self.call_method_internal(&hook, value.clone(), Vec::new());
            self.to_primitive_depth -= 1;
            let result = called?;

            // A primitive result wins; an object/array result is ignored (try the
            // next hook), matching OrdinaryToPrimitive.
            if !matches!(result, Value::Object(_) | Value::Array(_)) {
                return Ok(result);
            }
        }

        // No hook produced a primitive: fall back to the original object so the
        // caller's built-in coercion runs (plain object -> "[object Object]"/NaN).
        Ok(value.clone())
    }

    /// Like [`Self::call_function_internal`] but binds `this` to `receiver`, used
    /// when invoking a user `valueOf`/`toString` hook so the hook body can read
    /// `this`.
    fn call_method_internal(
        &mut self,
        callee: &Value,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        self.last_receiver_source = None;
        self.call_closure_internal(callee, args, Some(receiver))
    }

    /// Call a function value with the given arguments and run it to completion.
    /// Returns the function's return value.
    fn call_function_internal(&mut self, callee: &Value, args: Vec<Value>) -> Result<Value> {
        self.call_closure_internal(callee, args, None)
    }

    /// Shared body for the internal-call helpers: push a frame (optionally with a
    /// bound `this`) and run it to completion, returning the result.
    fn call_closure_internal(
        &mut self,
        callee: &Value,
        args: Vec<Value>,
        this_value: Option<Value>,
    ) -> Result<Value> {
        let closure = match callee {
            Value::Function(c) => c.clone(),
            other => {
                let msg = other.to_js_string(&self.heap);
                return Err(ZapcodeError::TypeError(format!("{} is not a function", msg)));
            }
        };

        let target_frame_depth = self.frames.len();
        self.push_call_frame(&closure, &args, this_value)?;

        // Run until the new frame returns
        loop {
            self.tracker.check_time(&self.limits)?;

            let frame = self.frames.last().unwrap();
            let instructions = match frame.func_index {
                Some(idx) => &self.program(frame.program_index).functions[idx].instructions,
                None => &self.program(frame.program_index).instructions,
            };

            if frame.ip >= instructions.len() {
                // End of function without explicit return
                if self.frames.len() > target_frame_depth + 1 {
                    // Inner function ended, pop and continue
                    self.frames.pop();
                    self.tracker.pop_frame();
                    self.push(Value::Undefined)?;
                    continue;
                } else {
                    // Our target function ended
                    self.frames.pop();
                    self.tracker.pop_frame();
                    return Ok(Value::Undefined);
                }
            }

            let instr = instructions[frame.ip].clone();
            let result = self.dispatch(instr);

            match result {
                Ok(Some(VmState::Complete(val))) => {
                    // A return happened that completed the top-level program.
                    // This shouldn't happen inside a callback but handle gracefully.
                    return Ok(val);
                }
                Ok(Some(VmState::Suspended { .. })) | Ok(Some(VmState::SuspendedMany { .. })) => {
                    return Err(ZapcodeError::RuntimeError(
                        "cannot call an external function inside an array-callback method \
                         (.map/.filter/.forEach/.reduce/...). Use a `for...of` loop with `await`, \
                         or `await Promise.all([toolA(), toolB(), ...])` with the calls written \
                         directly as array elements."
                            .to_string(),
                    ));
                }
                Ok(None) => {
                    // Check if the frame was popped by a Return instruction
                    if self.frames.len() == target_frame_depth {
                        // The function returned; return value is on the stack
                        return Ok(self.pop().unwrap_or(Value::Undefined));
                    }
                }
                Err(err) => {
                    // Try to catch (or run a finally) within the callback. Only a
                    // handler that lives *inside* this internal call (above the
                    // frame this call started at) may engage here; an outer
                    // handler belongs to the caller and is left for it.
                    let error_val = self.caught_error_value(&err);
                    if !self.route_thrown(error_val, target_frame_depth)? {
                        // Unwind the frame(s) this call pushed so the frame stack is
                        // restored to the caller's depth before propagating. Without
                        // this, a nested internal call (e.g. a cyclic ToPrimitive
                        // hook) that errors would leave orphaned frames and later
                        // desync the interpreter loop (`frames.last().unwrap()`).
                        while self.frames.len() > target_frame_depth {
                            self.frames.pop();
                            self.tracker.pop_frame();
                        }
                        return Err(err);
                    }
                }
            }
        }
    }

    /// Call a callback for each array element. Passes (item, index) — the full
    /// array reference (3rd JS argument) is only built lazily if the callback
    /// actually uses 3+ params, avoiding O(n²) cloning.
    fn call_element_callback(
        &mut self,
        callback: &Value,
        item: &Value,
        index: usize,
    ) -> Result<Value> {
        self.call_function_internal(callback, vec![item.clone(), Value::Int(index as i64)])
    }

    /// Check if a callback value is an async function that might suspend.
    fn is_async_callback(&self, callback: &Value) -> bool {
        if let Value::Function(closure) = callback {
            return self.current_function(closure.func_ref).is_async;
        }
        false
    }

    /// Start a continuation-based `.map()` call: push the continuation and set up
    /// the first callback invocation. Returns `None` to signal that the main
    /// `execute()` loop should drive the iteration.
    fn start_continuation_map(
        &mut self,
        callback: Value,
        arr: Vec<Value>,
    ) -> Result<Option<Value>> {
        if arr.is_empty() {
            let h = self.heap.alloc_array(Vec::new());
            return Ok(Some(Value::Array(h)));
        }

        // Validate callback type BEFORE pushing continuation
        let closure = match &callback {
            Value::Function(c) => c.clone(),
            _ => {
                return Err(ZapcodeError::TypeError(
                    "map callback is not a function".to_string(),
                ))
            }
        };

        let caller_frame_depth = self.frames.len();
        let first_item = arr[0].clone();

        self.push_call_frame(&closure, &[first_item, Value::Int(0)], None)?;
        let callback_frame_index = self.frames.len() - 1;

        self.continuations.push(Continuation::ArrayMap {
            callback,
            source: arr,
            results: Vec::new(),
            next_index: 0,
            caller_frame_depth,
            callback_frame_index,
        });

        Ok(None) // Signal: continuation in progress
    }

    /// Start a continuation-based `.forEach()` call.
    fn start_continuation_foreach(
        &mut self,
        callback: Value,
        arr: Vec<Value>,
    ) -> Result<Option<Value>> {
        if arr.is_empty() {
            return Ok(Some(Value::Undefined));
        }

        // Validate callback type BEFORE pushing continuation
        let closure = match &callback {
            Value::Function(c) => c.clone(),
            _ => {
                return Err(ZapcodeError::TypeError(
                    "forEach callback is not a function".to_string(),
                ))
            }
        };

        let caller_frame_depth = self.frames.len();
        let first_item = arr[0].clone();

        self.push_call_frame(&closure, &[first_item, Value::Int(0)], None)?;
        let callback_frame_index = self.frames.len() - 1;

        self.continuations.push(Continuation::ArrayForEach {
            callback,
            source: arr,
            next_index: 0,
            caller_frame_depth,
            callback_frame_index,
        });

        Ok(None)
    }

    /// Execute an array callback method (map, filter, reduce, forEach, etc.)
    /// Returns `Ok(Some(value))` if the method completed synchronously, or
    /// `Ok(None)` if a continuation was started (async callback).
    fn execute_array_callback_method(
        &mut self,
        handle: Handle,
        method: &str,
        all_args: Vec<Value>,
    ) -> Result<Option<Value>> {
        let callback = all_args.first().cloned().unwrap_or(Value::Undefined);
        // Snapshot the elements; callbacks may run guest code, so we iterate over
        // the snapshot (clone-out) rather than holding a heap borrow. `sort`
        // re-reads/writes the live slot via `handle` for in-place semantics.
        let arr = self.heap.array_vec(handle);

        match method {
            "map" => {
                // Use continuation-based execution for async callbacks
                if self.is_async_callback(&callback) {
                    return self.start_continuation_map(callback, arr);
                }
                let mut result = Vec::with_capacity(arr.len());
                for (i, item) in arr.iter().enumerate() {
                    result.push(self.call_element_callback(&callback, item, i)?);
                }
                let h = self.heap.alloc_array(result);
                Ok(Some(Value::Array(h)))
            }
            "filter" | "find" | "findIndex" | "findLast" | "findLastIndex" | "every" | "some"
            | "reduce" | "reduceRight" | "sort" | "flatMap" => {
                // Async callbacks are not supported for these methods
                if self.is_async_callback(&callback) {
                    return Err(ZapcodeError::RuntimeError(format!(
                        ".{}() does not support async callbacks — use .map() or a for-of loop instead",
                        method
                    )));
                }
                match method {
                    "filter" => {
                        let mut result = Vec::new();
                        for (i, item) in arr.iter().enumerate() {
                            if self.call_element_callback(&callback, item, i)?.is_truthy() {
                                result.push(item.clone());
                            }
                        }
                        let h = self.heap.alloc_array(result);
                        Ok(Some(Value::Array(h)))
                    }
                    "find" => {
                        for (i, item) in arr.iter().enumerate() {
                            if self.call_element_callback(&callback, item, i)?.is_truthy() {
                                return Ok(Some(item.clone()));
                            }
                        }
                        Ok(Some(Value::Undefined))
                    }
                    "findIndex" => {
                        for (i, item) in arr.iter().enumerate() {
                            if self.call_element_callback(&callback, item, i)?.is_truthy() {
                                return Ok(Some(Value::Int(i as i64)));
                            }
                        }
                        Ok(Some(Value::Int(-1)))
                    }
                    "findLast" => {
                        for (i, item) in arr.iter().enumerate().rev() {
                            if self.call_element_callback(&callback, item, i)?.is_truthy() {
                                return Ok(Some(item.clone()));
                            }
                        }
                        Ok(Some(Value::Undefined))
                    }
                    "findLastIndex" => {
                        for (i, item) in arr.iter().enumerate().rev() {
                            if self.call_element_callback(&callback, item, i)?.is_truthy() {
                                return Ok(Some(Value::Int(i as i64)));
                            }
                        }
                        Ok(Some(Value::Int(-1)))
                    }
                    "every" => {
                        for (i, item) in arr.iter().enumerate() {
                            if !self.call_element_callback(&callback, item, i)?.is_truthy() {
                                return Ok(Some(Value::Bool(false)));
                            }
                        }
                        Ok(Some(Value::Bool(true)))
                    }
                    "some" => {
                        for (i, item) in arr.iter().enumerate() {
                            if self.call_element_callback(&callback, item, i)?.is_truthy() {
                                return Ok(Some(Value::Bool(true)));
                            }
                        }
                        Ok(Some(Value::Bool(false)))
                    }
                    "reduce" => {
                        let mut acc = match all_args.get(1).cloned() {
                            Some(init) => Some(init),
                            None if !arr.is_empty() => Some(arr[0].clone()),
                            None => {
                                return Err(ZapcodeError::TypeError(
                                    "Reduce of empty array with no initial value".to_string(),
                                ));
                            }
                        };
                        let start = if all_args.get(1).is_some() { 0 } else { 1 };
                        for (i, item) in arr.iter().enumerate().skip(start) {
                            acc = Some(self.call_function_internal(
                                &callback,
                                vec![acc.unwrap(), item.clone(), Value::Int(i as i64)],
                            )?);
                        }
                        Ok(Some(acc.unwrap_or(Value::Undefined)))
                    }
                    "reduceRight" => {
                        let n = arr.len();
                        let mut acc = match all_args.get(1).cloned() {
                            Some(init) => Some(init),
                            None if n > 0 => Some(arr[n - 1].clone()),
                            None => {
                                return Err(ZapcodeError::TypeError(
                                    "Reduce of empty array with no initial value".to_string(),
                                ));
                            }
                        };
                        // Iterate from the end; skip the seed element when no
                        // initial value was supplied.
                        let skip_last = all_args.get(1).is_none();
                        for (i, item) in arr.iter().enumerate().rev() {
                            if skip_last && i == n - 1 {
                                continue;
                            }
                            acc = Some(self.call_function_internal(
                                &callback,
                                vec![acc.unwrap(), item.clone(), Value::Int(i as i64)],
                            )?);
                        }
                        Ok(Some(acc.unwrap_or(Value::Undefined)))
                    }
                    "sort" => {
                        let mut result = arr;
                        if matches!(callback, Value::Function(_)) {
                            let len = result.len();
                            for i in 1..len {
                                let mut j = i;
                                while j > 0 {
                                    let cmp = self
                                        .call_function_internal(
                                            &callback,
                                            vec![result[j - 1].clone(), result[j].clone()],
                                        )?
                                        .to_number();
                                    if cmp > 0.0 {
                                        result.swap(j - 1, j);
                                        j -= 1;
                                    } else {
                                        break;
                                    }
                                }
                            }
                        } else {
                            let heap = &self.heap;
                            result.sort_by_key(|a| a.to_js_string(heap));
                        }
                        // sort() mutates in place and returns the same array.
                        self.heap.set_array(handle, result);
                        Ok(Some(Value::Array(handle)))
                    }
                    "flatMap" => {
                        let mut result = Vec::new();
                        for (i, item) in arr.iter().enumerate() {
                            match self.call_element_callback(&callback, item, i)? {
                                Value::Array(inner) => result.extend(self.heap.array_vec(inner)),
                                other => result.push(other),
                            }
                        }
                        let h = self.heap.alloc_array(result);
                        Ok(Some(Value::Array(h)))
                    }
                    _ => unreachable!(),
                }
            }
            "forEach" => {
                // Use continuation-based execution for async callbacks
                if self.is_async_callback(&callback) {
                    return self.start_continuation_foreach(callback, arr);
                }
                for (i, item) in arr.iter().enumerate() {
                    self.call_element_callback(&callback, item, i)?;
                }
                Ok(Some(Value::Undefined))
            }
            _ => Err(ZapcodeError::TypeError(format!(
                "Unknown array callback method: {}",
                method
            ))),
        }
    }

    /// Execute .then(), .catch(), or .finally() on a resolved/rejected promise.
    ///
    /// The callback is run via the continuation machinery (a
    /// [`Continuation::PromiseCallback`]) rather than synchronously, so a tool
    /// (external) call inside the callback can suspend the VM and resume — this
    /// is what makes `primary().catch(() => fallbackTool())` work. The main
    /// `execute()` loop drives the callback; when it returns, the continuation
    /// shapes the result into the chain's next promise.
    ///
    /// Returns:
    /// - `PromiseMethodOutcome::Value(v)` — completed synchronously (no callback
    ///   to run); push `v`.
    /// - `PromiseMethodOutcome::ContinuationStarted` — a callback frame and
    ///   continuation were pushed; the caller must return `Ok(None)` so the
    ///   main loop drives it.
    /// - `PromiseMethodOutcome::NotAPromise` — the receiver was not a promise /
    ///   the method is unknown; fall through to the normal error path.
    fn execute_promise_method(
        &mut self,
        promise: Value,
        method: &str,
        args: Vec<Value>,
    ) -> Result<PromiseMethodOutcome> {
        let (status, value, reason) = if let Value::Object(h) = &promise {
            let map = self.heap.object_map(*h);
            let status = match map.get("status") {
                Some(Value::String(s)) => s.to_string(),
                _ => "pending".to_string(),
            };
            // A deferred single-call promise (N5): `.then`/`.catch`/`.finally`
            // forces its host call to settle. Suspend on the call now, recording
            // a `ResumeAction::PromiseMethod` so that on resume the settled value
            // is wrapped in a resolved promise and the method (with its callbacks)
            // re-runs through the normal promise-method path — which itself
            // supports a tool call inside the callback (N4). A *rejection* is
            // delivered via `resume_with_error`, which raises at the call site and
            // is shaped into a rejected promise there.
            if status == "pending_call" {
                let id = match map.get("__call_id__") {
                    Some(Value::Int(n)) => *n as u64,
                    _ => {
                        return Err(ZapcodeError::RuntimeError(
                            "internal error: pending_call promise missing __call_id__".to_string(),
                        ))
                    }
                };
                // Already settled (the promise was awaited before): run the method
                // synchronously against the cached resolved value — no re-invoke.
                if let Some(cached) = self.resolved.get(&id).cloned() {
                    let resolved = builtins::make_resolved_promise(cached, &mut self.heap);
                    return self.execute_promise_method(resolved, method, args);
                }
                self.resume_action = Some(ResumeAction::PromiseMethod {
                    method: method.to_string(),
                    args,
                });
                return match self.suspend_on_pending_call(id)? {
                    Some(state) => Ok(PromiseMethodOutcome::Suspend(state)),
                    None => unreachable!("suspend_on_pending_call always suspends"),
                };
            }
            let value = map.get("value").cloned().unwrap_or(Value::Undefined);
            let reason = map.get("reason").cloned().unwrap_or(Value::Undefined);
            (status, value, reason)
        } else {
            return Ok(PromiseMethodOutcome::NotAPromise);
        };

        let on_fulfilled = args.first().cloned().unwrap_or(Value::Undefined);
        let on_rejected = args.get(1).cloned().unwrap_or(Value::Undefined);

        match method {
            "then" => {
                if status == "resolved" {
                    if matches!(on_fulfilled, Value::Function(_)) {
                        self.start_promise_callback(
                            on_fulfilled,
                            vec![value],
                            PromiseCallbackMode::WrapResult,
                            promise,
                        )
                    } else {
                        // No callback — pass through the promise
                        Ok(PromiseMethodOutcome::Value(promise))
                    }
                } else if status == "rejected" {
                    if matches!(on_rejected, Value::Function(_)) {
                        self.start_promise_callback(
                            on_rejected,
                            vec![reason],
                            PromiseCallbackMode::WrapResult,
                            promise,
                        )
                    } else {
                        // No onRejected — pass through the rejection
                        Ok(PromiseMethodOutcome::Value(promise))
                    }
                } else {
                    Ok(PromiseMethodOutcome::Value(promise))
                }
            }
            "catch" => {
                if status == "rejected" {
                    let handler = args.first().cloned().unwrap_or(Value::Undefined);
                    if matches!(handler, Value::Function(_)) {
                        self.start_promise_callback(
                            handler,
                            vec![reason],
                            PromiseCallbackMode::WrapResult,
                            promise,
                        )
                    } else {
                        Ok(PromiseMethodOutcome::Value(promise))
                    }
                } else {
                    // Resolved — pass through
                    Ok(PromiseMethodOutcome::Value(promise))
                }
            }
            "finally" => {
                let handler = args.first().cloned().unwrap_or(Value::Undefined);
                if matches!(handler, Value::Function(_)) {
                    // finally callback receives no arguments and its return value
                    // is discarded — the original promise passes through.
                    self.start_promise_callback(
                        handler,
                        vec![],
                        PromiseCallbackMode::PassThrough,
                        promise,
                    )
                } else {
                    // No handler — pass through the original promise
                    Ok(PromiseMethodOutcome::Value(promise))
                }
            }
            _ => Ok(PromiseMethodOutcome::NotAPromise),
        }
    }

    /// Push a promise-callback frame plus a [`Continuation::PromiseCallback`] so
    /// the main `execute()` loop drives the callback. This lets a tool call
    /// inside the callback suspend the VM (the continuation is part of the
    /// serialized state and resumes cleanly).
    fn start_promise_callback(
        &mut self,
        callback: Value,
        args: Vec<Value>,
        mode: PromiseCallbackMode,
        original_promise: Value,
    ) -> Result<PromiseMethodOutcome> {
        let closure = match &callback {
            Value::Function(c) => c.clone(),
            _ => {
                return Err(ZapcodeError::TypeError(
                    "promise callback is not a function".to_string(),
                ))
            }
        };

        let caller_frame_depth = self.frames.len();
        self.push_call_frame(&closure, &args, None)?;
        let callback_frame_index = self.frames.len() - 1;

        self.continuations.push(Continuation::PromiseCallback {
            mode,
            original_promise,
            caller_frame_depth,
            callback_frame_index,
        });

        Ok(PromiseMethodOutcome::ContinuationStarted)
    }

    fn alloc_generator_id(&mut self) -> u64 {
        let id = self.next_generator_id;
        self.next_generator_id += 1;
        id
    }

    fn generator_next(&mut self, mut gen_obj: GeneratorObject, arg: Value) -> Result<Value> {
        if gen_obj.done {
            return Ok(self.make_iterator_result(Value::Undefined, true));
        }
        for (name, val) in &gen_obj.captured {
            if !self.globals.contains_key(name) {
                self.globals.insert(name.clone(), val.clone());
            }
        }
        let func_ref = gen_obj.func_ref;
        match gen_obj.suspended.take() {
            None => {
                let (params, local_count) = {
                    let func = self.current_function(func_ref);
                    (func.params.clone(), func.local_count)
                };
                self.tracker.push_frame();
                let mut locals = Vec::with_capacity(local_count);
                for param in params.iter() {
                    match param {
                        ParamPattern::Ident(name) => {
                            let val = gen_obj
                                .captured
                                .iter()
                                .find(|(n, _)| n == name)
                                .map(|(_, v)| v.clone())
                                .unwrap_or(Value::Undefined);
                            locals.push(val);
                        }
                        ParamPattern::Rest(name) => {
                            let val = gen_obj
                                .captured
                                .iter()
                                .find(|(n, _)| n == name)
                                .map(|(_, v)| v.clone())
                                .unwrap_or_else(|| Value::Array(self.heap.alloc_array(Vec::new())));
                            locals.push(val);
                        }
                        _ => {
                            locals.push(Value::Undefined);
                        }
                    }
                }
                let stack_base = self.stack.len();
                let env: HashMap<String, u64> = gen_obj.env.iter().cloned().collect();
                self.frames.push(CallFrame {
                    program_index: func_ref.program_id,
                    func_index: Some(func_ref.function_id),
                    ip: 0,
                    locals,
                    stack_base,
                    this_value: None,
                    receiver_source: None,
                    boxed: HashMap::new(),
                    env,
                });
                self.run_generator_until_yield_or_return(gen_obj)
            }
            Some(suspended) => {
                self.tracker.push_frame();
                let stack_base = self.stack.len();
                for val in &suspended.stack {
                    self.push(val.clone())?;
                }
                self.push(arg)?;
                let env: HashMap<String, u64> = gen_obj.env.iter().cloned().collect();
                self.frames.push(CallFrame {
                    program_index: func_ref.program_id,
                    func_index: Some(func_ref.function_id),
                    ip: suspended.ip,
                    locals: suspended.locals,
                    stack_base,
                    this_value: None,
                    receiver_source: None,
                    boxed: HashMap::new(),
                    env,
                });
                self.run_generator_until_yield_or_return(gen_obj)
            }
        }
    }

    /// Store generator state back into the globals registry.
    /// For done generators, the key is removed to prevent unbounded growth.
    fn store_generator(&mut self, gen_obj: GeneratorObject) {
        let gen_key = format!("__gen_{}", gen_obj.id);
        if gen_obj.done {
            self.globals.remove(&gen_key);
        } else {
            self.globals.insert(gen_key, Value::Generator(gen_obj));
        }
    }

    /// Mark a generator as done, store it, and return the final iterator result.
    fn finish_generator(&mut self, mut gen_obj: GeneratorObject, value: Value) -> Value {
        gen_obj.done = true;
        gen_obj.suspended = None;
        self.store_generator(gen_obj);
        self.make_iterator_result(value, true)
    }

    fn run_generator_until_yield_or_return(
        &mut self,
        mut gen_obj: GeneratorObject,
    ) -> Result<Value> {
        let target_frame_depth = self.frames.len() - 1;
        loop {
            self.tracker.check_time(&self.limits)?;
            let frame = self.frames.last().unwrap();
            let instructions = match frame.func_index {
                Some(idx) => &self.program(frame.program_index).functions[idx].instructions,
                None => &self.program(frame.program_index).instructions,
            };
            if frame.ip >= instructions.len() {
                if self.frames.len() > target_frame_depth + 1 {
                    let frame = self.frames.pop().unwrap();
                    self.tracker.pop_frame();
                    if let Some(this_val) = frame.this_value {
                        self.stack.truncate(frame.stack_base);
                        self.push(this_val)?;
                    } else {
                        self.push(Value::Undefined)?;
                    }
                    continue;
                }
                let frame = self.frames.pop().unwrap();
                self.tracker.pop_frame();
                self.stack.truncate(frame.stack_base);
                let result = self.finish_generator(gen_obj, Value::Undefined);
                return Ok(result);
            }
            let instr = instructions[frame.ip].clone();
            if matches!(instr, Instruction::Yield) {
                self.current_frame_mut().ip += 1;
                let yielded_value = self.pop()?;
                let frame = self.frames.pop().unwrap();
                self.tracker.pop_frame();
                let frame_stack: Vec<Value> = self.stack.drain(frame.stack_base..).collect();
                gen_obj.suspended = Some(SuspendedFrame {
                    ip: frame.ip,
                    locals: frame.locals,
                    stack: frame_stack,
                });
                gen_obj.done = false;
                self.store_generator(gen_obj);
                return Ok(self.make_iterator_result(yielded_value, false));
            }
            if matches!(instr, Instruction::Return) {
                self.current_frame_mut().ip += 1;
                let return_val = self.pop().unwrap_or(Value::Undefined);
                if self.frames.len() > target_frame_depth + 1 {
                    let frame = self.frames.pop().unwrap();
                    self.tracker.pop_frame();
                    self.stack.truncate(frame.stack_base);
                    self.push(return_val)?;
                    continue;
                }
                let frame = self.frames.pop().unwrap();
                self.tracker.pop_frame();
                self.stack.truncate(frame.stack_base);
                let result = self.finish_generator(gen_obj, return_val);
                return Ok(result);
            }
            let result = self.dispatch(instr);
            match result {
                Ok(Some(VmState::Complete(val))) => return Ok(val),
                Ok(Some(VmState::Suspended { .. })) | Ok(Some(VmState::SuspendedMany { .. })) => {
                    return Err(ZapcodeError::RuntimeError(
                        "cannot suspend inside a generator".to_string(),
                    ));
                }
                Ok(None) => {
                    if self.frames.len() == target_frame_depth {
                        let return_val = self.pop().unwrap_or(Value::Undefined);
                        let result = self.finish_generator(gen_obj, return_val);
                        return Ok(result);
                    }
                }
                Err(err) => {
                    // Only handlers inside the generator body (frames above the
                    // generator's base) may catch here; an outer handler belongs
                    // to the caller and is left for it.
                    let error_val = self.caught_error_value(&err);
                    if !self.route_thrown(error_val, target_frame_depth)? {
                        return Err(err);
                    }
                }
            }
        }
    }

    fn make_iterator_result(&mut self, value: Value, done: bool) -> Value {
        let mut obj = IndexMap::new();
        obj.insert(Arc::from("value"), value);
        obj.insert(Arc::from("done"), Value::Bool(done));
        Value::Object(self.heap.alloc_object(obj))
    }

    fn dispatch(&mut self, instr: Instruction) -> Result<Option<VmState>> {
        self.current_frame_mut().ip += 1;

        match instr {
            Instruction::Push(constant) => {
                let value = match constant {
                    Constant::Undefined => Value::Undefined,
                    Constant::Null => Value::Null,
                    Constant::Bool(b) => Value::Bool(b),
                    Constant::Int(n) => Value::Int(n),
                    Constant::Float(n) => Value::Float(n),
                    Constant::String(s) => Value::String(Arc::from(s.as_str())),
                };
                self.push(value)?;
            }
            Instruction::Pop => {
                self.pop()?;
            }
            Instruction::Dup => {
                let val = self.peek()?.clone();
                self.push(val)?;
            }
            Instruction::LoadLocal(idx) => {
                // Loading a local clears any pending builtin-global name: the
                // `last_global_name` shortcut (used so `Math.floor` resolves to a
                // builtin method) must only apply to a property read *immediately*
                // after the matching `LoadGlobal`. Otherwise a stale name leaks into
                // an unrelated member access — e.g. `String(o.zzz)` would wrongly
                // resolve missing `o.zzz` to a `String` builtin method.
                self.last_global_name = None;
                let frame_index = self.frames.len() - 1;
                let val = self.read_local(frame_index, idx);
                // If the slot is boxed, write-back must target the cell so the
                // owning frame and capturing closures stay in sync.
                if let Some(&cell) = self.current_frame().boxed.get(&idx) {
                    self.last_load_source = Some(ReceiverSource::Cell(cell));
                    self.last_place = Some(Place {
                        root: PlaceRoot::Cell(cell),
                        path: Vec::new(),
                    });
                } else {
                    self.last_load_source = Some(ReceiverSource::Local {
                        frame_index,
                        slot: idx,
                    });
                    self.last_place = Some(Place {
                        root: PlaceRoot::Local {
                            frame_index,
                            slot: idx,
                        },
                        path: Vec::new(),
                    });
                }
                self.push(val)?;
            }
            Instruction::StoreLocal(idx) => {
                let val = self.pop()?;
                let frame_index = self.frames.len() - 1;
                self.write_local(frame_index, idx, val);
            }
            Instruction::LoadGlobal(name) => {
                // A captured free variable resolves to its shared cell via the
                // current frame's env overlay before falling back to true globals.
                if let Some(&cell) = self.current_frame().env.get(name.as_str()) {
                    let val = self
                        .cells
                        .get(cell as usize)
                        .cloned()
                        .unwrap_or(Value::Undefined);
                    self.last_global_name = Some(name.clone());
                    self.last_load_source = Some(ReceiverSource::Cell(cell));
                    self.last_place = Some(Place {
                        root: PlaceRoot::Cell(cell),
                        path: Vec::new(),
                    });
                    self.push(val)?;
                    return Ok(None);
                }
                let val = self.globals.get(&name).cloned().unwrap_or(Value::Undefined);
                self.last_global_name = Some(name.clone());
                // Only track receiver source for user-defined globals — builtins
                // (console, Math, JSON, etc.) contain non-serializable BuiltinMethod
                // values that would break snapshot serialization if written back.
                if Self::BUILTIN_GLOBAL_NAMES.contains(&name.as_str()) {
                    self.last_load_source = None;
                    self.last_place = None;
                } else {
                    self.last_load_source = Some(ReceiverSource::Global(name.clone()));
                    self.last_place = Some(Place {
                        root: PlaceRoot::Global(name),
                        path: Vec::new(),
                    });
                }
                self.push(val)?;
            }
            Instruction::StoreGlobal(name) => {
                let val = self.pop()?;
                // Route writes to a captured variable through its shared cell.
                if let Some(&cell) = self.current_frame().env.get(name.as_str()) {
                    if let Some(slot_ref) = self.cells.get_mut(cell as usize) {
                        *slot_ref = val;
                    }
                } else {
                    self.globals.insert(name, val);
                }
            }
            Instruction::DeclareLocal(_) => {
                let frame = self.current_frame_mut();
                frame.locals.push(Value::Undefined);
            }

            // Arithmetic
            Instruction::Add => {
                let right = self.pop()?;
                let left = self.pop()?;
                // ToPrimitive(default) both operands so user valueOf/toString
                // hooks participate before the string-vs-number decision.
                let left = self.to_primitive(&left, ToPrimitiveHint::Default)?;
                let right = self.to_primitive(&right, ToPrimitiveHint::Default)?;
                let result = match (&left, &right) {
                    (Value::Int(a), Value::Int(b)) => match a.checked_add(*b) {
                        Some(r) => Value::Int(r),
                        None => Value::Float(*a as f64 + *b as f64),
                    },
                    (Value::Float(a), Value::Float(b)) => Value::Float(a + b),
                    (Value::Int(a), Value::Float(b)) => Value::Float(*a as f64 + b),
                    (Value::Float(a), Value::Int(b)) => Value::Float(a + *b as f64),
                    (Value::String(a), _) => {
                        let rhs = right.to_js_string(&self.heap);
                        let new_len = a.len().saturating_add(rhs.len());
                        if new_len > 10_000_000 {
                            return Err(ZapcodeError::AllocationLimitExceeded);
                        }
                        let mut s = a.to_string();
                        s.push_str(&rhs);
                        Value::String(Arc::from(s.as_str()))
                    }
                    (_, Value::String(b)) => {
                        let lhs = left.to_js_string(&self.heap);
                        let new_len = lhs.len().saturating_add(b.len());
                        if new_len > 10_000_000 {
                            return Err(ZapcodeError::AllocationLimitExceeded);
                        }
                        let mut s = lhs;
                        s.push_str(b);
                        Value::String(Arc::from(s.as_str()))
                    }
                    // JS `+`: if either operand ToPrimitives to a string (arrays,
                    // plain objects), the whole expression is string concatenation
                    // (e.g. `[1,2]+[3]` -> "1,23", `[]+{}` -> "[object Object]").
                    _ if coerces_to_string_in_add(&left) || coerces_to_string_in_add(&right) => {
                        let lhs = left.to_js_string(&self.heap);
                        let rhs = right.to_js_string(&self.heap);
                        let new_len = lhs.len().saturating_add(rhs.len());
                        if new_len > 10_000_000 {
                            return Err(ZapcodeError::AllocationLimitExceeded);
                        }
                        let mut s = lhs;
                        s.push_str(&rhs);
                        Value::String(Arc::from(s.as_str()))
                    }
                    _ => Value::Float(
                        left.to_number_heap(&self.heap) + right.to_number_heap(&self.heap),
                    ),
                };
                self.push(result)?;
            }
            Instruction::Sub => {
                let right = self.pop()?;
                let left = self.pop()?;
                let left = self.to_primitive(&left, ToPrimitiveHint::Number)?;
                let right = self.to_primitive(&right, ToPrimitiveHint::Number)?;
                let result = match (&left, &right) {
                    (Value::Int(a), Value::Int(b)) => match a.checked_sub(*b) {
                        Some(r) => Value::Int(r),
                        None => Value::Float(*a as f64 - *b as f64),
                    },
                    _ => Value::Float(
                        left.to_number_heap(&self.heap) - right.to_number_heap(&self.heap),
                    ),
                };
                self.push(result)?;
            }
            Instruction::Mul => {
                let right = self.pop()?;
                let left = self.pop()?;
                let left = self.to_primitive(&left, ToPrimitiveHint::Number)?;
                let right = self.to_primitive(&right, ToPrimitiveHint::Number)?;
                let result = match (&left, &right) {
                    (Value::Int(a), Value::Int(b)) => match a.checked_mul(*b) {
                        Some(r) => Value::Int(r),
                        None => Value::Float(*a as f64 * *b as f64),
                    },
                    _ => Value::Float(
                        left.to_number_heap(&self.heap) * right.to_number_heap(&self.heap),
                    ),
                };
                self.push(result)?;
            }
            Instruction::Div => {
                let right = self.pop()?;
                let left = self.pop()?;
                let left = self.to_primitive(&left, ToPrimitiveHint::Number)?;
                let right = self.to_primitive(&right, ToPrimitiveHint::Number)?;
                let result = Value::Float(
                    left.to_number_heap(&self.heap) / right.to_number_heap(&self.heap),
                );
                self.push(result)?;
            }
            Instruction::Rem => {
                let right = self.pop()?;
                let left = self.pop()?;
                let left = self.to_primitive(&left, ToPrimitiveHint::Number)?;
                let right = self.to_primitive(&right, ToPrimitiveHint::Number)?;
                let result = match (&left, &right) {
                    (Value::Int(a), Value::Int(b)) if *b != 0 => Value::Int(a % b),
                    _ => Value::Float(
                        left.to_number_heap(&self.heap) % right.to_number_heap(&self.heap),
                    ),
                };
                self.push(result)?;
            }
            Instruction::Pow => {
                let right = self.pop()?;
                let left = self.pop()?;
                let left = self.to_primitive(&left, ToPrimitiveHint::Number)?;
                let right = self.to_primitive(&right, ToPrimitiveHint::Number)?;
                let result = Value::Float(
                    left.to_number_heap(&self.heap)
                        .powf(right.to_number_heap(&self.heap)),
                );
                self.push(result)?;
            }
            Instruction::Neg => {
                let val = self.pop()?;
                let val = self.to_primitive(&val, ToPrimitiveHint::Number)?;
                let result = match val {
                    Value::Int(n) => Value::Int(-n),
                    _ => Value::Float(-val.to_number_heap(&self.heap)),
                };
                self.push(result)?;
            }
            Instruction::BitNot => {
                let val = self.pop()?;
                let n = js_to_int32(val.to_number_heap(&self.heap));
                self.push(Value::Int(!n as i64))?;
            }
            Instruction::BitAnd => {
                let right = self.pop()?;
                let left = self.pop()?;
                let result = js_to_int32(left.to_number_heap(&self.heap))
                    & js_to_int32(right.to_number_heap(&self.heap));
                self.push(Value::Int(result as i64))?;
            }
            Instruction::BitOr => {
                let right = self.pop()?;
                let left = self.pop()?;
                let result = js_to_int32(left.to_number_heap(&self.heap))
                    | js_to_int32(right.to_number_heap(&self.heap));
                self.push(Value::Int(result as i64))?;
            }
            Instruction::BitXor => {
                let right = self.pop()?;
                let left = self.pop()?;
                let result = js_to_int32(left.to_number_heap(&self.heap))
                    ^ js_to_int32(right.to_number_heap(&self.heap));
                self.push(Value::Int(result as i64))?;
            }
            Instruction::Shl => {
                let right = self.pop()?;
                let left = self.pop()?;
                let shift = js_to_uint32(right.to_number_heap(&self.heap)) & 0x1f;
                let result = js_to_int32(left.to_number_heap(&self.heap)).wrapping_shl(shift);
                self.push(Value::Int(result as i64))?;
            }
            Instruction::Shr => {
                let right = self.pop()?;
                let left = self.pop()?;
                let shift = js_to_uint32(right.to_number_heap(&self.heap)) & 0x1f;
                let result = js_to_int32(left.to_number_heap(&self.heap)).wrapping_shr(shift);
                self.push(Value::Int(result as i64))?;
            }
            Instruction::Ushr => {
                let right = self.pop()?;
                let left = self.pop()?;
                let shift = js_to_uint32(right.to_number_heap(&self.heap)) & 0x1f;
                // ToUint32 semantics: negative operands wrap (e.g. -1 >>> 0 === 4294967295).
                let result = js_to_uint32(left.to_number_heap(&self.heap)).wrapping_shr(shift);
                self.push(Value::Int(result as i64))?;
            }

            // Comparison
            Instruction::Eq => {
                let right = self.pop()?;
                let left = self.pop()?;
                self.push(Value::Bool(left.loose_eq(&right)))?;
            }
            Instruction::StrictEq => {
                let right = self.pop()?;
                let left = self.pop()?;
                self.push(Value::Bool(left.strict_eq(&right)))?;
            }
            Instruction::Neq => {
                let right = self.pop()?;
                let left = self.pop()?;
                self.push(Value::Bool(!left.loose_eq(&right)))?;
            }
            Instruction::StrictNeq => {
                let right = self.pop()?;
                let left = self.pop()?;
                self.push(Value::Bool(!left.strict_eq(&right)))?;
            }
            Instruction::Lt => {
                let right = self.pop()?;
                let left = self.pop()?;
                let left = self.to_primitive(&left, ToPrimitiveHint::Number)?;
                let right = self.to_primitive(&right, ToPrimitiveHint::Number)?;
                self.push(Value::Bool(js_less_than(&left, &right, &self.heap)))?;
            }
            Instruction::Lte => {
                let right = self.pop()?;
                let left = self.pop()?;
                let left = self.to_primitive(&left, ToPrimitiveHint::Number)?;
                let right = self.to_primitive(&right, ToPrimitiveHint::Number)?;
                // a <= b  <=>  !(b < a), but NaN must make it false either way.
                self.push(Value::Bool(js_less_than_or_equal(&left, &right, &self.heap)))?;
            }
            Instruction::Gt => {
                let right = self.pop()?;
                let left = self.pop()?;
                let left = self.to_primitive(&left, ToPrimitiveHint::Number)?;
                let right = self.to_primitive(&right, ToPrimitiveHint::Number)?;
                self.push(Value::Bool(js_less_than(&right, &left, &self.heap)))?;
            }
            Instruction::Gte => {
                let right = self.pop()?;
                let left = self.pop()?;
                let left = self.to_primitive(&left, ToPrimitiveHint::Number)?;
                let right = self.to_primitive(&right, ToPrimitiveHint::Number)?;
                self.push(Value::Bool(js_less_than_or_equal(&right, &left, &self.heap)))?;
            }

            // Logical
            Instruction::Not => {
                let val = self.pop()?;
                self.push(Value::Bool(!val.is_truthy()))?;
            }

            // Objects & Arrays
            Instruction::CreateArray(count) => {
                self.track_array_capacity(count)?;
                self.tracker.track_allocation(&self.limits)?;
                let mut arr = Vec::with_capacity(count);
                for _ in 0..count {
                    arr.push(self.pop()?);
                }
                arr.reverse();
                let h = self.heap.alloc_array(arr);
                self.push(Value::Array(h))?;
            }
            Instruction::CreateObject(count) => {
                self.tracker.track_allocation(&self.limits)?;
                let mut obj = IndexMap::new();
                // Pop key-value pairs (or spread values)
                let mut entries = Vec::new();
                for _ in 0..count {
                    let val = self.pop()?;
                    let key = self.pop()?;
                    entries.push((key, val));
                }
                entries.reverse();
                for (key, val) in entries {
                    match key {
                        Value::String(k) => {
                            obj.insert(k, val);
                        }
                        _ => {
                            let k: Arc<str> = Arc::from(key.to_js_string(&self.heap).as_str());
                            obj.insert(k, val);
                        }
                    }
                }
                let h = self.heap.alloc_object(obj);
                self.push(Value::Object(h))?;
            }
            Instruction::ObjectRest(excluded) => {
                let source = self.pop()?;
                let rest: IndexMap<Arc<str>, Value> = match source {
                    Value::Object(h) => self
                        .heap
                        .object_map(h)
                        .into_iter()
                        .filter(|(key, _)| {
                            !excluded.iter().any(|excluded| excluded == key.as_ref())
                        })
                        .collect(),
                    _ => IndexMap::new(),
                };
                let h = self.heap.alloc_object(rest);
                self.push(Value::Object(h))?;
            }
            Instruction::GetProperty(name) => {
                let obj = self.pop()?;
                // Place of the object we're reading the property from.
                let obj_place = self.last_place.take();
                let result = self.get_property(&obj, &name)?;
                // Consume the builtin-global shortcut: it applies only to this
                // single read immediately after a `LoadGlobal`, never to a later
                // chained access on the produced value.
                self.last_global_name = None;
                match result {
                    Value::BuiltinMethod {
                        object_name,
                        method_name,
                        ..
                    } => {
                        // Bind the receiver + write-back place onto the method so
                        // argument evaluation can't clobber it.
                        self.last_receiver_source = None;
                        self.push(Value::BuiltinMethod {
                            object_name,
                            method_name,
                            recv: Some(Box::new(obj)),
                            place: obj_place,
                        })?;
                    }
                    Value::Function(_) => {
                        // User method: keep the existing `this`-binding mechanism.
                        self.last_receiver = Some(obj);
                        self.last_receiver_source = self.last_load_source.take();
                        self.push(result)?;
                    }
                    other => {
                        // Plain value: extend the place path so a later method on
                        // it (e.g. `obj.items.push`) writes back to the right spot.
                        self.last_receiver_source = None;
                        self.last_place = obj_place.map(|mut p| {
                            p.path.push(PlaceSeg::Prop(name.to_string()));
                            p
                        });
                        self.push(other)?;
                    }
                }
            }
            Instruction::SetProperty(name) => {
                // Stack: [value_to_store, object] with object on top
                // (compile_store pushes object after the value)
                let obj_val = self.pop()?;
                let value = self.pop()?;
                match obj_val {
                    Value::Object(h) => {
                        // Mutate the heap slot in place; the handle is shared so the
                        // write is visible through every alias (reference semantics).
                        if let Some(obj) = self.heap.object_mut(h) {
                            obj.insert(Arc::from(name.as_str()), value);
                        }
                        // Push the (same) object handle back so compile_store can store it.
                        self.push(Value::Object(h))?;
                    }
                    _ => {
                        return Err(ZapcodeError::TypeError(format!(
                            "cannot set property '{}' on {}",
                            name,
                            obj_val.type_name()
                        )));
                    }
                }
            }
            Instruction::GetIndex => {
                let index = self.pop()?;
                let obj = self.pop()?;
                let obj_place = self.last_place.take();
                // The path step for this index access (for write-back chains).
                let seg = match (&obj, &index) {
                    (Value::Array(_), Value::Int(i)) if *i >= 0 => {
                        Some(PlaceSeg::Index(*i as usize))
                    }
                    (Value::Array(_), Value::Float(f)) if *f >= 0.0 => {
                        Some(PlaceSeg::Index(*f as usize))
                    }
                    (Value::Object(_), Value::String(key)) => Some(PlaceSeg::Prop(key.to_string())),
                    (Value::Object(_), _) => {
                        Some(PlaceSeg::Prop(index.to_js_string(&self.heap)))
                    }
                    _ => None,
                };
                let result = match (&obj, &index) {
                    (Value::Array(h), Value::Int(i)) => self
                        .heap
                        .array(*h)
                        .get(*i as usize)
                        .cloned()
                        .unwrap_or(Value::Undefined),
                    (Value::Array(h), Value::Float(f)) => self
                        .heap
                        .array(*h)
                        .get(*f as usize)
                        .cloned()
                        .unwrap_or(Value::Undefined),
                    (Value::Object(h), Value::String(key)) => self
                        .heap
                        .object(*h)
                        .and_then(|m| m.get(key.as_ref()).cloned())
                        .unwrap_or(Value::Undefined),
                    (Value::Object(h), _) => {
                        let key: Arc<str> = Arc::from(index.to_js_string(&self.heap).as_str());
                        self.heap
                            .object(*h)
                            .and_then(|m| m.get(key.as_ref()).cloned())
                            .unwrap_or(Value::Undefined)
                    }
                    (Value::String(s), Value::Int(i)) => s
                        .chars()
                        .nth(*i as usize)
                        .map(|c| Value::String(Arc::from(c.to_string().as_str())))
                        .unwrap_or(Value::Undefined),
                    _ => Value::Undefined,
                };
                match result {
                    Value::BuiltinMethod {
                        object_name,
                        method_name,
                        ..
                    } => {
                        let place = match (obj_place, seg) {
                            (Some(mut p), Some(s)) => {
                                p.path.push(s);
                                Some(p)
                            }
                            _ => None,
                        };
                        self.push(Value::BuiltinMethod {
                            object_name,
                            method_name,
                            recv: Some(Box::new(obj)),
                            place,
                        })?;
                    }
                    other => {
                        self.last_place = match (obj_place, seg) {
                            (Some(mut p), Some(s)) => {
                                p.path.push(s);
                                Some(p)
                            }
                            _ => None,
                        };
                        self.push(other)?;
                    }
                }
            }
            Instruction::SetIndex => {
                let index = self.pop()?;
                let obj = self.pop()?;
                let value = self.pop()?;
                match &obj {
                    Value::Array(h) => {
                        let cur_len = self.heap.array(*h).len();
                        let idx = match &index {
                            Value::Int(i) if *i >= 0 => *i as usize,
                            Value::Float(f) if *f >= 0.0 && *f == (*f as usize as f64) => {
                                *f as usize
                            }
                            _ => {
                                // Negative or non-numeric index: treat as no-op (like JS)
                                self.push(obj)?;
                                return Ok(None);
                            }
                        };
                        // Cap maximum sparse array growth to prevent memory exhaustion
                        if idx > cur_len + 1024 {
                            return Err(ZapcodeError::RuntimeError(format!(
                                "array index {} too far beyond length {}",
                                idx, cur_len
                            )));
                        }
                        // Mutate the heap slot in place (reference semantics).
                        if let Some(arr) = self.heap.array_mut(*h) {
                            while arr.len() <= idx {
                                arr.push(Value::Undefined);
                            }
                            arr[idx] = value;
                        }
                    }
                    Value::Object(h) => {
                        let key: Arc<str> = Arc::from(index.to_js_string(&self.heap).as_str());
                        if let Some(map) = self.heap.object_mut(*h) {
                            map.insert(key, value);
                        }
                    }
                    _ => {}
                }
                // Push the (same) handle back so compile_store can store it to the variable.
                self.push(obj)?;
            }
            Instruction::FreshenBinding(slot) => {
                let frame_index = self.frames.len() - 1;
                // Only meaningful if a closure captured this slot this iteration.
                if let Some(&cell) = self.current_frame().boxed.get(&slot) {
                    let val = self
                        .cells
                        .get(cell as usize)
                        .cloned()
                        .unwrap_or(Value::Undefined);
                    let new_cell = self.cells.len() as u64;
                    self.cells.push(val);
                    self.frames[frame_index].boxed.insert(slot, new_cell);
                }
            }
            Instruction::DeleteProperty(name) => {
                let obj = self.pop()?;
                if let Value::Object(h) = &obj {
                    if let Some(map) = self.heap.object_mut(*h) {
                        map.shift_remove(name.as_str());
                    }
                }
                self.push(obj)?;
            }
            Instruction::DeleteIndex => {
                let key = self.pop()?;
                let obj = self.pop()?;
                match &obj {
                    Value::Object(h) => {
                        let k = key.to_js_string(&self.heap);
                        if let Some(map) = self.heap.object_mut(*h) {
                            map.shift_remove(k.as_str());
                        }
                    }
                    Value::Array(h) => {
                        // `delete arr[i]` leaves a hole (undefined) without
                        // changing length, matching JS.
                        if let Value::Int(i) = &key {
                            if *i >= 0 {
                                if let Some(arr) = self.heap.array_mut(*h) {
                                    if (*i as usize) < arr.len() {
                                        arr[*i as usize] = Value::Undefined;
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
                self.push(obj)?;
            }
            Instruction::Spread => {
                // No-op marker; spread is expanded by the dedicated
                // Array/Object append instructions below.
            }
            Instruction::ArrayAppend => {
                self.tracker.track_allocation(&self.limits)?;
                let value = self.pop()?;
                let acc = match self.pop()? {
                    Value::Array(a) => a,
                    other => {
                        return Err(ZapcodeError::TypeError(format!(
                            "internal: ArrayAppend on {}",
                            other.type_name()
                        )))
                    }
                };
                if let Some(v) = self.heap.array_mut(acc) {
                    v.push(value);
                }
                self.push(Value::Array(acc))?;
            }
            Instruction::ArraySpreadAppend => {
                self.tracker.track_allocation(&self.limits)?;
                let iterable = self.pop()?;
                let acc = match self.pop()? {
                    Value::Array(a) => a,
                    other => {
                        return Err(ZapcodeError::TypeError(format!(
                            "internal: ArraySpreadAppend on {}",
                            other.type_name()
                        )))
                    }
                };
                let extra: Vec<Value> = match &iterable {
                    Value::Array(items) => self.heap.array_vec(*items),
                    Value::String(s) => s
                        .chars()
                        .map(|c| Value::String(Arc::from(c.to_string().as_str())))
                        .collect(),
                    Value::Object(_) if is_set_object(&iterable, &self.heap) => {
                        set_items(&iterable, &self.heap)
                    }
                    Value::Object(_) if is_map_object(&iterable, &self.heap) => {
                        map_entry_pairs(&iterable, &mut self.heap)
                    }
                    other => {
                        return Err(ZapcodeError::TypeError(format!(
                            "{} is not iterable (spread)",
                            other.type_name()
                        )))
                    }
                };
                if let Some(v) = self.heap.array_mut(acc) {
                    v.extend(extra);
                }
                self.push(Value::Array(acc))?;
            }
            Instruction::ArrayRestFrom(from) => {
                self.tracker.track_allocation(&self.limits)?;
                let value = self.pop()?;
                let rest: Vec<Value> = match value {
                    Value::Array(items) => {
                        self.heap.array_vec(items).into_iter().skip(from).collect()
                    }
                    _ => Vec::new(),
                };
                let h = self.heap.alloc_array(rest);
                self.push(Value::Array(h))?;
            }
            Instruction::ObjectInsert => {
                let value = self.pop()?;
                let key = self.pop()?;
                let acc = match self.pop()? {
                    Value::Object(o) => o,
                    other => {
                        return Err(ZapcodeError::TypeError(format!(
                            "internal: ObjectInsert on {}",
                            other.type_name()
                        )))
                    }
                };
                let key: Arc<str> = match key {
                    Value::String(k) => k,
                    other => Arc::from(other.to_js_string(&self.heap).as_str()),
                };
                if let Some(map) = self.heap.object_mut(acc) {
                    map.insert(key, value);
                }
                self.push(Value::Object(acc))?;
            }
            Instruction::ObjectSpreadAssign => {
                let source = self.pop()?;
                let acc = match self.pop()? {
                    Value::Object(o) => o,
                    other => {
                        return Err(ZapcodeError::TypeError(format!(
                            "internal: ObjectSpreadAssign on {}",
                            other.type_name()
                        )))
                    }
                };
                match source {
                    Value::Object(h) => {
                        let entries = self.heap.object_map(h);
                        if let Some(map) = self.heap.object_mut(acc) {
                            for (k, v) in entries {
                                map.insert(k, v);
                            }
                        }
                    }
                    // Spreading null/undefined is a no-op in JS; ignore others.
                    Value::Null | Value::Undefined => {}
                    _ => {}
                }
                self.push(Value::Object(acc))?;
            }
            Instruction::In => {
                let right = self.pop()?;
                let left = self.pop()?;
                let result = match &right {
                    Value::Object(h) => {
                        let key = left.to_js_string(&self.heap);
                        self.heap.object(*h).is_some_and(|m| m.contains_key(key.as_str()))
                    }
                    Value::Array(h) => {
                        let len = self.heap.array(*h).len();
                        match &left {
                            // Numeric index membership: `0 in [1,2]`.
                            Value::Int(i) => *i >= 0 && (*i as usize) < len,
                            // String keys: `"length"` is an own property of every
                            // array; numeric string keys like `"0"` are indices.
                            // (Inherited prototype methods such as "push"/"map"
                            // stay absent — own-key membership only.)
                            _ => {
                                let key = left.to_js_string(&self.heap);
                                if key == "length" {
                                    true
                                } else if let Ok(idx) = key.parse::<usize>() {
                                    idx < len
                                } else {
                                    false
                                }
                            }
                        }
                    }
                    _ => false,
                };
                self.push(Value::Bool(result))?;
            }
            Instruction::InstanceOf => {
                let right = self.pop()?;
                let left = self.pop()?;
                // Helper: does object handle `h` have key `k`?
                let has_key = |h: Handle, k: &str, heap: &Heap| -> bool {
                    heap.object(h).is_some_and(|m| m.contains_key(k))
                };
                // Check if left's __class__ matches right's __class_name__
                let result = if let Value::Object(class_h) = &right {
                    let class_obj = self.heap.object_map(*class_h);
                    if let Some(Value::String(ctor)) = class_obj.get("__builtin_constructor__") {
                        // Builtin constructors. Object matches any object/array/function;
                        // Array matches arrays; Error matches any error object; a specific
                        // error type matches by name; Map/Set by marker.
                        match ctor.as_ref() {
                            "Object" => matches!(
                                left,
                                Value::Object(_) | Value::Array(_) | Value::Function(_)
                            ),
                            "Array" => matches!(left, Value::Array(_)),
                            "Error" => {
                                matches!(&left, Value::Object(i) if has_key(*i, "__error__", &self.heap))
                            }
                            "TypeError" | "RangeError" | "SyntaxError" | "ReferenceError"
                            | "AggregateError" => match &left {
                                Value::Object(i) => {
                                    self.heap.object(*i).and_then(|m| m.get("name"))
                                        == Some(&Value::String(Arc::from(ctor.as_ref())))
                                }
                                _ => false,
                            },
                            "Map" => {
                                matches!(&left, Value::Object(i) if has_key(*i, "__map__", &self.heap))
                            }
                            "Set" => {
                                matches!(&left, Value::Object(i) if has_key(*i, "__set__", &self.heap))
                            }
                            "Date" => {
                                matches!(&left, Value::Object(i) if has_key(*i, "__date_ms__", &self.heap))
                            }
                            _ => false,
                        }
                    } else if let (Value::Object(instance), Some(class_name)) =
                        (&left, class_obj.get("__class_name__"))
                    {
                        let inst = self.heap.object_map(*instance);
                        // Match the instance's class or any of its ancestors.
                        match inst.get("__class_chain__") {
                            Some(Value::Array(chain)) => {
                                self.heap.array(*chain).contains(class_name)
                            }
                            _ => inst.get("__class__") == Some(class_name),
                        }
                    } else {
                        false
                    }
                } else {
                    false
                };
                self.push(Value::Bool(result))?;
            }

            // Functions
            Instruction::CreateClosure(func_idx) => {
                let closure = self.create_closure(func_idx);
                self.push(Value::Function(closure))?;
            }
            Instruction::Call(arg_count) => {
                let mut args = Vec::with_capacity(arg_count);
                for _ in 0..arg_count {
                    args.push(self.pop()?);
                }
                args.reverse();

                let callee = self.pop()?;
                match callee {
                    Value::Function(closure) => {
                        let func_ref = closure.func_ref;
                        let function = self.current_function(func_ref);
                        let is_generator = function.is_generator;

                        // Generator function: create a Generator object instead of running
                        if is_generator {
                            let params = function.params.clone();
                            let gen_id = self.alloc_generator_id();
                            // Capture args as named params so generator_next can restore them
                            let mut captured = closure.captured.clone();
                            for (i, param) in params.iter().enumerate() {
                                match param {
                                    ParamPattern::Ident(name) => {
                                        captured.push((
                                            name.clone(),
                                            args.get(i).cloned().unwrap_or(Value::Undefined),
                                        ));
                                    }
                                    ParamPattern::Rest(name) => {
                                        let rest: Vec<Value> = args[i..].to_vec();
                                        let h = self.heap.alloc_array(rest);
                                        captured.push((name.clone(), Value::Array(h)));
                                    }
                                    _ => {}
                                }
                            }
                            let gen_obj = GeneratorObject {
                                id: gen_id,
                                func_ref,
                                captured,
                                env: closure.env.clone(),
                                suspended: None,
                                done: false,
                            };
                            // Store in globals registry so we can look it up by ID later
                            self.globals.insert(
                                format!("__gen_{}", gen_id),
                                Value::Generator(gen_obj.clone()),
                            );
                            self.push(Value::Generator(gen_obj))?;
                            self.last_receiver = None;
                            self.last_receiver_source = None;
                        } else {
                            let this_value = self.last_receiver.take();
                            self.push_call_frame(&closure, &args, this_value)?;
                        }
                    }
                    Value::BuiltinMethod {
                        object_name,
                        method_name,
                        recv,
                        place,
                    } => {
                        // Receiver is carried on the method (immune to arg eval);
                        // fall back to the legacy slot for any unbound handle.
                        let receiver = recv.map(|b| *b).or_else(|| self.last_receiver.take());
                        let result = match object_name.as_ref() {
                            "__array__" => {
                                // Arrays are heap handles: mutating methods edit
                                // the slot in place, so the change is already
                                // visible through every alias. No write-back via
                                // `place` is needed (reference semantics).
                                let _ = &place;
                                if let Some(Value::Array(arr)) = &receiver {
                                    let arr = *arr;
                                    // Check if this is a callback method first
                                    match method_name.as_ref() {
                                        "map" | "filter" | "forEach" | "find" | "findIndex"
                                        | "findLast" | "findLastIndex" | "every" | "some"
                                        | "reduce" | "reduceRight" | "sort" | "flatMap" => {
                                            match self.execute_array_callback_method(
                                                arr,
                                                &method_name,
                                                args,
                                            )? {
                                                Some(val) => Some(val),
                                                None => {
                                                    // Continuation started — the main execute()
                                                    // loop will drive the callbacks. Don't push
                                                    // a result; just return Ok(None).
                                                    return Ok(None);
                                                }
                                            }
                                        }
                                        _ => builtins::call_builtin(
                                            &Value::Array(arr),
                                            &method_name,
                                            &args,
                                            &self.limits,
                                            &mut self.stdout,
                                            &mut self.heap,
                                        )?,
                                    }
                                } else {
                                    None
                                }
                            }
                            "__string__" => {
                                if let Some(Value::String(s)) = &receiver {
                                    builtins::call_builtin(
                                        &Value::String(s.clone()),
                                        &method_name,
                                        &args,
                                        &self.limits,
                                        &mut self.stdout,
                                        &mut self.heap,
                                    )?
                                } else {
                                    None
                                }
                            }
                            "__number__" => {
                                let n = receiver
                                    .as_ref()
                                    .map(|v| v.to_number_heap(&self.heap))
                                    .unwrap_or(f64::NAN);
                                builtins::call_number_method(n, &method_name, &args)?
                            }
                            "__object__" => match (&receiver, method_name.as_ref()) {
                                (Some(Value::Object(map)), "hasOwnProperty") => {
                                    let key = args
                                        .first()
                                        .map(|v| v.to_js_string(&self.heap))
                                        .unwrap_or_default();
                                    let has = self
                                        .heap
                                        .object(*map)
                                        .is_some_and(|m| m.contains_key(key.as_str()));
                                    Some(Value::Bool(has))
                                }
                                _ => None,
                            },
                            "__regexp__" => {
                                // The receiver handle is needed so /g exec/test can
                                // read and advance the `lastIndex` slot in place.
                                let regex_handle = match receiver.as_ref() {
                                    Some(Value::Object(h)) => Some(*h),
                                    _ => None,
                                };
                                match receiver
                                    .as_ref()
                                    .and_then(|v| builtins::regexp_parts(v, &self.heap))
                                {
                                    Some((pat, flags)) => builtins::call_regexp_method(
                                        &pat,
                                        &flags,
                                        &method_name,
                                        &args,
                                        regex_handle,
                                        &mut self.heap,
                                    )?,
                                    None => None,
                                }
                            }
                            "__generator__" => {
                                if let Some(Value::Generator(gen_obj)) = receiver {
                                    match method_name.as_ref() {
                                        "next" => {
                                            let arg =
                                                args.into_iter().next().unwrap_or(Value::Undefined);
                                            // Get the latest generator state from registry.
                                            // If the key is missing, the generator has finished
                                            // and was cleaned up — return done immediately.
                                            let gen_key = format!("__gen_{}", gen_obj.id);
                                            if let Some(Value::Generator(g)) =
                                                self.globals.remove(&gen_key)
                                            {
                                                let result = self.generator_next(g, arg)?;
                                                Some(result)
                                            } else {
                                                Some(
                                                    self.make_iterator_result(
                                                        Value::Undefined,
                                                        true,
                                                    ),
                                                )
                                            }
                                        }
                                        "return" => {
                                            let val =
                                                args.into_iter().next().unwrap_or(Value::Undefined);
                                            let gen_key = format!("__gen_{}", gen_obj.id);
                                            if let Some(Value::Generator(g)) =
                                                self.globals.remove(&gen_key)
                                            {
                                                let result = self.finish_generator(g, val);
                                                Some(result)
                                            } else {
                                                Some(self.make_iterator_result(val, true))
                                            }
                                        }
                                        _ => None,
                                    }
                                } else {
                                    None
                                }
                            }
                            "__promise__" => {
                                if let Some(promise) = receiver {
                                    match self
                                        .execute_promise_method(promise, &method_name, args)?
                                    {
                                        PromiseMethodOutcome::Value(v) => Some(v),
                                        // A continuation was started — the main
                                        // execute() loop drives the callback (and
                                        // suspends if it makes a tool call). Don't
                                        // push a result here.
                                        PromiseMethodOutcome::ContinuationStarted => {
                                            return Ok(None);
                                        }
                                        // A deferred single-call promise forced its
                                        // host call — propagate the suspension; the
                                        // recorded resume_action finishes the method.
                                        PromiseMethodOutcome::Suspend(state) => {
                                            return Ok(Some(state));
                                        }
                                        PromiseMethodOutcome::NotAPromise => None,
                                    }
                                } else {
                                    None
                                }
                            }
                            "__map__" => {
                                // Map is a heap handle; mutating methods edit the
                                // backing slot in place (reference semantics), so
                                // no `place` write-back is needed.
                                let _ = &place;
                                if let Some(Value::Object(map)) = receiver {
                                    if method_name.as_ref() == "forEach" {
                                        // cb(value, key, map) — needs guest closure.
                                        let cb = args.first().cloned().unwrap_or(Value::Undefined);
                                        let pairs = map_entry_pairs(
                                            &Value::Object(map),
                                            &mut self.heap,
                                        );
                                        for pair in pairs {
                                            if let Value::Array(ph) = pair {
                                                let kv = self.heap.array_vec(ph);
                                                let k = kv
                                                    .first()
                                                    .cloned()
                                                    .unwrap_or(Value::Undefined);
                                                let v = kv
                                                    .get(1)
                                                    .cloned()
                                                    .unwrap_or(Value::Undefined);
                                                self.call_function_internal(
                                                    &cb,
                                                    vec![v, k, Value::Object(map)],
                                                )?;
                                            }
                                        }
                                        Some(Value::Undefined)
                                    } else {
                                        execute_map_method(
                                            map,
                                            &method_name,
                                            &args,
                                            &mut self.heap,
                                        )
                                    }
                                } else {
                                    None
                                }
                            }
                            "__set__" => {
                                let _ = &place;
                                if let Some(Value::Object(map)) = receiver {
                                    if method_name.as_ref() == "forEach" {
                                        // cb(value, value, set) — needs guest closure.
                                        let cb = args.first().cloned().unwrap_or(Value::Undefined);
                                        let items =
                                            set_items(&Value::Object(map), &self.heap);
                                        for item in items {
                                            self.call_function_internal(
                                                &cb,
                                                vec![
                                                    item.clone(),
                                                    item.clone(),
                                                    Value::Object(map),
                                                ],
                                            )?;
                                        }
                                        Some(Value::Undefined)
                                    } else {
                                        execute_set_method(
                                            map,
                                            &method_name,
                                            &args,
                                            &mut self.heap,
                                        )
                                    }
                                } else {
                                    None
                                }
                            }
                            "__date__" => {
                                if let Some(Value::Object(date)) = receiver {
                                    let map = self.heap.object_map(date);
                                    execute_date_method(&map, &method_name)
                                } else {
                                    None
                                }
                            }
                            // Math.random is stateful — served by the VM's
                            // deterministic PRNG rather than the stateless builtin.
                            "Math" if method_name.as_ref() == "random" => {
                                Some(Value::Float(self.next_random()))
                            }
                            // Array.from(source, mapFn): mapFn may be a guest
                            // closure, so apply it here rather than in the pure
                            // builtin.
                            "Array"
                                if method_name.as_ref() == "from"
                                    && matches!(args.get(1), Some(Value::Function(_))) =>
                            {
                                let src = builtins::array_from_source(
                                    args.first().unwrap_or(&Value::Undefined),
                                    &mut self.heap,
                                );
                                let map_fn = args[1].clone();
                                let mut out = Vec::with_capacity(src.len());
                                for (i, item) in src.iter().enumerate() {
                                    out.push(self.call_element_callback(&map_fn, item, i)?);
                                }
                                let h = self.heap.alloc_array(out);
                                Some(Value::Array(h))
                            }
                            global_name => builtins::call_global_method(
                                global_name,
                                &method_name,
                                &args,
                                &mut self.stdout,
                                &mut self.heap,
                            )?,
                        };
                        match result {
                            Some(val) => self.push(val)?,
                            None => {
                                return Err(ZapcodeError::TypeError(format!(
                                    "{}.{} is not a function",
                                    object_name, method_name
                                )));
                            }
                        }
                    }
                    // Bare type-conversion functions: String(x)/Number(x)/Boolean(x),
                    // represented as objects carrying a "__global_fn__" marker.
                    Value::Object(h)
                        if self
                            .heap
                            .object(h)
                            .is_some_and(|m| m.contains_key("__global_fn__")) =>
                    {
                        let kind = match self.heap.object(h).and_then(|m| m.get("__global_fn__")) {
                            Some(Value::String(s)) => s.clone(),
                            _ => Arc::from(""),
                        };
                        // String(x)/Number(x) run ToPrimitive on their argument so
                        // a user valueOf/toString hook is honored (String -> string
                        // hint, Number -> number hint).
                        let mut args = args;
                        match kind.as_ref() {
                            "String" => {
                                if let Some(first) = args.first().cloned() {
                                    args[0] = self.to_primitive(&first, ToPrimitiveHint::String)?;
                                }
                            }
                            "Number" => {
                                if let Some(first) = args.first().cloned() {
                                    args[0] = self.to_primitive(&first, ToPrimitiveHint::Number)?;
                                }
                            }
                            _ => {}
                        }
                        let value = builtins::call_global_fn(kind.as_ref(), &args, &mut self.heap)?;
                        self.push(value)?;
                    }
                    _ => {
                        let msg = callee.to_js_string(&self.heap);
                        return Err(ZapcodeError::TypeError(format!("{} is not a function", msg)));
                    }
                }
            }
            Instruction::Return => {
                let return_val = self.pop().unwrap_or(Value::Undefined);
                // A `return` must run any `finally` blocks it escapes in the
                // current call frame before the function actually returns.
                if self.route_abrupt(Completion::Return(return_val.clone()))? {
                    return Ok(None);
                }
                if let Some(state) = self.perform_return(return_val)? {
                    return Ok(Some(state));
                }
            }
            Instruction::CallExternal(name, arg_count) => {
                if !self.external_functions.contains(&name) {
                    return Err(ZapcodeError::UnknownExternalFunction(name));
                }
                let mut args = Vec::with_capacity(arg_count);
                for _ in 0..arg_count {
                    args.push(self.pop()?);
                }
                args.reverse();
                // Suspend execution
                let snapshot = ZapcodeSnapshot::capture(self)?;
                return Ok(Some(VmState::Suspended {
                    function_name: name,
                    args,
                    snapshot,
                }));
            }
            // Spread calls: expand the flattened args array onto the stack, then
            // re-dispatch the normal call. The `ip -= 1` compensates for the ip
            // increment the re-dispatched instruction performs.
            Instruction::CallSpread => {
                let items = match self.pop()? {
                    Value::Array(a) => self.heap.array_vec(a),
                    _ => Vec::new(),
                };
                let n = items.len();
                for item in items {
                    self.push(item)?;
                }
                self.current_frame_mut().ip -= 1;
                return self.dispatch(Instruction::Call(n));
            }
            Instruction::CallExternalSpread(name) => {
                let items = match self.pop()? {
                    Value::Array(a) => self.heap.array_vec(a),
                    _ => Vec::new(),
                };
                let n = items.len();
                for item in items {
                    self.push(item)?;
                }
                self.current_frame_mut().ip -= 1;
                return self.dispatch(Instruction::CallExternal(name, n));
            }
            Instruction::CallExternalDeferred(name, arg_count) => {
                if !self.external_functions.contains(&name) {
                    return Err(ZapcodeError::UnknownExternalFunction(name));
                }
                let mut args = Vec::with_capacity(arg_count);
                for _ in 0..arg_count {
                    args.push(self.pop()?);
                }
                args.reverse();
                let id = self.next_call_id;
                self.next_call_id += 1;
                self.pending_calls
                    .push(PendingExternalCall { id, name, args });
                self.push(Value::Pending(id))?;
            }
            Instruction::MakeBatchPromise(kind, n) => {
                let mut items = Vec::with_capacity(n);
                for _ in 0..n {
                    items.push(self.pop()?);
                }
                items.reverse();
                // Mark as a pending-batch promise tagged with the combinator;
                // the await resolves it per `kind`.
                let items_arr = Value::Array(self.heap.alloc_array(items));
                let mut obj = IndexMap::new();
                obj.insert(Arc::from("__promise__"), Value::Bool(true));
                obj.insert(Arc::from("status"), Value::String(Arc::from("pending_all")));
                obj.insert(Arc::from("__batch_kind__"), Value::String(Arc::from(kind.as_str())));
                obj.insert(Arc::from("items"), items_arr);
                let h = self.heap.alloc_object(obj);
                self.push(Value::Object(h))?;
            }
            Instruction::MakeCallPromise => {
                // Wrap the just-registered deferred call (a `Value::Pending(id)`
                // on the stack) in a single-call Promise object. The host call is
                // deferred until the promise is awaited or driven by
                // `.then`/`.catch`/`.finally` (N5).
                let id = match self.pop()? {
                    Value::Pending(id) => id,
                    other => {
                        return Err(ZapcodeError::RuntimeError(format!(
                            "internal error: MakeCallPromise expected a pending call, got {}",
                            other.type_name()
                        )))
                    }
                };
                let mut obj = IndexMap::new();
                obj.insert(Arc::from("__promise__"), Value::Bool(true));
                obj.insert(Arc::from("status"), Value::String(Arc::from("pending_call")));
                obj.insert(Arc::from("__call_id__"), Value::Int(id as i64));
                let h = self.heap.alloc_object(obj);
                self.push(Value::Object(h))?;
            }

            // Control flow
            Instruction::Jump(target) => {
                self.current_frame_mut().ip = target;
            }
            Instruction::JumpIfFalse(target) => {
                let val = self.pop()?;
                if !val.is_truthy() {
                    self.current_frame_mut().ip = target;
                }
            }
            Instruction::JumpIfTrue(target) => {
                let val = self.pop()?;
                if val.is_truthy() {
                    self.current_frame_mut().ip = target;
                }
            }
            Instruction::JumpIfNullish(target) => {
                let val = self.peek()?;
                if matches!(val, Value::Null | Value::Undefined) {
                    self.current_frame_mut().ip = target;
                }
            }

            // Loops
            Instruction::SetupLoop => {}
            Instruction::Break(target) => {
                // Run any `finally` blocks this break escapes (try statements
                // that enclose the break but are enclosed by the loop), then jump.
                if !self.route_abrupt(Completion::Break(target))? {
                    self.current_frame_mut().ip = target;
                }
            }
            Instruction::Continue(target) => {
                if !self.route_abrupt(Completion::Continue(target))? {
                    self.current_frame_mut().ip = target;
                }
            }

            // Iterators
            Instruction::GetIterator => {
                let val = self.pop()?;
                // Build an iterator object `[items_array, index]` from the items.
                let iter_from_items = |items: Vec<Value>, heap: &mut Heap| -> Value {
                    let inner = Value::Array(heap.alloc_array(items));
                    Value::Array(heap.alloc_array(vec![inner, Value::Int(0)]))
                };
                match val {
                    Value::Array(arr) => {
                        // Push an iterator object: [array, index]. Reuse the array
                        // handle so iteration sees live elements.
                        let iter_obj =
                            Value::Array(self.heap.alloc_array(vec![Value::Array(arr), Value::Int(0)]));
                        self.push(iter_obj)?;
                    }
                    Value::String(s) => {
                        let chars: Vec<Value> = s
                            .chars()
                            .map(|c| Value::String(Arc::from(c.to_string().as_str())))
                            .collect();
                        let iter_obj = iter_from_items(chars, &mut self.heap);
                        self.push(iter_obj)?;
                    }
                    Value::Generator(gen_obj) => {
                        let iter_obj = Value::Array(self.heap.alloc_array(vec![
                            Value::String(Arc::from("__gen__")),
                            Value::Int(gen_obj.id as i64),
                            Value::Bool(false),
                        ]));
                        self.push(iter_obj)?;
                    }
                    // Map iterates as [key, value] pairs; Set as its items.
                    Value::Object(_) if is_set_object(&val, &self.heap) => {
                        let items = set_items(&val, &self.heap);
                        let iter_obj = iter_from_items(items, &mut self.heap);
                        self.push(iter_obj)?;
                    }
                    Value::Object(_) if is_map_object(&val, &self.heap) => {
                        let pairs = map_entry_pairs(&val, &mut self.heap);
                        let iter_obj = iter_from_items(pairs, &mut self.heap);
                        self.push(iter_obj)?;
                    }
                    _ => {
                        return Err(ZapcodeError::TypeError(format!(
                            "{} is not iterable",
                            val.type_name()
                        )));
                    }
                }
            }
            Instruction::IteratorNext => {
                let iter = self.pop()?;
                let items = match &iter {
                    Value::Array(h) => self.heap.array_vec(*h),
                    _ => return Err(ZapcodeError::RuntimeError("invalid iterator state".into())),
                };
                // Check for generator iterator (3-element sentinel)
                if items.len() == 3 {
                    if let Value::String(s) = &items[0] {
                        if s.as_ref() == "__gen__" {
                            let gen_id = match &items[1] {
                                Value::Int(id) => *id as u64,
                                _ => return Err(ZapcodeError::RuntimeError("bad gen iter".into())),
                            };
                            let gen_key = format!("__gen_{}", gen_id);
                            let gen_obj = if let Some(Value::Generator(g)) =
                                self.globals.remove(&gen_key)
                            {
                                g
                            } else {
                                let done_iter = self.heap.alloc_array(vec![
                                    Value::String(Arc::from("__gen__")),
                                    Value::Int(gen_id as i64),
                                    Value::Bool(true),
                                ]);
                                self.push(Value::Array(done_iter))?;
                                self.push(Value::Undefined)?;
                                return Ok(None);
                            };
                            let result = self.generator_next(gen_obj, Value::Undefined)?;
                            if let Value::Object(obj_h) = &result {
                                let obj = self.heap.object_map(*obj_h);
                                let done = obj
                                    .get("done")
                                    .is_some_and(|v| matches!(v, Value::Bool(true)));
                                let value = obj.get("value").cloned().unwrap_or(Value::Undefined);
                                let new_iter = self.heap.alloc_array(vec![
                                    Value::String(Arc::from("__gen__")),
                                    Value::Int(gen_id as i64),
                                    Value::Bool(done),
                                ]);
                                self.push(Value::Array(new_iter))?;
                                self.push(value)?;
                            } else {
                                self.push(iter)?;
                                self.push(Value::Undefined)?;
                            }
                            return Ok(None);
                        }
                    }
                }
                if items.len() == 2 {
                    let inner = match &items[0] {
                        Value::Array(a) => *a,
                        _ => return Err(ZapcodeError::RuntimeError("invalid iterator".into())),
                    };
                    let idx = match &items[1] {
                        Value::Int(i) => *i as usize,
                        _ => return Err(ZapcodeError::RuntimeError("invalid iterator".into())),
                    };
                    let value = self.heap.array(inner).get(idx).cloned();
                    // Update iterator: same inner array handle, advanced index.
                    let new_iter = self
                        .heap
                        .alloc_array(vec![Value::Array(inner), Value::Int((idx + 1) as i64)]);
                    self.push(Value::Array(new_iter))?;
                    self.push(value.unwrap_or(Value::Undefined))?;
                } else {
                    return Err(ZapcodeError::RuntimeError("invalid iterator state".into()));
                }
            }
            Instruction::IteratorDone => {
                let value = self.pop()?;
                let items = match self.peek()? {
                    Value::Array(h) => self.heap.array_vec(*h),
                    _ => {
                        self.push(value)?;
                        self.push(Value::Bool(true))?;
                        return Ok(None);
                    }
                };
                // Check for generator iterator first
                if items.len() == 3 {
                    if let Value::String(s) = &items[0] {
                        if s.as_ref() == "__gen__" {
                            let done = matches!(&items[2], Value::Bool(true));
                            if !done {
                                self.push(value)?;
                            }
                            self.push(Value::Bool(done))?;
                            return Ok(None);
                        }
                    }
                }
                if items.len() == 2 {
                    let inner = match &items[0] {
                        Value::Array(a) => *a,
                        _ => {
                            self.push(value)?;
                            self.push(Value::Bool(true))?;
                            return Ok(None);
                        }
                    };
                    let idx = match &items[1] {
                        Value::Int(i) => *i as usize,
                        _ => {
                            self.push(value)?;
                            self.push(Value::Bool(true))?;
                            return Ok(None);
                        }
                    };
                    let done = idx > self.heap.array(inner).len();
                    if !done {
                        // Push value back for the binding
                        self.push(value)?;
                    }
                    self.push(Value::Bool(done))?;
                } else {
                    self.push(value)?;
                    self.push(Value::Bool(true))?;
                }
            }

            // Error handling
            Instruction::SetupTry {
                catch_ip,
                finally_ip,
                region_end,
            } => {
                // `has_catch` is encoded by the compiler: when there is a catch
                // handler, catch_ip points at it; when there is none but there is
                // a finally, catch_ip == finally_ip (so a throw runs the finally).
                let has_catch = match finally_ip {
                    Some(fin) => catch_ip != fin,
                    None => true,
                };
                // ip was already advanced past this instruction; the statement
                // region begins at the SetupTry itself.
                let region_start = self.current_frame().ip - 1;
                self.try_stack.push(TryInfo {
                    catch_ip,
                    frame_depth: self.frames.len(),
                    stack_depth: self.stack.len(),
                    finally_ip,
                    has_catch,
                    pending: None,
                    in_finally: false,
                    region_start,
                    region_end,
                });
            }
            Instruction::Throw => {
                let val = self.pop()?;
                let msg = val.to_js_string(&self.heap);
                // Preserve the thrown value so a `catch` binding sees it verbatim
                // (string/object/…), while uncaught throws still report `msg`.
                self.pending_throw = Some(val);
                return Err(ZapcodeError::RuntimeError(msg));
            }
            Instruction::EndTry => {
                self.try_stack.pop();
            }
            Instruction::EnterFinallyNormal(finally_ip) => {
                // The try (or catch) body completed normally; transition the
                // active try frame into its finally phase with a Normal pending
                // completion and run the finally body.
                if let Some(info) = self.try_stack.last_mut() {
                    info.pending = Some(Completion::Normal);
                    info.in_finally = true;
                    info.has_catch = false;
                }
                self.current_frame_mut().ip = finally_ip;
            }
            Instruction::EndFinally => {
                // The finally body finished without its own abrupt completion;
                // resume whatever the body/catch was doing.
                let info = self.try_stack.pop();
                let completion = info.and_then(|i| i.pending).unwrap_or(Completion::Normal);
                if let Some(state) = self.resume_completion(completion)? {
                    return Ok(Some(state));
                }
            }

            // Typeof
            Instruction::TypeOf => {
                let val = self.pop()?;
                // `typeof null === "object"` (a long-standing JS quirk).
                // Callable builtin markers (bare global fns like `String`/`parseInt`,
                // builtin constructors like `Object`/`Map`, and `Symbol`) are objects
                // internally but must report as "function" (O8 needs
                // `typeof Symbol === "function"`). Pure namespaces (`Math`, `JSON`,
                // `console`) carry no callable marker and stay "object".
                let type_str = match &val {
                    Value::Null => "object",
                    // A produced Symbol value reports `typeof === "symbol"`.
                    Value::Object(h)
                        if self.heap.object(*h).is_some_and(|m| m.contains_key("__symbol__")) =>
                    {
                        "symbol"
                    }
                    // Callable builtin markers (bare global fns like `String`/`parseInt`,
                    // builtin constructors like `Object`/`Map`, and the `Symbol`
                    // factory) are objects internally but must report as "function"
                    // (O8 needs `typeof Symbol === "function"`). Pure namespaces
                    // (`Math`, `JSON`, `console`) carry no callable marker and stay
                    // "object".
                    Value::Object(h)
                        if self.heap.object(*h).is_some_and(|m| {
                            m.contains_key("__global_fn__")
                                || m.contains_key("__builtin_constructor__")
                        }) =>
                    {
                        "function"
                    }
                    other => other.type_name(),
                };
                self.push(Value::String(Arc::from(type_str)))?;
            }

            // Void
            Instruction::Void => {
                self.pop()?;
                self.push(Value::Undefined)?;
            }

            // Update
            Instruction::Increment => {
                let val = self.pop()?;
                let result = match val {
                    Value::Int(n) => Value::Int(n + 1),
                    _ => Value::Float(val.to_number_heap(&self.heap) + 1.0),
                };
                self.push(result)?;
            }
            Instruction::Decrement => {
                let val = self.pop()?;
                let result = match val {
                    Value::Int(n) => Value::Int(n - 1),
                    _ => Value::Float(val.to_number_heap(&self.heap) - 1.0),
                };
                self.push(result)?;
            }

            // Template literals
            Instruction::ConcatStrings(count) => {
                let mut parts = Vec::with_capacity(count);
                for _ in 0..count {
                    parts.push(self.pop()?);
                }
                parts.reverse();
                // ToPrimitive(string) each interpolated value so a user toString
                // hook is honored before string coercion.
                let mut result = String::new();
                for v in parts {
                    let prim = self.to_primitive(&v, ToPrimitiveHint::String)?;
                    result.push_str(&prim.to_js_string(&self.heap));
                }
                self.push(Value::String(Arc::from(result.as_str())))?;
            }

            // Destructuring
            Instruction::DestructureObject(keys) => {
                let obj = self.pop()?;
                for key in keys {
                    let val = self.get_property(&obj, &key)?;
                    self.push(val)?;
                }
            }
            Instruction::DestructureArray(count) => {
                let arr = self.pop()?;
                match arr {
                    Value::Array(h) => {
                        let items = self.heap.array_vec(h);
                        for i in 0..count {
                            self.push(items.get(i).cloned().unwrap_or(Value::Undefined))?;
                        }
                    }
                    _ => {
                        for _ in 0..count {
                            self.push(Value::Undefined)?;
                        }
                    }
                }
            }

            Instruction::Nop => {}

            // Generators
            Instruction::CreateGenerator(_func_idx) => {
                // Generator creation is handled at Call time via is_generator check.
            }
            Instruction::Yield => {
                // Yield is handled in run_generator_until_yield_or_return.
                // Reaching here means yield outside a generator function.
                return Err(ZapcodeError::RuntimeError(
                    "yield can only be used inside a generator function".to_string(),
                ));
            }

            Instruction::Await => {
                // Check if the value on the stack is a Promise object.
                // If resolved, unwrap its value. If rejected, throw its reason.
                // If it's a regular (non-promise) value, leave it as-is.
                let val = self.pop()?;
                if matches!(val, Value::Pending(_)) {
                    // A deferred call only ever lives inside a Promise.all batch;
                    // awaiting one bare is a compiler invariant violation.
                    return Err(ZapcodeError::RuntimeError(
                        "internal error: awaited a deferred call outside Promise.all".to_string(),
                    ));
                }
                if builtins::is_promise(&val, &self.heap) {
                    if let Value::Object(h) = &val {
                        let map = self.heap.object_map(*h);
                        let status = map.get("status").cloned().unwrap_or(Value::Undefined);
                        match status {
                            Value::String(s) if s.as_ref() == "resolved" => {
                                let inner = map.get("value").cloned().unwrap_or(Value::Undefined);
                                self.push(inner)?;
                            }
                            Value::String(s) if s.as_ref() == "rejected" => {
                                let reason = map.get("reason").cloned().unwrap_or(Value::Undefined);
                                let reason = reason.to_js_string(&self.heap);
                                return Err(ZapcodeError::RuntimeError(format!(
                                    "Unhandled promise rejection: {}",
                                    reason
                                )));
                            }
                            Value::String(s) if s.as_ref() == "pending_all" => {
                                let items = match map.get("items") {
                                    Some(Value::Array(a)) => self.heap.array_vec(*a),
                                    _ => Vec::new(),
                                };
                                let kind = match map.get("__batch_kind__") {
                                    Some(Value::String(k)) => match k.as_ref() {
                                        "race" => BatchKind::Race,
                                        "any" => BatchKind::Any,
                                        "allSettled" => BatchKind::AllSettled,
                                        _ => BatchKind::All,
                                    },
                                    _ => BatchKind::All,
                                };
                                // Suspend on the whole batch (or resolve inline if
                                // every element is already available).
                                return self.await_batch(kind, items);
                            }
                            Value::String(s) if s.as_ref() == "pending_call" => {
                                // A deferred single-call promise (N5): trigger its
                                // host call now. Suspend with the call's name/args;
                                // resume pushes the settled value, which is exactly
                                // what `await` should produce. The value is also
                                // cached under the call id (`CacheValue`) so a
                                // second await/then on the same promise reuses it.
                                let id = match map.get("__call_id__") {
                                    Some(Value::Int(n)) => *n as u64,
                                    _ => {
                                        return Err(ZapcodeError::RuntimeError(
                                            "internal error: pending_call promise missing __call_id__"
                                                .to_string(),
                                        ))
                                    }
                                };
                                // Already settled (awaited before): reuse the value.
                                if let Some(cached) = self.resolved.get(&id).cloned() {
                                    self.push(cached)?;
                                } else {
                                    self.resume_action = Some(ResumeAction::CacheValue { id });
                                    return self.suspend_on_pending_call(id);
                                }
                            }
                            _ => {
                                // Unknown status — pass through
                                self.push(val)?;
                            }
                        }
                    } else {
                        self.push(val)?;
                    }
                } else {
                    // Not a promise — pass through (await on non-promise returns the value)
                    self.push(val)?;
                }
            }

            // Classes
            Instruction::CreateClass {
                name,
                n_methods,
                n_statics,
                has_super,
            } => {
                // Stack layout (top to bottom):
                // constructor closure (or undefined)
                // n_methods * (closure, method_name_string) pairs
                // n_statics * (closure, method_name_string) pairs
                // [optional super class if has_super]

                let constructor = self.pop()?;

                // Pop instance methods
                let mut prototype = IndexMap::new();
                for _ in 0..n_methods {
                    let method_closure = self.pop()?;
                    let method_name = self.pop()?;
                    if let Value::String(mn) = method_name {
                        prototype.insert(mn, method_closure);
                    }
                }

                // Pop static methods
                let mut statics = IndexMap::new();
                for _ in 0..n_statics {
                    let method_closure = self.pop()?;
                    let method_name = self.pop()?;
                    if let Value::String(mn) = method_name {
                        statics.insert(mn, method_closure);
                    }
                }

                // Pop super class if present
                let super_class = if has_super { Some(self.pop()?) } else { None };

                // If super class, copy its prototype methods to ours (inheritance)
                if let Some(Value::Object(sc)) = &super_class {
                    if let Some(Value::Object(super_proto_h)) =
                        self.heap.object(*sc).and_then(|m| m.get("__prototype__").cloned())
                    {
                        // Super prototype methods go first, then our own (which override)
                        let mut merged = self.heap.object_map(super_proto_h);
                        for (k, v) in prototype {
                            merged.insert(k, v);
                        }
                        prototype = merged;
                    }
                }

                // The inheritance chain of class names (self first), so
                // `instanceof` matches ancestor classes too.
                let mut chain = vec![Value::String(Arc::from(name.as_str()))];
                if let Some(Value::Object(sc)) = &super_class {
                    let scm = self.heap.object_map(*sc);
                    match scm.get("__class_chain__") {
                        Some(Value::Array(c)) => chain.extend(self.heap.array_vec(*c)),
                        _ => {
                            if let Some(n) = scm.get("__class_name__") {
                                chain.push(n.clone());
                            }
                        }
                    }
                }

                let chain_arr = Value::Array(self.heap.alloc_array(chain));
                let proto_obj = Value::Object(self.heap.alloc_object(prototype));

                // Build the class object
                let mut class_obj = IndexMap::new();
                class_obj.insert(
                    Arc::from("__class_name__"),
                    Value::String(Arc::from(name.as_str())),
                );
                class_obj.insert(Arc::from("__class_chain__"), chain_arr);
                class_obj.insert(Arc::from("__constructor__"), constructor);
                class_obj.insert(Arc::from("__prototype__"), proto_obj);

                // Store super class reference for super() calls
                if let Some(sc) = super_class {
                    class_obj.insert(Arc::from("__super__"), sc);
                }

                // Add static methods directly on the class object
                for (k, v) in statics {
                    class_obj.insert(k, v);
                }

                let h = self.heap.alloc_object(class_obj);
                self.push(Value::Object(h))?;
            }

            Instruction::Construct(arg_count) => {
                let mut args = Vec::with_capacity(arg_count);
                for _ in 0..arg_count {
                    args.push(self.pop()?);
                }
                args.reverse();

                let callee = self.pop()?;

                if let Value::Object(obj_h) = &callee {
                    let builtin_ctor = self
                        .heap
                        .object(*obj_h)
                        .and_then(|m| m.get("__builtin_constructor__"))
                        .and_then(|v| match v {
                            Value::String(s) => Some(s.clone()),
                            _ => None,
                        });
                    if let Some(name) = builtin_ctor {
                        match name.as_ref() {
                            "Map" => {
                                // Accepts an array of [k, v] pairs OR another Map.
                                let arg = args.first().cloned().unwrap_or(Value::Undefined);
                                let entries = build_map_entries(&arg, &mut self.heap);
                                let m = make_map_object(entries, &mut self.heap);
                                self.push(m)?;
                                return Ok(None);
                            }
                            "Set" => {
                                // Accepts an array, string, Set, or Map.
                                let arg = args.first().cloned().unwrap_or(Value::Undefined);
                                let items = iterable_items(&arg, &mut self.heap);
                                let s = make_set_object(items, &mut self.heap);
                                self.push(s)?;
                                return Ok(None);
                            }
                            "Date" => {
                                let d = construct_date(&args, &mut self.heap);
                                self.push(d)?;
                                return Ok(None);
                            }
                            "Error" | "TypeError" | "RangeError" | "SyntaxError"
                            | "ReferenceError" => {
                                let msg = args
                                    .first()
                                    .map(|v| v.to_js_string(&self.heap))
                                    .unwrap_or_default();
                                let e = make_error_object(name.as_ref(), &msg, &mut self.heap);
                                self.push(e)?;
                                return Ok(None);
                            }
                            "AggregateError" => {
                                // new AggregateError(errors, message)
                                let errors = match args.first().cloned() {
                                    Some(v) => v,
                                    None => Value::Array(self.heap.alloc_array(Vec::new())),
                                };
                                let msg = args
                                    .get(1)
                                    .map(|v| v.to_js_string(&self.heap))
                                    .unwrap_or_default();
                                let e = make_error_object("AggregateError", &msg, &mut self.heap);
                                if let Value::Object(m) = &e {
                                    if let Some(map) = self.heap.object_mut(*m) {
                                        map.insert(Arc::from("errors"), errors);
                                    }
                                }
                                self.push(e)?;
                                return Ok(None);
                            }
                            "Array" => {
                                // new Array(n): n empty slots; new Array(a, b, ...): [a, b, ...].
                                let arr = match args.as_slice() {
                                    [single] => match single {
                                        Value::Int(n) if *n >= 0 => {
                                            let len = *n as usize;
                                            self.track_array_capacity(len)?;
                                            vec![Value::Undefined; len]
                                        }
                                        Value::Float(f) if f.fract() == 0.0 && *f >= 0.0 => {
                                            let len = *f as usize;
                                            self.track_array_capacity(len)?;
                                            vec![Value::Undefined; len]
                                        }
                                        other => vec![other.clone()],
                                    },
                                    other => other.to_vec(),
                                };
                                let h = self.heap.alloc_array(arr);
                                self.push(Value::Array(h))?;
                                return Ok(None);
                            }
                            "Object" => {
                                match args.first() {
                                    Some(v @ (Value::Object(_) | Value::Array(_))) => {
                                        self.push(v.clone())?
                                    }
                                    _ => {
                                        let h = self.heap.alloc_object(IndexMap::new());
                                        self.push(Value::Object(h))?
                                    }
                                }
                                return Ok(None);
                            }
                            _ => {}
                        }
                    }
                }

                let is_user_class = matches!(&callee, Value::Object(h)
                    if self.heap.object(*h).is_some_and(|m| m.contains_key("__class_name__")));
                match &callee {
                    Value::Object(class_h) if is_user_class => {
                        let class_obj = self.heap.object_map(*class_h);
                        // Create a new instance object
                        let mut instance = IndexMap::new();

                        // Copy prototype methods onto the instance
                        if let Some(Value::Object(proto_h)) = class_obj.get("__prototype__") {
                            for (k, v) in self.heap.object_map(*proto_h) {
                                instance.insert(k.clone(), v.clone());
                            }
                        }

                        // Store class reference(s) for instanceof.
                        if let Some(class_name) = class_obj.get("__class_name__") {
                            instance.insert(Arc::from("__class__"), class_name.clone());
                        }
                        if let Some(chain) = class_obj.get("__class_chain__") {
                            instance.insert(Arc::from("__class_chain__"), chain.clone());
                        }

                        let instance_val = Value::Object(self.heap.alloc_object(instance));

                        // Call the constructor with `this` bound to the instance.
                        // A subclass with no own constructor forwards to the nearest
                        // ancestor constructor (implicit `constructor(...a){ super(...a) }`).
                        match find_class_constructor(&class_obj, &self.heap) {
                            Some(Value::Function(closure)) => {
                                // Clear receiver source — constructors should not
                                // write back to a receiver variable.
                                self.last_receiver_source = None;
                                self.push_call_frame(&closure, &args, Some(instance_val))?;
                                self.last_receiver = None;
                            }
                            _ => {
                                self.push(instance_val)?;
                            }
                        }
                    }
                    Value::Function(closure) => {
                        // `new` on a plain function — just call it
                        let closure = closure.clone();
                        self.push_call_frame(&closure, &args, None)?;
                        self.last_receiver = None;
                    }
                    _ => {
                        let msg = callee.to_js_string(&self.heap);
                        return Err(ZapcodeError::TypeError(format!(
                            "{} is not a constructor",
                            msg
                        )));
                    }
                }
            }

            Instruction::LoadThis => {
                // See LoadLocal: clear any stale builtin-global name so a property
                // read on `this` can't pick up the wrong builtin method.
                self.last_global_name = None;
                // Walk frames from top to find the nearest `this` value
                let this_val = self
                    .frames
                    .iter()
                    .rev()
                    .find_map(|f| f.this_value.clone())
                    .unwrap_or(Value::Undefined);
                // Establish a place so `this.items.push(...)` writes back to the
                // instance (persisted to the receiver when the method returns).
                self.last_place = Some(Place {
                    root: PlaceRoot::This,
                    path: Vec::new(),
                });
                self.push(this_val)?;
            }
            Instruction::StoreThis => {
                let val = self.pop()?;
                // Update this_value in the nearest frame that has one
                for frame in self.frames.iter_mut().rev() {
                    if frame.this_value.is_some() {
                        frame.this_value = Some(val);
                        break;
                    }
                }
            }
            Instruction::CallSuper { arg_count, class } => {
                let mut args = Vec::with_capacity(arg_count);
                for _ in 0..arg_count {
                    args.push(self.pop()?);
                }
                args.reverse();

                // Get current `this` value (the instance being constructed)
                let this_val = self
                    .frames
                    .iter()
                    .rev()
                    .find_map(|f| f.this_value.clone())
                    .unwrap_or(Value::Undefined);

                // Resolve the super-class constructor. Prefer the lexically-defining
                // class (`class.__super__`), which is correct even when several
                // classes share a single ancestor; fall back to the legacy global
                // scan for any bytecode compiled before the class name was threaded.
                let super_class_handle = class
                    .as_deref()
                    .and_then(|name| self.super_class_handle_of(name))
                    .or_else(|| self.any_super_class_handle());

                let super_ctor = super_class_handle.and_then(|sh| {
                    // Walk to the nearest ancestor that actually defines a
                    // constructor, so chained empty subclasses still forward args.
                    let parent = self.heap.object(sh)?.clone();
                    find_class_constructor(&parent, &self.heap)
                });

                if let Some(Value::Function(closure)) = super_ctor {
                    self.last_receiver_source = None;
                    self.push_call_frame(&closure, &args, Some(this_val))?;
                    self.last_receiver = None;
                } else {
                    // No super constructor found — push undefined
                    self.push(Value::Undefined)?;
                }
            }
            Instruction::LoadSuperMethod { class, method } => {
                // `super.method(...)`: bind the current `this` and push the parent
                // method so the following `Call` invokes it with this receiver.
                let this_val = self
                    .frames
                    .iter()
                    .rev()
                    .find_map(|f| f.this_value.clone())
                    .unwrap_or(Value::Undefined);

                let m = self.super_prototype_member(&class, &method);
                match m {
                    Some(func @ Value::Function(_)) => {
                        self.last_receiver = Some(this_val);
                        self.last_receiver_source = None;
                        self.push(func)?;
                    }
                    _ => {
                        return Err(ZapcodeError::TypeError(format!(
                            "(intermediate value).{} is not a function",
                            method
                        )));
                    }
                }
            }
            Instruction::LoadSuperProp { class, prop } => {
                // `super.prop` read — fetch from the super prototype; absent data
                // properties yield undefined (instance fields live on `this`).
                let v = self
                    .super_prototype_member(&class, &prop)
                    .unwrap_or(Value::Undefined);
                self.push(v)?;
            }
        }

        Ok(None)
    }

    /// Resolve the class object stored in globals under `name`, returning its
    /// heap handle if it's an actual class (has `__class_name__`). Classes are
    /// bound to globals by their declared name, which is how `super` finds its
    /// lexically-defining class at runtime.
    fn class_handle_by_name(&self, name: &str) -> Option<Handle> {
        match self.globals.get(name) {
            Some(Value::Object(h))
                if self
                    .heap
                    .object(*h)
                    .is_some_and(|m| m.contains_key("__class_name__")) =>
            {
                Some(*h)
            }
            _ => None,
        }
    }

    /// The `__super__` class handle of the class named `name`, if any.
    fn super_class_handle_of(&self, name: &str) -> Option<Handle> {
        let class_h = self.class_handle_by_name(name)?;
        match self.heap.object(class_h).and_then(|m| m.get("__super__")) {
            Some(Value::Object(sh)) => Some(*sh),
            _ => None,
        }
    }

    /// Legacy fallback: the first class in globals that declares a `__super__`.
    /// Only used for bytecode compiled before the defining-class name was carried
    /// on `CallSuper`; ambiguous with multiple subclasses but preserves old behavior.
    fn any_super_class_handle(&self) -> Option<Handle> {
        let handles: Vec<Handle> = self
            .globals
            .values()
            .filter_map(|v| match v {
                Value::Object(h) => Some(*h),
                _ => None,
            })
            .collect();
        handles.into_iter().find_map(|h| {
            match self.heap.object(h).and_then(|m| m.get("__super__")) {
                Some(Value::Object(sh)) => Some(*sh),
                _ => None,
            }
        })
    }

    /// Look up `member` on the super prototype of the class named `class`.
    /// The super class's `__prototype__` is itself already flattened with its own
    /// ancestors' methods, so this resolves inherited members through the chain.
    fn super_prototype_member(&self, class: &str, member: &str) -> Option<Value> {
        let super_h = self.super_class_handle_of(class)?;
        let proto_h = match self.heap.object(super_h).and_then(|m| m.get("__prototype__")) {
            Some(Value::Object(ph)) => *ph,
            _ => return None,
        };
        self.heap
            .object(proto_h)
            .and_then(|m| m.get(member).cloned())
    }

    fn get_property(&self, obj: &Value, name: &str) -> Result<Value> {
        // Property access on null/undefined throws TypeError (like JS)
        if matches!(obj, Value::Null | Value::Undefined) {
            return Err(ZapcodeError::TypeError(format!(
                "Cannot read properties of {} (reading '{}')",
                obj.to_js_string(&self.heap),
                name
            )));
        }
        match obj {
            Value::Object(h) => {
                let map = self.heap.object_map(*h);
                // Check if property exists as a real value on the object
                if let Some(val) = map.get(name) {
                    if !matches!(val, Value::Undefined) {
                        return Ok(val.clone());
                    }
                }
                // Check if this is a promise instance — expose .then/.catch/.finally
                if builtins::is_promise(obj, &self.heap) && is_promise_method(name) {
                    return Ok(builtin_method("__promise__", name));
                }
                if is_map_object(obj, &self.heap) {
                    if name == "size" {
                        let n = match map.get("__entries__") {
                            Some(Value::Array(e)) => self.heap.array(*e).len(),
                            _ => 0,
                        };
                        return Ok(Value::Int(n as i64));
                    }
                    if is_map_method(name) {
                        return Ok(builtin_method("__map__", name));
                    }
                }
                if is_set_object(obj, &self.heap) {
                    if name == "size" {
                        let n = set_items(obj, &self.heap).len();
                        return Ok(Value::Int(n as i64));
                    }
                    if is_set_method(name) {
                        return Ok(builtin_method("__set__", name));
                    }
                }
                if is_date_object(obj, &self.heap) && is_date_method(name) {
                    return Ok(builtin_method("__date__", name));
                }
                if builtins::regexp_parts(obj, &self.heap).is_some()
                    && matches!(name, "test" | "exec")
                {
                    return Ok(builtin_method("__regexp__", name));
                }
                // Check if this is a known global object — return builtin method handle
                if let Some(global_name) = &self.last_global_name {
                    if Self::BUILTIN_GLOBAL_NAMES.contains(&global_name.as_str()) {
                        return Ok(builtin_method(global_name.as_str(), name));
                    }
                }
                // Object.prototype instance methods.
                if matches!(name, "hasOwnProperty") {
                    return Ok(builtin_method("__object__", name));
                }
                Ok(Value::Undefined)
            }
            Value::Array(h) => match name {
                "length" => Ok(Value::Int(self.heap.array(*h).len() as i64)),
                _ if is_array_method(name) => Ok(builtin_method("__array__", name)),
                _ => {
                    if let Ok(idx) = name.parse::<usize>() {
                        Ok(self.heap.array(*h).get(idx).cloned().unwrap_or(Value::Undefined))
                    } else {
                        Ok(Value::Undefined)
                    }
                }
            },
            Value::String(s) => match name {
                "length" => Ok(Value::Int(s.chars().count() as i64)),
                _ if is_string_method(name) => Ok(builtin_method("__string__", name)),
                _ => Ok(Value::Undefined),
            },
            Value::Generator(_) => match name {
                "next" | "return" | "throw" => Ok(builtin_method("__generator__", name)),
                _ => Ok(Value::Undefined),
            },
            Value::Int(_) | Value::Float(_) if builtins::is_number_method(name) => {
                Ok(builtin_method("__number__", name))
            }
            _ => Ok(Value::Undefined),
        }
    }
}

/// Construct an unbound builtin-method handle. The receiver and write-back place
/// are attached by the `GetProperty`/`GetIndex` handlers.
fn builtin_method(object_name: &str, method_name: &str) -> Value {
    Value::BuiltinMethod {
        object_name: Arc::from(object_name),
        method_name: Arc::from(method_name),
        recv: None,
        place: None,
    }
}

// Re-export for the ParamPattern type used in function calls
use crate::parser::ir::{DestructureField, ParamPattern};

/// Read a named field from `value` (Undefined if missing / not an object).
fn field_of(value: &Value, key: &str, heap: &Heap) -> Value {
    match value {
        Value::Object(h) => heap
            .object(*h)
            .and_then(|m| m.get(key).cloned())
            .unwrap_or(Value::Undefined),
        _ => Value::Undefined,
    }
}

/// Extract the locals bound by a (possibly nested) destructuring pattern from
/// `value`, pushing them in declaration order to mirror `declare_destructure_locals`.
fn extract_pattern(pattern: &ParamPattern, value: &Value, out: &mut Vec<Value>, heap: &mut Heap) {
    match pattern {
        ParamPattern::Ident(_) | ParamPattern::Rest(_) => out.push(value.clone()),
        ParamPattern::DefaultValue { pattern, .. } => extract_pattern(pattern, value, out, heap),
        ParamPattern::ObjectDestructure(fields) => extract_object_fields(fields, value, out, heap),
        ParamPattern::ArrayDestructure(elems) => {
            for (i, elem) in elems.iter().enumerate() {
                if let Some(p) = elem {
                    if matches!(p, ParamPattern::Rest(_)) {
                        // `...rest`: collect the remaining elements as an array.
                        let rest_items: Vec<Value> = match value {
                            Value::Array(a) => heap.array(*a).iter().skip(i).cloned().collect(),
                            _ => Vec::new(),
                        };
                        out.push(Value::Array(heap.alloc_array(rest_items)));
                        continue;
                    }
                    let item = match value {
                        Value::Array(a) => heap.array(*a).get(i).cloned().unwrap_or(Value::Undefined),
                        _ => Value::Undefined,
                    };
                    extract_pattern(p, &item, out, heap);
                }
            }
        }
    }
}

fn extract_object_fields(
    fields: &[DestructureField],
    value: &Value,
    out: &mut Vec<Value>,
    heap: &mut Heap,
) {
    let mut consumed: Vec<String> = Vec::new();
    for field in fields {
        if field.rest {
            let rest_map: IndexMap<Arc<str>, Value> = match value {
                Value::Object(h) => heap
                    .object_map(*h)
                    .into_iter()
                    .filter(|(k, _)| !consumed.iter().any(|c| c == k.as_ref()))
                    .collect(),
                _ => IndexMap::new(),
            };
            out.push(Value::Object(heap.alloc_object(rest_map)));
        } else if let Some(nested) = &field.nested {
            consumed.push(field.key.clone());
            let child = field_of(value, &field.key, heap);
            extract_pattern(nested, &child, out, heap);
        } else {
            consumed.push(field.key.clone());
            out.push(field_of(value, &field.key, heap));
        }
    }
}

/// JS `ToInt32`: truncate, take modulo 2^32, then interpret as a signed 32-bit
/// integer. A plain `f64 as i32` cast in Rust *saturates* at i32::MIN/MAX, which
/// is wrong for operands >= 2^31 (e.g. `4294967296 | 0` must be 0, not i32::MAX).
fn js_to_int32(n: f64) -> i32 {
    if !n.is_finite() || n == 0.0 {
        return 0;
    }
    let m = n.trunc().rem_euclid(4_294_967_296.0); // [0, 2^32)
    if m >= 2_147_483_648.0 {
        (m - 4_294_967_296.0) as i32
    } else {
        m as i32
    }
}

/// JS `ToUint32`: like [`js_to_int32`] but the result is an unsigned 32-bit value.
fn js_to_uint32(n: f64) -> u32 {
    if !n.is_finite() || n == 0.0 {
        return 0;
    }
    n.trunc().rem_euclid(4_294_967_296.0) as u32
}

/// JS abstract relational comparison `left < right`. When both operands are
/// strings the comparison is lexicographic; otherwise it is numeric (NaN
/// operands make the result false, as `f64` ordering already does).
fn js_less_than(left: &Value, right: &Value, heap: &Heap) -> bool {
    if let (Value::String(a), Value::String(b)) = (left, right) {
        return a.as_ref() < b.as_ref();
    }
    left.to_number_heap(heap) < right.to_number_heap(heap)
}

/// JS `left <= right` (lexicographic for two strings, numeric otherwise).
fn js_less_than_or_equal(left: &Value, right: &Value, heap: &Heap) -> bool {
    if let (Value::String(a), Value::String(b)) = (left, right) {
        return a.as_ref() <= b.as_ref();
    }
    left.to_number_heap(heap) <= right.to_number_heap(heap)
}

/// True for reference values that coerce to a string in JS `+` (their
/// `ToPrimitive` with a default hint yields a string for the built-in cases:
/// arrays join, plain objects become `[object Object]`).
fn coerces_to_string_in_add(v: &Value) -> bool {
    matches!(
        v,
        Value::Array(_)
            | Value::Object(_)
            | Value::Function(_)
            | Value::BuiltinMethod { .. }
            | Value::Generator(_)
            | Value::Pending(_)
    )
}

/// SameValueZero comparison (`===` but `NaN` equals `NaN`). Used for Map keys
/// and Set membership so `NaN` behaves as a single value, like JS.
fn same_value_zero(a: &Value, b: &Value) -> bool {
    if a.strict_eq(b) {
        return true;
    }
    matches!((a, b), (Value::Float(x), Value::Float(y)) if x.is_nan() && y.is_nan())
}

/// Coerce a constructor argument into a list of items for `new Set(...)`:
/// arrays yield their elements, strings their characters, Sets their items, and
/// Maps their `[key, value]` pairs.
fn iterable_items(v: &Value, heap: &mut Heap) -> Vec<Value> {
    match v {
        Value::Array(a) => heap.array_vec(*a),
        Value::String(s) => s
            .chars()
            .map(|c| Value::String(Arc::from(c.to_string().as_str())))
            .collect(),
        Value::Object(_) if is_set_object(v, heap) => set_items(v, heap),
        Value::Object(_) if is_map_object(v, heap) => map_entry_pairs(v, heap),
        _ => Vec::new(),
    }
}

/// The `[key, value]` array pairs of a Map object's internal entries.
fn map_entry_pairs(v: &Value, heap: &mut Heap) -> Vec<Value> {
    let Value::Object(h) = v else {
        return Vec::new();
    };
    let map = heap.object_map(*h);
    match map.get("__entries__") {
        Some(Value::Array(eh)) => {
            let entries = heap.array_vec(*eh);
            let mut out = Vec::with_capacity(entries.len());
            for e in entries {
                if let Value::Object(em_h) = e {
                    let em = heap.object_map(em_h);
                    let pair = heap.alloc_array(vec![
                        em.get("key").cloned().unwrap_or(Value::Undefined),
                        em.get("value").cloned().unwrap_or(Value::Undefined),
                    ]);
                    out.push(Value::Array(pair));
                }
            }
            out
        }
        _ => Vec::new(),
    }
}

/// Build Map entry objects from a constructor argument (an array of `[k, v]`
/// pairs, or another Map). Later duplicate keys overwrite earlier ones.
fn build_map_entries(arg: &Value, heap: &mut Heap) -> Vec<Value> {
    let pairs = iterable_items(arg, heap);
    // Track entries as plain (key, value) tuples first, then alloc objects, so
    // we never hold a heap borrow across the dedup loop.
    let mut entries: Vec<(Value, Value)> = Vec::new();
    for p in pairs {
        let (k, v) = match p {
            Value::Array(kv) => {
                let kv = heap.array_vec(kv);
                (
                    kv.first().cloned().unwrap_or(Value::Undefined),
                    kv.get(1).cloned().unwrap_or(Value::Undefined),
                )
            }
            _ => (Value::Undefined, Value::Undefined),
        };
        if let Some(slot) = entries.iter_mut().find(|(ek, _)| same_value_zero(ek, &k)) {
            slot.1 = v;
        } else {
            entries.push((k, v));
        }
    }
    entries
        .into_iter()
        .map(|(k, v)| {
            let mut em = IndexMap::new();
            em.insert(Arc::from("key"), k);
            em.insert(Arc::from("value"), v);
            Value::Object(heap.alloc_object(em))
        })
        .collect()
}

fn make_map_object(entries: Vec<Value>, heap: &mut Heap) -> Value {
    let entries_arr = Value::Array(heap.alloc_array(entries));
    let mut obj = IndexMap::new();
    obj.insert(Arc::from("__map__"), Value::Bool(true));
    obj.insert(Arc::from("__entries__"), entries_arr);
    Value::Object(heap.alloc_object(obj))
}

fn is_map_object(value: &Value, heap: &Heap) -> bool {
    match value {
        Value::Object(h) => matches!(
            heap.object(*h).and_then(|m| m.get("__map__")),
            Some(Value::Bool(true))
        ),
        _ => false,
    }
}

fn is_map_method(name: &str) -> bool {
    matches!(
        name,
        "get" | "set" | "has" | "delete" | "clear" | "keys" | "values" | "entries" | "forEach"
    )
}

fn is_set_object(value: &Value, heap: &Heap) -> bool {
    match value {
        Value::Object(h) => matches!(
            heap.object(*h).and_then(|m| m.get("__set__")),
            Some(Value::Bool(true))
        ),
        _ => false,
    }
}

fn is_set_method(name: &str) -> bool {
    matches!(
        name,
        "add" | "has" | "delete" | "clear" | "values" | "keys" | "forEach"
    )
}

/// Build a Set object from items, de-duplicating by strict equality.
fn make_set_object(items: Vec<Value>, heap: &mut Heap) -> Value {
    let mut unique: Vec<Value> = Vec::new();
    for item in items {
        if !unique.iter().any(|u| same_value_zero(u, &item)) {
            unique.push(item);
        }
    }
    let items_arr = Value::Array(heap.alloc_array(unique));
    let mut obj = IndexMap::new();
    obj.insert(Arc::from("__set__"), Value::Bool(true));
    obj.insert(Arc::from("__items__"), items_arr);
    Value::Object(heap.alloc_object(obj))
}

fn set_items(value: &Value, heap: &Heap) -> Vec<Value> {
    let Value::Object(h) = value else {
        return Vec::new();
    };
    match heap.object(*h).and_then(|m| m.get("__items__")) {
        Some(Value::Array(ih)) => heap.array_vec(*ih),
        _ => Vec::new(),
    }
}

/// Execute a Set method. The set object is a heap handle; mutations edit its
/// internal `__items__` array in place, so changes are visible through aliases.
fn execute_set_method(
    set_handle: Handle,
    method: &str,
    args: &[Value],
    heap: &mut Heap,
) -> Option<Value> {
    // The handle of the inner `__items__` array.
    let items_handle = match heap.object(set_handle).and_then(|m| m.get("__items__")) {
        Some(Value::Array(ih)) => *ih,
        _ => return None,
    };
    let items = heap.array_vec(items_handle);
    let arg = args.first().cloned().unwrap_or(Value::Undefined);
    match method {
        "has" => Some(Value::Bool(items.iter().any(|i| same_value_zero(i, &arg)))),
        "add" => {
            if !items.iter().any(|i| same_value_zero(i, &arg)) {
                if let Some(v) = heap.array_mut(items_handle) {
                    v.push(arg);
                }
            }
            Some(Value::Object(set_handle))
        }
        "delete" => {
            let mut removed = false;
            if let Some(v) = heap.array_mut(items_handle) {
                let before = v.len();
                v.retain(|i| !same_value_zero(i, &arg));
                removed = v.len() != before;
            }
            Some(Value::Bool(removed))
        }
        "clear" => {
            heap.set_array(items_handle, Vec::new());
            Some(Value::Undefined)
        }
        "values" | "keys" => Some(Value::Array(heap.alloc_array(items))),
        _ => None,
    }
}

/// The constructor a class should run: its own, or (for a subclass with no own
/// constructor) the nearest ancestor's, so args forward to `super`.
fn find_class_constructor(class_obj: &IndexMap<Arc<str>, Value>, heap: &Heap) -> Option<Value> {
    match class_obj.get("__constructor__") {
        Some(Value::Function(c)) => Some(Value::Function(c.clone())),
        _ => match class_obj.get("__super__") {
            Some(Value::Object(sc)) => {
                let parent = heap.object(*sc)?.clone();
                find_class_constructor(&parent, heap)
            }
            _ => None,
        },
    }
}

fn make_error_object(name: &str, message: &str, heap: &mut Heap) -> Value {
    let mut obj = IndexMap::new();
    obj.insert(Arc::from("__error__"), Value::Bool(true));
    obj.insert(Arc::from("name"), Value::String(Arc::from(name)));
    obj.insert(Arc::from("message"), Value::String(Arc::from(message)));
    obj.insert(
        Arc::from("stack"),
        Value::String(Arc::from(format!("{}: {}", name, message).as_str())),
    );
    Value::Object(heap.alloc_object(obj))
}

/// Execute a Map method. The map object is a heap handle; mutations edit its
/// internal `__entries__` array (and entry objects) in place via the heap, so
/// changes are visible through every alias (reference semantics).
fn execute_map_method(
    map_handle: Handle,
    method: &str,
    args: &[Value],
    heap: &mut Heap,
) -> Option<Value> {
    let entries_handle = match heap.object(map_handle).and_then(|m| m.get("__entries__")) {
        Some(Value::Array(eh)) => *eh,
        _ => return None,
    };
    let entries = heap.array_vec(entries_handle);
    let key = args.first().cloned().unwrap_or(Value::Undefined);

    // Locate the entry-object handle whose "key" matches `key` (SameValueZero).
    let find_entry = |entries: &[Value], heap: &Heap| -> Option<Handle> {
        entries.iter().find_map(|entry| match entry {
            Value::Object(eh)
                if heap
                    .object(*eh)
                    .and_then(|e| e.get("key"))
                    .is_some_and(|item| same_value_zero(item, &key)) =>
            {
                Some(*eh)
            }
            _ => None,
        })
    };

    match method {
        "get" => {
            let value = find_entry(&entries, heap)
                .and_then(|eh| heap.object(eh).and_then(|e| e.get("value").cloned()))
                .unwrap_or(Value::Undefined);
            Some(value)
        }
        "has" => Some(Value::Bool(find_entry(&entries, heap).is_some())),
        "set" => {
            let value = args.get(1).cloned().unwrap_or(Value::Undefined);
            if let Some(eh) = find_entry(&entries, heap) {
                if let Some(e) = heap.object_mut(eh) {
                    e.insert(Arc::from("value"), value);
                }
            } else {
                let mut entry = IndexMap::new();
                entry.insert(Arc::from("key"), key);
                entry.insert(Arc::from("value"), value);
                let eh = heap.alloc_object(entry);
                if let Some(v) = heap.array_mut(entries_handle) {
                    v.push(Value::Object(eh));
                }
            }
            Some(Value::Object(map_handle))
        }
        "delete" => {
            let target = find_entry(&entries, heap);
            let mut deleted = false;
            if let Some(eh) = target {
                if let Some(v) = heap.array_mut(entries_handle) {
                    let before = v.len();
                    v.retain(|e| !matches!(e, Value::Object(h) if *h == eh));
                    deleted = v.len() != before;
                }
            }
            Some(Value::Bool(deleted))
        }
        "clear" => {
            heap.set_array(entries_handle, Vec::new());
            Some(Value::Undefined)
        }
        "keys" => {
            let keys: Vec<Value> = entries
                .iter()
                .filter_map(|e| match e {
                    Value::Object(eh) => heap.object(*eh).and_then(|m| m.get("key").cloned()),
                    _ => None,
                })
                .collect();
            Some(Value::Array(heap.alloc_array(keys)))
        }
        "values" => {
            let vals: Vec<Value> = entries
                .iter()
                .filter_map(|e| match e {
                    Value::Object(eh) => heap.object(*eh).and_then(|m| m.get("value").cloned()),
                    _ => None,
                })
                .collect();
            Some(Value::Array(heap.alloc_array(vals)))
        }
        "entries" => {
            let pairs: Vec<(Value, Value)> = entries
                .iter()
                .filter_map(|e| match e {
                    Value::Object(eh) => heap.object(*eh).map(|m| {
                        (
                            m.get("key").cloned().unwrap_or(Value::Undefined),
                            m.get("value").cloned().unwrap_or(Value::Undefined),
                        )
                    }),
                    _ => None,
                })
                .collect();
            let out: Vec<Value> = pairs
                .into_iter()
                .map(|(k, v)| Value::Array(heap.alloc_array(vec![k, v])))
                .collect();
            Some(Value::Array(heap.alloc_array(out)))
        }
        _ => None,
    }
}

fn make_date_object(millis: i64, heap: &mut Heap) -> Value {
    let mut obj = IndexMap::new();
    obj.insert(Arc::from("__date_ms__"), Value::Int(millis));
    Value::Object(heap.alloc_object(obj))
}

/// An Invalid Date (its time value is NaN).
fn make_invalid_date(heap: &mut Heap) -> Value {
    let mut obj = IndexMap::new();
    obj.insert(Arc::from("__date_ms__"), Value::Float(f64::NAN));
    Value::Object(heap.alloc_object(obj))
}

/// Build a Date from constructor arguments:
/// - none: epoch 0 (the sandbox has no wall clock, for deterministic replay);
/// - one string: parse it (ISO / `YYYY-MM-DD`); invalid -> Invalid Date;
/// - one number: epoch millis;
/// - 2+ numbers: `(year, monthIndex, day, hours, minutes, seconds, ms)` in UTC.
fn construct_date(args: &[Value], heap: &mut Heap) -> Value {
    match args {
        [] => make_date_object(0, heap),
        [single] => match single {
            Value::String(s) => match parse_date_string(s) {
                Some(ms) => make_date_object(ms, heap),
                None => make_invalid_date(heap),
            },
            other => {
                let n = other.to_number();
                if n.is_finite() {
                    make_date_object(n as i64, heap)
                } else {
                    make_invalid_date(heap)
                }
            }
        },
        _ => {
            let part = |i: usize, default: i64| -> i64 {
                args.get(i).map(|v| v.to_number() as i64).unwrap_or(default)
            };
            let year = part(0, 1970);
            let month = part(1, 0); // 0-indexed
            let day = part(2, 1);
            let hours = part(3, 0);
            let minutes = part(4, 0);
            let seconds = part(5, 0);
            let ms = part(6, 0);
            let days = days_from_civil(year, month + 1, day);
            let millis = days * 86_400_000
                + hours * 3_600_000
                + minutes * 60_000
                + seconds * 1_000
                + ms;
            make_date_object(millis, heap)
        }
    }
}

/// `Date.UTC(year, monthIndex, day, ...)` -> epoch millis (UTC).
pub(crate) fn date_utc_millis(args: &[Value]) -> f64 {
    let part = |i: usize, default: i64| -> i64 {
        args.get(i).map(|v| v.to_number() as i64).unwrap_or(default)
    };
    let year = part(0, 1970);
    let month = part(1, 0);
    let day = part(2, 1);
    let hours = part(3, 0);
    let minutes = part(4, 0);
    let seconds = part(5, 0);
    let ms = part(6, 0);
    (days_from_civil(year, month + 1, day) * 86_400_000
        + hours * 3_600_000
        + minutes * 60_000
        + seconds * 1_000
        + ms) as f64
}

/// Days since the Unix epoch for a civil (year, month 1-12, day) date (UTC).
/// Inverse of [`civil_from_days`] (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Parse a JS date string to epoch millis (UTC). Supports `YYYY-MM-DD`,
/// `YYYY-MM-DDTHH:MM[:SS[.fff]]` with an optional `Z` or `±HH:MM` offset, and a
/// space separator. Returns None for anything it can't parse.
pub(crate) fn parse_date_string(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date_part, time_part) = match s.split_once(['T', ' ']) {
        Some((d, t)) => (d, Some(t)),
        None => (s, None),
    };
    let mut dp = date_part.split('-');
    let year: i64 = dp.next()?.trim().parse().ok()?;
    let month: i64 = match dp.next() {
        Some(m) => m.parse().ok()?,
        None => 1,
    };
    let day: i64 = match dp.next() {
        Some(d) => d.parse().ok()?,
        None => 1,
    };
    if dp.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut millis = days_from_civil(year, month, day) * 86_400_000;

    if let Some(time) = time_part {
        let time = time.trim();
        // Split off a trailing timezone designator.
        let (clock, tz_offset_min): (&str, i64) = if let Some(rest) = time.strip_suffix('Z') {
            (rest, 0)
        } else if let Some(idx) = time.rfind(['+', '-']).filter(|&i| i >= 5) {
            // i >= 5 avoids matching a '-' inside the time (HH:MM:SS has none).
            let (clock, tz) = time.split_at(idx);
            let sign = if tz.starts_with('-') { -1 } else { 1 };
            let tz = &tz[1..];
            let mut tzp = tz.split(':');
            let oh: i64 = tzp.next()?.parse().ok()?;
            let om: i64 = tzp.next().map_or(Ok(0), |x| x.parse()).ok()?;
            (clock, sign * (oh * 60 + om))
        } else {
            (time, 0)
        };
        let mut cp = clock.split(':');
        let hours: i64 = cp.next()?.parse().ok()?;
        let minutes: i64 = cp.next().map_or(Ok(0), |x| x.parse()).ok()?;
        let (seconds, frac_ms): (i64, i64) = match cp.next() {
            Some(sec) => match sec.split_once('.') {
                Some((whole, frac)) => {
                    let ms_str: String = frac.chars().take(3).collect();
                    let ms_str = format!("{:0<3}", ms_str);
                    (whole.parse().ok()?, ms_str.parse().ok()?)
                }
                None => (sec.parse().ok()?, 0),
            },
            None => (0, 0),
        };
        millis += hours * 3_600_000 + minutes * 60_000 + seconds * 1_000 + frac_ms;
        millis -= tz_offset_min * 60_000;
    }
    Some(millis)
}

fn is_date_object(value: &Value, heap: &Heap) -> bool {
    match value {
        Value::Object(h) => heap.object(*h).is_some_and(|m| m.contains_key("__date_ms__")),
        _ => false,
    }
}

fn is_date_method(name: &str) -> bool {
    matches!(
        name,
        "toISOString"
            | "getTime"
            | "valueOf"
            | "getUTCFullYear"
            | "getFullYear"
            | "getUTCMonth"
            | "getMonth"
            | "getUTCDate"
            | "getDate"
            | "getUTCDay"
            | "getDay"
            | "getUTCHours"
            | "getHours"
            | "getUTCMinutes"
            | "getMinutes"
            | "getUTCSeconds"
            | "getSeconds"
            | "getUTCMilliseconds"
            | "getMilliseconds"
            | "toJSON"
            | "toString"
            | "toDateString"
            | "getTimezoneOffset"
    )
}

fn execute_date_method(map: &IndexMap<Arc<str>, Value>, method: &str) -> Option<Value> {
    let millis_f = match map.get("__date_ms__") {
        Some(Value::Int(millis)) => *millis as f64,
        Some(Value::Float(millis)) => *millis,
        _ => 0.0,
    };
    // An Invalid Date: time-based getters are NaN, formatters say "Invalid Date".
    if millis_f.is_nan() {
        return Some(match method {
            "getTime" | "valueOf" | "getUTCFullYear" | "getFullYear" | "getUTCMonth"
            | "getMonth" | "getUTCDate" | "getDate" | "getUTCDay" | "getDay" | "getUTCHours"
            | "getHours" | "getUTCMinutes" | "getMinutes" | "getUTCSeconds" | "getSeconds"
            | "getUTCMilliseconds" | "getMilliseconds" | "getTimezoneOffset" => {
                Value::Float(f64::NAN)
            }
            _ => Value::String(Arc::from("Invalid Date")),
        });
    }
    let millis = millis_f as i64;
    // getTimezoneOffset is 0 (the sandbox runs in UTC).
    if method == "getTimezoneOffset" {
        return Some(Value::Int(0));
    }
    if matches!(method, "toJSON" | "toString" | "toDateString") {
        return Some(Value::String(Arc::from(unix_millis_to_iso(millis).as_str())));
    }
    let seconds = millis.div_euclid(1000);
    let ms = millis.rem_euclid(1000);
    let days = seconds.div_euclid(86_400);
    let sod = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    // 1970-01-01 was a Thursday (4); 0 = Sunday.
    let dow = (days + 4).rem_euclid(7);
    match method {
        "getTime" | "valueOf" => Some(Value::Int(millis)),
        "toISOString" => Some(Value::String(Arc::from(
            unix_millis_to_iso(millis).as_str(),
        ))),
        // No timezone in the sandbox — local getters alias the UTC ones.
        "getUTCFullYear" | "getFullYear" => Some(Value::Int(year)),
        "getUTCMonth" | "getMonth" => Some(Value::Int((month as i64) - 1)), // 0-indexed
        "getUTCDate" | "getDate" => Some(Value::Int(day as i64)),
        "getUTCDay" | "getDay" => Some(Value::Int(dow)),
        "getUTCHours" | "getHours" => Some(Value::Int(sod / 3_600)),
        "getUTCMinutes" | "getMinutes" => Some(Value::Int((sod % 3_600) / 60)),
        "getUTCSeconds" | "getSeconds" => Some(Value::Int(sod % 60)),
        "getUTCMilliseconds" | "getMilliseconds" => Some(Value::Int(ms)),
        _ => None,
    }
}

pub(crate) fn unix_millis_to_iso(millis: i64) -> String {
    let seconds = millis.div_euclid(1000);
    let ms = millis.rem_euclid(1000);
    let days = seconds.div_euclid(86_400);
    let second_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = second_of_day / 3_600;
    let minute = (second_of_day % 3_600) / 60;
    let second = second_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{ms:03}Z")
}

fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (year, month as u32, day as u32)
}

fn is_array_method(name: &str) -> bool {
    matches!(
        name,
        "push"
            | "pop"
            | "shift"
            | "unshift"
            | "splice"
            | "slice"
            | "concat"
            | "join"
            | "reverse"
            | "sort"
            | "indexOf"
            | "lastIndexOf"
            | "includes"
            | "find"
            | "findIndex"
            | "findLast"
            | "findLastIndex"
            | "map"
            | "filter"
            | "reduce"
            | "reduceRight"
            | "forEach"
            | "every"
            | "some"
            | "flat"
            | "flatMap"
            | "fill"
            | "copyWithin"
            | "at"
            | "entries"
            | "keys"
            | "values"
    )
}

fn is_string_method(name: &str) -> bool {
    matches!(
        name,
        "charAt"
            | "charCodeAt"
            | "codePointAt"
            | "indexOf"
            | "lastIndexOf"
            | "includes"
            | "startsWith"
            | "endsWith"
            | "slice"
            | "substring"
            | "substr"
            | "toUpperCase"
            | "toLowerCase"
            | "trim"
            | "trimStart"
            | "trimEnd"
            | "trimLeft"
            | "trimRight"
            | "padStart"
            | "padEnd"
            | "repeat"
            | "replace"
            | "replaceAll"
            | "split"
            | "concat"
            | "at"
            | "match"
            | "matchAll"
            | "search"
            | "normalize"
            | "localeCompare"
    )
}

fn is_promise_method(name: &str) -> bool {
    matches!(name, "then" | "catch" | "finally")
}

/// Main entry point: compile and run TypeScript code.
pub struct ZapcodeRun {
    source: String,
    #[allow(dead_code)]
    inputs: Vec<String>,
    external_functions: Vec<String>,
    limits: ResourceLimits,
}

impl ZapcodeRun {
    pub fn new(
        source: String,
        inputs: Vec<String>,
        external_functions: Vec<String>,
        limits: ResourceLimits,
    ) -> Result<Self> {
        Ok(Self {
            source,
            inputs,
            external_functions,
            limits,
        })
    }

    pub fn run(&self, input_values: Vec<(String, Value)>) -> Result<RunResult> {
        self.run_with_input_heap(input_values, Heap::new())
    }

    /// Like [`run`], but seeds the VM with `input_heap` — the heap that backs any
    /// array/object `Value`s in `input_values`. Builtin globals are appended on
    /// top of this heap (see [`Vm::with_programs_and_heap`]) so the handles in the
    /// supplied inputs remain valid. Host bindings allocate compound inputs into a
    /// fresh heap and pass it here so the handles line up.
    pub fn run_with_input_heap(
        &self,
        input_values: Vec<(String, Value)>,
        input_heap: Heap,
    ) -> Result<RunResult> {
        let mut root_span = SpanBuilder::new("zapcode.run");

        // Parse
        let parse_span = SpanBuilder::new("parse");
        let program = match crate::parser::parse(&self.source) {
            Ok(p) => {
                root_span.add_child(parse_span.finish_ok());
                p
            }
            Err(e) => {
                root_span.add_child(parse_span.finish_error(&e.to_string()));
                let _trace = ExecutionTrace {
                    root: root_span.finish(TraceStatus::Error),
                };
                return Err(e);
            }
        };

        // Compile
        let compile_span = SpanBuilder::new("compile");
        let ext_set: HashSet<String> = self.external_functions.iter().cloned().collect();
        let compiled = match crate::compiler::compile_with_externals(&program, ext_set.clone()) {
            Ok(c) => {
                root_span.add_child(compile_span.finish_ok());
                c
            }
            Err(e) => {
                root_span.add_child(compile_span.finish_error(&e.to_string()));
                let _trace = ExecutionTrace {
                    root: root_span.finish(TraceStatus::Error),
                };
                return Err(e);
            }
        };

        // Execute
        let execute_span = SpanBuilder::new("execute");
        // An empty input heap is the common case (primitive or no inputs); take
        // the plain constructor so the seeded-heap path stays opt-in.
        let mut vm = if input_heap.is_empty() {
            Vm::new(compiled, self.limits.clone(), ext_set)
        } else {
            Vm::with_programs_and_heap(vec![compiled], self.limits.clone(), ext_set, input_heap)
        };

        for (name, value) in input_values {
            vm.globals.insert(name, value);
        }

        let state = match vm.run() {
            Ok(s) => {
                let status = match &s {
                    VmState::Complete(_) => TraceStatus::Ok,
                    VmState::Suspended {
                        function_name,
                        args,
                        ..
                    } => {
                        let mut span = execute_span;
                        span.set_attr("zapcode.suspended_on", function_name);
                        span.set_attr("zapcode.args_count", args.len());
                        root_span.add_child(span.finish(TraceStatus::Ok));
                        let trace = ExecutionTrace {
                            root: root_span.finish_ok(),
                        };
                        return Ok(RunResult {
                            state: s,
                            stdout: vm.stdout,
                            heap: vm.heap,
                            trace,
                        });
                    }
                    VmState::SuspendedMany { calls, .. } => {
                        let mut span = execute_span;
                        span.set_attr("zapcode.suspended_on", "Promise.all batch");
                        span.set_attr("zapcode.args_count", calls.len());
                        root_span.add_child(span.finish(TraceStatus::Ok));
                        let trace = ExecutionTrace {
                            root: root_span.finish_ok(),
                        };
                        return Ok(RunResult {
                            state: s,
                            stdout: vm.stdout,
                            heap: vm.heap,
                            trace,
                        });
                    }
                };
                root_span.add_child(execute_span.finish(status));
                s
            }
            Err(e) => {
                root_span.add_child(execute_span.finish_error(&e.to_string()));
                let _trace = ExecutionTrace {
                    root: root_span.finish(TraceStatus::Error),
                };
                return Err(e);
            }
        };

        let trace = ExecutionTrace {
            root: root_span.finish_ok(),
        };

        Ok(RunResult {
            state,
            stdout: vm.stdout,
            heap: vm.heap,
            trace,
        })
    }

    /// Start execution. Like `run()`, but returns the raw `VmState` directly
    /// instead of wrapping it in a `RunResult`. This is the primary entry point
    /// for code that needs to handle suspension / snapshot / resume.
    pub fn start(&self, input_values: Vec<(String, Value)>) -> Result<VmState> {
        let result = self.run(input_values)?;
        Ok(result.state)
    }

    pub fn run_simple(&self) -> Result<Value> {
        let result = self.run(Vec::new())?;
        match result.state {
            VmState::Complete(v) => Ok(v),
            VmState::Suspended { function_name, .. } => Err(ZapcodeError::RuntimeError(format!(
                "execution suspended on external function '{}' — use run() instead",
                function_name
            ))),
            VmState::SuspendedMany { .. } => Err(ZapcodeError::RuntimeError(
                "execution suspended on a Promise.all batch — use run() instead".to_string(),
            )),
        }
    }
}

/// Result of running a Zapcode program.
#[derive(Debug)]
pub struct RunResult {
    pub state: VmState,
    pub stdout: String,
    /// The object heap at the end of the run. Needed to resolve the `Handle`s in
    /// `Value::Array`/`Value::Object` returned in `state` — e.g. to read array
    /// elements or coerce a returned array/object to a string. For a suspended
    /// run it is the heap as of the suspension point.
    pub heap: Heap,
    /// Execution trace covering parse → compile → execute.
    pub trace: ExecutionTrace,
}

impl RunResult {
    /// Build a `RunResult` after a snapshot resume, taking the heap and stdout
    /// from the resumed VM. The trace covers only the resume span (parse/compile
    /// already happened in the original run).
    pub(crate) fn from_resume(state: VmState, vm: Vm) -> Self {
        let mut root = SpanBuilder::new("zapcode.resume");
        root.add_child(SpanBuilder::new("resume").finish_ok());
        RunResult {
            state,
            stdout: vm.stdout,
            heap: vm.heap,
            trace: ExecutionTrace {
                root: root.finish_ok(),
            },
        }
    }
}

/// Quick helper to evaluate a TypeScript expression.
pub fn eval_ts(source: &str) -> Result<Value> {
    let runner = ZapcodeRun::new(
        source.to_string(),
        Vec::new(),
        Vec::new(),
        ResourceLimits::default(),
    )?;
    runner.run_simple()
}

/// Evaluate TypeScript and return both the value and stdout output.
pub fn eval_ts_with_output(source: &str) -> Result<(Value, String)> {
    let runner = ZapcodeRun::new(
        source.to_string(),
        Vec::new(),
        Vec::new(),
        ResourceLimits::default(),
    )?;
    let result = runner.run(Vec::new())?;
    match result.state {
        VmState::Complete(v) => Ok((v, result.stdout)),
        VmState::Suspended { function_name, .. } => Err(ZapcodeError::RuntimeError(format!(
            "execution suspended on external function '{}'",
            function_name
        ))),
        VmState::SuspendedMany { .. } => Err(ZapcodeError::RuntimeError(
            "execution suspended on a Promise.all batch".to_string(),
        )),
    }
}
