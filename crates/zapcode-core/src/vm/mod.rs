use std::collections::{BTreeMap, HashMap, HashSet};
use std::mem::size_of;
use std::sync::Arc;

use indexmap::IndexMap;

use crate::compiler::instruction::{BatchKind, Constant, Instruction};
use crate::jsstring::JsString;
use crate::compiler::CompiledProgram;
use crate::error::{Result, ZapcodeError};
use crate::heap::{Handle, Heap};
use crate::sandbox::{ResourceLimits, ResourceTracker};
use crate::snapshot::ZapcodeSnapshot;
use crate::trace::{ExecutionTrace, SpanBuilder, TraceStatus};
use crate::value::{
    Closure, FunctionRef, GeneratorObject, Place, PlaceRoot, PlaceSeg, SuspendedFrame, Value,
    MAX_RENDER_DEPTH,
};

mod builtins;

/// Re-exported so the napi binding can apply the SAME exact reserved-marker
/// filter when marshalling guest objects across the host boundary.
pub use builtins::is_internal_marker_key;
/// Shared ECMA-262 `Number::toString` formatter, used both by VM string
/// coercion (`value.rs`) and the builtin number methods so every path agrees.
pub(crate) use builtins::format_number;

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
    /// True for a synthesized class field-initializer frame. Such a frame has a
    /// bound `this` (so `this.x` resolves) but must NOT be treated as a
    /// constructor on return: an implicit/explicit `undefined` return is the
    /// field's value, not the instance. These frames run synchronously inside
    /// `Construct`, so they are never serialized mid-flight.
    #[serde(default)]
    pub(crate) is_field_init: bool,
    /// Local slots that have been promoted to shared upvalue cells (captured by
    /// a nested closure): slot -> cell id. Reads/writes of these slots route
    /// through the cell arena so the closure and this frame stay in sync.
    /// `BTreeMap` (not `HashMap`) so a decoded frame re-serializes to the
    /// same bytes — snapshot determinism (content-addressing) requires it.
    #[serde(default)]
    pub(crate) boxed: BTreeMap<usize, u64>,
    /// Free-variable bindings for a closure frame: name -> cell id. A name found
    /// here shadows the global of the same name (LoadGlobal/StoreGlobal consult
    /// it first), connecting captured names to their shared cells.
    /// `BTreeMap` for the same determinism reason as `boxed`.
    #[serde(default)]
    pub(crate) env: BTreeMap<String, u64>,
    /// Set on an async function body frame the first time it detaches at an
    /// `await` (microtask-design Stage 3): the pending result promise the
    /// caller received at detach time. A frame with this set has NO caller
    /// expecting a pushed return value — completion *settles* this promise
    /// (return → resolve with adoption, escaped throw → reject) instead.
    #[serde(default)]
    pub(crate) async_result: Option<Value>,
    /// True for a frame running a class constructor (or a forwarded `super`
    /// constructor): an implicit/`undefined` return yields the instance.
    /// Ordinary methods return `undefined` like JS — substituting `this` for
    /// every method that returned nothing (the old behavior) also clobbered
    /// the receiver's source binding on the way out (`new T()` inside a
    /// static method overwrote the global class `T` with the instance).
    #[serde(default)]
    pub(crate) is_constructor: bool,
}

/// A parked async function call (microtask-design Stage 3): its body frame,
/// detached at an `await`, plus the expression stack above the frame's base
/// and any try-frames covering the body (depths stored *relative* so they
/// rebase on resume). Restored by a `ResumeAsync` microtask (`Microtask.task`)
/// when the awaited promise settles; the result promise the caller holds
/// lives in `frame.async_result`. Serialized with the snapshot, so a parked
/// task survives a host-call suspension like every other piece of async state.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct AsyncTask {
    pub(crate) frame: CallFrame,
    /// The frame's expression stack slice (above its old `stack_base`).
    pub(crate) stack: Vec<Value>,
    /// Try frames covering the body. `frame_depth` is stored relative to the
    /// frames below the task frame; `stack_depth` relative to its stack base.
    pub(crate) try_entries: Vec<TryInfo>,
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
    /// Running a drained [`Microtask`]'s handler — a `.then()`/`.catch()`/
    /// `.finally()` callback, which may itself make an external (tool) call
    /// and therefore must be able to suspend. The main `execute()` loop
    /// drives the callback the same way it drives array callbacks. The
    /// chain's dependent promise already exists (created when the method
    /// enqueued), so when the callback's frame pops, its return value
    /// *settles* `result_promise` (with thenable adoption) and nothing is
    /// pushed. A `throw` escaping the callback rejects `result_promise`
    /// instead of propagating to the code that happened to be running when
    /// the drain started (microtasks are their own turn — see the `Err` arm
    /// of `execute()`).
    MicrotaskReaction {
        mode: PromiseCallbackMode,
        /// The dependent promise to settle with the callback's outcome.
        result_promise: Value,
        /// The receiver's settled outcome (`PassThrough`/`finally` re-settles
        /// `result_promise` with this, discarding the callback's return value).
        original_value: Value,
        original_is_rejection: bool,
        caller_frame_depth: usize,
        callback_frame_index: usize,
    },
    /// Driving one `gen.next(arg)` pull in the MAIN loop (generator-mainloop
    /// Stage 0): the generator's body frame runs like any call, so a tool
    /// call inside it suspends the whole VM durably. `Yield` detaches the
    /// frame back into the generator object and answers the pull inline
    /// (popping this continuation); a `return`/fall-off pops the frame
    /// normally and this continuation shapes `{value, done: true}`. The
    /// in-flight generator object rides here (it was removed from the
    /// registry for the pull, exactly like the legacy nested driver).
    GeneratorNext {
        gen_obj: GeneratorObject,
        caller_frame_depth: usize,
        callback_frame_index: usize,
        /// How the pull's answer is shaped: `false` → an iterator-result
        /// object (the `.next()` method); `true` → the `IteratorNext`
        /// protocol pair `[ ["__gen__", id, done], value ]` (for…of /
        /// for-await / `yield*` loops).
        for_of: bool,
    },
    /// Running a `new Promise(executor)` executor synchronously. When its
    /// frame pops, the executor's return value is DISCARDED and the new
    /// promise is pushed as the `new` expression's value. A throw escaping
    /// the executor rejects the promise (a no-op if a capability already
    /// settled it) instead of propagating — the spec'd constructor catch.
    PromiseExecutor {
        /// The promise under construction (settled by the resolve/reject
        /// capability functions handed to the executor).
        promise: Value,
        caller_frame_depth: usize,
        callback_frame_index: usize,
    },
}

/// A queued microtask (a Promise reaction). When drained, run `handler(value)`
/// — or, with no handler, pass `value` straight through — and settle
/// `result_promise` with the outcome (which in turn enqueues *its* reactions).
/// Draining the queue only after the synchronous run completes is what gives
/// `.then`/`await` their JS ordering. Serialized so a suspension mid-drain (a
/// host call inside a `.then`) resumes with the queue intact.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Microtask {
    /// The `.then`/`.catch`/`.finally` handler, or `Undefined` for a pass-through
    /// reaction (e.g. `.then(undefined)` on a settled promise).
    pub(crate) handler: Value,
    /// The settled value (fulfilment value or rejection reason) passed to the handler.
    pub(crate) value: Value,
    /// True if `value` is a rejection reason (the reaction is on the reject path).
    pub(crate) is_rejection: bool,
    pub(crate) mode: PromiseCallbackMode,
    /// The dependent promise to settle with the handler's outcome.
    pub(crate) result_promise: Value,
    /// `Some(id)` makes this a **ResumeAsync** job instead of a reaction: the
    /// drain restores parked [`AsyncTask`] `id` and delivers `value` at its
    /// `await` site (or rethrows it when `is_rejection`). `handler` and
    /// `result_promise` are unused (`Undefined`).
    #[serde(default)]
    pub(crate) task: Option<u64>,
}

/// A pending `setTimeout` callback. The VM has no clock — `delay` is a
/// deterministic ORDERING key (smaller fires first, ties broken by creation
/// order), which preserves real-JS relative timer ordering without wall
/// time. Timers fire as macrotasks: at the top-level drain, after the
/// microtask queue empties (and the per-tick unhandled-rejection check
/// passes), the earliest timer's callback runs as a job. Serialized so a
/// suspension with timers in flight resumes them.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct TimerEntry {
    pub(crate) id: u64,
    pub(crate) delay: f64,
    pub(crate) seq: u64,
    pub(crate) callback: Value,
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
    /// Completed synchronously; the value should be pushed. With the microtask
    /// model this is the only success shape: the handler never runs inline —
    /// the value is the receiver (pass-through) or a new pending promise that
    /// the enqueued/registered reaction settles during the drain.
    Value(Value),
    /// The method was called on a deferred single-call promise (N5), forcing its
    /// host call: the VM must suspend on it. The dispatch caller returns this
    /// `VmState` up to the host; on resume, `resume_action` finishes the method.
    Suspend(VmState),
    /// The receiver was not a promise, or the method is unknown.
    NotAPromise,
}

/// Decoded operands of a `CreateClass` instruction, passed to `Vm::create_class`.
struct CreateClassParts<'a> {
    name: &'a str,
    n_methods: usize,
    n_statics: usize,
    n_getters: usize,
    n_setters: usize,
    n_static_getters: usize,
    n_static_setters: usize,
    n_fields: usize,
    n_static_fields: usize,
    has_super: bool,
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
    /// One-shot flag: when set, the very next frame pushed by `push_call_frame`
    /// is a class field-initializer (its `undefined` return is the field value,
    /// not a constructor `this`). Consumed immediately on push. Transient —
    /// never serialized and never live across a suspension.
    pub(crate) next_frame_is_field_init: bool,
    /// One-shot: tag the next pushed frame as a constructor frame (set by
    /// `Construct`/`CallSuper` before `push_call_frame`).
    pub(crate) next_frame_is_constructor: bool,
    /// What to do with the value the host pushes when resuming a suspension that
    /// was triggered by *consuming a deferred single-call promise* (N5). For a
    /// plain `await p` this is `None` (the pushed value is exactly the await
    /// result). For `p.then(cb)`/`.catch`/`.finally` it carries the promise
    /// method + callbacks so the resumed value is wrapped in a settled promise
    /// and the callback chain runs. Serialized so it survives dump/load/resume.
    pub(crate) resume_action: Option<ResumeAction>,
    /// The microtask (Promise-reaction) queue. `.then`/`.catch`/`.finally`
    /// enqueue here instead of running inline; the queue is drained after the
    /// synchronous run completes (and after each host-call resume), giving JS
    /// Promise ordering. Serialized so a suspension mid-drain resumes cleanly.
    pub(crate) microtasks: std::collections::VecDeque<Microtask>,
    /// Promises that settled rejected with nobody to handle the rejection
    /// (no reject reaction at settle time). Cleared when a handler attaches
    /// (`.catch` on a rejected promise) or the rejection is consumed (`await`,
    /// host boundary). Anything still here at end-of-drain surfaces as an
    /// "Unhandled promise rejection" error — JS's unhandled-rejection event,
    /// made deterministic. Serialized: a suspension mid-drain must not lose it.
    pub(crate) unhandled_rejections: Vec<Handle>,
    /// Async function calls parked at an `await` (Stage 3), keyed by task id.
    /// `BTreeMap` so the serialized snapshot is deterministic. Resumed by
    /// `ResumeAsync` microtasks; tasks still parked at end-of-drain simply
    /// never finish (their awaited promise can no longer settle), matching a
    /// Node process exiting with pending awaits.
    pub(crate) async_tasks: BTreeMap<u64, AsyncTask>,
    /// Id source for [`AsyncTask`]s (like `next_call_id`).
    pub(crate) next_async_task_id: u64,
    /// Try-frames covering a suspended generator body, keyed by generator id
    /// and stashed at `yield` (depths stored relative; rebased on resume).
    /// Kept out of `SuspendedFrame` so `value.rs` need not know `TryInfo`.
    /// Without this, a `yield` inside `try` leaked the try-frame onto
    /// `try_stack` pointing at a popped frame (latent, pre-main-loop bug).
    pub(crate) generator_try_frames: BTreeMap<u64, Vec<TryInfo>>,
    /// When non-zero, `heap[0..builtin_base)` is this build's builtin template
    /// (see [`builtin_template`]) and `globals` reuses its handles. Lets
    /// snapshots elide the template prefix and restores skip re-registration.
    /// Zero for heaps seeded with caller data (builtins appended on top).
    pub(crate) builtin_base: usize,
    /// Pending `setTimeout` callbacks (see [`TimerEntry`]).
    pub(crate) timers: Vec<TimerEntry>,
    pub(crate) next_timer_id: u64,
}

/// The builtin globals and the heap slots backing them, built once. Every
/// fresh-heap VM starts as a clone of this template, so its layout (slot
/// order and handles) is identical across VMs and across processes of the
/// same build — which is what lets snapshots elide the prefix.
pub(crate) fn builtin_template() -> &'static (HashMap<String, Value>, Heap) {
    static TEMPLATE: std::sync::OnceLock<(HashMap<String, Value>, Heap)> =
        std::sync::OnceLock::new();
    TEMPLATE.get_or_init(|| {
        let mut globals = HashMap::new();
        let mut heap = Heap::new();
        builtins::register_globals(&mut globals, &mut heap);
        (globals, heap)
    })
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
    /// A plain `await p` on a deferred single-call promise forced its call. The
    /// resumed value is the await result (pushed by the host) — but we also cache
    /// it under the call id so a *second* `await p` / `p.then(...)` on the same
    /// promise reuses the settled value instead of re-invoking the host (matching
    /// JS, where a promise settles once).
    CacheValue { id: u64 },
    /// A *microtask* callback returned a deferred single-call promise (thenable
    /// adoption mid-drain): the drain forced that call. On resume, settle
    /// `result_promise` with the host outcome — or, when `pass_original` is set
    /// (`finally`), with the receiver's original outcome `(value, is_rejection)`
    /// once the forced call completes. The drain then continues from the
    /// top-level overflow branch.
    SettleResult {
        result_promise: Value,
        pass_original: Option<(Value, bool)>,
    },
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
        // Registering the builtin globals dominates first-execution latency
        // (~50 heap objects + marker allocations), so the fresh-heap path
        // clones a once-built template instead of rebuilding. The clone is
        // layout-identical to a direct registration (same handles in the same
        // order), so snapshot determinism is unaffected. A non-empty seeded
        // heap (session chunks, compound inputs) must keep registering on
        // top so the caller's existing handles stay valid.
        let (globals, heap, builtin_base) = if heap.is_empty() {
            let (g, h) = builtin_template();
            (g.clone(), h.clone(), h.len())
        } else {
            let mut globals = HashMap::new();
            let mut heap = heap;
            builtins::register_globals(&mut globals, &mut heap);
            (globals, heap, 0)
        };

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
            next_frame_is_field_init: false,
            next_frame_is_constructor: false,
            resume_action: None,
            microtasks: std::collections::VecDeque::new(),
            unhandled_rejections: Vec::new(),
            async_tasks: BTreeMap::new(),
            next_async_task_id: 0,
            generator_try_frames: BTreeMap::new(),
            builtin_base,
            timers: Vec::new(),
            next_timer_id: 0,
        }
    }

    /// Names of all builtin globals registered by `register_globals`.
    pub(crate) const BUILTIN_GLOBAL_NAMES: &'static [&'static str] = &[
        "console",
        "JSON",
        "Object",
        "Array",
        "Function",
        "Math",
        "Promise",
        "Map",
        "RegExp",
        "Date",
        "String",
        "Number",
        "Boolean",
        "parseInt",
        "parseFloat",
        "isNaN",
        "isFinite",
        "encodeURIComponent",
        "decodeURIComponent",
        "encodeURI",
        "decodeURI",
        "btoa",
        "atob",
        "setTimeout",
        "clearTimeout",
        "clearInterval",
        "queueMicrotask",
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
        builtin_base: usize,
    ) -> Self {
        // A heap that still starts with this build's builtin template reuses
        // its handles: the template globals point straight into the restored
        // prefix, so nothing is appended. This keeps snapshot size constant
        // across suspend/resume hops (re-registration used to append ~40
        // duplicate builtin objects per resume) and preserves any guest
        // mutation of a builtin object across the hop, matching the
        // in-memory run. Heaps without the prefix (seeded with caller data)
        // keep the append path so their handles stay valid.
        let (template_globals, template_heap) = builtin_template();
        let (mut globals, heap, restored_base) =
            if builtin_base == template_heap.len() && heap.len() >= builtin_base {
                (template_globals.clone(), heap, builtin_base)
            } else {
                let mut globals = HashMap::new();
                let mut heap = heap;
                builtins::register_globals(&mut globals, &mut heap);
                (globals, heap, 0)
            };
        // Overlay user globals (user globals take precedence if names collide)
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
            next_frame_is_field_init: false,
            next_frame_is_constructor: false,
            resume_action: None,
            microtasks: std::collections::VecDeque::new(),
            unhandled_rejections: Vec::new(),
            async_tasks: BTreeMap::new(),
            next_async_task_id: 0,
            generator_try_frames: BTreeMap::new(),
            builtin_base: restored_base,
            timers: Vec::new(),
            next_timer_id: 0,
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
        self.execute_to_host()
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
            // A microtask handler returned a deferred single-call promise; its
            // forced host call has now settled into `settled`. Settle the
            // chain's dependent promise: the host outcome for `.then`/`.catch`
            // adoption, or the receiver's original outcome for `.finally`
            // (which awaited the cleanup promise but discards its value). The
            // caller's `execute()` then continues the drain. Nothing is pushed
            // — the chain expression already received the dependent promise.
            ResumeAction::SettleResult {
                result_promise,
                pass_original,
            } => {
                let (outcome, is_rejection) = if let Some(original) = pass_original {
                    original
                } else if let Value::Object(h) = &settled {
                    let map = self.heap.object_map(*h);
                    match map.get("status") {
                        Some(Value::String(s)) if s.as_ref() == "rejected" => (
                            map.get("reason").cloned().unwrap_or(Value::Undefined),
                            true,
                        ),
                        _ => (
                            map.get("value").cloned().unwrap_or(Value::Undefined),
                            false,
                        ),
                    }
                } else {
                    (settled, false)
                };
                self.settle_promise(&result_promise, outcome, is_rejection)?;
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
                    None => self.execute_to_host(),
                };
            }
        }
        // Route the rejection to the nearest catch or finally (running finallys
        // on the way), exactly as the execute loop does for a runtime error.
        if self.route_thrown(error.clone(), 0)? {
            self.execute_to_host()
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

    /// True if `frame` runs an `async` function body, so its return value is
    /// shaped into a resolved Promise on the way out. Async *generators* are
    /// excluded: their returns flow through the iterator protocol, not the
    /// async-call return path. (A `throw` escaping an async body still
    /// propagates synchronously — rejected-promise shaping arrives with await
    /// suspension, microtask-design Stage 3.)
    fn frame_is_async(&self, frame: &CallFrame) -> bool {
        frame.func_index.is_some_and(|fi| {
            self.program(frame.program_index)
                .functions
                .get(fi)
                .is_some_and(|f| f.is_async && !f.is_generator)
        })
    }

    /// Register a `.then`/`.catch`/`.finally` reaction on a *pending* internal
    /// promise. The record lives on the promise object (so it serializes with
    /// the heap); `settle_promise` turns each record into a microtask. For
    /// `then(f, r)` pass both handlers; for `catch(h)` only `on_rejected`; for
    /// `finally(h)` pass `h` as both with `PassThrough` mode.
    fn register_reaction(
        &mut self,
        receiver: Handle,
        on_fulfilled: Value,
        on_rejected: Value,
        mode: PromiseCallbackMode,
        result: Value,
    ) -> Result<()> {
        let mut record = IndexMap::new();
        record.insert(Arc::from("on_fulfilled"), on_fulfilled);
        record.insert(Arc::from("on_rejected"), on_rejected);
        record.insert(
            Arc::from("mode"),
            Value::String(JsString::from(match mode {
                PromiseCallbackMode::WrapResult => "wrap",
                PromiseCallbackMode::PassThrough => "pass",
            })),
        );
        record.insert(Arc::from("result"), result);
        let rec = Value::Object(self.heap.alloc_object(record));
        let reactions = match self.heap.object(receiver).and_then(|m| m.get("__reactions__")) {
            Some(Value::Array(a)) => *a,
            _ => {
                return Err(ZapcodeError::RuntimeError(
                    "internal error: pending promise has no reaction list".to_string(),
                ))
            }
        };
        self.heap
            .array_mut(reactions)
            .ok_or_else(|| {
                ZapcodeError::RuntimeError(
                    "internal error: promise reaction list is not an array".to_string(),
                )
            })?
            .push(rec);
        Ok(())
    }

    /// Settle a pending internal promise: flip its status, record the outcome,
    /// and enqueue every registered reaction as a microtask (FIFO — this is
    /// what gives `.then` chains their tick order). Settling an
    /// already-settled promise is a no-op (spec double-settle). A rejection
    /// with no reactions is marked unhandled; the mark clears when a reject
    /// handler attaches or the rejection is consumed, and anything still
    /// marked at end-of-drain surfaces as an error.
    ///
    /// `outcome` must be a plain (non-promise) value — adoption of a returned
    /// promise is the *caller's* job (see the `MicrotaskReaction` arm of
    /// `process_continuation`).
    fn settle_promise(
        &mut self,
        promise: &Value,
        outcome: Value,
        is_rejection: bool,
    ) -> Result<()> {
        let Value::Object(h) = promise else {
            return Err(ZapcodeError::RuntimeError(
                "internal error: settle_promise on a non-object".to_string(),
            ));
        };
        let reactions = {
            let map = self.heap.object_mut(*h).ok_or_else(|| {
                ZapcodeError::RuntimeError(
                    "internal error: settle_promise on a dangling handle".to_string(),
                )
            })?;
            if !matches!(map.get("status"), Some(Value::String(s)) if s.as_ref() == "pending") {
                return Ok(()); // already settled — no-op
            }
            let reactions = match map.shift_remove("__reactions__") {
                Some(Value::Array(a)) => a,
                _ => {
                    return Err(ZapcodeError::RuntimeError(
                        "internal error: pending promise has no reaction list".to_string(),
                    ))
                }
            };
            if is_rejection {
                map.insert(
                    Arc::from("status"),
                    Value::String(JsString::from("rejected")),
                );
                map.insert(Arc::from("reason"), outcome.clone());
            } else {
                map.insert(
                    Arc::from("status"),
                    Value::String(JsString::from("resolved")),
                );
                map.insert(Arc::from("value"), outcome.clone());
            }
            reactions
        };
        let records = self.heap.array_vec(reactions);
        if is_rejection && records.is_empty() {
            self.unhandled_rejections.push(*h);
        }
        for rec in records {
            let Value::Object(rh) = rec else { continue };
            let map = self.heap.object_map(rh);
            // A "sink" record consumes the settlement silently (a decided
            // race/any absorbed this loser) — no job, no unhandled mark.
            if matches!(map.get("mode"), Some(Value::String(s)) if s.as_ref() == "sink") {
                continue;
            }
            // A "combine" record feeds this settlement into a lowered
            // combinator (`lower_internal_batch`) — the job carries the
            // record itself as its handler; the drain applies it.
            if matches!(map.get("mode"), Some(Value::String(s)) if s.as_ref() == "combine") {
                self.microtasks.push_back(Microtask {
                    handler: Value::Object(rh),
                    value: outcome.clone(),
                    is_rejection,
                    mode: PromiseCallbackMode::WrapResult,
                    result_promise: Value::Undefined,
                    task: None,
                });
                continue;
            }
            // A "task" record parks no handler — it resumes the AsyncTask
            // awaiting this promise (microtask-design Stage 3, ResumeAsync).
            if matches!(map.get("mode"), Some(Value::String(s)) if s.as_ref() == "task") {
                let task = match map.get("task") {
                    Some(Value::Int(id)) => Some(*id as u64),
                    _ => None,
                };
                self.microtasks.push_back(Microtask {
                    handler: Value::Undefined,
                    value: outcome.clone(),
                    is_rejection,
                    mode: PromiseCallbackMode::WrapResult,
                    result_promise: Value::Undefined,
                    task,
                });
                continue;
            }
            let handler = if is_rejection {
                map.get("on_rejected").cloned().unwrap_or(Value::Undefined)
            } else {
                map.get("on_fulfilled").cloned().unwrap_or(Value::Undefined)
            };
            let mode = match map.get("mode") {
                Some(Value::String(s)) if s.as_ref() == "pass" => PromiseCallbackMode::PassThrough,
                _ => PromiseCallbackMode::WrapResult,
            };
            let result_promise = map.get("result").cloned().unwrap_or(Value::Undefined);
            self.microtasks.push_back(Microtask {
                handler,
                value: outcome.clone(),
                is_rejection,
                mode,
                result_promise,
                task: None,
            });
        }
        Ok(())
    }

    /// The settled outcome of a `race`/`any` batch element, if it has one:
    /// a plain value counts as fulfilled; a settled internal promise yields
    /// its value/reason; a resolved-cache hit for a deferred host call counts
    /// as fulfilled. In-flight elements (pending chains, untriggered host
    /// calls, nested batches) return `None`.
    fn batch_element_outcome(&self, v: &Value) -> Option<(Value, bool)> {
        match v {
            Value::Pending(id) => self.resolved.get(id).cloned().map(|c| (c, false)),
            Value::Object(h) => {
                let map = self.heap.object(*h)?;
                if !matches!(map.get("__promise__"), Some(Value::Bool(true))) {
                    return Some((v.clone(), false));
                }
                match map.get("status") {
                    Some(Value::String(s)) if s.as_ref() == "resolved" => Some((
                        map.get("value").cloned().unwrap_or(Value::Undefined),
                        false,
                    )),
                    Some(Value::String(s)) if s.as_ref() == "rejected" => Some((
                        map.get("reason").cloned().unwrap_or(Value::Undefined),
                        true,
                    )),
                    _ => None,
                }
            }
            other => Some((other.clone(), false)),
        }
    }

    /// Register a *sink* reaction on a pending internal promise: its eventual
    /// settlement is consumed silently (no microtask, no unhandled-rejection
    /// mark). Used for the losing elements of a decided `race`/`any` — the
    /// combinator absorbed responsibility for them.
    fn register_sink_reaction(&mut self, receiver: Handle) -> Result<()> {
        let mut record = IndexMap::new();
        record.insert(Arc::from("mode"), Value::String(JsString::from("sink")));
        let rec = Value::Object(self.heap.alloc_object(record));
        let reactions = match self.heap.object(receiver).and_then(|m| m.get("__reactions__")) {
            Some(Value::Array(a)) => *a,
            _ => {
                return Err(ZapcodeError::RuntimeError(
                    "internal error: pending promise has no reaction list".to_string(),
                ))
            }
        };
        self.heap
            .array_mut(reactions)
            .ok_or_else(|| {
                ZapcodeError::RuntimeError(
                    "internal error: promise reaction list is not an array".to_string(),
                )
            })?
            .push(rec);
        Ok(())
    }

    /// Register a *ResumeAsync* reaction on a pending internal promise: when
    /// it settles, parked [`AsyncTask`] `task_id` resumes with the outcome.
    fn register_task_reaction(&mut self, receiver: Handle, task_id: u64) -> Result<()> {
        let mut record = IndexMap::new();
        record.insert(Arc::from("mode"), Value::String(JsString::from("task")));
        record.insert(Arc::from("task"), Value::Int(task_id as i64));
        let rec = Value::Object(self.heap.alloc_object(record));
        let reactions = match self.heap.object(receiver).and_then(|m| m.get("__reactions__")) {
            Some(Value::Array(a)) => *a,
            _ => {
                return Err(ZapcodeError::RuntimeError(
                    "internal error: pending promise has no reaction list".to_string(),
                ))
            }
        };
        self.heap
            .array_mut(reactions)
            .ok_or_else(|| {
                ZapcodeError::RuntimeError(
                    "internal error: promise reaction list is not an array".to_string(),
                )
            })?
            .push(rec);
        Ok(())
    }

    /// Register a *combine* reaction on a pending internal promise: when it
    /// settles, the outcome feeds element `index` of the lowered combinator
    /// `batch` (see [`Self::lower_internal_batch`]).
    fn register_combine_reaction(&mut self, receiver: Handle, batch: Value, index: usize) -> Result<()> {
        self.tracker.track_allocation(&self.limits)?;
        let mut record = IndexMap::new();
        record.insert(Arc::from("mode"), Value::String(JsString::from("combine")));
        record.insert(Arc::from("batch"), batch);
        record.insert(Arc::from("index"), Value::Int(index as i64));
        let rec = Value::Object(self.heap.alloc_object(record));
        let reactions = match self.heap.object(receiver).and_then(|m| m.get("__reactions__")) {
            Some(Value::Array(a)) => *a,
            _ => {
                return Err(ZapcodeError::RuntimeError(
                    "internal error: pending promise has no reaction list".to_string(),
                ))
            }
        };
        self.heap
            .array_mut(reactions)
            .ok_or_else(|| {
                ZapcodeError::RuntimeError(
                    "internal error: promise reaction list is not an array".to_string(),
                )
            })?
            .push(rec);
        Ok(())
    }

    /// True for a `pending_all` batch every element of which is *internal* —
    /// already settled, a plain value, or a microtask-pending chain — with at
    /// least one of the latter. Such a batch holds no host call to force, so
    /// `.then`/`.catch`/`.finally` on it must lower it to a real pending
    /// promise ([`Self::lower_internal_batch`]) instead of suspending.
    fn batch_all_internal(&self, v: &Value) -> bool {
        let Value::Object(h) = v else { return false };
        let Some(map) = self.heap.object(*h) else {
            return false;
        };
        if !matches!(map.get("status"), Some(Value::String(s)) if s.as_ref() == "pending_all") {
            return false;
        }
        let items = match map.get("items") {
            Some(Value::Array(a)) => self.heap.array_vec(*a),
            _ => return false,
        };
        items.iter().any(|i| self.is_internal_pending(i))
            && items
                .iter()
                .all(|i| self.is_internal_pending(i) || self.batch_element_outcome(i).is_some())
    }

    /// Lower a combinator over internal elements only into a REAL pending
    /// promise: settled elements record their outcome now, each pending
    /// element gets a "combine" reaction, and per-kind progress lives on the
    /// promise object (`__combine_kind__`/`__combine_values__`/
    /// `__combine_remaining__` — plain heap data, so it snapshots for free).
    /// This is what makes `Promise.all([chain]).then(cb)` work even with an
    /// empty microtask queue: there is no host call to force and no queued
    /// job to borrow a tick from, so the batch must settle from the drain
    /// like any other promise.
    fn lower_internal_batch(&mut self, promise: &Value) -> Result<()> {
        let Value::Object(h) = promise else {
            return Ok(());
        };
        let map = self.heap.object_map(*h);
        let items = match map.get("items") {
            Some(Value::Array(a)) => self.heap.array_vec(*a),
            _ => Vec::new(),
        };
        let kind = match map.get("__batch_kind__") {
            Some(Value::String(k)) => k.to_string(),
            _ => "all".to_string(),
        };
        let n = items.len();
        self.track_array_capacity(n)?;
        self.tracker.track_allocation(&self.limits)?;
        let values_h = self.heap.alloc_array(vec![Value::Undefined; n]);
        self.tracker.track_allocation(&self.limits)?;
        let reactions_h = self.heap.alloc_array(Vec::new());
        {
            let m = self.heap.object_mut(*h).ok_or_else(|| {
                ZapcodeError::RuntimeError(
                    "internal error: lower_internal_batch on a dangling handle".to_string(),
                )
            })?;
            m.insert(
                Arc::from("status"),
                Value::String(JsString::from("pending")),
            );
            m.insert(Arc::from("__reactions__"), Value::Array(reactions_h));
            m.insert(
                Arc::from("__combine_kind__"),
                Value::String(JsString::from(kind.as_str())),
            );
            m.insert(Arc::from("__combine_values__"), Value::Array(values_h));
            m.insert(Arc::from("__combine_remaining__"), Value::Int(n as i64));
            m.shift_remove("items");
            m.shift_remove("__batch_kind__");
        }
        for (i, item) in items.iter().enumerate() {
            if self.is_internal_pending(item) {
                if let Value::Object(eh) = item {
                    self.register_combine_reaction(*eh, promise.clone(), i)?;
                }
            } else if let Some((v, rej)) = self.batch_element_outcome(item) {
                // Already settled: fold it in now. This can decide the batch
                // early (a rejected `all` element, a `race`/`any` winner);
                // later combine reactions then absorb silently.
                self.combine_apply(promise, i, v, rej)?;
            }
        }
        Ok(())
    }

    /// Run one drained combine job: the reaction `record` carries the lowered
    /// batch and the element's index; apply the element's outcome to it.
    fn combine_step(&mut self, record: Handle, value: Value, is_rejection: bool) -> Result<()> {
        let rec = self.heap.object_map(record);
        let batch = rec.get("batch").cloned().unwrap_or(Value::Undefined);
        let index = match rec.get("index") {
            Some(Value::Int(i)) => *i as usize,
            _ => 0,
        };
        self.combine_apply(&batch, index, value, is_rejection)
    }

    /// Feed one element outcome into a lowered combinator promise. A batch
    /// that already settled (decided `race`/`any`, rejected `all`) absorbs
    /// the outcome silently, exactly like a sink reaction.
    fn combine_apply(
        &mut self,
        batch: &Value,
        index: usize,
        value: Value,
        is_rejection: bool,
    ) -> Result<()> {
        let Value::Object(bh) = batch else {
            return Ok(());
        };
        let map = self.heap.object_map(*bh);
        if !matches!(map.get("status"), Some(Value::String(s)) if s.as_ref() == "pending") {
            return Ok(()); // already decided — absorb
        }
        let kind = match map.get("__combine_kind__") {
            Some(Value::String(s)) => s.to_string(),
            _ => return Ok(()),
        };
        let values_h = match map.get("__combine_values__") {
            Some(Value::Array(a)) => *a,
            _ => return Ok(()),
        };
        let remaining = match map.get("__combine_remaining__") {
            Some(Value::Int(n)) => *n,
            _ => 0,
        };
        let mut store = |vm: &mut Self, slot_value: Value| {
            if let Some(slot) = vm
                .heap
                .array_mut(values_h)
                .and_then(|a| a.get_mut(index))
            {
                *slot = slot_value;
            }
            if let Some(m) = vm.heap.object_mut(*bh) {
                m.insert(
                    Arc::from("__combine_remaining__"),
                    Value::Int(remaining - 1),
                );
            }
        };
        match kind.as_str() {
            "race" => self.settle_promise(batch, value, is_rejection),
            "any" => {
                if !is_rejection {
                    return self.settle_promise(batch, value, false);
                }
                store(self, value);
                if remaining - 1 == 0 {
                    let errors = self.heap.array_vec(values_h);
                    let agg = builtins::make_aggregate_error(errors, &mut self.heap);
                    return self.settle_promise(batch, agg, true);
                }
                Ok(())
            }
            "allSettled" => {
                self.tracker.track_allocation(&self.limits)?;
                let mut entry = IndexMap::new();
                if is_rejection {
                    entry.insert(
                        Arc::from("status"),
                        Value::String(JsString::from("rejected")),
                    );
                    entry.insert(Arc::from("reason"), value);
                } else {
                    entry.insert(
                        Arc::from("status"),
                        Value::String(JsString::from("fulfilled")),
                    );
                    entry.insert(Arc::from("value"), value);
                }
                let entry = Value::Object(self.heap.alloc_object(entry));
                store(self, entry);
                if remaining - 1 == 0 {
                    return self.settle_promise(batch, Value::Array(values_h), false);
                }
                Ok(())
            }
            _ /* all */ => {
                if is_rejection {
                    return self.settle_promise(batch, value, true);
                }
                store(self, value);
                if remaining - 1 == 0 {
                    return self.settle_promise(batch, Value::Array(values_h), false);
                }
                Ok(())
            }
        }
    }

    /// Dispatch the timer/microtask scheduling globals — they mutate VM
    /// state, so the pure `builtins::call_global_fn` cannot host them.
    /// Returns `None` for any other global (the caller falls through).
    fn call_scheduler_global(&mut self, kind: &str, args: &[Value]) -> Option<Result<Value>> {
        match kind {
            "setTimeout" => {
                let callback = args.first().cloned().unwrap_or(Value::Undefined);
                let delay = args.get(1).map(|v| v.to_number()).unwrap_or(0.0);
                let delay = if delay.is_finite() && delay > 0.0 { delay } else { 0.0 };
                let id = self.next_timer_id;
                self.next_timer_id += 1;
                self.timers.push(TimerEntry {
                    id,
                    delay,
                    seq: id,
                    callback,
                });
                Some(Ok(Value::Int(id as i64)))
            }
            "clearTimeout" | "clearInterval" => {
                if let Some(Value::Int(id)) = args.first() {
                    let id = *id as u64;
                    self.timers.retain(|t| t.id != id);
                }
                Some(Ok(Value::Undefined))
            }
            "queueMicrotask" => {
                let callback = args.first().cloned().unwrap_or(Value::Undefined);
                let result_promise = builtins::make_pending_promise(&mut self.heap);
                self.microtasks.push_back(Microtask {
                    handler: callback,
                    value: Value::Undefined,
                    is_rejection: false,
                    mode: PromiseCallbackMode::WrapResult,
                    result_promise,
                    task: None,
                });
                Some(Ok(Value::Undefined))
            }
            _ => None,
        }
    }

    /// Remove and return the next due timer: smallest delay, creation order
    /// breaking ties — the deterministic analogue of the timer wheel.
    fn pop_due_timer(&mut self) -> Option<TimerEntry> {
        let idx = self
            .timers
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                a.delay
                    .partial_cmp(&b.delay)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.seq.cmp(&b.seq))
            })
            .map(|(i, _)| i)?;
        Some(self.timers.remove(idx))
    }

    /// Run a fired timer's callback as a job (the macrotask body): a frame
    /// driven by the main loop via the microtask machinery, so a tool call
    /// inside the callback suspends normally.
    fn start_timer_job(&mut self, t: TimerEntry) -> Result<()> {
        let result_promise = builtins::make_pending_promise(&mut self.heap);
        self.start_microtask(Microtask {
            handler: t.callback,
            value: Value::Undefined,
            is_rejection: false,
            mode: PromiseCallbackMode::WrapResult,
            result_promise,
            task: None,
        })
    }

    /// Clear an unhandled-rejection mark: a reject handler attached to the
    /// promise, or its rejection was consumed (`await` rethrow, host boundary).
    fn mark_rejection_handled(&mut self, h: Handle) {
        self.unhandled_rejections.retain(|m| *m != h);
    }

    /// True for a *microtask-pending* internal promise (a `.then` chain link
    /// that settles during the drain) — as opposed to `pending_call`/
    /// `pending_all` host promises or the never-settling pinned promises
    /// (e.g. `Promise.race([])`), which carry no reaction list.
    fn is_internal_pending(&self, v: &Value) -> bool {
        if let Value::Object(h) = v {
            if let Some(m) = self.heap.object(*h) {
                return matches!(m.get("__promise__"), Some(Value::Bool(true)))
                    && matches!(m.get("status"), Some(Value::String(s)) if s.as_ref() == "pending")
                    && m.contains_key("__reactions__");
            }
        }
        false
    }

    /// True for a `pending_all` batch promise holding at least one
    /// microtask-pending chain element (which only the drain can settle).
    fn batch_has_pending_chains(&self, v: &Value) -> bool {
        let Value::Object(h) = v else { return false };
        let Some(map) = self.heap.object(*h) else {
            return false;
        };
        if !matches!(map.get("status"), Some(Value::String(s)) if s.as_ref() == "pending_all") {
            return false;
        }
        let items = match map.get("items") {
            Some(Value::Array(a)) => self.heap.array_vec(*a),
            _ => return false,
        };
        items.iter().any(|i| self.is_internal_pending(i))
    }

    /// Begin running a drained microtask. A callable handler gets a frame plus
    /// a [`Continuation::MicrotaskReaction`] — the main loop drives it, so a
    /// tool call inside the handler suspends normally. A non-callable handler
    /// is the pass-through reaction: settle `result_promise` with the
    /// receiver's outcome directly, no frames (the caller's loop just
    /// continues; any reactions this settle enqueued run on later passes).
    fn start_microtask(&mut self, m: Microtask) -> Result<()> {
        if let Some(task_id) = m.task {
            return self.resume_async_task(task_id, m.value, m.is_rejection);
        }
        // A combine job: the handler slot carries the reaction record itself
        // (guest handlers can never collide — non-callables are sanitized to
        // `Undefined` at registration).
        if let Value::Object(rh) = &m.handler {
            if matches!(
                self.heap.object(*rh).and_then(|r| r.get("mode")),
                Some(Value::String(s)) if s.as_ref() == "combine"
            ) {
                return self.combine_step(*rh, m.value, m.is_rejection);
            }
            // A capability handed straight to the scheduler —
            // `setTimeout(resolve, ms)` / `queueMicrotask(reject)` — is a
            // callable marker, not a Function: invoke it (settle its
            // promise with the job's value).
            let capability = {
                let map = self.heap.object_map(*rh);
                map.contains_key("__promise_capability__").then(|| {
                    (
                        map.get("__promise_capability__")
                            .cloned()
                            .unwrap_or(Value::Undefined),
                        matches!(map.get("__capability_reject__"), Some(Value::Bool(true))),
                    )
                })
            };
            if let Some((promise, is_reject)) = capability {
                self.settle_capability(&promise, m.value, is_reject)?;
                return Ok(());
            }
        }
        if let Value::Function(closure) = &m.handler {
            let closure = closure.clone();
            let args = match m.mode {
                PromiseCallbackMode::WrapResult => vec![m.value.clone()],
                PromiseCallbackMode::PassThrough => Vec::new(),
            };
            let caller_frame_depth = self.frames.len();
            self.push_call_frame(&closure, &args, None)?;
            self.continuations.push(Continuation::MicrotaskReaction {
                mode: m.mode,
                result_promise: m.result_promise,
                original_value: m.value,
                original_is_rejection: m.is_rejection,
                caller_frame_depth,
                callback_frame_index: self.frames.len() - 1,
            });
        } else {
            self.settle_promise(&m.result_promise, m.value, m.is_rejection)?;
        }
        Ok(())
    }

    /// True when the current frame is an `async` function body (not the
    /// top-level program) — a frame `await` may detach into an [`AsyncTask`].
    fn current_frame_is_async_body(&self) -> bool {
        self.frames.len() > 1 && self.frame_is_async(self.frames.last().unwrap())
    }

    /// Detach the current async-body frame into a parked [`AsyncTask`]
    /// (Stage 3). The caller resumes at the instruction after the call with
    /// the async call's pending result promise on the stack — exactly the
    /// shape of an early return, so call sites (including `.then(async …)`
    /// handler continuations) need no special handling. Try-frames covering
    /// the body travel with the task, depths stored relative for rebasing.
    /// Returns the new task id; the caller schedules its resumption (an
    /// immediate ResumeAsync tick, or a task reaction on the awaited promise).
    fn detach_async_task(&mut self) -> Result<u64> {
        let mut frame = self.frames.pop().unwrap();
        self.tracker.pop_frame();
        // First detach creates the result promise and hands it to the caller
        // (whose Call is waiting for a pushed value). A re-detach — a later
        // await in a body that was *resumed* by the drain — keeps the
        // original promise and pushes NOTHING: the frame sits on top of the
        // drain, not on top of a caller.
        let first_detach = frame.async_result.is_none();
        let result_promise = match frame.async_result.clone() {
            Some(p) => p,
            None => {
                let p = builtins::make_pending_promise(&mut self.heap);
                frame.async_result = Some(p.clone());
                p
            }
        };
        let below = self.frames.len();
        let stack = self.stack.split_off(frame.stack_base);
        let mut try_entries = Vec::new();
        while matches!(self.try_stack.last(), Some(t) if t.frame_depth > below) {
            let mut t = self.try_stack.pop().unwrap();
            t.frame_depth -= below; // relative: 1 == the task frame itself
            t.stack_depth = t.stack_depth.saturating_sub(frame.stack_base);
            try_entries.push(t);
        }
        try_entries.reverse();
        let id = self.next_async_task_id;
        self.next_async_task_id += 1;
        self.async_tasks.insert(
            id,
            AsyncTask {
                frame,
                stack,
                try_entries,
            },
        );
        if first_detach {
            self.push(result_promise)?;
        }
        Ok(id)
    }

    /// Restore parked [`AsyncTask`] `id` on top of the current frames and
    /// deliver the awaited outcome at its `await` site: push `value` as the
    /// await result, or — for a rejection — rethrow the *original* reason
    /// inside the body (its own try/catch may handle it; escaping rejects the
    /// task's result promise).
    fn resume_async_task(&mut self, id: u64, value: Value, is_rejection: bool) -> Result<()> {
        let task = self.async_tasks.remove(&id).ok_or_else(|| {
            ZapcodeError::RuntimeError(format!("internal error: unknown async task {id}"))
        })?;
        let below = self.frames.len();
        let stack_base = self.stack.len();
        let mut frame = task.frame;
        frame.stack_base = stack_base;
        self.tracker.push_frame();
        for v in task.stack {
            self.push(v)?;
        }
        for mut t in task.try_entries {
            t.frame_depth += below;
            t.stack_depth += stack_base;
            self.try_stack.push(t);
        }
        self.frames.push(frame);
        if is_rejection {
            if !self.route_thrown(value, below)? {
                let reason = self.pending_throw.take().unwrap_or(Value::Undefined);
                self.reject_detached_body(below, reason)?;
            }
        } else {
            self.push(value)?;
        }
        Ok(())
    }

    /// Park the current async body at this `await` and schedule an immediate
    /// ResumeAsync tick delivering `value` (or rethrowing it when
    /// `is_rejection`). `await` always yields at least one microtask turn
    /// (spec), even for non-promise and already-settled operands — this is
    /// what gives async interleaving its Node order.
    fn park_and_tick(&mut self, value: Value, is_rejection: bool) -> Result<()> {
        let id = self.detach_async_task()?;
        self.microtasks.push_back(Microtask {
            handler: Value::Undefined,
            value,
            is_rejection,
            mode: PromiseCallbackMode::WrapResult,
            result_promise: Value::Undefined,
            task: Some(id),
        });
        Ok(())
    }

    /// Top-level `await` tick: the operand is already settled (or a plain
    /// value) but reactions are queued. The top-level frame cannot detach, so
    /// deliver the outcome through a fresh pending promise whose settling
    /// microtask sits at the END of the current queue, and re-dispatch this
    /// Await against it — the drain-until-settled path then runs exactly the
    /// already-queued jobs first. This is Node's "the resumption is enqueued
    /// after the current queue" order: jobs that those reactions enqueue run
    /// AFTER the continuation. With an empty queue the caller delivers
    /// inline — there is nothing to interleave with, so no tick is
    /// observable.
    fn requeue_top_level_await(&mut self, value: Value, is_rejection: bool) -> Result<()> {
        let sentinel = builtins::make_pending_promise(&mut self.heap);
        // Mark the sentinel so the re-dispatched Await delivers its settled
        // outcome INLINE — exactly one tick per await. Without the marker the
        // re-await would requeue again while jobs remain, draining the whole
        // queue (jobs enqueued during the tick must run AFTER this
        // continuation, as in Node).
        if let Value::Object(h) = &sentinel {
            if let Some(map) = self.heap.object_mut(*h) {
                map.insert(Arc::from("__await_tick__"), Value::Bool(true));
            }
        }
        self.microtasks.push_back(Microtask {
            handler: Value::Undefined,
            value,
            is_rejection,
            mode: PromiseCallbackMode::WrapResult,
            result_promise: sentinel.clone(),
            task: None,
        });
        self.push(sentinel)?;
        self.current_frame_mut().ip -= 1;
        Ok(())
    }

    /// Invoke a Promise-executor capability (`resolve`/`reject` from
    /// `new Promise(executor)`). Settling an already-settled promise is a
    /// no-op (spec). `resolve` adopts thenables: a settled promise unwraps,
    /// a pending chain forwards via a handler-less reaction, and a deferred
    /// host call is forced (the VM suspends; `SettleResult` finishes on
    /// resume — the call's `undefined` result is pre-pushed so the resumed
    /// stack is balanced). `reject` never adopts (spec).
    fn settle_capability(
        &mut self,
        promise: &Value,
        value: Value,
        is_rejection: bool,
    ) -> Result<Option<VmState>> {
        if is_rejection {
            self.settle_promise(promise, value, true)?;
            return Ok(None);
        }
        if builtins::is_promise(&value, &self.heap) {
            if let Value::Object(h) = &value {
                let map = self.heap.object_map(*h);
                match map.get("status") {
                    Some(Value::String(s)) if s.as_ref() == "resolved" => {
                        let inner = map.get("value").cloned().unwrap_or(Value::Undefined);
                        self.settle_promise(promise, inner, false)?;
                    }
                    Some(Value::String(s)) if s.as_ref() == "rejected" => {
                        let reason = map.get("reason").cloned().unwrap_or(Value::Undefined);
                        self.mark_rejection_handled(*h);
                        self.settle_promise(promise, reason, true)?;
                    }
                    Some(Value::String(s)) if s.as_ref() == "pending" => {
                        self.register_reaction(
                            *h,
                            Value::Undefined,
                            Value::Undefined,
                            PromiseCallbackMode::WrapResult,
                            promise.clone(),
                        )?;
                    }
                    Some(Value::String(s)) if s.as_ref() == "pending_call" => {
                        let id = match map.get("__call_id__") {
                            Some(Value::Int(n)) => *n as u64,
                            _ => {
                                return Err(ZapcodeError::RuntimeError(
                                    "internal error: pending_call promise missing __call_id__"
                                        .to_string(),
                                ))
                            }
                        };
                        if let Some(cached) = self.resolved.get(&id).cloned() {
                            self.settle_promise(promise, cached, false)?;
                        } else {
                            // Balance the stack for the resume: the
                            // capability call itself evaluates to undefined.
                            self.push(Value::Undefined)?;
                            self.resume_action = Some(ResumeAction::SettleResult {
                                result_promise: promise.clone(),
                                pass_original: None,
                            });
                            return self.suspend_on_pending_call(id);
                        }
                    }
                    _ => {
                        // pending_all: settle with the batch promise itself —
                        // an `await` of the chain re-awaits it.
                        self.settle_promise(promise, value, false)?;
                    }
                }
                return Ok(None);
            }
        }
        self.settle_promise(promise, value, false)?;
        Ok(None)
    }

    /// Settle a detached async body's result promise with its return value,
    /// adopting thenables: a returned settled promise unwraps one level, a
    /// returned pending chain forwards via a handler-less reaction, and a
    /// returned deferred host call is forced (suspending the VM; the recorded
    /// `SettleResult` settles on resume — hence the `Option<VmState>`).
    fn settle_async_return(&mut self, promise: Value, value: Value) -> Result<Option<VmState>> {
        if builtins::is_promise(&value, &self.heap) {
            if let Value::Object(h) = &value {
                let map = self.heap.object_map(*h);
                match map.get("status") {
                    Some(Value::String(s)) if s.as_ref() == "resolved" => {
                        let inner = map.get("value").cloned().unwrap_or(Value::Undefined);
                        self.settle_promise(&promise, inner, false)?;
                    }
                    Some(Value::String(s)) if s.as_ref() == "rejected" => {
                        let reason = map.get("reason").cloned().unwrap_or(Value::Undefined);
                        self.mark_rejection_handled(*h);
                        self.settle_promise(&promise, reason, true)?;
                    }
                    Some(Value::String(s)) if s.as_ref() == "pending" => {
                        self.register_reaction(
                            *h,
                            Value::Undefined,
                            Value::Undefined,
                            PromiseCallbackMode::WrapResult,
                            promise.clone(),
                        )?;
                    }
                    Some(Value::String(s)) if s.as_ref() == "pending_call" => {
                        let id = match map.get("__call_id__") {
                            Some(Value::Int(n)) => *n as u64,
                            _ => {
                                return Err(ZapcodeError::RuntimeError(
                                    "internal error: pending_call promise missing __call_id__"
                                        .to_string(),
                                ))
                            }
                        };
                        if let Some(cached) = self.resolved.get(&id).cloned() {
                            self.settle_promise(&promise, cached, false)?;
                        } else {
                            self.resume_action = Some(ResumeAction::SettleResult {
                                result_promise: promise,
                                pass_original: None,
                            });
                            return self.suspend_on_pending_call(id);
                        }
                    }
                    _ => {
                        // pending_all (a batch promise): settle with the
                        // promise object itself — an `await` of the result
                        // re-awaits it (the resolved-with-a-promise arm).
                        self.settle_promise(&promise, value, false)?;
                    }
                }
                return Ok(None);
            }
        }
        self.settle_promise(&promise, value, false)?;
        Ok(None)
    }

    /// Unwind a detached async body whose base frame sits at index `base`
    /// (helper frames above included) and reject its result promise with
    /// `reason`. Used when a throw escapes the body or its awaited promise
    /// rejected with no handler inside the body.
    fn reject_detached_body(&mut self, base: usize, reason: Value) -> Result<()> {
        let stack_base = self.frames[base].stack_base;
        let promise = self.frames[base].async_result.clone();
        while self.frames.len() > base {
            self.frames.pop();
            self.tracker.pop_frame();
        }
        self.stack.truncate(stack_base);
        if let Some(p) = promise {
            self.settle_promise(&p, reason, true)?;
        }
        Ok(())
    }

    /// The deterministic end-of-drain unhandled-rejection report (R3): if any
    /// promise settled rejected with nobody handling it by the time the
    /// program and its microtasks finished, the run fails with the first one.
    fn unhandled_rejection_error(&self) -> Option<ZapcodeError> {
        let h = self.unhandled_rejections.first()?;
        let reason = self
            .heap
            .object(*h)
            .and_then(|m| m.get("reason"))
            .cloned()
            .unwrap_or(Value::Undefined);
        Some(ZapcodeError::RuntimeError(format!(
            "Unhandled promise rejection: {}",
            reason.to_js_string(&self.heap)
        )))
    }

    /// Run the main loop and shape the finished state for the host. The
    /// program result is implicitly awaited (like a runtime awaiting its entry
    /// point): completing with a *settled* internal promise — the value of an
    /// unawaited `async f()` call or a bare `Promise.resolve(...)` — delivers
    /// the fulfilled value, and a rejected one surfaces as an
    /// unhandled-rejection error. Deferred host-call promises (`pending_call`/
    /// `pending_all`) pass through untouched: their calls were never triggered.
    /// Every host-facing entry (`run_program`, `resume_*`) must route its final
    /// `execute()` through this, so a promise object never leaks to the host.
    fn execute_to_host(&mut self) -> Result<VmState> {
        let state = self.execute()?;
        let VmState::Complete(val) = state else {
            return Ok(state);
        };
        if builtins::is_promise(&val, &self.heap) {
            if let Value::Object(h) = &val {
                let map = self.heap.object_map(*h);
                match map.get("status") {
                    Some(Value::String(s)) if s.as_ref() == "resolved" => {
                        let inner = map.get("value").cloned().unwrap_or(Value::Undefined);
                        return Ok(VmState::Complete(inner));
                    }
                    Some(Value::String(s)) if s.as_ref() == "rejected" => {
                        let reason = map.get("reason").cloned().unwrap_or(Value::Undefined);
                        return Err(ZapcodeError::RuntimeError(format!(
                            "Unhandled promise rejection: {}",
                            reason.to_js_string(&self.heap)
                        )));
                    }
                    _ => {}
                }
            }
        }
        Ok(VmState::Complete(val))
    }

    /// Pop the current call frame and deliver `return_val` to the caller (the
    /// body of the old `Return` instruction). Returns `VmState::Complete` at the
    /// top level. The caller must already have run any escaped `finally` blocks.
    fn perform_return(&mut self, return_val: Value) -> Result<Option<VmState>> {
        if self.frames.len() <= 1 {
            // Top-level return: route through the end-of-program branch of
            // `execute()` so queued microtasks drain (and unhandled rejections
            // report) before completion — park the value on the stack and jump
            // past the last instruction.
            if !self.microtasks.is_empty() || !self.unhandled_rejections.is_empty() {
                self.push(return_val)?;
                let frame = self.frames.last().unwrap();
                let end = match frame.func_index {
                    Some(idx) => self.program(frame.program_index).functions[idx]
                        .instructions
                        .len(),
                    None => self.program(frame.program_index).instructions.len(),
                };
                self.current_frame_mut().ip = end;
                return Ok(None);
            }
            return Ok(Some(VmState::Complete(return_val)));
        }

        let frame = self.frames.pop().unwrap();
        self.tracker.pop_frame();

        // A field-initializer frame binds `this` but is not a constructor: its
        // return value IS the field value (even `undefined`), so skip the
        // constructor `this`-rewrite below.
        // Only a CONSTRUCTOR substitutes the instance for an implicit/
        // `undefined` return (`new C()` evaluates to `this`); an ordinary
        // method returning nothing returns `undefined`, like JS. `this`
        // mutations need no write-back — the receiver is a shared heap
        // handle, so every alias already sees them; the old eager
        // write-back let `new T()` inside a static method overwrite the
        // class binding itself.
        let actual_return = if frame.is_constructor && matches!(return_val, Value::Undefined) {
            frame.this_value.clone().unwrap_or(Value::Undefined)
        } else {
            return_val
        };

        // A detached async body (Stage 3) has no caller frame expecting a
        // pushed value — the caller got the result promise at detach time.
        // Settle it with the return value (with thenable adoption) instead.
        if let Some(promise) = frame.async_result.clone() {
            self.stack.truncate(frame.stack_base);
            return self.settle_async_return(promise, actual_return);
        }

        // An async function's result is a resolved Promise (a returned promise is
        // adopted). So `f().then(...)` works and `await f()` unwraps it.
        let actual_return = if self.frame_is_async(&frame) {
            builtins::make_resolved_promise(actual_return, &mut self.heap)
        } else {
            actual_return
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
                self.apply_batch_resume_action()?;
                self.execute_to_host()
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
                self.consume_batch_element_rejections(&batch.items);
                let array = match batch.kind {
                    BatchKind::AllSettled => self.build_settled_array(batch.items)?,
                    _ => self.build_batch_array(batch.items)?,
                };
                let h = self.heap.alloc_array(array);
                self.push(Value::Array(h))?;
                self.apply_batch_resume_action()?;
                self.execute_to_host()
            }
        }
    }

    /// If a `.then`/`.catch`/`.finally` on the batch promise forced this
    /// batch (see `execute_promise_method`'s `pending_all` arm), run the
    /// recorded method against the just-pushed assembled result — the batch
    /// analogue of the single-call `PromiseMethod` handling in
    /// `resume_execution`. The method's dependent promise replaces the raw
    /// result on the stack; the following `execute_to_host` drives any
    /// enqueued reaction.
    fn apply_batch_resume_action(&mut self) -> Result<()> {
        if let Some(action) = self.resume_action.take() {
            let assembled = self.pop()?;
            let settled = builtins::make_resolved_promise(assembled, &mut self.heap);
            if let Some(state) = self.run_resume_action(action, settled)? {
                // Promise methods on assembled results never re-suspend
                // synchronously (the batch's calls are all resolved).
                return Err(ZapcodeError::RuntimeError(format!(
                    "internal error: unexpected suspension applying a batch promise method: {state:?}"
                )));
            }
        }
        Ok(())
    }

    /// A combinator consuming its elements handles their rejections: a
    /// rejected `.then`-chain element feeding `Promise.all`/`allSettled`/...
    /// becomes the batch's outcome (a rejection that propagates, or a
    /// `{status:"rejected"}` entry), so it must not also report as an
    /// unhandled rejection at end-of-drain.
    fn consume_batch_element_rejections(&mut self, items: &[Value]) {
        for item in items {
            if let Value::Object(h) = item {
                self.mark_rejection_handled(*h);
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
                            entry.insert(Arc::from("status"), Value::String(JsString::from("rejected")));
                            entry.insert(
                                Arc::from("reason"),
                                map.get("reason").cloned().unwrap_or(Value::Undefined),
                            );
                        }
                        _ => {
                            entry.insert(
                                Arc::from("status"),
                                Value::String(JsString::from("fulfilled")),
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
                    entry.insert(Arc::from("status"), Value::String(JsString::from("fulfilled")));
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
            self.consume_batch_element_rejections(&items);
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
        let mut calls = calls;
        let snapshot = ZapcodeSnapshot::capture_with(self, &mut |f| {
            for c in calls.iter_mut() {
                for v in c.args.iter_mut() {
                    v.for_each_handle_mut(f);
                }
            }
        })?;
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
        let mut args = pc.args;
        let snapshot = ZapcodeSnapshot::capture_with_values(self, &mut args)?;
        Ok(Some(VmState::Suspended {
            function_name: pc.name,
            args,
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
                        // Branded so `e instanceof Error` is true (Node:
                        // AggregateError extends Error).
                        agg.insert(Arc::from("__error__"), Value::Bool(true));
                        agg.insert(
                            Arc::from("name"),
                            Value::String(JsString::from("AggregateError")),
                        );
                        agg.insert(
                            Arc::from("message"),
                            Value::String(JsString::from("All promises were rejected")),
                        );
                        agg.insert(Arc::from("errors"), errors_arr);
                        let reason = Value::Object(self.heap.alloc_object(agg));
                        let msg = reason.to_js_string(&self.heap);
                        // Rethrow the AggregateError object itself (identity
                        // preserved for guest catch), as at any await site.
                        self.pending_throw = Some(reason);
                        return Err(ZapcodeError::RuntimeError(format!(
                            "Unhandled promise rejection: {}",
                            msg
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
            ZapcodeError::RangeError(s) => ("RangeError", s.clone()),
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

    /// JS `Function.prototype.length`: the count of leading parameters that have
    /// neither a default value nor are a rest element (counting stops at the
    /// first such parameter).
    fn function_arity(&self, func_ref: FunctionRef) -> i64 {
        let f = match self
            .program(func_ref.program_id)
            .functions
            .get(func_ref.function_id)
        {
            Some(f) => f,
            None => return 0,
        };
        let mut n = 0i64;
        for p in &f.params {
            match p {
                crate::parser::ir::ParamPattern::DefaultValue { .. }
                | crate::parser::ir::ParamPattern::Rest(_) => break,
                _ => n += 1,
            }
        }
        n
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
                // extract the fields from the argument into those slots. A
                // pattern with a field-level default keeps the raw argument in
                // a hidden temp first, so the compiler prologue can repair the
                // leaves when the field arrives undefined.
                ParamPattern::ObjectDestructure(_) | ParamPattern::ArrayDestructure(_) => {
                    let arg = args.get(i).cloned().unwrap_or(Value::Undefined);
                    if param.has_field_level_default() {
                        locals.push(arg.clone());
                    }
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
        let env: BTreeMap<String, u64> = closure.env.iter().cloned().collect();

        // Consume the one-shot field-initializer flag (set by `call_field_init`).
        let is_field_init = std::mem::take(&mut self.next_frame_is_field_init);
        let is_constructor = std::mem::take(&mut self.next_frame_is_constructor);

        self.frames.push(CallFrame {
            program_index: closure.func_ref.program_id,
            func_index: Some(closure.func_ref.function_id),
            ip: 0,
            locals,
            stack_base: self.stack.len(),
            this_value,
            receiver_source,
            boxed: BTreeMap::new(),
            env,
            is_field_init,
            async_result: None,
            is_constructor,
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
            boxed: BTreeMap::new(),
            env: BTreeMap::new(),
            is_field_init: false,
            async_result: None,
            is_constructor: false,
        });

        self.execute_to_host()
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
                    // The synchronous run is over — drain the microtask queue
                    // before completing. Each drained reaction runs as its own
                    // turn in this same loop (its callback frames sit on top of
                    // the finished top-level frame, whose ip stays past the
                    // end, so control returns here for the next microtask). A
                    // host call inside a reaction suspends with the remaining
                    // queue in the snapshot; resume re-enters this drain.
                    if let Some(m) = self.microtasks.pop_front() {
                        self.start_microtask(m)?;
                        continue;
                    }
                    // End of drain: a rejection nobody handled fails the run
                    // deterministically (JS's unhandled-rejection event). Like
                    // Node, this fires per-MACROTASK — before the next timer.
                    if let Some(err) = self.unhandled_rejection_error() {
                        return Err(err);
                    }
                    // Microtasks done: fire the next due timer (a macrotask),
                    // then drain whatever it queued.
                    if let Some(t) = self.pop_due_timer() {
                        self.start_timer_job(t)?;
                        continue;
                    }
                    // Top-level: return last value on stack or undefined
                    let result = if self.stack.is_empty() {
                        Value::Undefined
                    } else {
                        self.stack.pop().unwrap_or(Value::Undefined)
                    };
                    return Ok(VmState::Complete(result));
                } else {
                    // Return from function (fell off the end → implicit undefined)
                    let frame = self.frames.pop().unwrap();
                    self.tracker.pop_frame();
                    // A detached async body resolves its result promise with
                    // undefined; nothing is pushed (no caller frame).
                    if let Some(promise) = frame.async_result.clone() {
                        self.stack.truncate(frame.stack_base);
                        if let Some(state) = self.settle_async_return(promise, Value::Undefined)? {
                            return Ok(state);
                        }
                        continue;
                    }
                    let is_async = self.frame_is_async(&frame);
                    // A CONSTRUCTOR falling off the end returns `this`; an
                    // ordinary method returns `undefined` (see the Return
                    // arm for the same rule and its rationale).
                    let ret = if frame.is_constructor {
                        self.stack.truncate(frame.stack_base);
                        frame.this_value.unwrap_or(Value::Undefined)
                    } else {
                        Value::Undefined
                    };
                    // An async function falling off the end resolves with undefined.
                    let ret = if is_async {
                        builtins::make_resolved_promise(ret, &mut self.heap)
                    } else {
                        ret
                    };
                    self.push(ret)?;
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
                    let error_val = self.caught_error_value(&err);
                    // A throw inside a *microtask* callback is its own turn: it
                    // may route to handlers within the callback, but escaping
                    // the callback rejects the reaction's chain promise — it
                    // must NOT reach handlers of whatever code happened to be
                    // running when the drain started. This is what makes
                    // `p.then(throwing).catch(h)` deliver the throw to `h`.
                    let microtask_boundary = match self.continuations.last() {
                        Some(Continuation::MicrotaskReaction {
                            caller_frame_depth,
                            callback_frame_index,
                            ..
                        })
                        | Some(Continuation::PromiseExecutor {
                            caller_frame_depth,
                            callback_frame_index,
                            ..
                        }) if self.frames.len() > *caller_frame_depth
                            && self.frames.len() >= *callback_frame_index =>
                        {
                            Some(*caller_frame_depth)
                        }
                        _ => None,
                    };
                    // An *async function body* is a boundary too (Stage 3): a
                    // throw escaping it rejects the call's result promise —
                    // for a detached body that settles the promise the caller
                    // already holds; for a body that has not yet awaited, the
                    // caller receives an (unhandled-marked) rejected promise
                    // as the call's value. Either way `f().catch(h)` sees the
                    // body throw, and the caller's own try/catch does not
                    // (matching Node).
                    let async_boundary = self
                        .frames
                        .iter()
                        .enumerate()
                        .skip(1)
                        .rev()
                        .find(|(_, f)| f.async_result.is_some() || self.frame_is_async(f))
                        .map(|(i, _)| i);
                    // The innermost boundary wins. (When an async handler frame
                    // is both watched by a MicrotaskReaction and async, the
                    // indices coincide and either path settles the same chain.)
                    let boundary = match (microtask_boundary, async_boundary) {
                        (Some(d), Some(j)) if j > d => Some((j, true)),
                        (Some(d), _) => Some((d, false)),
                        (None, Some(j)) => Some((j, true)),
                        (None, None) => None,
                    };
                    match boundary {
                        Some((depth, is_async_frame)) if is_async_frame => {
                            if !self.route_thrown(error_val, depth)? {
                                let reason = self.pending_throw.take().unwrap_or(Value::Undefined);
                                if self.frames[depth].async_result.is_some() {
                                    // Detached body: reject the promise the
                                    // caller received at detach time.
                                    self.reject_detached_body(depth, reason)?;
                                } else {
                                    // Not yet detached: the call evaluates to
                                    // a rejected promise (marked unhandled
                                    // until someone consumes it).
                                    let stack_base = self.frames[depth].stack_base;
                                    while self.frames.len() > depth {
                                        self.frames.pop();
                                        self.tracker.pop_frame();
                                    }
                                    self.stack.truncate(stack_base);
                                    let rejected =
                                        builtins::make_rejected_promise(reason, &mut self.heap);
                                    if let Value::Object(h) = &rejected {
                                        self.unhandled_rejections.push(*h);
                                    }
                                    self.push(rejected)?;
                                    // The pop looks like a return — a watching
                                    // continuation (e.g. `.then(async …)`)
                                    // must fire and adopt the rejection.
                                    if let Some(state) = self.process_continuation()? {
                                        return Ok(state);
                                    }
                                }
                            }
                        }
                        Some((depth, _)) => {
                            if !self.route_thrown(error_val, depth)? {
                                // No handler inside the callback — unwind its
                                // frames and reject the chain with the original
                                // thrown value (identity preserved for `.catch`).
                                let reason = self.pending_throw.take().unwrap_or(Value::Undefined);
                                let stack_base = self.frames[depth].stack_base;
                                while self.frames.len() > depth {
                                    self.frames.pop();
                                    self.tracker.pop_frame();
                                }
                                self.stack.truncate(stack_base);
                                match self.continuations.pop() {
                                    Some(Continuation::MicrotaskReaction {
                                        result_promise, ..
                                    }) => {
                                        self.settle_promise(&result_promise, reason, true)?;
                                    }
                                    Some(Continuation::PromiseExecutor { promise, .. }) => {
                                        // The spec'd constructor catch: the
                                        // throw rejects the promise (no-op if
                                        // a capability already settled it) and
                                        // the `new` expression still evaluates
                                        // to the promise.
                                        self.settle_promise(&promise, reason, true)?;
                                        self.push(promise)?;
                                    }
                                    _ => {}
                                }
                            }
                        }
                        None => {
                            if !self.route_thrown(error_val, 0)? {
                                // No handler remains — propagate to the host.
                                return Err(err);
                            }
                        }
                    }
                    // A throw that unwound out of a main-loop generator pull
                    // leaves its continuation stale: the generator becomes
                    // done (Node) and the exception IS the pull's answer.
                    while let Some(Continuation::GeneratorNext {
                        caller_frame_depth,
                        ..
                    }) = self.continuations.last()
                    {
                        if self.frames.len() > *caller_frame_depth {
                            break;
                        }
                        let Some(Continuation::GeneratorNext { mut gen_obj, .. }) =
                            self.continuations.pop()
                        else {
                            unreachable!("checked above");
                        };
                        gen_obj.done = true;
                        gen_obj.suspended = None;
                        self.generator_try_frames.remove(&gen_obj.id);
                        self.store_generator(gen_obj);
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
            Continuation::MicrotaskReaction {
                callback_frame_index,
                caller_frame_depth,
                ..
            } => (*callback_frame_index, *caller_frame_depth),
            Continuation::PromiseExecutor {
                callback_frame_index,
                caller_frame_depth,
                ..
            } => (*callback_frame_index, *caller_frame_depth),
            Continuation::GeneratorNext {
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

        // The callback's result is collected AS-IS — an async callback hands
        // its result promise to the driver, exactly as in Node:
        // `arr.map(async cb)` yields an array of PROMISES (consume them with
        // `Promise.all`/`allSettled`/`await`), never eagerly-unwrapped values.
        // A rejected result stays an unhandled-marked promise until something
        // consumes it; if nothing ever does, the end-of-drain check surfaces
        // it — the deterministic analogue of Node's unhandledRejection event.

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
            Continuation::MicrotaskReaction {
                mode,
                result_promise,
                original_value,
                original_is_rejection,
                ..
            } => {
                // A drained reaction's handler returned: settle the chain's
                // dependent promise (created when `.then` enqueued). Nothing is
                // pushed — the chain expression already received the promise.
                //
                // A returned *deferred single-call promise* (bare tool call,
                // N5) is adopted by forcing its host call now; the recorded
                // `SettleResult` action settles the chain on resume.
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
                        // Already settled (awaited before): use the cached value.
                        if let Some(cached) = self.resolved.get(&id).cloned() {
                            match mode {
                                PromiseCallbackMode::WrapResult => {
                                    self.settle_promise(&result_promise, cached, false)?;
                                }
                                PromiseCallbackMode::PassThrough => {
                                    self.settle_promise(
                                        &result_promise,
                                        original_value,
                                        original_is_rejection,
                                    )?;
                                }
                            }
                            return Ok(None);
                        }
                        self.resume_action = Some(ResumeAction::SettleResult {
                            result_promise,
                            pass_original: match mode {
                                PromiseCallbackMode::WrapResult => None,
                                PromiseCallbackMode::PassThrough => {
                                    Some((original_value, original_is_rejection))
                                }
                            },
                        });
                        return self.suspend_on_pending_call(id);
                    }
                }
                match mode {
                    PromiseCallbackMode::WrapResult => {
                        // Thenable adoption of internal promises.
                        if builtins::is_promise(&callback_result, &self.heap) {
                            if let Value::Object(h) = &callback_result {
                                let map = self.heap.object_map(*h);
                                match map.get("status") {
                                    Some(Value::String(s)) if s.as_ref() == "resolved" => {
                                        let inner =
                                            map.get("value").cloned().unwrap_or(Value::Undefined);
                                        self.settle_promise(&result_promise, inner, false)?;
                                    }
                                    Some(Value::String(s)) if s.as_ref() == "rejected" => {
                                        let reason =
                                            map.get("reason").cloned().unwrap_or(Value::Undefined);
                                        self.mark_rejection_handled(*h);
                                        self.settle_promise(&result_promise, reason, true)?;
                                    }
                                    Some(Value::String(s)) if s.as_ref() == "pending" => {
                                        // Adopt a microtask-pending promise: a
                                        // handler-less reaction forwards its
                                        // eventual outcome to the chain.
                                        self.register_reaction(
                                            *h,
                                            Value::Undefined,
                                            Value::Undefined,
                                            PromiseCallbackMode::WrapResult,
                                            result_promise.clone(),
                                        )?;
                                    }
                                    _ => {
                                        // pending_all (a batch promise): settle
                                        // with the promise object itself — an
                                        // `await` of the chain re-awaits it (the
                                        // resolved-with-a-promise adoption arm).
                                        self.settle_promise(
                                            &result_promise,
                                            callback_result,
                                            false,
                                        )?;
                                    }
                                }
                                return Ok(None);
                            }
                        }
                        self.settle_promise(&result_promise, callback_result, false)?;
                    }
                    PromiseCallbackMode::PassThrough => {
                        // `.finally`: discard the handler's return value and
                        // re-settle with the receiver's original outcome.
                        self.settle_promise(
                            &result_promise,
                            original_value,
                            original_is_rejection,
                        )?;
                    }
                }
                Ok(None)
            }
            Continuation::GeneratorNext { gen_obj, for_of, .. } => {
                // The body returned (or fell off the end): the generator is
                // done; answer the pull with {value, done: true} — or, for a
                // for…of pull, the protocol pair [done-triple, return-value].
                let gen_id = gen_obj.id;
                let is_async_gen = self.current_function(gen_obj.func_ref).is_async;
                if for_of {
                    let _ = self.finish_generator(gen_obj, Value::Undefined);
                    let triple = self.gen_iter_triple(gen_id, true)?;
                    self.push(triple)?;
                    self.push(callback_result)?;
                } else {
                    let res = self.finish_generator(gen_obj, callback_result);
                    let res = if is_async_gen {
                        builtins::make_resolved_promise(res, &mut self.heap)
                    } else {
                        res
                    };
                    self.push(res)?;
                }
                Ok(None)
            }
            Continuation::PromiseExecutor { promise, .. } => {
                // The executor returned: its value is discarded (spec); the
                // `new Promise(...)` expression evaluates to the promise,
                // settled or not. (`callback_result` was already popped.)
                let _ = callback_result;
                self.push(promise)?;
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
    /// Heap-aware `===`. Identical to `Value::strict_eq` except that two distinct
    /// objects that are both registered symbols (`Symbol.for`) compare equal when
    /// they share a registry key — so `Symbol.for('x') === Symbol.for('x')`.
    fn strict_eq_heap(&self, left: &Value, right: &Value) -> bool {
        if let (Value::Object(a), Value::Object(b)) = (left, right) {
            if a != b {
                if let (Some(ma), Some(mb)) = (self.heap.object(*a), self.heap.object(*b)) {
                    if let (Some(ka), Some(kb)) =
                        (ma.get("__symbol_for__"), mb.get("__symbol_for__"))
                    {
                        return ka.strict_eq(kb);
                    }
                }
            }
        }
        left.strict_eq(right)
    }

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
        // 5 is comfortably above any legitimate valueOf->toString fallback nesting
        // (which is at most a couple of levels) yet well below native-stack
        // exhaustion across the various interpreter frame sizes on the recursion
        // path (a cyclic `toString(){ return "" + this }` is caught cleanly here).
        if self.to_primitive_depth >= 5 {
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
        // A builtin static passed as a callback (`Object.groupBy(xs, Math.floor)`,
        // `arr.map(JSON.stringify)`): dispatch through the pure builtin layer.
        if let Value::BuiltinMethod {
            object_name,
            method_name,
            ..
        } = callee
        {
            if let Some(v) = builtins::call_global_method(
                object_name,
                method_name,
                &args,
                &mut self.stdout,
                &mut self.heap,
            )? {
                return Ok(v);
            }
        }
        // Bare conversion globals (`String`/`Number`/`Boolean`, marker
        // objects) are callable as callbacks too — `arr.map(String)` etc. —
        // with the same ToPrimitive shaping the Call instruction applies.
        if let Value::Object(h) = callee {
            let kind = self
                .heap
                .object(*h)
                .and_then(|m| m.get("__global_fn__"))
                .and_then(|v| match v {
                    Value::String(s) => Some(s.to_string()),
                    _ => None,
                });
            if let Some(kind) = kind {
                if let Some(result) = self.call_scheduler_global(&kind, &args) {
                    return result;
                }
                let mut args = args;
                match kind.as_str() {
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
                return builtins::call_global_fn(&kind, &args, &mut self.heap);
            }
        }
        self.call_closure_internal(callee, args, None)
    }

    /// Run a synthesized class field-initializer closure with `this` bound to the
    /// instance/class object, returning the field's value. Unlike a constructor,
    /// an `undefined` result stays `undefined` (see `CallFrame::is_field_init`).
    /// The one-shot `next_frame_is_field_init` flag tags the pushed frame without
    /// adding a wrapper to the hot internal-call path.
    fn call_field_init(&mut self, callee: &Value, receiver: Value) -> Result<Value> {
        self.last_receiver_source = None;
        self.next_frame_is_field_init = true;
        let r = self.call_closure_internal(callee, Vec::new(), Some(receiver));
        // Cleared in push_call_frame on success; reset here defensively in case
        // the callee wasn't a function and no frame was pushed.
        self.next_frame_is_field_init = false;
        r
    }

    /// Run a `JSON.parse` reviver with `this` bound to the holder object, per the
    /// ES `InternalizeJSONProperty` spec. Reuses the field-initializer frame flag
    /// so the reviver's return value is preserved verbatim — crucially, a reviver
    /// returning `undefined` (to delete a property) must NOT trigger the
    /// constructor-style "rewrite undefined-return to `this`" path, which would
    /// otherwise splice the holder back into the result and create a cycle.
    fn call_reviver(&mut self, callee: &Value, holder: Value, args: Vec<Value>) -> Result<Value> {
        self.last_receiver_source = None;
        self.next_frame_is_field_init = true;
        let r = self.call_closure_internal(callee, args, Some(holder));
        // Cleared in push_call_frame on success; reset here defensively in case
        // the callee wasn't a function and no frame was pushed.
        self.next_frame_is_field_init = false;
        r
    }

    /// Shared body for the internal-call helpers: push a frame (optionally with a
    /// bound `this`) and run it to completion, returning the result.
    /// Build (and register) a generator object for a call to a generator
    /// function: args are captured as named params so the first pull can
    /// bind them; the receiver (when this is a method call) rides as the
    /// body's permanent `this`.
    fn make_generator_object(
        &mut self,
        closure: &Closure,
        args: &[Value],
        this_value: Option<Value>,
    ) -> GeneratorObject {
        let params = self.current_function(closure.func_ref).params.clone();
        let gen_id = self.alloc_generator_id();
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
            func_ref: closure.func_ref,
            captured,
            env: closure.env.clone(),
            suspended: None,
            done: false,
            this_value: this_value.map(Box::new),
        };
        self.globals
            .insert(format!("__gen_{}", gen_id), Value::Generator(gen_obj.clone()));
        gen_obj
    }

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

        // A generator function called internally (e.g. a `*[Symbol.iterator]`
        // method invoked by the iteration protocol) returns its generator
        // object — running the body here would hit `Yield` with no driver.
        if self.current_function(closure.func_ref).is_generator {
            let gen_obj = self.make_generator_object(&closure, &args, this_value);
            return Ok(Value::Generator(gen_obj));
        }

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

    /// `String.prototype.replace` / `replaceAll` with a FUNCTION replacer.
    /// Invokes the callback per match with `(match, p1, p2, …, offset, string)`
    /// (named groups appended as a final `groups` object when present) and
    /// substitutes the callback's string-coerced return value. The pure builtin
    /// can't reach the guest-closure call path, so this runs at the VM layer.
    fn string_replace_with_function(
        &mut self,
        s: &str,
        method: &str,
        args: &[Value],
    ) -> Result<Value> {
        let replacer = args[1].clone();
        // Regex search: walk matches (all for /g or replaceAll, else first).
        if let Some((pat, flags)) = args.first().and_then(|v| builtins::regexp_parts(v, &self.heap))
        {
            let re = builtins::compile_regex(&pat, &flags)?;
            let global = method == "replaceAll" || flags.contains('g');
            let has_named = re.capture_names().any(|n| n.is_some());
            // Collect match spans + captured groups up front; the regex borrows
            // `s`, so we can't hold it across the guest call that needs `&mut self`.
            struct MatchInfo {
                start: usize,
                end: usize,
                whole: String,
                groups: Vec<Value>,
                named: Vec<(String, Value)>,
            }
            let mut matches: Vec<MatchInfo> = Vec::new();
            for caps in re.captures_iter(s) {
                let m0 = caps.get(0).unwrap();
                let mut groups = Vec::with_capacity(caps.len().saturating_sub(1));
                for i in 1..caps.len() {
                    groups.push(
                        caps.get(i)
                            .map(|g| Value::String(JsString::from(g.as_str())))
                            .unwrap_or(Value::Undefined),
                    );
                }
                let named = if has_named {
                    re.capture_names()
                        .flatten()
                        .map(|name| {
                            (
                                name.to_string(),
                                caps.name(name)
                                    .map(|g| Value::String(JsString::from(g.as_str())))
                                    .unwrap_or(Value::Undefined),
                            )
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                matches.push(MatchInfo {
                    start: m0.start(),
                    end: m0.end(),
                    whole: m0.as_str().to_string(),
                    groups,
                    named,
                });
                if !global {
                    break;
                }
            }
            let mut out = String::with_capacity(s.len());
            let mut last = 0usize;
            for info in matches {
                out.push_str(&s[last..info.start]);
                // offset is the char index of the match start (JS uses code-unit
                // index; we align with the rest of the codebase on char count).
                let offset = s[..info.start].chars().count() as i64;
                let mut call_args: Vec<Value> =
                    Vec::with_capacity(info.groups.len() + info.named.len() + 3);
                call_args.push(Value::String(JsString::from(info.whole.as_str())));
                call_args.extend(info.groups);
                call_args.push(Value::Int(offset));
                call_args.push(Value::String(s.clone().into()));
                if !info.named.is_empty() {
                    let mut g: IndexMap<Arc<str>, Value> = IndexMap::new();
                    for (k, v) in info.named {
                        g.insert(Arc::from(k.as_str()), v);
                    }
                    call_args.push(Value::Object(self.heap.alloc_object(g)));
                }
                let result = self.call_function_internal(&replacer, call_args)?;
                out.push_str(&result.to_js_string(&self.heap));
                last = info.end;
            }
            out.push_str(&s[last..]);
            return Ok(Value::String(JsString::from(out.as_str())));
        }

        // String search: `replace` substitutes the first occurrence, `replaceAll`
        // every occurrence. The callback receives (match, offset, string).
        let search = args
            .first()
            .map(|v| v.to_js_string(&self.heap))
            .unwrap_or_default();
        let subject = s.to_string();
        if search.is_empty() {
            // Empty search matches at position 0 (and replaceAll between every
            // char). Keep it simple and JS-correct for the common case: a single
            // call at offset 0 prepending the result. JS replaceAll('') inserts at
            // every boundary; that's an edge case we don't expect agents to hit, so
            // handle only the first-position case to stay predictable.
            let result =
                self.call_function_internal(&replacer, vec![
                    Value::String(JsString::from("")),
                    Value::Int(0),
                    Value::String(s.clone().into()),
                ])?;
            let ins = result.to_js_string(&self.heap);
            return Ok(Value::String(JsString::from(format!("{}{}", ins, subject).as_str())));
        }
        let mut out = String::with_capacity(subject.len());
        let mut search_from = 0usize;
        let replace_all = method == "replaceAll";
        loop {
            match subject[search_from..].find(&search) {
                Some(rel) => {
                    let abs = search_from + rel;
                    out.push_str(&subject[search_from..abs]);
                    let offset = subject[..abs].chars().count() as i64;
                    let result = self.call_function_internal(&replacer, vec![
                        Value::String(JsString::from(search.as_str())),
                        Value::Int(offset),
                        Value::String(s.clone().into()),
                    ])?;
                    out.push_str(&result.to_js_string(&self.heap));
                    search_from = abs + search.len();
                    if !replace_all {
                        break;
                    }
                }
                None => break,
            }
        }
        out.push_str(&subject[search_from..]);
        Ok(Value::String(JsString::from(out.as_str())))
    }

    /// `JSON.stringify(value, replacer?, space?)`. Honors a user `toJSON()` on
    /// plain objects and a FUNCTION replacer `(key, value)` (both need the
    /// guest-closure call path). Array replacers / indent reuse the builtin's
    /// formatting helpers.
    fn json_stringify(&mut self, args: &[Value]) -> Result<Value> {
        let value = args.first().cloned().unwrap_or(Value::Undefined);
        let replacer_fn = match args.get(1) {
            Some(Value::Function(_)) => args.get(1).cloned(),
            _ => None,
        };
        // Array replacer (whitelist of keys) — only when not a function.
        let whitelist: Option<Vec<String>> = if replacer_fn.is_some() {
            None
        } else {
            match args.get(1) {
                Some(Value::Array(h)) => Some(
                    self.heap
                        .array_vec(*h)
                        .iter()
                        .filter_map(|v| match v {
                            Value::String(s) => Some(s.to_string()),
                            Value::Int(n) => Some(n.to_string()),
                            _ => None,
                        })
                        .collect(),
                ),
                _ => None,
            }
        };
        let indent = match args.get(2) {
            Some(Value::Int(n)) if *n > 0 => Some(" ".repeat((*n).min(10) as usize)),
            Some(Value::Float(n)) if *n >= 1.0 => Some(" ".repeat((*n as usize).min(10))),
            Some(Value::String(s)) if !s.is_empty() => Some(s.to_string()),
            _ => None,
        };
        // The replacer is applied at the root with key `""` and the value (JS
        // SerializeJSONProperty applies it to a synthetic holder `{ "": value }`;
        // we don't bind `this` since the receiver-writeback machinery would
        // corrupt the value being serialized — replacers that read `this` are an
        // accepted gap).
        let transformed = if let Some(ref f) = replacer_fn {
            self.call_function_internal(f, vec![
                Value::String(JsString::from("")),
                value.clone(),
            ])?
        } else {
            value
        };
        let mut seen: Vec<Handle> = Vec::new();
        match self.serialize_json_dynamic(
            &transformed,
            replacer_fn.as_ref(),
            whitelist.as_deref(),
            indent.as_deref(),
            0,
            &mut seen,
        )? {
            Some(s) => Ok(Value::String(JsString::from(s.as_str()))),
            None => Ok(Value::Undefined),
        }
    }

    /// Recursive JSON serializer that honors `toJSON()` hooks and a function
    /// replacer. Returns `None` for values JSON omits (undefined / functions) so
    /// callers drop object props / emit `null` in arrays.
    fn serialize_json_dynamic(
        &mut self,
        val: &Value,
        replacer: Option<&Value>,
        whitelist: Option<&[String]>,
        indent: Option<&str>,
        depth: usize,
        seen: &mut Vec<Handle>,
    ) -> Result<Option<String>> {
        // Defense-in-depth: bound nesting so a pathologically deep (but acyclic)
        // structure can't overflow the native stack. True reference cycles are
        // caught by the `seen` set in the array/object arms below; this guards the
        // deep-but-finite case. Mirrors the pure `builtins::serialize_json` guard so
        // the toJSON/replacer-aware VM path is just as crash-safe.
        if depth > MAX_RENDER_DEPTH {
            return Err(ZapcodeError::RuntimeError(format!(
                "JSON nesting depth exceeded (max {})",
                MAX_RENDER_DEPTH
            )));
        }
        // Honor a `toJSON()` method on objects (plain objects and Date alike). The
        // hook's return value is serialized in place of the object.
        let val = if let Value::Object(h) = val {
            let to_json = self
                .heap
                .object(*h)
                .and_then(|m| m.get("toJSON"))
                .filter(|v| matches!(v, Value::Function(_)))
                .cloned();
            if let Some(f) = to_json {
                std::borrow::Cow::Owned(self.call_method_internal(&f, val.clone(), vec![])?)
            } else {
                std::borrow::Cow::Borrowed(val)
            }
        } else {
            std::borrow::Cow::Borrowed(val)
        };
        let val: &Value = &val;
        Ok(match val {
            Value::Undefined
            | Value::Function(_)
            | Value::BuiltinMethod { .. }
            | Value::Generator(_)
            | Value::Pending(_) => None,
            Value::Null => Some("null".to_string()),
            Value::Bool(b) => Some(b.to_string()),
            Value::Int(n) => Some(n.to_string()),
            Value::Float(n) => Some(if n.is_finite() {
                builtins::format_number(*n)
            } else {
                "null".to_string()
            }),
            // JSON.stringify(bigint) throws a TypeError in JS.
            Value::BigInt(_) => {
                return Err(ZapcodeError::TypeError(
                    "Do not know how to serialize a BigInt".to_string(),
                ))
            }
            Value::String(s) => Some(builtins::json_escape_string(s)),
            Value::Array(h) => {
                if seen.contains(h) {
                    return Err(ZapcodeError::TypeError(
                        "Converting circular structure to JSON".to_string(),
                    ));
                }
                seen.push(*h);
                let items_src = self.heap.array_vec(*h);
                let mut items: Vec<String> = Vec::with_capacity(items_src.len());
                for (i, item) in items_src.iter().enumerate() {
                    // Apply the replacer with the array index (as a string) as key.
                    let item = if let Some(f) = replacer {
                        self.call_function_internal(f, vec![
                            Value::String(JsString::from(i.to_string().as_str())),
                            item.clone(),
                        ])?
                    } else {
                        item.clone()
                    };
                    let s = self
                        .serialize_json_dynamic(&item, replacer, whitelist, indent, depth + 1, seen)?
                        .unwrap_or_else(|| "null".to_string());
                    items.push(s);
                }
                seen.pop();
                Some(builtins::join_json_array(&items, indent, depth))
            }
            Value::Object(h) => {
                // Clone the map out: the per-entry replacer call borrows `&mut self`,
                // which would conflict with a live borrow into the heap.
                let map = match self.heap.object(*h) {
                    Some(m) => m.clone(),
                    None => return Ok(Some("{}".to_string())),
                };
                // Date with no toJSON hook handled above falls here only if it had
                // its hook stripped; emit ISO via __date_ms__ for safety.
                if let Some(ms) = map.get("__date_ms__") {
                    let ms = ms.to_number() as i64;
                    return Ok(Some(builtins::json_escape_string(&unix_millis_to_iso(ms))));
                }
                if map.contains_key("__map__")
                    || map.contains_key("__set__")
                    || map.contains_key("__regexp__")
                    || map.contains_key("__error__")
                {
                    return Ok(Some("{}".to_string()));
                }
                if seen.contains(h) {
                    return Err(ZapcodeError::TypeError(
                        "Converting circular structure to JSON".to_string(),
                    ));
                }
                seen.push(*h);
                let mut pairs: Vec<(String, String)> = Vec::new();
                // ECMA-262 key order: integer-index keys ascending, then string
                // keys in insertion order (same order as Object.keys / for-in).
                for k in builtins::ordered_visible_keys(&map) {
                    let v = match map.get(&k) {
                        Some(v) => v,
                        None => continue,
                    };
                    if let Some(w) = whitelist {
                        if !w.iter().any(|x| x == k.as_ref()) {
                            continue;
                        }
                    }
                    // Accessor keys serialize their getter's RESULT, not the
                    // stored function (setter-only -> undefined -> omitted).
                    let v = self.enumerable_value(&Value::Object(*h), &k, v.clone())?;
                    let v = if let Some(f) = replacer {
                        self.call_function_internal(f, vec![
                            Value::String(k.clone().into()),
                            v,
                        ])?
                    } else {
                        v
                    };
                    if let Some(s) =
                        self.serialize_json_dynamic(&v, replacer, whitelist, indent, depth + 1, seen)?
                    {
                        pairs.push((k.to_string(), s));
                    }
                }
                seen.pop();
                Some(builtins::join_json_object(&pairs, indent, depth))
            }
        })
    }

    /// `JSON.parse(text, reviver)`: parse normally, then walk the result
    /// bottom-up calling `reviver(key, value)` with `this` bound to the holder.
    /// A reviver returning `undefined` deletes the property.
    fn json_parse_with_reviver(&mut self, args: &[Value]) -> Result<Value> {
        let text = match args.first() {
            Some(Value::String(s)) => s.to_string(),
            _ => {
                return Err(ZapcodeError::TypeError(
                    "JSON.parse requires a string argument".to_string(),
                ))
            }
        };
        let reviver = args[1].clone();
        let parsed = builtins::json_to_value(&text, &mut self.heap)?;
        // Root holder: { "": parsed }.
        let holder = {
            let mut m: IndexMap<Arc<str>, Value> = IndexMap::new();
            m.insert(Arc::from(""), parsed);
            Value::Object(self.heap.alloc_object(m))
        };
        self.revive_walk(&holder, "", &reviver)
    }

    /// Recursively revive a value: process children first, then call the reviver
    /// for this `key`. Returning `undefined` from the reviver removes the entry.
    fn revive_walk(&mut self, holder: &Value, key: &str, reviver: &Value) -> Result<Value> {
        let value = match holder {
            Value::Object(h) => self
                .heap
                .object(*h)
                .and_then(|m| m.get(key))
                .cloned()
                .unwrap_or(Value::Undefined),
            // An ARRAY holder reads its element by index — without this, every
            // element of a revived array was visited as `undefined`.
            Value::Array(h) => key
                .parse::<usize>()
                .ok()
                .and_then(|i| self.heap.array(*h).get(i).cloned())
                .unwrap_or(Value::Undefined),
            _ => Value::Undefined,
        };
        match &value {
            Value::Array(h) => {
                let len = self.heap.array_vec(*h).len();
                for i in 0..len {
                    let ik = i.to_string();
                    let revived = self.revive_walk(&value, &ik, reviver)?;
                    if let Some(arr) = self.heap.array_mut(*h) {
                        if i < arr.len() {
                            // Array elements are kept (set to undefined if dropped),
                            // mirroring JS InternalizeJSONProperty for arrays.
                            arr[i] = revived;
                        }
                    }
                }
            }
            Value::Object(h) => {
                let keys: Vec<Arc<str>> = self
                    .heap
                    .object(*h)
                    .map(|m| m.keys().cloned().collect())
                    .unwrap_or_default();
                for k in keys {
                    let revived = self.revive_walk(&value, &k, reviver)?;
                    if matches!(revived, Value::Undefined) {
                        if let Some(m) = self.heap.object_mut(*h) {
                            m.shift_remove(k.as_ref());
                        }
                    } else if let Some(m) = self.heap.object_mut(*h) {
                        m.insert(k, revived);
                    }
                }
            }
            _ => {}
        }
        // Call reviver(key, value) with `this` bound to the HOLDER object, per
        // the ES `InternalizeJSONProperty` spec, so a reviver that reads
        // `this[otherKey]` sees its sibling values. `call_reviver` preserves the
        // return value verbatim (an `undefined` return deletes the property and
        // must NOT be rewritten to the holder).
        self.call_reviver(
            reviver,
            holder.clone(),
            vec![Value::String(JsString::from(key)), value],
        )
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
            | "reduce" | "reduceRight" | "sort" | "toSorted" | "flatMap" => {
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
                    "toSorted" => {
                        // Like sort, but returns a NEW array and leaves the
                        // original untouched (ES2023).
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
                        let h = self.heap.alloc_array(result);
                        Ok(Some(Value::Array(h)))
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
    /// The handler never runs here (microtask-design Stage 1): a settled
    /// receiver enqueues a [`Microtask`], a microtask-pending receiver
    /// registers a reaction, and either way the method's value is a new
    /// pending promise the reaction settles during the drain. The drain runs
    /// the handler via [`Continuation::MicrotaskReaction`] in the main
    /// `execute()` loop, so a tool (external) call inside the handler can
    /// suspend the VM and resume — this is what makes
    /// `primary().catch(() => fallbackTool())` work.
    ///
    /// Returns:
    /// - `PromiseMethodOutcome::Value(v)` — push `v` (the dependent promise,
    ///   or the receiver passed through when no applicable handler).
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
            // A combinator over internal elements only (no host call to
            // force): lower it to a real pending promise — combine reactions
            // settle it from the drain — and re-run the method through the
            // ordinary pending/settled paths. Without this, `.then` on such a
            // batch hits the legacy pass-through below and drops its handler.
            if status == "pending_all" && self.batch_all_internal(&promise) {
                self.lower_internal_batch(&promise)?;
                return self.execute_promise_method(promise.clone(), method, args);
            }
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
            // A *batch* promise (`Promise.all/race/any/allSettled` over host
            // calls): `.then`/`.catch`/`.finally` forces the batch, exactly
            // like the single-call path above — suspend on all of its calls
            // with a `PromiseMethod` action that re-runs the method against
            // the assembled result on resume (`resume_many` applies it; a
            // host-rejected combinator routes through `resume_with_error`,
            // delivering a rejected promise to the method). Batches that mix
            // in microtask-pending chain elements keep the legacy
            // pass-through (their chains can only settle in the main loop) —
            // `await` the combinator instead.
            if status == "pending_all"
                && !{
                    let items = match map.get("items") {
                        Some(Value::Array(a)) => self.heap.array_vec(*a),
                        _ => Vec::new(),
                    };
                    items.iter().any(|i| self.is_internal_pending(i))
                }
            {
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
                self.resume_action = Some(ResumeAction::PromiseMethod {
                    method: method.to_string(),
                    args,
                });
                return match self.await_batch(kind, items)? {
                    Some(state) => Ok(PromiseMethodOutcome::Suspend(state)),
                    None => {
                        // Every element was already settled: the batch value
                        // was assembled and pushed inline. Run the method on
                        // it now, as the resume path would have.
                        let action = self.resume_action.take();
                        let Some(ResumeAction::PromiseMethod { method, args }) = action else {
                            unreachable!("recorded action consumed unexpectedly");
                        };
                        let assembled = self.pop()?;
                        let settled =
                            builtins::make_resolved_promise(assembled, &mut self.heap);
                        self.execute_promise_method(settled, &method, args)
                    }
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
        let receiver = match &promise {
            Value::Object(h) => *h,
            _ => unreachable!("non-objects returned NotAPromise above"),
        };

        // A settled receiver enqueues a microtask; a microtask-pending receiver
        // registers a reaction (enqueued when it settles). Either way the
        // method's value is a NEW pending promise the reaction will settle —
        // the handler no longer runs inline (microtask-design Stage 1).
        // A method with no applicable callable handler keeps the cheap
        // pass-through of the receiver itself.
        // Each method on a settled / pending receiver returns a NEW dependent
        // promise (`p.then() !== p`, spec), settled by an enqueued/registered
        // reaction. A non-callable handler is the pass-through reaction
        // (`Undefined` handler forwards the outcome). Only the still-special
        // statuses (`pending_call`/`pending_all`/unknown) keep the legacy
        // pass-the-receiver-through behavior.
        match method {
            "then" => {
                let pass = |h: Value| {
                    if matches!(h, Value::Function(_)) {
                        h
                    } else {
                        Value::Undefined
                    }
                };
                match status.as_str() {
                    "resolved" => {
                        let result = builtins::make_pending_promise(&mut self.heap);
                        self.microtasks.push_back(Microtask {
                            handler: pass(on_fulfilled),
                            value,
                            is_rejection: false,
                            mode: PromiseCallbackMode::WrapResult,
                            result_promise: result.clone(),
                            task: None,
                        });
                        Ok(PromiseMethodOutcome::Value(result))
                    }
                    "rejected" => {
                        // With no onRejected the rejection forwards to (and
                        // marks) the dependent promise; either way this
                        // receiver now has a consumer.
                        self.mark_rejection_handled(receiver);
                        let result = builtins::make_pending_promise(&mut self.heap);
                        self.microtasks.push_back(Microtask {
                            handler: pass(on_rejected),
                            value: reason,
                            is_rejection: true,
                            mode: PromiseCallbackMode::WrapResult,
                            result_promise: result.clone(),
                            task: None,
                        });
                        Ok(PromiseMethodOutcome::Value(result))
                    }
                    "pending" if self.is_internal_pending(&promise) => {
                        let result = builtins::make_pending_promise(&mut self.heap);
                        self.register_reaction(
                            receiver,
                            on_fulfilled,
                            on_rejected,
                            PromiseCallbackMode::WrapResult,
                            result.clone(),
                        )?;
                        Ok(PromiseMethodOutcome::Value(result))
                    }
                    _ => Ok(PromiseMethodOutcome::Value(promise)),
                }
            }
            "catch" => {
                let handler = args.first().cloned().unwrap_or(Value::Undefined);
                match status.as_str() {
                    "resolved" => {
                        let result = builtins::make_pending_promise(&mut self.heap);
                        self.microtasks.push_back(Microtask {
                            handler: Value::Undefined,
                            value,
                            is_rejection: false,
                            mode: PromiseCallbackMode::WrapResult,
                            result_promise: result.clone(),
                            task: None,
                        });
                        Ok(PromiseMethodOutcome::Value(result))
                    }
                    "rejected" => {
                        self.mark_rejection_handled(receiver);
                        let result = builtins::make_pending_promise(&mut self.heap);
                        self.microtasks.push_back(Microtask {
                            handler: if matches!(handler, Value::Function(_)) {
                                handler
                            } else {
                                Value::Undefined
                            },
                            value: reason,
                            is_rejection: true,
                            mode: PromiseCallbackMode::WrapResult,
                            result_promise: result.clone(),
                            task: None,
                        });
                        Ok(PromiseMethodOutcome::Value(result))
                    }
                    "pending" if self.is_internal_pending(&promise) => {
                        let result = builtins::make_pending_promise(&mut self.heap);
                        self.register_reaction(
                            receiver,
                            Value::Undefined,
                            handler,
                            PromiseCallbackMode::WrapResult,
                            result.clone(),
                        )?;
                        Ok(PromiseMethodOutcome::Value(result))
                    }
                    _ => Ok(PromiseMethodOutcome::Value(promise)),
                }
            }
            "finally" => {
                let handler = args.first().cloned().unwrap_or(Value::Undefined);
                // A non-callable handler degenerates to a value/rejection
                // pass-through (same observable as `finally(() => {})`).
                let (handler, mode) = if matches!(handler, Value::Function(_)) {
                    (handler, PromiseCallbackMode::PassThrough)
                } else {
                    (Value::Undefined, PromiseCallbackMode::WrapResult)
                };
                match status.as_str() {
                    "resolved" => {
                        let result = builtins::make_pending_promise(&mut self.heap);
                        self.microtasks.push_back(Microtask {
                            handler,
                            value,
                            is_rejection: false,
                            mode,
                            result_promise: result.clone(),
                            task: None,
                        });
                        Ok(PromiseMethodOutcome::Value(result))
                    }
                    "rejected" => {
                        // `finally` observes but does not consume the rejection:
                        // responsibility transfers to the result promise, which
                        // re-settles rejected (and is marked then if unhandled).
                        self.mark_rejection_handled(receiver);
                        let result = builtins::make_pending_promise(&mut self.heap);
                        self.microtasks.push_back(Microtask {
                            handler,
                            value: reason,
                            is_rejection: true,
                            mode,
                            result_promise: result.clone(),
                            task: None,
                        });
                        Ok(PromiseMethodOutcome::Value(result))
                    }
                    "pending" if self.is_internal_pending(&promise) => {
                        let result = builtins::make_pending_promise(&mut self.heap);
                        self.register_reaction(
                            receiver,
                            handler.clone(),
                            handler,
                            mode,
                            result.clone(),
                        )?;
                        Ok(PromiseMethodOutcome::Value(result))
                    }
                    _ => Ok(PromiseMethodOutcome::Value(promise)),
                }
            }
            _ => Ok(PromiseMethodOutcome::NotAPromise),
        }
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
                let env: BTreeMap<String, u64> = gen_obj.env.iter().cloned().collect();
                self.frames.push(CallFrame {
                    program_index: func_ref.program_id,
                    func_index: Some(func_ref.function_id),
                    ip: 0,
                    locals,
                    stack_base,
                    this_value: gen_obj.this_value.clone().map(|b| *b),
                    receiver_source: None,
                    boxed: BTreeMap::new(),
                    env,
                    is_field_init: false,
                    async_result: None,
                    is_constructor: false,
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
                self.restore_generator_try_frames(gen_obj.id, self.frames.len(), stack_base);
                let env: BTreeMap<String, u64> = gen_obj.env.iter().cloned().collect();
                self.frames.push(CallFrame {
                    program_index: func_ref.program_id,
                    func_index: Some(func_ref.function_id),
                    ip: suspended.ip,
                    locals: suspended.locals,
                    stack_base,
                    this_value: gen_obj.this_value.clone().map(|b| *b),
                    receiver_source: None,
                    boxed: suspended.boxed,
                    env,
                    is_field_init: false,
                    async_result: None,
                    is_constructor: false,
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
        self.generator_try_frames.remove(&gen_obj.id);
        self.store_generator(gen_obj);
        self.make_iterator_result(value, true)
    }

    /// Stash the try-frames covering a suspending generator body (frames
    /// remaining below it = `below`), depths stored relative for rebasing.
    /// Without this a `yield` inside `try` left entries on `try_stack`
    /// pointing at the popped frame.
    fn stash_generator_try_frames(&mut self, gen_id: u64, below: usize, stack_base: usize) {
        let mut entries = Vec::new();
        while matches!(self.try_stack.last(), Some(t) if t.frame_depth > below) {
            let mut t = self.try_stack.pop().unwrap();
            t.frame_depth -= below;
            t.stack_depth = t.stack_depth.saturating_sub(stack_base);
            entries.push(t);
        }
        entries.reverse();
        if entries.is_empty() {
            self.generator_try_frames.remove(&gen_id);
        } else {
            self.generator_try_frames.insert(gen_id, entries);
        }
    }

    /// Restore a generator's stashed try-frames for a resumed pull. Call with
    /// the frame count *below* the about-to-run body frame and its stack base.
    fn restore_generator_try_frames(&mut self, gen_id: u64, below: usize, stack_base: usize) {
        if let Some(entries) = self.generator_try_frames.remove(&gen_id) {
            for mut t in entries {
                t.frame_depth += below;
                t.stack_depth += stack_base;
                self.try_stack.push(t);
            }
        }
    }

    /// Begin one `gen.next(arg)` pull driven by the MAIN loop
    /// (generator-mainloop Stage 0): push the body frame — fresh, or restored
    /// from the suspended state with its try-frames — plus a
    /// [`Continuation::GeneratorNext`]. The main loop drives the body, so a
    /// tool call inside it suspends the whole VM durably. A done generator
    /// answers `{value: undefined, done: true}` immediately; a re-entrant
    /// pull (`g.next()` inside g's own body) is a TypeError, as in Node.
    fn start_generator_pull(
        &mut self,
        mut gen_obj: GeneratorObject,
        arg: Value,
        for_of: bool,
    ) -> Result<()> {
        if gen_obj.done {
            if for_of {
                let triple = self.gen_iter_triple(gen_obj.id, true)?;
                self.push(triple)?;
                self.push(Value::Undefined)?;
            } else {
                let res = self.make_iterator_result(Value::Undefined, true);
                let res = if self.current_function(gen_obj.func_ref).is_async {
                    builtins::make_resolved_promise(res, &mut self.heap)
                } else {
                    res
                };
                self.push(res)?;
            }
            return Ok(());
        }
        if self.continuations.iter().any(
            |c| matches!(c, Continuation::GeneratorNext { gen_obj: g, .. } if g.id == gen_obj.id),
        ) {
            return Err(ZapcodeError::TypeError(
                "Generator is already running".to_string(),
            ));
        }
        for (name, val) in &gen_obj.captured {
            if !self.globals.contains_key(name) {
                self.globals.insert(name.clone(), val.clone());
            }
        }
        let func_ref = gen_obj.func_ref;
        let caller_frame_depth = self.frames.len();
        match gen_obj.suspended.take() {
            None => {
                let params = self.current_function(func_ref).params.clone();
                self.tracker.push_frame();
                let mut locals = Vec::with_capacity(params.len());
                for (i, param) in params.iter().enumerate() {
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
                                .unwrap_or_else(|| {
                                    Value::Array(self.heap.alloc_array(Vec::new()))
                                });
                            let _ = i;
                            locals.push(val);
                        }
                        _ => locals.push(Value::Undefined),
                    }
                }
                let stack_base = self.stack.len();
                let env: BTreeMap<String, u64> = gen_obj.env.iter().cloned().collect();
                self.frames.push(CallFrame {
                    program_index: func_ref.program_id,
                    func_index: Some(func_ref.function_id),
                    ip: 0,
                    locals,
                    stack_base,
                    this_value: gen_obj.this_value.clone().map(|b| *b),
                    receiver_source: None,
                    boxed: BTreeMap::new(),
                    env,
                    is_field_init: false,
                    async_result: None,
                    is_constructor: false,
                });
            }
            Some(suspended) => {
                self.tracker.push_frame();
                let stack_base = self.stack.len();
                for val in &suspended.stack {
                    self.push(val.clone())?;
                }
                // The `.next(arg)` value becomes the yield expression's value.
                self.push(arg)?;
                self.restore_generator_try_frames(gen_obj.id, caller_frame_depth, stack_base);
                let env: BTreeMap<String, u64> = gen_obj.env.iter().cloned().collect();
                self.frames.push(CallFrame {
                    program_index: func_ref.program_id,
                    func_index: Some(func_ref.function_id),
                    ip: suspended.ip,
                    locals: suspended.locals,
                    stack_base,
                    this_value: gen_obj.this_value.clone().map(|b| *b),
                    receiver_source: None,
                    boxed: suspended.boxed,
                    env,
                    is_field_init: false,
                    async_result: None,
                    is_constructor: false,
                });
            }
        }
        self.continuations.push(Continuation::GeneratorNext {
            gen_obj,
            caller_frame_depth,
            callback_frame_index: self.frames.len() - 1,
            for_of,
        });
        Ok(())
    }

    /// The `IteratorNext` protocol's generator-iterator marker:
    /// `["__gen__", id, done]`.
    fn gen_iter_triple(&mut self, gen_id: u64, done: bool) -> Result<Value> {
        self.track_array_capacity(3)?;
        self.tracker.track_allocation(&self.limits)?;
        let h = self.heap.alloc_array(vec![
            Value::String(JsString::from("__gen__")),
            Value::Int(gen_id as i64),
            Value::Bool(done),
        ]);
        Ok(Value::Array(h))
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
                    // The popped frame may be a microtask handler started by
                    // an `await` of a pending chain inside this generator
                    // body — the continuation must settle its chain promise
                    // (and pops the pushed value itself), exactly as the main
                    // loop does. A tool call inside such a handler cannot
                    // suspend here.
                    if self.process_continuation()?.is_some() {
                        return Err(ZapcodeError::RuntimeError(
                            "cannot suspend inside a generator".to_string(),
                        ));
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
            // Intercept Yield only for THIS generator's body frame. A deeper
            // frame's Yield belongs to a main-loop pull of another generator
            // running inside the body (`it.next()`), handled by dispatch via
            // its GeneratorNext continuation.
            if matches!(instr, Instruction::Yield)
                && self.frames.len() == target_frame_depth + 1
            {
                self.current_frame_mut().ip += 1;
                let yielded_value = self.pop()?;
                let frame = self.frames.pop().unwrap();
                self.tracker.pop_frame();
                let frame_stack: Vec<Value> = self.stack.drain(frame.stack_base..).collect();
                let below = self.frames.len();
                self.stash_generator_try_frames(gen_obj.id, below, frame.stack_base);
                gen_obj.suspended = Some(SuspendedFrame {
                    ip: frame.ip,
                    locals: frame.locals,
                    stack: frame_stack,
                    boxed: frame.boxed,
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
                    // Settle a microtask handler's chain (see the ip-overflow
                    // arm above) — without this, an `await <chain>` inside
                    // the generator body leaks the handler's return value
                    // onto the generator's stack.
                    if self.process_continuation()?.is_some() {
                        return Err(ZapcodeError::RuntimeError(
                            "cannot suspend inside a generator".to_string(),
                        ));
                    }
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

    /// Materialize an iterable into a Vec of its elements, consuming it.
    /// Handles arrays (copied), strings (per char), built-in Set/Map (as
    /// `[k,v]` pairs), generators (driven to completion), and plain objects
    /// exposing a custom `[Symbol.iterator]()` method (the iterator protocol is
    /// invoked: call `[Symbol.iterator]()`, then repeatedly call `.next()` until
    /// a `{ done: true }` result). Used by spread and array destructuring.
    /// Items still to yield from a built-in array-iterator object (from its
    /// current cursor onward), or `None` if `val` isn't such an iterator.
    fn array_iterator_remaining(&self, val: &Value) -> Option<Vec<Value>> {
        let Value::Object(h) = val else { return None };
        let m = self.heap.object(*h)?;
        if !matches!(m.get("__array_iterator__"), Some(Value::Bool(true))) {
            return None;
        }
        let items_h = match m.get("__items__") {
            Some(Value::Array(a)) => *a,
            _ => return None,
        };
        let cursor = match m.get("__cursor__") {
            Some(Value::Int(i)) => (*i).max(0) as usize,
            _ => 0,
        };
        Some(self.heap.array_vec(items_h).into_iter().skip(cursor).collect())
    }

    fn drain_iterable(&mut self, val: Value) -> Result<Vec<Value>> {
        if let Some(items) = self.array_iterator_remaining(&val) {
            // Spread/Array.from/destructuring fully consume the iterator, so
            // advance its cursor past the end.
            if let Value::Object(h) = &val {
                let len = match self.heap.object(*h).and_then(|m| m.get("__items__").cloned()) {
                    Some(Value::Array(a)) => Some(self.heap.array(a).len()),
                    _ => None,
                };
                if let (Some(len), Some(m)) = (len, self.heap.object_mut(*h)) {
                    m.insert(Arc::from("__cursor__"), Value::Int(len as i64));
                }
            }
            return Ok(items);
        }
        match &val {
            Value::Array(a) => Ok(self.heap.array_vec(*a)),
            Value::String(s) => Ok(s
                .chars()
                .map(|c| Value::String(JsString::from(c.to_string().as_str())))
                .collect()),
            Value::Object(_) if is_set_object(&val, &self.heap) => Ok(set_items(&val, &self.heap)),
            Value::Object(_) if is_map_object(&val, &self.heap) => {
                Ok(map_entry_pairs(&val, &mut self.heap))
            }
            Value::Generator(gen_obj) => self.drain_generator(gen_obj.clone()),
            Value::Object(_) => {
                if let Some(items) = self.drain_custom_iterator(&val)? {
                    Ok(items)
                } else {
                    Err(ZapcodeError::TypeError(format!(
                        "{} is not iterable",
                        val.type_name()
                    )))
                }
            }
            other => Err(ZapcodeError::TypeError(format!(
                "{} is not iterable",
                other.type_name()
            ))),
        }
    }

    /// Drive a generator to completion, collecting each yielded value.
    fn drain_generator(&mut self, mut gen_obj: GeneratorObject) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        loop {
            self.tracker.track_allocation(&self.limits)?;
            let result = self.generator_next(gen_obj.clone(), Value::Undefined)?;
            let (value, done) = match &result {
                Value::Object(h) => {
                    let m = self.heap.object_map(*h);
                    (
                        m.get("value").cloned().unwrap_or(Value::Undefined),
                        m.get("done").is_some_and(|v| matches!(v, Value::Bool(true))),
                    )
                }
                _ => (Value::Undefined, true),
            };
            if done {
                break;
            }
            out.push(value);
            // Reload the (now-suspended) generator state so the next pull
            // resumes from where this one left off.
            let gen_key = format!("__gen_{}", gen_obj.id);
            match self.globals.get(&gen_key) {
                Some(Value::Generator(g)) => gen_obj = g.clone(),
                _ => break,
            }
        }
        Ok(out)
    }

    /// If `val` is a plain object exposing a callable `[Symbol.iterator]`,
    /// run the iterator protocol and return its elements; otherwise `None`.
    fn drain_custom_iterator(&mut self, val: &Value) -> Result<Option<Vec<Value>>> {
        let Value::Object(h) = val else {
            return Ok(None);
        };
        // A `[Symbol.iterator]` computed key stringifies to the symbol object's
        // debug form; we resolve the method by reading the well-known key.
        let iter_fn = match self.heap.object(*h) {
            Some(m) => m.get(builtins::SYMBOL_ITERATOR_KEY).cloned(),
            None => None,
        };
        let Some(iter_fn) = iter_fn else {
            return Ok(None);
        };
        if !matches!(iter_fn, Value::Function(_)) {
            return Ok(None);
        }
        // Call obj[Symbol.iterator]() (with `this` bound to the object) to
        // obtain the iterator object.
        let iterator = self.call_method_internal(&iter_fn, val.clone(), vec![])?;
        // A generator METHOD (`*[Symbol.iterator]() { … }`) returns a
        // generator object — drain it through the generator machinery.
        if let Value::Generator(gen_obj) = iterator {
            return Ok(Some(self.drain_generator(gen_obj)?));
        }
        let iter_h = match &iterator {
            Value::Object(ih) => *ih,
            _ => {
                return Err(ZapcodeError::TypeError(
                    "[Symbol.iterator]() did not return an object".into(),
                ))
            }
        };
        let next_fn = match self.heap.object(iter_h) {
            Some(m) => m.get("next").cloned(),
            None => None,
        };
        let next_fn = match next_fn {
            Some(v @ Value::Function(_)) => v,
            _ => {
                return Err(ZapcodeError::TypeError(
                    "iterator has no next() method".into(),
                ))
            }
        };
        let mut out = Vec::new();
        loop {
            self.tracker.track_allocation(&self.limits)?;
            let result = self.call_method_internal(&next_fn, iterator.clone(), vec![])?;
            let (value, done) = match &result {
                Value::Object(rh) => {
                    let m = self.heap.object_map(*rh);
                    (
                        m.get("value").cloned().unwrap_or(Value::Undefined),
                        m.get("done").is_some_and(|v| v.is_truthy()),
                    )
                }
                _ => (Value::Undefined, true),
            };
            if done {
                break;
            }
            out.push(value);
        }
        Ok(Some(out))
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
                    Constant::BigInt(v) => Value::BigInt(v.clone()),
                    Constant::String(s) => Value::String(JsString::from(s)),
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
                if let Some(&cell) = self.current_frame().env.get(name.as_ref()) {
                    let val = self
                        .cells
                        .get(cell as usize)
                        .cloned()
                        .unwrap_or(Value::Undefined);
                    self.last_global_name = Some(name.clone().to_string());
                    self.last_load_source = Some(ReceiverSource::Cell(cell));
                    self.last_place = Some(Place {
                        root: PlaceRoot::Cell(cell),
                        path: Vec::new(),
                    });
                    self.push(val)?;
                    return Ok(None);
                }
                let val = self.globals.get(name.as_ref()).cloned().unwrap_or(Value::Undefined);
                self.last_global_name = Some(name.to_string());
                // Only track receiver source for user-defined globals — builtins
                // (console, Math, JSON, etc.) contain non-serializable BuiltinMethod
                // values that would break snapshot serialization if written back.
                if Self::BUILTIN_GLOBAL_NAMES.contains(&name.as_ref()) {
                    self.last_load_source = None;
                    self.last_place = None;
                } else {
                    self.last_load_source = Some(ReceiverSource::Global(name.clone().to_string()));
                    self.last_place = Some(Place {
                        root: PlaceRoot::Global(name.to_string()),
                        path: Vec::new(),
                    });
                }
                self.push(val)?;
            }
            Instruction::StoreGlobal(name) => {
                let val = self.pop()?;
                // Route writes to a captured variable through its shared cell.
                if let Some(&cell) = self.current_frame().env.get(name.as_ref()) {
                    if let Some(slot_ref) = self.cells.get_mut(cell as usize) {
                        *slot_ref = val;
                    }
                } else {
                    self.globals.insert(name.to_string(), val);
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
                        Some(r) => int_arith_result(r),
                        None => Value::Float(*a as f64 + *b as f64),
                    },
                    (Value::Float(a), Value::Float(b)) => Value::Float(a + b),
                    (Value::Int(a), Value::Float(b)) => Value::Float(*a as f64 + b),
                    (Value::Float(a), Value::Int(b)) => Value::Float(a + *b as f64),
                    (Value::BigInt(a), Value::BigInt(b)) => Value::BigInt(a + b),
                    // Concatenate by UTF-16 code units so a trailing lone high
                    // surrogate and a leading lone low surrogate RE-PAIR into the
                    // astral char (e.g. `"\uD83D" + "\uDE00"` === "😀"), matching JS.
                    (Value::String(a), _) => {
                        let mut units = a.units().into_owned();
                        let rhs: Vec<u16> = match &right {
                            Value::String(b) => b.units().into_owned(),
                            _ => right.to_js_string(&self.heap).encode_utf16().collect(),
                        };
                        if units.len().saturating_add(rhs.len()) > 10_000_000 {
                            return Err(ZapcodeError::AllocationLimitExceeded);
                        }
                        units.extend_from_slice(&rhs);
                        Value::String(JsString::from_units(&units))
                    }
                    (_, Value::String(b)) => {
                        let mut units: Vec<u16> =
                            left.to_js_string(&self.heap).encode_utf16().collect();
                        let rhs = b.units();
                        if units.len().saturating_add(rhs.len()) > 10_000_000 {
                            return Err(ZapcodeError::AllocationLimitExceeded);
                        }
                        units.extend_from_slice(&rhs);
                        Value::String(JsString::from_units(&units))
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
                        Value::String(JsString::from(s.as_str()))
                    }
                    _ if matches!(left, Value::BigInt(_)) || matches!(right, Value::BigInt(_)) => {
                        return Err(mix_bigint_error());
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
                        Some(r) => int_arith_result(r),
                        None => Value::Float(*a as f64 - *b as f64),
                    },
                    (Value::BigInt(a), Value::BigInt(b)) => Value::BigInt(a - b),
                    _ if matches!(left, Value::BigInt(_)) || matches!(right, Value::BigInt(_)) => {
                        return Err(mix_bigint_error());
                    }
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
                        Some(r) => int_arith_result(r),
                        None => Value::Float(*a as f64 * *b as f64),
                    },
                    (Value::BigInt(a), Value::BigInt(b)) => Value::BigInt(a * b),
                    _ if matches!(left, Value::BigInt(_)) || matches!(right, Value::BigInt(_)) => {
                        return Err(mix_bigint_error());
                    }
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
                let result = match (&left, &right) {
                    (Value::BigInt(a), Value::BigInt(b)) => {
                        if num_traits::Zero::is_zero(b) {
                            return Err(ZapcodeError::RangeError("Division by zero".to_string()));
                        }
                        // BigInt division truncates toward zero.
                        Value::BigInt(a / b)
                    }
                    _ if matches!(left, Value::BigInt(_)) || matches!(right, Value::BigInt(_)) => {
                        return Err(mix_bigint_error());
                    }
                    _ => Value::Float(
                        left.to_number_heap(&self.heap) / right.to_number_heap(&self.heap),
                    ),
                };
                self.push(result)?;
            }
            Instruction::Rem => {
                let right = self.pop()?;
                let left = self.pop()?;
                let left = self.to_primitive(&left, ToPrimitiveHint::Number)?;
                let right = self.to_primitive(&right, ToPrimitiveHint::Number)?;
                let result = match (&left, &right) {
                    (Value::Int(a), Value::Int(b)) if *b != 0 => Value::Int(a % b),
                    (Value::BigInt(a), Value::BigInt(b)) => {
                        if num_traits::Zero::is_zero(b) {
                            return Err(ZapcodeError::RangeError("Division by zero".to_string()));
                        }
                        Value::BigInt(a % b)
                    }
                    _ if matches!(left, Value::BigInt(_)) || matches!(right, Value::BigInt(_)) => {
                        return Err(mix_bigint_error());
                    }
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
                let result = match (&left, &right) {
                    (Value::BigInt(a), Value::BigInt(b)) => {
                        // Exponent must be a non-negative integer that fits a u32.
                        match num_traits::ToPrimitive::to_u32(b) {
                            Some(e) => Value::BigInt(a.pow(e)),
                            None if num_traits::Signed::is_negative(b) => {
                                return Err(ZapcodeError::RangeError(
                                    "Exponent must be non-negative".to_string(),
                                ))
                            }
                            None => {
                                return Err(ZapcodeError::RangeError(
                                    "BigInt exponent is too large".to_string(),
                                ))
                            }
                        }
                    }
                    _ if matches!(left, Value::BigInt(_)) || matches!(right, Value::BigInt(_)) => {
                        return Err(mix_bigint_error());
                    }
                    _ => Value::Float(
                        left.to_number_heap(&self.heap)
                            .powf(right.to_number_heap(&self.heap)),
                    ),
                };
                self.push(result)?;
            }
            Instruction::Neg => {
                let val = self.pop()?;
                let val = self.to_primitive(&val, ToPrimitiveHint::Number)?;
                let result = match val {
                    // Negating integer 0 must produce the IEEE-754 negative zero,
                    // not the integer 0 — otherwise the sign is lost and
                    // `1 / -0` yields +Infinity and `Object.is(-0, 0)` yields
                    // true, both diverging from JS. ToString still renders it as
                    // "0" (see format_number), so this is invisible except where
                    // the sign is observable (division, Object.is/SameValue).
                    Value::Int(0) => Value::Float(-0.0),
                    // checked_neg guards the lone overflow case (-i64::MIN),
                    // which would otherwise panic and abort the host.
                    Value::Int(n) => match n.checked_neg() {
                        Some(r) => Value::Int(r),
                        None => Value::Float(-(n as f64)),
                    },
                    Value::BigInt(n) => Value::BigInt(-n),
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
                self.push(Value::Bool(self.strict_eq_heap(&left, &right)))?;
            }
            Instruction::Neq => {
                let right = self.pop()?;
                let left = self.pop()?;
                self.push(Value::Bool(!left.loose_eq(&right)))?;
            }
            Instruction::StrictNeq => {
                let right = self.pop()?;
                let left = self.pop()?;
                self.push(Value::Bool(!self.strict_eq_heap(&left, &right)))?;
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
                            obj.insert(Arc::from(k.as_str()), val);
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
                // Accessor getter: if this object carries a getter for `name`
                // (instance accessor, or a static accessor on a class object),
                // invoke it with `this` bound to the object and use its result.
                let getter = self
                    .instance_accessor(&obj, "__getters__", &name)
                    .or_else(|| self.instance_accessor(&obj, "__static_getters__", &name));
                if let Some(getter) = getter {
                    self.last_global_name = None;
                    self.last_receiver_source = None;
                    self.last_place = None;
                    let v = self.call_method_internal(&getter, obj, Vec::new())?;
                    self.push(v)?;
                    return Ok(None);
                }
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
                // Accessor setter: if this object carries a setter for `name`
                // (instance accessor, or a static accessor on a class object),
                // invoke it with `this` bound to the object instead of storing a
                // data property (accessor properties have no backing slot).
                let setter = self
                    .instance_accessor(&obj_val, "__setters__", &name)
                    .or_else(|| self.instance_accessor(&obj_val, "__static_setters__", &name));
                if let Some(setter) = setter {
                    self.call_method_internal(&setter, obj_val.clone(), vec![value])?;
                    self.push(obj_val)?;
                    return Ok(None);
                }
                match obj_val {
                    Value::Object(h) => {
                        // A frozen object, or a non-writable own property defined
                        // via Object.defineProperty({writable:false}), silently
                        // ignores the write (sloppy mode).
                        let non_writable = self
                            .heap
                            .object(h)
                            .is_some_and(|m| builtins::marker_contains(m, "__non_writable__", &name));
                        if !is_frozen_object(h, &self.heap) && !non_writable {
                            // Mutate the heap slot in place; the handle is shared so the
                            // write is visible through every alias (reference semantics).
                            if let Some(obj) = self.heap.object_mut(h) {
                                obj.insert(Arc::from(name), value);
                            }
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
                    // String index access reads the UTF-16 code unit at the index
                    // (an astral char is two units; a split yields a lone surrogate).
                    (Value::String(s), Value::Int(i)) => {
                        if *i < 0 {
                            Value::Undefined
                        } else {
                            unit_at(s, *i as usize)
                        }
                    }
                    (Value::String(s), Value::Float(f)) if *f >= 0.0 && f.fract() == 0.0 => {
                        unit_at(s, *f as usize)
                    }
                    // A string-typed numeric subscript (`"hello"["1"]`) reads the
                    // unit at that index, like JS (property-key -> integer index).
                    (Value::String(s), Value::String(key)) => match key.parse::<usize>() {
                        Ok(i) => unit_at(s, i),
                        Err(_) if key.as_ref() == "length" => Value::Int(s.len_utf16() as i64),
                        Err(_) => Value::Undefined,
                    },
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
                        // Frozen objects ignore index writes too.
                        if !is_frozen_object(*h, &self.heap) {
                            let key: Arc<str> = Arc::from(index.to_js_string(&self.heap).as_str());
                            if let Some(map) = self.heap.object_mut(*h) {
                                map.insert(key, value);
                            }
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
                    // Frozen objects are non-configurable: delete is a no-op.
                    if !is_frozen_object(*h, &self.heap) {
                        if let Some(map) = self.heap.object_mut(*h) {
                            map.shift_remove(name.as_ref());
                        }
                    }
                }
                self.push(obj)?;
            }
            Instruction::DeleteIndex => {
                let key = self.pop()?;
                let obj = self.pop()?;
                match &obj {
                    Value::Object(h) => {
                        if !is_frozen_object(*h, &self.heap) {
                            let k = key.to_js_string(&self.heap);
                            if let Some(map) = self.heap.object_mut(*h) {
                                map.shift_remove(k.as_str());
                            }
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
                // Generators, arrays, strings, Sets/Maps, and plain objects with a
                // custom `[Symbol.iterator]` are all consumed via `drain_iterable`.
                let extra: Vec<Value> = self.drain_iterable(iterable).map_err(|e| match e {
                    ZapcodeError::TypeError(msg) => {
                        ZapcodeError::TypeError(format!("{msg} (spread)"))
                    }
                    other => other,
                })?;
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
                    Value::String(k) => Arc::from(k.as_str()),
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
                        let src = Value::Object(h);
                        // Non-enumerable own properties (Object.defineProperty with
                        // enumerable:false) are not spread.
                        let non_enum = builtins::marker_list(&entries, "__non_enum__");
                        // Resolve values first (an accessor key contributes its
                        // getter's RESULT, not the stored function); invoking a
                        // getter needs &mut self, which would conflict with the
                        // object_mut borrow below.
                        let mut resolved: Vec<(Arc<str>, Value)> = Vec::new();
                        for (k, v) in entries {
                            // Spread copies own enumerable properties. Skip
                            // reserved internal markers (so `{...instance}`
                            // doesn't leak `__class__`/brands), but keep real
                            // user keys that merely start with `__`.
                            if builtins::is_internal_marker_key(&k)
                                || non_enum.iter().any(|n| n == k.as_ref())
                            {
                                continue;
                            }
                            let val = self.enumerable_value(&src, &k, v)?;
                            resolved.push((k, val));
                        }
                        if let Some(map) = self.heap.object_mut(acc) {
                            for (k, v) in resolved {
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
                // Every object inherits Object.prototype's members, so `in`
                // reports them even though our object model stores no
                // prototype chain (`'toString' in {}` is true in JS).
                fn universal_proto_member(key: &str) -> bool {
                    matches!(
                        key,
                        "toString"
                            | "toLocaleString"
                            | "valueOf"
                            | "hasOwnProperty"
                            | "isPrototypeOf"
                            | "propertyIsEnumerable"
                            | "constructor"
                    )
                }
                let result = match &right {
                    Value::Object(h) => {
                        let key = left.to_js_string(&self.heap);
                        universal_proto_member(&key)
                            || self
                                .heap
                                .object(*h)
                                .is_some_and(|m| m.contains_key(key.as_str()))
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
                                if key == "length"
                                    || universal_proto_member(&key)
                                    || is_array_method(&key)
                                {
                                    true
                                } else if let Ok(idx) = key.parse::<usize>() {
                                    idx < len
                                } else {
                                    false
                                }
                            }
                        }
                    }
                    // Functions are objects in JS; `"x" in fn` is valid (no
                    // inspectable own data keys in this subset, so `false`).
                    Value::Function(_) => false,
                    // The RHS of `in` must be an object. Node throws a catchable
                    // TypeError for primitives — e.g. `"length" in "abc"`.
                    _ => {
                        return Err(ZapcodeError::TypeError(
                            "Cannot use 'in' operator to search in a non-object".to_string(),
                        ));
                    }
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
                // The RHS of `instanceof` must be callable (a constructor). Node
                // throws a catchable TypeError otherwise — e.g. `x instanceof 5`
                // or `x instanceof ({})`. Callable values here are user functions,
                // builtin constructors (`__builtin_constructor__`), user classes
                // (`__class_name__`), and bare global fns (`__global_fn__`).
                let rhs_callable = match &right {
                    Value::Function(_) => true,
                    Value::Object(h) => self.heap.object(*h).is_some_and(|m| {
                        m.contains_key("__builtin_constructor__")
                            || m.contains_key("__class_name__")
                            || m.contains_key("__global_fn__")
                    }),
                    _ => false,
                };
                if !rhs_callable {
                    return Err(ZapcodeError::TypeError(
                        "Right-hand side of 'instanceof' is not callable".to_string(),
                    ));
                }
                // A user function value is an instance of the `Function` builtin
                // (and of `Object`). Handle it before the object-map inspection
                // below, which only covers `Value::Object` left operands.
                if let Value::Function(_) = &left {
                    let matches_fn = matches!(&right, Value::Object(h)
                        if self.heap.object(*h).is_some_and(|m| matches!(
                            m.get("__builtin_constructor__"),
                            Some(Value::String(s)) if matches!(s.as_ref(), "Function" | "Object"))));
                    self.push(Value::Bool(matches_fn))?;
                    return Ok(None);
                }
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
                                        == Some(&Value::String(JsString::from(ctor.as_ref())))
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
                            "RegExp" => {
                                matches!(&left, Value::Object(i) if has_key(*i, "__regexp__", &self.heap))
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
                            // A generator METHOD binds the receiver as `this`
                            // for the whole body's lifetime.
                            let this_value = self.last_receiver.take();
                            let gen_obj =
                                self.make_generator_object(&closure, &args, this_value);
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
                                        | "reduce" | "reduceRight" | "sort" | "toSorted"
                                        | "flatMap" => {
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
                                    // replace/replaceAll with a FUNCTION replacer must
                                    // invoke the callback per match with
                                    // (match, ...groups, offset, string) and splice in
                                    // its return value. The pure builtin can't reach the
                                    // call path, so handle it here at the dispatch layer.
                                    if matches!(method_name.as_ref(), "replace" | "replaceAll")
                                        && matches!(args.get(1), Some(Value::Function(_)))
                                    {
                                        Some(self.string_replace_with_function(
                                            &s,
                                            &method_name,
                                            &args,
                                        )?)
                                    } else {
                                        builtins::call_builtin(
                                            &Value::String(s.clone().into()),
                                            &method_name,
                                            &args,
                                            &self.limits,
                                            &mut self.stdout,
                                            &mut self.heap,
                                        )?
                                    }
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
                            "__bigint__" => {
                                let n = match receiver.as_ref() {
                                    Some(Value::BigInt(n)) => n.clone(),
                                    _ => num_bigint::BigInt::from(0),
                                };
                                builtins::call_bigint_method(&n, &method_name, &args)?
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
                                                // Main-loop pull (Stage 0): the
                                                // body frame runs like any call
                                                // — tool calls inside it can
                                                // suspend the VM durably.
                                                self.start_generator_pull(g, arg, false)?;
                                                return Ok(None);
                                            } else if self.continuations.iter().any(|c| {
                                                matches!(c, Continuation::GeneratorNext { gen_obj: g, .. }
                                                    if g.id == gen_obj.id)
                                            }) {
                                                // The registry entry is out
                                                // because THIS generator is
                                                // mid-pull: re-entrant next().
                                                return Err(ZapcodeError::TypeError(
                                                    "Generator is already running".to_string(),
                                                ));
                                            } else {
                                                let res = self.make_iterator_result(
                                                    Value::Undefined,
                                                    true,
                                                );
                                                let res = if self
                                                    .current_function(gen_obj.func_ref)
                                                    .is_async
                                                {
                                                    builtins::make_resolved_promise(
                                                        res,
                                                        &mut self.heap,
                                                    )
                                                } else {
                                                    res
                                                };
                                                Some(res)
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
                                    // A combinator batch holding microtask-pending
                                    // chain elements: settle them first — one
                                    // drained job per re-dispatch of this Call —
                                    // so the method can force the batch with every
                                    // chain element settled. This is what makes
                                    // `Promise.all([p.then(f), …]).then(cb)` work.
                                    if self.batch_has_pending_chains(&promise)
                                        && !self.batch_all_internal(&promise)
                                    {
                                        if let Some(m) = self.microtasks.pop_front() {
                                            let callee = Value::BuiltinMethod {
                                                object_name: object_name.clone(),
                                                method_name: method_name.clone(),
                                                recv: Some(Box::new(promise.clone())),
                                                place: place.clone(),
                                            };
                                            self.push(callee)?;
                                            for a in &args {
                                                self.push(a.clone())?;
                                            }
                                            self.current_frame_mut().ip -= 1;
                                            self.start_microtask(m)?;
                                            return Ok(None);
                                        }
                                    }
                                    match self
                                        .execute_promise_method(promise, &method_name, args)?
                                    {
                                        PromiseMethodOutcome::Value(v) => Some(v),
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
                            "__array_iterator__" => {
                                // `.next()` on a built-in iterator: yield the item
                                // at the cursor as { value, done } and advance it.
                                let _ = &place;
                                if let Some(Value::Object(it)) = receiver {
                                    let (items_h, cursor) = match self.heap.object(it) {
                                        Some(m) => {
                                            let ih = match m.get("__items__") {
                                                Some(Value::Array(a)) => Some(*a),
                                                _ => None,
                                            };
                                            let c = match m.get("__cursor__") {
                                                Some(Value::Int(i)) => (*i).max(0) as usize,
                                                _ => 0,
                                            };
                                            (ih, c)
                                        }
                                        None => (None, 0),
                                    };
                                    match items_h {
                                        Some(ih) => {
                                            let items = self.heap.array_vec(ih);
                                            if cursor < items.len() {
                                                let v = items[cursor].clone();
                                                if let Some(m) = self.heap.object_mut(it) {
                                                    m.insert(
                                                        Arc::from("__cursor__"),
                                                        Value::Int((cursor + 1) as i64),
                                                    );
                                                }
                                                Some(self.make_iterator_result(v, false))
                                            } else {
                                                Some(self.make_iterator_result(Value::Undefined, true))
                                            }
                                        }
                                        None => Some(self.make_iterator_result(Value::Undefined, true)),
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
                                    if let Some(new_ms) = date_setter_millis(
                                        &self.heap.object_map(date),
                                        &method_name,
                                        &args,
                                    ) {
                                        // Mutate the shared slot in place; the
                                        // setter returns the new timestamp,
                                        // like JS.
                                        if let Some(m) = self.heap.object_mut(date) {
                                            m.insert(
                                                Arc::from("__date_ms__"),
                                                Value::Float(new_ms),
                                            );
                                        }
                                        Some(Value::Float(new_ms))
                                    } else {
                                        let map = self.heap.object_map(date);
                                        execute_date_method(&map, &method_name)
                                    }
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
                                // Charge the produced array against the memory limit
                                // before invoking the mapFn per element, so a giant
                                // `{length:n}` source can't allocate untracked.
                                self.track_array_capacity(src.len())?;
                                let map_fn = args[1].clone();
                                let mut out = Vec::with_capacity(src.len());
                                for (i, item) in src.iter().enumerate() {
                                    out.push(self.call_element_callback(&map_fn, item, i)?);
                                }
                                let h = self.heap.alloc_array(out);
                                Some(Value::Array(h))
                            }
                            // Handing a promise to a combinator attaches a
                            // reaction to it in real JS, so an already-rejected
                            // element's rejection is CONSUMED by the combinator
                            // (`Promise.allSettled([rejected])` reports it; it
                            // must not surface as an unhandled rejection).
                            // Clear the marks, then run the normal builtin.
                            "Promise"
                                if matches!(
                                    method_name.as_ref(),
                                    "all" | "allSettled" | "race" | "any"
                                ) =>
                            {
                                if let Some(Value::Array(h)) = args.first() {
                                    for item in self.heap.array_vec(*h) {
                                        if let Value::Object(eh) = item {
                                            self.mark_rejection_handled(eh);
                                        }
                                    }
                                }
                                builtins::call_global_method(
                                    "Promise",
                                    &method_name,
                                    &args,
                                    &mut self.stdout,
                                    &mut self.heap,
                                )?
                            }
                            // Object.groupBy / Map.groupBy take a guest key
                            // callback, so they group here (the pure builtin
                            // layer cannot call closures). The callback runs
                            // through the internal drive, like a JSON
                            // replacer — a tool call inside it cannot suspend.
                            "Object" | "Map" if method_name.as_ref() == "groupBy" => {
                                let items = match args.first() {
                                    Some(Value::Array(h)) => self.heap.array_vec(*h),
                                    _ => Vec::new(),
                                };
                                let callback =
                                    args.get(1).cloned().unwrap_or(Value::Undefined);
                                let mut keyed = Vec::with_capacity(items.len());
                                for (i, item) in items.iter().enumerate() {
                                    let key = self.call_function_internal(
                                        &callback,
                                        vec![item.clone(), Value::Int(i as i64)],
                                    )?;
                                    keyed.push((key, item.clone()));
                                }
                                let value = if object_name.as_ref() == "Object" {
                                    // Plain object keyed by ToPropertyKey; group
                                    // order is first-occurrence, like Node.
                                    let mut groups: IndexMap<Arc<str>, Vec<Value>> =
                                        IndexMap::new();
                                    for (key, item) in keyed {
                                        let k = key.to_js_string(&self.heap);
                                        groups.entry(Arc::from(k.as_str())).or_default().push(item);
                                    }
                                    self.tracker.track_allocation(&self.limits)?;
                                    let mut obj = IndexMap::new();
                                    for (k, group) in groups {
                                        self.track_array_capacity(group.len())?;
                                        let h = self.heap.alloc_array(group);
                                        obj.insert(k, Value::Array(h));
                                    }
                                    Some(Value::Object(self.heap.alloc_object(obj)))
                                } else {
                                    // Map keyed by SameValueZero identity.
                                    let mut groups: Vec<(Value, Vec<Value>)> = Vec::new();
                                    for (key, item) in keyed {
                                        match groups.iter_mut().find(|(k, _)| {
                                            builtins::same_value_zero(k, &key)
                                        }) {
                                            Some((_, g)) => g.push(item),
                                            None => groups.push((key, vec![item])),
                                        }
                                    }
                                    self.tracker.track_allocation(&self.limits)?;
                                    let entries = groups
                                        .into_iter()
                                        .map(|(k, g)| {
                                            let gh = self.heap.alloc_array(g);
                                            let mut em = IndexMap::new();
                                            em.insert(Arc::from("key"), k);
                                            em.insert(Arc::from("value"), Value::Array(gh));
                                            Value::Object(self.heap.alloc_object(em))
                                        })
                                        .collect();
                                    Some(make_map_object(entries, &mut self.heap))
                                };
                                value
                            }
                            // JSON.stringify must honor a user `toJSON()` on plain
                            // objects and a FUNCTION replacer; JSON.parse must invoke a
                            // reviver. All three need the guest-closure call path, so
                            // route through the VM here rather than the pure builtin.
                            "JSON" if method_name.as_ref() == "stringify" => {
                                Some(self.json_stringify(&args)?)
                            }
                            "JSON"
                                if method_name.as_ref() == "parse"
                                    && matches!(args.get(1), Some(Value::Function(_))) =>
                            {
                                Some(self.json_parse_with_reviver(&args)?)
                            }
                            // Array.from(arrayLike) / Array.of(...) allocate an
                            // array sized from guest-controlled input. Charge the
                            // length against the memory limit *before* the builtin
                            // materializes the Vec, so a huge `{length:n}` can't
                            // bypass the limit (untracked multi-GB allocation).
                            "Array"
                                if method_name.as_ref() == "from"
                                    || method_name.as_ref() == "of" =>
                            {
                                let len = if method_name.as_ref() == "of" {
                                    args.len()
                                } else {
                                    builtins::array_from_source_len(
                                        args.first().unwrap_or(&Value::Undefined),
                                        &self.heap,
                                    )
                                };
                                self.track_array_capacity(len)?;
                                builtins::call_global_method(
                                    "Array",
                                    &method_name,
                                    &args,
                                    &mut self.stdout,
                                    &mut self.heap,
                                )?
                            }
                            // Object.values/entries on an object with accessors
                            // must invoke getters for the values; the pure builtin
                            // can't call guest closures, so resolve here. (Without
                            // accessors, fall through to the builtin unchanged.)
                            "Object"
                                if matches!(method_name.as_ref(), "values" | "entries")
                                    && self.has_accessors(
                                        args.first().unwrap_or(&Value::Undefined),
                                    ) =>
                            {
                                let obj = args[0].clone();
                                let Value::Object(h) = &obj else { unreachable!() };
                                let keys =
                                    builtins::ordered_visible_keys(&self.heap.object_map(*h));
                                let want_entries = method_name.as_ref() == "entries";
                                let mut out = Vec::with_capacity(keys.len());
                                for k in keys {
                                    let stored = self
                                        .heap
                                        .object(*h)
                                        .and_then(|m| m.get(k.as_ref()).cloned())
                                        .unwrap_or(Value::Undefined);
                                    let val = self.enumerable_value(&obj, &k, stored)?;
                                    if want_entries {
                                        let pair = self.heap.alloc_array(vec![
                                            Value::String(k.clone().into()),
                                            val,
                                        ]);
                                        out.push(Value::Array(pair));
                                    } else {
                                        out.push(val);
                                    }
                                }
                                Some(Value::Array(self.heap.alloc_array(out)))
                            }
                            // Object.assign with an accessor-bearing source copies
                            // the getter's RESULT (spread semantics), not the fn.
                            "Object"
                                if method_name.as_ref() == "assign"
                                    && args.iter().skip(1).any(|s| self.has_accessors(s)) =>
                            {
                                let target = match args.first() {
                                    Some(Value::Object(h)) => *h,
                                    _ => self.heap.alloc_object(IndexMap::new()),
                                };
                                for src in args.iter().skip(1).cloned().collect::<Vec<_>>() {
                                    let Value::Object(sh) = &src else { continue };
                                    let keys =
                                        builtins::ordered_visible_keys(&self.heap.object_map(*sh));
                                    for k in keys {
                                        let stored = self
                                            .heap
                                            .object(*sh)
                                            .and_then(|m| m.get(k.as_ref()).cloned())
                                            .unwrap_or(Value::Undefined);
                                        let val = self.enumerable_value(&src, &k, stored)?;
                                        if let Some(m) = self.heap.object_mut(target) {
                                            m.insert(k, val);
                                        }
                                    }
                                }
                                Some(Value::Object(target))
                            }
                            // Object.fromEntries accepts ANY iterable of [k, v]
                            // pairs (Map/Set/array-iterator/custom), not just a
                            // plain array. Drain non-array iterables here (the pure
                            // builtin only knows arrays), then reuse the builtin.
                            "Object"
                                if method_name.as_ref() == "fromEntries"
                                    && !matches!(args.first(), Some(Value::Array(_))) =>
                            {
                                let src = args.first().cloned().unwrap_or(Value::Undefined);
                                let pairs = self.drain_iterable(src)?;
                                let arr = self.heap.alloc_array(pairs);
                                builtins::call_global_method(
                                    "Object",
                                    "fromEntries",
                                    &[Value::Array(arr)],
                                    &mut self.stdout,
                                    &mut self.heap,
                                )?
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
                            Some(Value::String(s)) => Arc::<str>::from(s.as_str()),
                            _ => Arc::from(""),
                        };
                        // setTimeout/clearTimeout/queueMicrotask mutate VM
                        // scheduling state — dispatch here, not in builtins.
                        if let Some(result) = self.call_scheduler_global(kind.as_ref(), &args) {
                            let value = result?;
                            self.push(value)?;
                            return Ok(None);
                        }
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
                    // `RegExp(pattern, flags)` called WITHOUT `new` behaves like the
                    // constructor (JS allows both forms).
                    Value::Object(h)
                        if self.heap.object(h).is_some_and(|m| {
                            matches!(m.get("__builtin_constructor__"),
                                Some(Value::String(s)) if s.as_ref() == "RegExp")
                        }) =>
                    {
                        let r = self.construct_regexp(&args)?;
                        self.push(r)?;
                    }
                    // A Promise-executor capability: calling `resolve(v)` /
                    // `reject(r)` settles the constructed promise (no-op once
                    // settled). The call expression evaluates to undefined.
                    Value::Object(h)
                        if self
                            .heap
                            .object(h)
                            .is_some_and(|m| m.contains_key("__promise_capability__")) =>
                    {
                        let (promise, is_reject) = {
                            let m = self.heap.object_map(h);
                            (
                                m.get("__promise_capability__")
                                    .cloned()
                                    .unwrap_or(Value::Undefined),
                                matches!(
                                    m.get("__capability_reject__"),
                                    Some(Value::Bool(true))
                                ),
                            )
                        };
                        let arg = args.first().cloned().unwrap_or(Value::Undefined);
                        if let Some(state) = self.settle_capability(&promise, arg, is_reject)? {
                            return Ok(Some(state));
                        }
                        self.push(Value::Undefined)?;
                        self.last_receiver = None;
                    }
                    // `Function(...)` called WITHOUT `new` is equally forbidden;
                    // the rejection is a catchable runtime sandbox violation.
                    Value::Object(h)
                        if self.heap.object(h).is_some_and(|m| {
                            matches!(m.get("__builtin_constructor__"),
                                Some(Value::String(s)) if s.as_ref() == "Function")
                        }) =>
                    {
                        return Err(ZapcodeError::SandboxViolation(
                            "Function constructor is forbidden in the sandbox".to_string(),
                        ));
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
                if !self.external_functions.contains(name.as_ref() as &str) {
                    return Err(ZapcodeError::UnknownExternalFunction(name.to_string()));
                }
                let mut args = Vec::with_capacity(arg_count);
                for _ in 0..arg_count {
                    args.push(self.pop()?);
                }
                args.reverse();
                // Suspend execution
                let mut args = args;
                let snapshot = ZapcodeSnapshot::capture_with_values(self, &mut args)?;
                return Ok(Some(VmState::Suspended {
                    function_name: name.to_string(),
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
                if !self.external_functions.contains(name.as_ref() as &str) {
                    return Err(ZapcodeError::UnknownExternalFunction(name.to_string()));
                }
                let mut args = Vec::with_capacity(arg_count);
                for _ in 0..arg_count {
                    args.push(self.pop()?);
                }
                args.reverse();
                let id = self.next_call_id;
                self.next_call_id += 1;
                self.pending_calls.push(PendingExternalCall {
                    id,
                    name: name.to_string(),
                    args,
                });
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
                obj.insert(Arc::from("status"), Value::String(JsString::from("pending_all")));
                obj.insert(Arc::from("__batch_kind__"), Value::String(JsString::from(kind.as_str())));
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
                obj.insert(Arc::from("status"), Value::String(JsString::from("pending_call")));
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
                            .map(|c| Value::String(JsString::from(c.to_string().as_str())))
                            .collect();
                        let iter_obj = iter_from_items(chars, &mut self.heap);
                        self.push(iter_obj)?;
                    }
                    Value::Generator(gen_obj) => {
                        let iter_obj = Value::Array(self.heap.alloc_array(vec![
                            Value::String(JsString::from("__gen__")),
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
                    // A built-in array-iterator (Map/Set/array .keys()/.values()/
                    // .entries()): iterate its items from the current cursor.
                    Value::Object(_) if is_array_iterator(&val, &self.heap) => {
                        let items = self.array_iterator_remaining(&val).unwrap_or_default();
                        let iter_obj = iter_from_items(items, &mut self.heap);
                        self.push(iter_obj)?;
                    }
                    // A plain object exposing a custom `[Symbol.iterator]()`:
                    // run the iterator protocol to materialize its elements, then
                    // hand `for...of` a normal array-iterator over them.
                    Value::Object(_) => {
                        match self.drain_custom_iterator(&val)? {
                            Some(items) => {
                                let iter_obj = iter_from_items(items, &mut self.heap);
                                self.push(iter_obj)?;
                            }
                            None => {
                                return Err(ZapcodeError::TypeError(format!(
                                    "{} is not iterable",
                                    val.type_name()
                                )));
                            }
                        }
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
                            if let Some(Value::Generator(g)) = self.globals.remove(&gen_key) {
                                // Main-loop pull (Stage 1): the body frame
                                // runs like any call — tool calls inside a
                                // for…of-driven generator suspend durably.
                                self.start_generator_pull(g, Value::Undefined, true)?;
                            } else if self.continuations.iter().any(|c| {
                                matches!(c, Continuation::GeneratorNext { gen_obj: g, .. }
                                    if g.id == gen_id)
                            }) {
                                return Err(ZapcodeError::TypeError(
                                    "Generator is already running".to_string(),
                                ));
                            } else {
                                let done_iter = self.gen_iter_triple(gen_id, true)?;
                                self.push(done_iter)?;
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
            Instruction::IterableToArray => {
                // Used by array destructuring (`const [a,b] = x`). Only GENERATORS
                // and plain objects with a custom `[Symbol.iterator]` need to be
                // materialized into an array so the positional element reads can
                // index them. Arrays/strings/Sets/Maps and lenient non-iterables
                // (number/null/plain object) are left UNCHANGED so the existing
                // index-based destructure path keeps its current behavior.
                let val = self.pop()?;
                match &val {
                    Value::Generator(gen_obj) => {
                        let items = self.drain_generator(gen_obj.clone())?;
                        let h = self.heap.alloc_array(items);
                        self.push(Value::Array(h))?;
                    }
                    Value::Object(_) => match self.drain_custom_iterator(&val)? {
                        Some(items) => {
                            let h = self.heap.alloc_array(items);
                            self.push(Value::Array(h))?;
                        }
                        None => self.push(val)?,
                    },
                    _ => self.push(val)?,
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
                                // A user class value is callable (constructor), so
                                // `typeof Class === "function"` like JS.
                                || m.contains_key("__class_name__")
                                // The resolve/reject capabilities handed to a
                                // `new Promise(executor)` are callable markers.
                                || m.contains_key("__promise_capability__")
                        }) =>
                    {
                        "function"
                    }
                    other => other.type_name(),
                };
                self.push(Value::String(JsString::from(type_str)))?;
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
                    Value::Int(n) => match n.checked_add(1) {
                        Some(r) => int_arith_result(r),
                        None => Value::Float(n as f64 + 1.0),
                    },
                    _ => Value::Float(val.to_number_heap(&self.heap) + 1.0),
                };
                self.push(result)?;
            }
            Instruction::Decrement => {
                let val = self.pop()?;
                let result = match val {
                    Value::Int(n) => match n.checked_sub(1) {
                        Some(r) => int_arith_result(r),
                        None => Value::Float(n as f64 - 1.0),
                    },
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
                self.push(Value::String(JsString::from(result.as_str())))?;
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
                // A main-loop pull (Continuation::GeneratorNext) detaches the
                // body frame back into the generator object and answers the
                // pull inline. (The legacy nested driver intercepts Yield
                // before dispatch, so reaching here with no pull in flight is
                // yield outside a generator.)
                if !matches!(
                    self.continuations.last(),
                    Some(Continuation::GeneratorNext { .. })
                ) {
                    return Err(ZapcodeError::RuntimeError(
                        "yield can only be used inside a generator function".to_string(),
                    ));
                }
                let Some(Continuation::GeneratorNext {
                    mut gen_obj,
                    for_of,
                    ..
                }) = self.continuations.pop()
                else {
                    unreachable!("checked above");
                };
                let yielded = self.pop()?;
                let frame = self.frames.pop().unwrap();
                self.tracker.pop_frame();
                let below = self.frames.len();
                let stack_tail: Vec<Value> = self.stack.drain(frame.stack_base..).collect();
                self.stash_generator_try_frames(gen_obj.id, below, frame.stack_base);
                // dispatch() pre-incremented ip, so the frame resumes just
                // past this Yield.
                let gen_id = gen_obj.id;
                let is_async_gen = self.current_function(gen_obj.func_ref).is_async;
                gen_obj.suspended = Some(SuspendedFrame {
                    ip: frame.ip,
                    locals: frame.locals,
                    stack: stack_tail,
                    boxed: frame.boxed,
                });
                gen_obj.done = false;
                self.store_generator(gen_obj);
                if for_of {
                    let triple = self.gen_iter_triple(gen_id, false)?;
                    self.push(triple)?;
                    self.push(yielded)?;
                } else {
                    let res = self.make_iterator_result(yielded, false);
                    // An async generator's next() answers a Promise of the
                    // iterator result (Node), so `it.next().then(...)` and
                    // `await it.next()` both work.
                    let res = if is_async_gen {
                        builtins::make_resolved_promise(res, &mut self.heap)
                    } else {
                        res
                    };
                    self.push(res)?;
                }
            }

            Instruction::Await => {
                // Check if the value on the stack is a Promise object.
                // Inside an async function body, `await` *parks the call*
                // (microtask-design Stage 3): the frame detaches into an
                // AsyncTask, the caller receives the call's pending result
                // promise, and a ResumeAsync microtask (or a reaction on the
                // awaited promise) continues the body later — so `await`
                // always yields a tick. At top level, `await` keeps the
                // Stage-1 inline semantics (unwrap settled values; drain the
                // queue for a pending chain). Host-call promises
                // (`pending_call`/`pending_all`) suspend the whole VM in both
                // positions — that is the durable-execution boundary.
                let val = self.pop()?;
                if matches!(val, Value::Pending(_)) {
                    // A deferred call only ever lives inside a Promise.all batch;
                    // awaiting one bare is a compiler invariant violation.
                    return Err(ZapcodeError::RuntimeError(
                        "internal error: awaited a deferred call outside Promise.all".to_string(),
                    ));
                }
                let in_async_body = self.current_frame_is_async_body();
                if builtins::is_promise(&val, &self.heap) {
                    if let Value::Object(h) = &val {
                        let map = self.heap.object_map(*h);
                        let status = map.get("status").cloned().unwrap_or(Value::Undefined);
                        match status {
                            Value::String(s) if s.as_ref() == "resolved" => {
                                let inner = map.get("value").cloned().unwrap_or(Value::Undefined);
                                if builtins::is_promise(&inner, &self.heap) {
                                    // Resolved *with* a promise (e.g. a `.then`
                                    // callback returned `Promise.all(...)`):
                                    // adopt by re-awaiting the inner promise.
                                    if in_async_body {
                                        // Park ON the Await so the resumed task
                                        // re-awaits the delivered inner promise.
                                        self.current_frame_mut().ip -= 1;
                                        self.park_and_tick(inner, false)?;
                                    } else {
                                        self.push(inner)?;
                                        self.current_frame_mut().ip -= 1;
                                    }
                                } else if in_async_body {
                                    self.park_and_tick(inner, false)?;
                                } else if !self.microtasks.is_empty()
                                    && !map.contains_key("__await_tick__")
                                {
                                    // Top-level await yields a tick too: the
                                    // queued reactions run before this
                                    // continuation (Node module TLA order).
                                    // (A settled tick sentinel delivers
                                    // inline — its tick already happened.)
                                    self.requeue_top_level_await(inner, false)?;
                                } else {
                                    self.push(inner)?;
                                }
                            }
                            Value::String(s) if s.as_ref() == "rejected" => {
                                // The rejection is consumed here: it surfaces at
                                // the await site (guest try/catch or the host).
                                self.mark_rejection_handled(*h);
                                let reason = map.get("reason").cloned().unwrap_or(Value::Undefined);
                                if in_async_body {
                                    self.park_and_tick(reason, true)?;
                                } else if !self.microtasks.is_empty()
                                    && !map.contains_key("__await_tick__")
                                {
                                    // Queued reactions run before the rethrow
                                    // (the sentinel re-await rejects then).
                                    self.requeue_top_level_await(reason, true)?;
                                } else {
                                    // Rethrow the *original* reason (identity
                                    // preserved for guest catch) via
                                    // pending_throw; the message below is what
                                    // the host sees when nothing catches.
                                    let msg = reason.to_js_string(&self.heap);
                                    self.pending_throw = Some(reason);
                                    return Err(ZapcodeError::RuntimeError(format!(
                                        "Unhandled promise rejection: {}",
                                        msg
                                    )));
                                }
                            }
                            Value::String(s) if s.as_ref() == "pending" => {
                                if in_async_body {
                                    if self.is_internal_pending(&val) {
                                        // Park the body; the promise's settle
                                        // enqueues the ResumeAsync.
                                        let id = self.detach_async_task()?;
                                        self.register_task_reaction(*h, id)?;
                                    } else {
                                        // A never-settling promise (e.g.
                                        // `Promise.race([])`): the body parks
                                        // forever — its result promise simply
                                        // never settles, like Node exiting
                                        // with a pending await.
                                        let _ = self.detach_async_task()?;
                                    }
                                } else if let Some(m) = self.microtasks.pop_front() {
                                    // Top level: run queued microtasks one per
                                    // pass (re-dispatching this Await) until
                                    // the chain settles. Callback frames run
                                    // in the main loop, so a tool call inside
                                    // a handler suspends normally — with this
                                    // frame's ip parked on the Await.
                                    self.push(val)?;
                                    self.current_frame_mut().ip -= 1;
                                    self.start_microtask(m)?;
                                } else if let Some(t) = self.pop_due_timer() {
                                    // Microtasks dry but timers pending: a
                                    // top-level await on a timer-settled
                                    // promise (`await new Promise(r =>
                                    // setTimeout(r, ms))`) fires the next
                                    // macrotask and re-awaits.
                                    self.push(val)?;
                                    self.current_frame_mut().ip -= 1;
                                    self.start_timer_job(t)?;
                                } else {
                                    // Queue is dry and the promise can never
                                    // settle (e.g. `Promise.race([])`) — pass
                                    // the still-pending promise through, the
                                    // pinned pre-microtask behavior.
                                    self.push(val)?;
                                }
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
                                // race/any settle with the FIRST element to
                                // settle (race) / fulfill (any) in tick order,
                                // scanning between drained jobs — as in Node,
                                // a shallower chain in a later slot beats a
                                // deeper chain in an earlier one.
                                if matches!(kind, BatchKind::Race | BatchKind::Any) {
                                    let winner = items.iter().enumerate().find_map(|(i, item)| {
                                        self.batch_element_outcome(item).and_then(
                                            |(outcome, rejected)| {
                                                if rejected && matches!(kind, BatchKind::Any) {
                                                    None // any skips rejections
                                                } else {
                                                    Some((i, outcome, rejected))
                                                }
                                            },
                                        )
                                    });
                                    if let Some((idx, outcome, rejected)) = winner {
                                        // Losers' future rejections are the
                                        // combinator's to absorb, not unhandled.
                                        for (i, item) in items.iter().enumerate() {
                                            if i == idx {
                                                continue;
                                            }
                                            if let Value::Object(lh) = item {
                                                self.mark_rejection_handled(*lh);
                                                if self.is_internal_pending(item) {
                                                    self.register_sink_reaction(*lh)?;
                                                }
                                            }
                                        }
                                        if rejected {
                                            if let Value::Object(wh) = &items[idx] {
                                                self.mark_rejection_handled(*wh);
                                            }
                                            let msg = outcome.to_js_string(&self.heap);
                                            self.pending_throw = Some(outcome);
                                            return Err(ZapcodeError::RuntimeError(format!(
                                                "Unhandled promise rejection: {}",
                                                msg
                                            )));
                                        }
                                        self.push(outcome)?;
                                        return Ok(None);
                                    }
                                }
                                // Settle microtask-pending elements (`.then`
                                // chains inside the combinator) before the batch
                                // is classified/assembled — one microtask per
                                // pass, re-dispatching this Await.
                                if items.iter().any(|i| self.is_internal_pending(i)) {
                                    if let Some(m) = self.microtasks.pop_front() {
                                        self.push(val)?;
                                        self.current_frame_mut().ip -= 1;
                                        self.start_microtask(m)?;
                                        return Ok(None);
                                    }
                                }
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
                                // Already settled (awaited before): reuse the
                                // value — but still yield the await tick.
                                if let Some(cached) = self.resolved.get(&id).cloned() {
                                    if in_async_body {
                                        self.park_and_tick(cached, false)?;
                                    } else if !self.microtasks.is_empty() {
                                        self.requeue_top_level_await(cached, false)?;
                                    } else {
                                        self.push(cached)?;
                                    }
                                } else {
                                    self.resume_action = Some(ResumeAction::CacheValue { id });
                                    return self.suspend_on_pending_call(id);
                                }
                            }
                            _ => {
                                // Unknown status — treat as a plain value.
                                if in_async_body {
                                    self.park_and_tick(val, false)?;
                                } else if !self.microtasks.is_empty() {
                                    self.requeue_top_level_await(val, false)?;
                                } else {
                                    self.push(val)?;
                                }
                            }
                        }
                    } else if in_async_body {
                        self.park_and_tick(val, false)?;
                    } else if !self.microtasks.is_empty() {
                        self.requeue_top_level_await(val, false)?;
                    } else {
                        self.push(val)?;
                    }
                } else if in_async_body {
                    // `await` of a non-promise still yields one tick (spec).
                    self.park_and_tick(val, false)?;
                } else if !self.microtasks.is_empty() {
                    self.requeue_top_level_await(val, false)?;
                } else {
                    // Not a promise — pass through (await on non-promise returns the value)
                    self.push(val)?;
                }
            }

            // Classes
            Instruction::CreateClass(spec) => {
                // Delegated to a dedicated method so this large arm does not bloat
                // the `dispatch` stack frame (which recurses through ToPrimitive).
                self.create_class(CreateClassParts {
                    name: &spec.name,
                    n_methods: spec.n_methods,
                    n_statics: spec.n_statics,
                    n_getters: spec.n_getters,
                    n_setters: spec.n_setters,
                    n_static_getters: spec.n_static_getters,
                    n_static_setters: spec.n_static_setters,
                    n_fields: spec.n_fields,
                    n_static_fields: spec.n_static_fields,
                    has_super: spec.has_super,
                })?;
            }

            Instruction::Construct(arg_count) => {
                let mut args = Vec::with_capacity(arg_count);
                for _ in 0..arg_count {
                    args.push(self.pop()?);
                }
                args.reverse();

                let callee = self.pop()?;

                // The result of `new X(...)` is a fresh instance, never the
                // global `X`. Consume the builtin-global shortcut here so a
                // chained read like `new Error("e").cause` resolves against the
                // instance (own props / undefined) instead of mistaking it for a
                // method on the global `Error` and returning a phantom function.
                self.last_global_name = None;

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
                            "Promise" => {
                                // `new Promise(executor)`: run the executor
                                // synchronously with serializable resolve /
                                // reject capability objects that settle the
                                // new pending promise (microtask machinery
                                // takes it from there). The executor's frame
                                // is driven by the main loop, so a tool call
                                // inside it suspends normally; its return
                                // value is discarded and the `new` expression
                                // evaluates to the promise (see
                                // `Continuation::PromiseExecutor`).
                                let executor = args.first().cloned().unwrap_or(Value::Undefined);
                                let Value::Function(closure) = executor else {
                                    return Err(ZapcodeError::TypeError(format!(
                                        "Promise resolver {} is not a function",
                                        args.first()
                                            .cloned()
                                            .unwrap_or(Value::Undefined)
                                            .to_js_string(&self.heap)
                                    )));
                                };
                                let promise = builtins::make_pending_promise(&mut self.heap);
                                let mut make_capability = |reject: bool, heap: &mut Heap| {
                                    let mut cap = IndexMap::new();
                                    cap.insert(
                                        Arc::from("__promise_capability__"),
                                        promise.clone(),
                                    );
                                    cap.insert(
                                        Arc::from("__capability_reject__"),
                                        Value::Bool(reject),
                                    );
                                    Value::Object(heap.alloc_object(cap))
                                };
                                let resolve = make_capability(false, &mut self.heap);
                                let reject = make_capability(true, &mut self.heap);
                                let caller_frame_depth = self.frames.len();
                                self.push_call_frame(&closure, &[resolve, reject], None)?;
                                self.continuations.push(Continuation::PromiseExecutor {
                                    promise,
                                    caller_frame_depth,
                                    callback_frame_index: self.frames.len() - 1,
                                });
                                // The main loop drives the executor frame; the
                                // continuation pushes the promise when it pops.
                                return Ok(None);
                            }
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
                            // WeakMap/WeakSet are backed by the same machinery as
                            // Map/Set (get/set/has/delete and add/has/delete work).
                            // We don't model the weak-reference GC semantics; they're
                            // also not iterable in JS, which matches our use.
                            "WeakMap" => {
                                let arg = args.first().cloned().unwrap_or(Value::Undefined);
                                let entries = build_map_entries(&arg, &mut self.heap);
                                let m = make_map_object(entries, &mut self.heap);
                                self.push(m)?;
                                return Ok(None);
                            }
                            "WeakSet" => {
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
                            "RegExp" => {
                                let r = self.construct_regexp(&args)?;
                                self.push(r)?;
                                return Ok(None);
                            }
                            "Error" | "TypeError" | "RangeError" | "SyntaxError"
                            | "ReferenceError" => {
                                let msg = args
                                    .first()
                                    .map(|v| v.to_js_string(&self.heap))
                                    .unwrap_or_default();
                                let e = make_error_object(name.as_ref(), &msg, &mut self.heap);
                                // ES2022 `new Error(msg, { cause })`: if the
                                // options bag has an own `cause` (even undefined),
                                // it becomes the error's `cause` property.
                                let cause = match args.get(1) {
                                    Some(Value::Object(opts)) => {
                                        self.heap.object(*opts).and_then(|m| m.get("cause").cloned())
                                    }
                                    _ => None,
                                };
                                if let (Some(cause), Value::Object(em)) = (cause, &e) {
                                    if let Some(map) = self.heap.object_mut(*em) {
                                        map.insert(Arc::from("cause"), cause);
                                    }
                                }
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
                            // `new Function(...)` is forbidden — but the rejection is
                            // a (catchable) runtime sandbox violation, not a fatal
                            // parse-time abort. The `Function` global itself exists so
                            // `typeof`/`instanceof` work.
                            "Function" => {
                                return Err(ZapcodeError::SandboxViolation(
                                    "Function constructor is forbidden in the sandbox"
                                        .to_string(),
                                ));
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
                        // A subclass of a built-in Error gets the `__error__` brand so
                        // `e instanceof Error` is true and it stringifies as an error.
                        // The default `name` is the error base (e.g. "Error"); a
                        // constructor that assigns `this.name` overrides it.
                        if let Some(Value::String(base)) = class_obj.get("__error_base__") {
                            instance.insert(Arc::from("__error__"), Value::Bool(true));
                            instance
                                .entry(Arc::from("name"))
                                .or_insert_with(|| Value::String(base.clone().into()));
                        }
                        // Carry accessor descriptors onto the instance so a later
                        // GetProperty/SetProperty invokes the getter/setter body.
                        if let Some(g @ Value::Object(_)) = class_obj.get("__getters__") {
                            instance.insert(Arc::from("__getters__"), g.clone());
                        }
                        if let Some(s @ Value::Object(_)) = class_obj.get("__setters__") {
                            instance.insert(Arc::from("__setters__"), s.clone());
                        }

                        // Collect this class's own instance field initializers so we
                        // can run them on the new instance below (before the
                        // constructor body for a base class, matching JS field-init
                        // ordering for the supported base-class case).
                        let field_inits: Vec<(Arc<str>, Value)> =
                            match class_obj.get("__field_inits__") {
                                Some(Value::Array(h)) => self
                                    .heap
                                    .array_vec(*h)
                                    .into_iter()
                                    .filter_map(|pair| match pair {
                                        Value::Array(ph) => {
                                            let p = self.heap.array_vec(ph);
                                            match (p.first(), p.get(1)) {
                                                (Some(Value::String(n)), Some(init)) => {
                                                    Some((Arc::from(n.as_str()), init.clone()))
                                                }
                                                _ => None,
                                            }
                                        }
                                        _ => None,
                                    })
                                    .collect(),
                                _ => Vec::new(),
                            };

                        let instance_val = Value::Object(self.heap.alloc_object(instance));

                        // Run instance field initializers with `this` bound to the
                        // instance, so `x = 10` and `y = this.x * 2` install values.
                        if let Value::Object(inst_h) = &instance_val {
                            let inst_h = *inst_h;
                            for (fname, init) in field_inits {
                                let v = self.call_field_init(&init, instance_val.clone())?;
                                if let Some(map) = self.heap.object_mut(inst_h) {
                                    map.insert(fname, v);
                                }
                            }
                        }

                        // Call the constructor with `this` bound to the instance.
                        // A subclass with no own constructor forwards to the nearest
                        // ancestor constructor (implicit `constructor(...a){ super(...a) }`).
                        match find_class_constructor(&class_obj, &self.heap) {
                            Some(Value::Function(closure)) => {
                                // Clear receiver source — constructors should not
                                // write back to a receiver variable.
                                self.last_receiver_source = None;
                                self.next_frame_is_constructor = true;
                                self.push_call_frame(&closure, &args, Some(instance_val))?;
                                self.last_receiver = None;
                            }
                            _ => {
                                // No user constructor. A built-in Error subclass with
                                // no own constructor still has an implicit
                                // `constructor(...a){ super(...a) }`, so forward the
                                // message argument (and derive the stack) like JS.
                                if class_obj.contains_key("__error_base__") {
                                    if let Value::Object(inst_h) = &instance_val {
                                        let inst_h = *inst_h;
                                        let msg = match args.first() {
                                            Some(v) if !matches!(v, Value::Undefined) => {
                                                v.to_js_string(&self.heap)
                                            }
                                            _ => String::new(),
                                        };
                                        let name = self
                                            .heap
                                            .object(inst_h)
                                            .and_then(|m| m.get("name"))
                                            .map(|v| v.to_js_string(&self.heap))
                                            .unwrap_or_else(|| "Error".to_string());
                                        let stack = if msg.is_empty() {
                                            name
                                        } else {
                                            format!("{}: {}", name, msg)
                                        };
                                        if let Some(map) = self.heap.object_mut(inst_h) {
                                            map.insert(
                                                Arc::from("message"),
                                                Value::String(JsString::from(msg.as_str())),
                                            );
                                            map.insert(
                                                Arc::from("stack"),
                                                Value::String(JsString::from(stack.as_str())),
                                            );
                                        }
                                    }
                                }
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
                    self.next_frame_is_constructor = true;
                    self.push_call_frame(&closure, &args, Some(this_val))?;
                    self.last_receiver = None;
                } else {
                    // No user super constructor. If the chain bottoms out at a
                    // built-in Error, `super(message)` runs the Error constructor:
                    // set `this.message`/`this.stack` (and a default `name`) on the
                    // instance, matching real JS. Otherwise it's a no-op.
                    let error_base = class
                        .as_deref()
                        .and_then(|name| self.class_error_base_of(name));
                    if let (Some(base), Value::Object(inst_h)) = (error_base, &this_val) {
                        // Only set message when an argument was passed and isn't
                        // undefined (JS leaves message as the prototype "" otherwise).
                        let msg = match args.first() {
                            Some(Value::Undefined) | None => None,
                            Some(v) => Some(v.to_js_string(&self.heap)),
                        };
                        if let Some(map) = self.heap.object_mut(*inst_h) {
                            let name = match map.get("name") {
                                Some(Value::String(n)) => n.to_string(),
                                _ => base.to_string(),
                            };
                            if let Some(m) = &msg {
                                map.insert(Arc::from("message"), Value::String(JsString::from(m.as_str())));
                            }
                            let stack_msg = msg.as_deref().unwrap_or("");
                            let stack = if stack_msg.is_empty() {
                                name.clone()
                            } else {
                                format!("{}: {}", name, stack_msg)
                            };
                            map.insert(Arc::from("stack"), Value::String(JsString::from(stack.as_str())));
                        }
                    }
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

    /// Construct a regex object (the same `{__regexp__, pattern, flags, lastIndex}`
    /// shape a regex literal compiles to) from `new RegExp(pattern, flags)` /
    /// `RegExp(pattern, flags)`. `pattern` may be a string or an existing regex
    /// (whose source/flags are copied; an explicit `flags` arg overrides). The
    /// pattern is validated up front so a bad pattern throws like JS.
    fn construct_regexp(&mut self, args: &[Value]) -> Result<Value> {
        let (pattern, src_flags) = match args.first() {
            Some(v) => match builtins::regexp_parts(v, &self.heap) {
                Some((p, f)) => (p, f),
                None => match v {
                    Value::Undefined | Value::Null => (String::new(), String::new()),
                    other => (other.to_js_string(&self.heap), String::new()),
                },
            },
            None => (String::new(), String::new()),
        };
        let flags = match args.get(1) {
            Some(Value::Undefined) | None => src_flags,
            Some(f) => f.to_js_string(&self.heap),
        };
        // Validate the pattern/flags eagerly (mirrors JS throwing on a bad regex).
        builtins::compile_regex(&pattern, &flags)?;
        let mut obj = IndexMap::new();
        obj.insert(Arc::from("__regexp__"), Value::Bool(true));
        obj.insert(Arc::from("pattern"), Value::String(JsString::from(pattern.as_str())));
        obj.insert(Arc::from("flags"), Value::String(JsString::from(flags.as_str())));
        obj.insert(Arc::from("lastIndex"), Value::Int(0));
        Ok(Value::Object(self.heap.alloc_object(obj)))
    }

    /// Resolve a static member `name` inherited from a class's `__super__` chain.
    /// Statics are stored directly on the class object (alongside the internal
    /// `__…__` keys), so we walk parent class objects skipping those internals.
    /// Returns the first match, mirroring JS static inheritance through the
    /// constructor's prototype chain.
    fn inherited_static_member(&self, class_map: &IndexMap<Arc<str>, Value>, name: &str) -> Option<Value> {
        let mut current = match class_map.get("__super__") {
            Some(Value::Object(h)) => *h,
            _ => return None,
        };
        loop {
            let parent = self.heap.object(current)?;
            if let Some(v) = parent.get(name) {
                if !matches!(v, Value::Undefined) {
                    return Some(v.clone());
                }
            }
            current = match parent.get("__super__") {
                Some(Value::Object(h)) => *h,
                _ => return None,
            };
        }
    }

    /// The built-in Error base name a class (named `name`) transitively extends,
    /// if any — used so `super(message)` in an Error subclass runs the Error
    /// constructor (sets `.message`/`.stack`).
    fn class_error_base_of(&self, name: &str) -> Option<Arc<str>> {
        let class_h = self.class_handle_by_name(name)?;
        match self.heap.object(class_h).and_then(|m| m.get("__error_base__")) {
            Some(Value::String(base)) => Some(Arc::from(base.as_str())),
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

    /// Build a class object from the groups pushed before a `CreateClass`
    /// instruction (see `CreateClassParts` / the instruction docs) and leave it on
    /// the stack. Extracted out of `dispatch` so its locals don't enlarge the
    /// recursive interpreter frame.
    #[inline(never)]
    fn create_class(&mut self, parts: CreateClassParts<'_>) -> Result<()> {
        let CreateClassParts {
            name,
            n_methods,
            n_statics,
            n_getters,
            n_setters,
            n_static_getters,
            n_static_setters,
            n_fields,
            n_static_fields,
            has_super,
        } = parts;

        // Stack layout (top to bottom — popped in this order):
        //   constructor closure (or undefined)
        //   n_methods         * (name, closure) pairs   instance methods
        //   n_statics         * (name, closure) pairs   static methods
        //   n_getters         * (name, closure) pairs   instance getters
        //   n_setters         * (name, closure) pairs   instance setters
        //   n_static_getters  * (name, closure) pairs   static getters
        //   n_static_setters  * (name, closure) pairs   static setters
        //   n_fields          * (name, init_closure) pairs
        //   n_static_fields   * (name, init_closure) pairs
        //   [optional super class if has_super]

        let constructor = self.pop()?;
        let mut prototype = self.pop_named_closure_pairs(n_methods)?;
        let statics = self.pop_named_closure_pairs(n_statics)?;
        let getters = self.pop_named_closure_pairs(n_getters)?;
        let setters = self.pop_named_closure_pairs(n_setters)?;
        let static_getters = self.pop_named_closure_pairs(n_static_getters)?;
        let static_setters = self.pop_named_closure_pairs(n_static_setters)?;
        // Field-initializer (name, closure) pairs, in declaration order.
        let field_inits = self.pop_named_closure_pairs(n_fields)?;
        let static_field_inits = self.pop_named_closure_pairs(n_static_fields)?;

        // Pop super class if present
        let super_class = if has_super { Some(self.pop()?) } else { None };

        // Accessor descriptors for this class's instances. Start from the
        // super prototype's accessors (inherited) and let our own override.
        let mut getters = getters;
        let mut setters = setters;

        // If super class, copy its prototype methods to ours (inheritance)
        if let Some(Value::Object(sc)) = &super_class {
            let scm = self.heap.object_map(*sc);
            if let Some(Value::Object(super_proto_h)) = scm.get("__prototype__").cloned() {
                // Super prototype methods go first, then our own (which override)
                let mut merged = self.heap.object_map(super_proto_h);
                for (k, v) in prototype {
                    merged.insert(k, v);
                }
                prototype = merged;
            }
            // Inherit accessor descriptors (our own override per-name below).
            if let Some(Value::Object(sg)) = scm.get("__getters__").cloned() {
                let mut merged = self.heap.object_map(sg);
                for (k, v) in getters {
                    merged.insert(k, v);
                }
                getters = merged;
            }
            if let Some(Value::Object(ss)) = scm.get("__setters__").cloned() {
                let mut merged = self.heap.object_map(ss);
                for (k, v) in setters {
                    merged.insert(k, v);
                }
                setters = merged;
            }
        }

        // The inheritance chain of class names (self first), so
        // `instanceof` matches ancestor classes too.
        let mut chain = vec![Value::String(JsString::from(name))];
        // If the class (transitively) extends a built-in Error, record the
        // error base name so instances get the `__error__` brand (making
        // `e instanceof Error` true) and `super(message)` sets `.message`.
        let mut error_base: Option<Arc<str>> = None;
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
            // A built-in Error constructor as the direct super.
            if let Some(Value::String(ctor)) = scm.get("__builtin_constructor__") {
                if is_error_ctor_name(ctor) {
                    error_base = Some(Arc::from(ctor.as_str()));
                }
            }
            // Or a user class that itself extends an Error.
            if error_base.is_none() {
                if let Some(Value::String(base)) = scm.get("__error_base__") {
                    error_base = Some(Arc::from(base.as_str()));
                }
            }
        }

        let chain_arr = Value::Array(self.heap.alloc_array(chain));
        let proto_obj = Value::Object(self.heap.alloc_object(prototype));

        // Build the class object
        let mut class_obj = IndexMap::new();
        class_obj.insert(Arc::from("__class_name__"), Value::String(JsString::from(name)));
        class_obj.insert(Arc::from("__class_chain__"), chain_arr);
        class_obj.insert(Arc::from("__constructor__"), constructor);
        class_obj.insert(Arc::from("__prototype__"), proto_obj);

        // Accessor descriptors (getters/setters) shared by every instance.
        if !getters.is_empty() {
            let g = Value::Object(self.heap.alloc_object(getters));
            class_obj.insert(Arc::from("__getters__"), g);
        }
        if !setters.is_empty() {
            let s = Value::Object(self.heap.alloc_object(setters));
            class_obj.insert(Arc::from("__setters__"), s);
        }
        // Static accessors live on the class object itself.
        if !static_getters.is_empty() {
            let g = Value::Object(self.heap.alloc_object(static_getters));
            class_obj.insert(Arc::from("__static_getters__"), g);
        }
        if !static_setters.is_empty() {
            let s = Value::Object(self.heap.alloc_object(static_setters));
            class_obj.insert(Arc::from("__static_setters__"), s);
        }

        // Instance field initializers: a list of [name, init_closure] pairs
        // run on each new instance (after super() in a derived class).
        if !field_inits.is_empty() {
            let mut pairs = Vec::with_capacity(field_inits.len());
            for (k, v) in field_inits {
                let pair = vec![Value::String(k.into()), v];
                pairs.push(Value::Array(self.heap.alloc_array(pair)));
            }
            let arr = Value::Array(self.heap.alloc_array(pairs));
            class_obj.insert(Arc::from("__field_inits__"), arr);
        }

        // Store super class reference for super() calls
        if let Some(sc) = super_class {
            class_obj.insert(Arc::from("__super__"), sc);
        }
        // Record the built-in Error base so instances are branded as errors.
        if let Some(base) = error_base {
            class_obj.insert(Arc::from("__error_base__"), Value::String(base.into()));
        }

        // Add static methods directly on the class object
        for (k, v) in statics {
            class_obj.insert(k, v);
        }

        let h = self.heap.alloc_object(class_obj);

        // Run static field initializers with `this` bound to the class
        // object, so `static count = 5` installs `Class.count === 5`
        // (and `static b = this.a` can read earlier static fields).
        let class_val = Value::Object(h);
        for (k, init) in static_field_inits {
            let v = self.call_field_init(&init, class_val.clone())?;
            if let Some(map) = self.heap.object_mut(h) {
                map.insert(k, v);
            }
        }

        self.push(class_val)?;
        Ok(())
    }

    /// Pop `n` `(name_string, closure)` pairs off the stack (the closure is on top
    /// of each pair, pushed after its name) into a map preserving DECLARATION order
    /// (they were pushed in declaration order, so they pop in reverse). Used by
    /// `CreateClass` to collect method/accessor/field-init groups.
    fn pop_named_closure_pairs(&mut self, n: usize) -> Result<IndexMap<Arc<str>, Value>> {
        let mut popped = Vec::with_capacity(n);
        for _ in 0..n {
            let closure = self.pop()?;
            let name = self.pop()?;
            popped.push((name, closure));
        }
        // Reverse back to declaration order before inserting.
        let mut out = IndexMap::new();
        for (name, closure) in popped.into_iter().rev() {
            if let Value::String(nm) = name {
                out.insert(Arc::from(nm.as_str()), closure);
            }
        }
        Ok(out)
    }

    /// If `obj` carries an accessor descriptor map under `kind`
    /// (`"__getters__"`/`"__setters__"` for instances, or
    /// `"__static_getters__"`/`"__static_setters__"` for class objects) with an
    /// entry for `name`, return the accessor closure to invoke (with `this` bound
    /// to `obj`). The instance keys are ignored on a class object itself (an
    /// instance getter belongs to the prototype, not the constructor).
    /// Whether `v` is an object carrying accessor descriptors (object-literal or
    /// class getters/setters). Used to decide when an enumeration surface must
    /// invoke getters rather than read stored values.
    fn has_accessors(&self, v: &Value) -> bool {
        matches!(v, Value::Object(h) if self.heap.object(*h).is_some_and(|m| {
            m.contains_key("__getters__") || m.contains_key("__setters__")
        }))
    }

    /// The value of own key `name` for *value-producing* enumeration surfaces
    /// (JSON.stringify, spread/Object.assign, Object.values/entries): if `name`
    /// is an accessor, invoke its getter with `this` bound to `obj`; a
    /// setter-only accessor reads as `undefined` (like JS). Otherwise return the
    /// already-cloned `stored` value (e.g. a plain data prop or a method fn).
    fn enumerable_value(&mut self, obj: &Value, name: &str, stored: Value) -> Result<Value> {
        if let Some(getter) = self.instance_accessor(obj, "__getters__", name) {
            return self.call_method_internal(&getter, obj.clone(), Vec::new());
        }
        if self.instance_accessor(obj, "__setters__", name).is_some() {
            return Ok(Value::Undefined);
        }
        Ok(stored)
    }

    fn instance_accessor(&self, obj: &Value, kind: &str, name: &str) -> Option<Value> {
        let Value::Object(h) = obj else { return None };
        let map = self.heap.object(*h)?;
        let is_class_object = map.contains_key("__class_name__");
        let is_instance_key = matches!(kind, "__getters__" | "__setters__");
        if is_class_object && is_instance_key {
            return None;
        }
        let descs = match map.get(kind)? {
            Value::Object(d) => *d,
            _ => return None,
        };
        match self.heap.object(descs)?.get(name) {
            Some(f @ Value::Function(_)) => Some(f.clone()),
            _ => None,
        }
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
                // Reflection on a class value: `Class.name` is the declared name,
                // and static members not found above are inherited from `__super__`
                // (statics walk the constructor's prototype chain in JS).
                if map.contains_key("__class_name__") {
                    if name == "name" {
                        if let Some(n @ Value::String(_)) = map.get("__class_name__") {
                            return Ok(n.clone());
                        }
                    }
                    if let Some(v) = self.inherited_static_member(&map, name) {
                        return Ok(v);
                    }
                }
                // `instance.constructor` resolves to the instance's class value.
                // Instances carry `__class__` (the class name); the class object is
                // bound to that name in globals.
                if name == "constructor" && map.contains_key("__class__") {
                    if let Some(Value::String(cname)) = map.get("__class__") {
                        if let Some(ch) = self.class_handle_by_name(cname) {
                            return Ok(Value::Object(ch));
                        }
                    }
                }
                // `.constructor` on a built-in object resolves to the matching
                // global constructor (so `({}).constructor === Object`,
                // `(new TypeError).constructor === TypeError`, etc.).
                if name == "constructor" {
                    let ctor = if map.contains_key("__error__") {
                        match map.get("name") {
                            Some(Value::String(s)) if is_error_ctor_name(s) => s.to_string(),
                            _ => "Error".to_string(),
                        }
                    } else if map.contains_key("__map__") {
                        "Map".to_string()
                    } else if map.contains_key("__set__") {
                        "Set".to_string()
                    } else if map.contains_key("__date_ms__") {
                        "Date".to_string()
                    } else if map.contains_key("__regexp__") {
                        "RegExp".to_string()
                    } else if map.contains_key("__builtin_constructor__")
                        || map.contains_key("__class_name__")
                        || map.contains_key("__global_fn__")
                    {
                        // A constructor/function value's own `.constructor` is Function.
                        "Function".to_string()
                    } else {
                        "Object".to_string()
                    };
                    if let Some(g) = self.globals.get(&ctor) {
                        return Ok(g.clone());
                    }
                }
                // A built-in constructor / type-conversion global exposes its
                // `.name` (e.g. `Object.name === "Object"`, `TypeError.name`).
                if name == "name" {
                    if let Some(Value::String(n)) = map
                        .get("__builtin_constructor__")
                        .or_else(|| map.get("__global_fn__"))
                    {
                        return Ok(Value::String(n.clone()));
                    }
                }
                // Check if this is a promise instance — expose .then/.catch/.finally
                if builtins::is_promise(obj, &self.heap) && is_promise_method(name) {
                    return Ok(builtin_method("__promise__", name));
                }
                // Built-in array-iterator (.keys()/.values()/.entries()): expose
                // `.next()`; iteration itself is handled by GetIterator.
                if is_array_iterator(obj, &self.heap) && name == "next" {
                    return Ok(builtin_method("__array_iterator__", name));
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
                if let Some((pattern, flags)) = builtins::regexp_parts(obj, &self.heap) {
                    if matches!(name, "test" | "exec") {
                        return Ok(builtin_method("__regexp__", name));
                    }
                    // Read-only regex accessors derived from source/flags.
                    match name {
                        "source" => {
                            let s = if pattern.is_empty() { "(?:)" } else { &pattern };
                            return Ok(Value::String(JsString::from(s)));
                        }
                        "global" => return Ok(Value::Bool(flags.contains('g'))),
                        "ignoreCase" => return Ok(Value::Bool(flags.contains('i'))),
                        "multiline" => return Ok(Value::Bool(flags.contains('m'))),
                        "dotAll" => return Ok(Value::Bool(flags.contains('s'))),
                        "sticky" => return Ok(Value::Bool(flags.contains('y'))),
                        _ => {}
                    }
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
                "constructor" => Ok(self.globals.get("Array").cloned().unwrap_or(Value::Undefined)),
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
                "length" => Ok(Value::Int(s.len_utf16() as i64)),
                "constructor" => Ok(self.globals.get("String").cloned().unwrap_or(Value::Undefined)),
                _ if is_string_method(name) => Ok(builtin_method("__string__", name)),
                _ => Ok(Value::Undefined),
            },
            Value::Int(_) | Value::Float(_) if name == "constructor" => {
                Ok(self.globals.get("Number").cloned().unwrap_or(Value::Undefined))
            }
            Value::Bool(_) if name == "constructor" => {
                Ok(self.globals.get("Boolean").cloned().unwrap_or(Value::Undefined))
            }
            Value::Function(closure) => match name {
                // Function reflection: `.length` is the arity (params before the
                // first default/rest), `.name` is the declared/inferred name.
                "length" => Ok(Value::Int(self.function_arity(closure.func_ref))),
                "name" => {
                    let n = self
                        .current_function(closure.func_ref)
                        .name
                        .clone()
                        .unwrap_or_default();
                    Ok(Value::String(JsString::from(n.as_str())))
                }
                _ => Ok(Value::Undefined),
            },
            Value::Generator(_) => match name {
                "next" | "return" | "throw" => Ok(builtin_method("__generator__", name)),
                _ => Ok(Value::Undefined),
            },
            Value::Int(_) | Value::Float(_) if builtins::is_number_method(name) => {
                Ok(builtin_method("__number__", name))
            }
            Value::BigInt(_) if builtins::is_bigint_method(name) => {
                Ok(builtin_method("__bigint__", name))
            }
            Value::BigInt(_) if name == "constructor" => {
                Ok(self.globals.get("BigInt").cloned().unwrap_or(Value::Undefined))
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
/// True if `name` is one of the built-in Error constructor names.
fn is_error_ctor_name(name: &str) -> bool {
    matches!(
        name,
        "Error"
            | "TypeError"
            | "RangeError"
            | "SyntaxError"
            | "ReferenceError"
            | "AggregateError"
    )
}

/// True if the object at `h` has been `Object.freeze`d (carries the internal
/// `__frozen__` brand). Frozen objects reject property/index writes and deletes.
fn is_frozen_object(h: Handle, heap: &Heap) -> bool {
    matches!(
        heap.object(h).and_then(|m| m.get("__frozen__")),
        Some(Value::Bool(true))
    )
}

/// JS `Number.MAX_SAFE_INTEGER` (`2^53 - 1`). Integer results outside
/// `[-MAX_SAFE_INTEGER, MAX_SAFE_INTEGER]` cannot be represented exactly by an
/// f64, so for parity with JS double arithmetic we must stop tracking them as a
/// precise i64 and let the value round like a double would.
const MAX_SAFE_INTEGER_I64: i64 = 9_007_199_254_740_991;

/// Build the [`Value`] for an i64 arithmetic result while matching JS double
/// semantics past the safe-integer boundary.
///
/// JS has no integer type: every number is an f64. zapcode keeps small integers
/// as `Value::Int` for speed/precision, but the result of `a (+|-|*) b` must
/// behave like a double once its magnitude exceeds `MAX_SAFE_INTEGER`, where
/// doubles can no longer represent consecutive integers (e.g. `2^53 + 1`
/// rounds to `2^53`). Keeping the exact i64 there would diverge from Node, so
/// we fall back to f64 and let it round.
fn int_arith_result(r: i64) -> Value {
    if r.unsigned_abs() <= MAX_SAFE_INTEGER_I64 as u64 {
        Value::Int(r)
    } else {
        Value::Float(r as f64)
    }
}

/// The TypeError JS throws when a BigInt is combined with a non-BigInt in
/// arithmetic (`10n + 5`, `2n ** 3`, etc.).
fn mix_bigint_error() -> ZapcodeError {
    ZapcodeError::TypeError(
        "Cannot mix BigInt and other types, use explicit conversions".to_string(),
    )
}

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
            .map(|c| Value::String(JsString::from(c.to_string().as_str())))
            .collect(),
        Value::Object(_) if is_array_iterator(v, heap) => {
            let Value::Object(h) = v else { return Vec::new() };
            let (items_h, cursor) = match heap.object(*h) {
                Some(m) => (
                    match m.get("__items__") {
                        Some(Value::Array(a)) => Some(*a),
                        _ => None,
                    },
                    match m.get("__cursor__") {
                        Some(Value::Int(i)) => (*i).max(0) as usize,
                        _ => 0,
                    },
                ),
                None => (None, 0),
            };
            match items_h {
                Some(a) => heap.array_vec(a).into_iter().skip(cursor).collect(),
                None => Vec::new(),
            }
        }
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
        "add" | "has" | "delete" | "clear" | "values" | "keys" | "entries" | "forEach"
    )
}

fn is_array_iterator(value: &Value, heap: &Heap) -> bool {
    matches!(
        value,
        Value::Object(h) if matches!(
            heap.object(*h).and_then(|m| m.get("__array_iterator__")),
            Some(Value::Bool(true))
        )
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
        "values" | "keys" => Some(builtins::make_array_iterator(items, heap)),
        "entries" => {
            // Set entries are [value, value] pairs (the value doubles as key).
            let pairs: Vec<Value> = items
                .into_iter()
                .map(|v| Value::Array(heap.alloc_array(vec![v.clone(), v])))
                .collect();
            Some(builtins::make_array_iterator(pairs, heap))
        }
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
    obj.insert(Arc::from("name"), Value::String(JsString::from(name)));
    obj.insert(Arc::from("message"), Value::String(JsString::from(message)));
    obj.insert(
        Arc::from("stack"),
        Value::String(JsString::from(format!("{}: {}", name, message).as_str())),
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
            Some(builtins::make_array_iterator(keys, heap))
        }
        "values" => {
            let vals: Vec<Value> = entries
                .iter()
                .filter_map(|e| match e {
                    Value::Object(eh) => heap.object(*eh).and_then(|m| m.get("value").cloned()),
                    _ => None,
                })
                .collect();
            Some(builtins::make_array_iterator(vals, heap))
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
            Some(builtins::make_array_iterator(out, heap))
        }
        _ => None,
    }
}

/// Maximum absolute time value (in ms) of a valid JS Date: ±8.64e15. Anything
/// beyond this is an Invalid Date (ECMA-262 "time clip").
const MAX_TIME_MS: u64 = 8_640_000_000_000_000;

/// A year whose magnitude exceeds this can never yield an in-range time value
/// (the valid window is roughly ±271821..=275760). Reject earlier years before
/// the `days_from_civil` arithmetic so a huge bare-integer string can't overflow
/// the i64 math (which would panic in debug builds); they all map to NaN anyway.
const MAX_TIME_YEAR: i64 = 300_000;

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
                // ECMA-262 time-clip: only finite |t| <= 8.64e15 is a valid Date.
                if n.is_finite() && (n as i64).unsigned_abs() <= MAX_TIME_MS {
                    make_date_object(n as i64, heap)
                } else {
                    make_invalid_date(heap)
                }
            }
        },
        _ => match date_from_components(args) {
            Some(millis) => make_date_object(millis, heap),
            None => make_invalid_date(heap),
        },
    }
}

/// `Date.UTC(year, monthIndex, day, ...)` -> epoch millis (UTC), or NaN for any
/// non-finite component / out-of-range time value (matches Node).
pub(crate) fn date_utc_millis(args: &[Value]) -> f64 {
    date_from_components(args).map_or(f64::NAN, |ms| ms as f64)
}

/// Shared `(year, monthIndex, day, hours, minutes, seconds, ms)` -> epoch millis
/// used by `Date.UTC` and the multi-arg `Date` constructor. Returns None (→ NaN /
/// Invalid Date) if any supplied component is non-finite or the resulting time
/// value falls outside ±8.64e15. Applies ECMA-262 MakeFullYear: a year in 0..=99
/// maps to 1900+year.
fn date_from_components(args: &[Value]) -> Option<i64> {
    // ToNumber each supplied component; short-circuit to NaN on any non-finite.
    let part = |i: usize, default: i64| -> Option<i64> {
        match args.get(i) {
            Some(v) => {
                let n = v.to_number();
                if n.is_finite() {
                    Some(n as i64)
                } else {
                    None
                }
            }
            None => Some(default),
        }
    };
    let year = part(0, 1970)?;
    let month = part(1, 0)?; // 0-indexed
    let day = part(2, 1)?;
    let hours = part(3, 0)?;
    let minutes = part(4, 0)?;
    let seconds = part(5, 0)?;
    let ms = part(6, 0)?;
    // MakeFullYear: a two-digit year (0..=99) is offset into the 1900s.
    let mut year = year as i128;
    if (0..=99).contains(&year) {
        year += 1900;
    }
    // Normalize a (possibly out-of-0..11) month into the year so `days_from_civil`
    // only ever sees a bounded month. Done in i128 so huge components never panic.
    year += (month as i128).div_euclid(12);
    let month0 = (month as i128).rem_euclid(12); // 0..=11
    // Reject years outside the representable window before the i64 math (which
    // would otherwise overflow / panic in debug builds); they are NaN anyway.
    if year.unsigned_abs() > MAX_TIME_YEAR as u128 {
        return None;
    }
    // First-of-month day count uses bounded inputs; the arbitrary `day` is folded
    // in afterward in i128 to stay overflow-free.
    let days = days_from_civil(year as i64, (month0 as i64) + 1, 1) as i128 + (day as i128 - 1);
    let millis = days * 86_400_000
        + hours as i128 * 3_600_000
        + minutes as i128 * 60_000
        + seconds as i128 * 1_000
        + ms as i128;
    if millis.unsigned_abs() > MAX_TIME_MS as u128 {
        return None;
    }
    Some(millis as i64)
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
    // A year beyond the representable Date window (e.g. a bare far-future integer
    // string like "1234567890123") is Invalid Date; bail before the i64 math.
    if year.unsigned_abs() > MAX_TIME_YEAR as u64 {
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
        // Range-check the clock like Node: minutes/seconds 0..=59 and hours
        // 0..=24, with 24:00:00.000 the only value allowed at hour 24 (any
        // non-zero minute/second/ms makes it Invalid Date, not a rollover).
        if !(0..=59).contains(&minutes) || !(0..=59).contains(&seconds) {
            return None;
        }
        if hours == 24 {
            if minutes != 0 || seconds != 0 || frac_ms != 0 {
                return None;
            }
        } else if !(0..=23).contains(&hours) {
            return None;
        }
        millis += hours * 3_600_000 + minutes * 60_000 + seconds * 1_000 + frac_ms;
        millis -= tz_offset_min * 60_000;
    }
    // A time value outside ±8.64e15 ms is an Invalid Date (so a bare far-future
    // year string like "1234567890123" yields NaN rather than a garbage date).
    if millis.unsigned_abs() > MAX_TIME_MS {
        return None;
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
            | "setTime"
            | "setUTCFullYear"
            | "setFullYear"
            | "setUTCMonth"
            | "setMonth"
            | "setUTCDate"
            | "setDate"
            | "setUTCHours"
            | "setHours"
            | "setUTCMinutes"
            | "setMinutes"
            | "setUTCSeconds"
            | "setSeconds"
            | "setUTCMilliseconds"
            | "setMilliseconds"
    )
}

/// The new `__date_ms__` for a Date mutator call, or `None` for non-setters.
/// Decompose the current timestamp into UTC civil fields, override the
/// field(s) the setter names (with the spec's optional trailing arguments),
/// recompose. Local setters alias the UTC ones (the sandbox runs in UTC).
fn date_setter_millis(
    map: &IndexMap<Arc<str>, Value>,
    method: &str,
    args: &[Value],
) -> Option<f64> {
    let cur = match map.get("__date_ms__") {
        Some(Value::Int(ms)) => *ms as f64,
        Some(Value::Float(ms)) => *ms,
        _ => 0.0,
    };
    let arg = |i: usize| args.get(i).map(|v| v.to_number());
    if method == "setTime" {
        return Some(arg(0).unwrap_or(f64::NAN));
    }
    let base = if cur.is_nan() { 0.0 } else { cur };
    let millis = base as i64;
    let seconds = millis.div_euclid(1000);
    let days = seconds.div_euclid(86_400);
    let sod = seconds.rem_euclid(86_400);
    let (mut year, mut month, mut day) = {
        let (y, m, d) = civil_from_days(days);
        (y as f64, m as f64, d as f64)
    };
    let mut hour = (sod / 3_600) as f64;
    let mut minute = ((sod % 3_600) / 60) as f64;
    let mut second = (sod % 60) as f64;
    let mut ms = millis.rem_euclid(1000) as f64;
    match method {
        "setUTCFullYear" | "setFullYear" => {
            year = arg(0)?;
            if let Some(v) = arg(1) {
                month = v + 1.0;
            }
            if let Some(v) = arg(2) {
                day = v;
            }
        }
        "setUTCMonth" | "setMonth" => {
            month = arg(0)? + 1.0;
            if let Some(v) = arg(1) {
                day = v;
            }
        }
        "setUTCDate" | "setDate" => day = arg(0)?,
        "setUTCHours" | "setHours" => {
            hour = arg(0)?;
            if let Some(v) = arg(1) {
                minute = v;
            }
            if let Some(v) = arg(2) {
                second = v;
            }
            if let Some(v) = arg(3) {
                ms = v;
            }
        }
        "setUTCMinutes" | "setMinutes" => {
            minute = arg(0)?;
            if let Some(v) = arg(1) {
                second = v;
            }
            if let Some(v) = arg(2) {
                ms = v;
            }
        }
        "setUTCSeconds" | "setSeconds" => {
            second = arg(0)?;
            if let Some(v) = arg(1) {
                ms = v;
            }
        }
        "setUTCMilliseconds" | "setMilliseconds" => ms = arg(0)?,
        _ => return None,
    }
    if !(year.is_finite() && month.is_finite() && day.is_finite())
        || !(hour.is_finite() && minute.is_finite() && second.is_finite() && ms.is_finite())
    {
        return Some(f64::NAN);
    }
    // Out-of-range fields roll over (month 12 -> next January, day 0 ->
    // last of previous month), exactly the civil-day arithmetic JS does.
    let month_total = (year as i64) * 12 + (month as i64) - 1;
    let norm_year = month_total.div_euclid(12);
    let norm_month = month_total.rem_euclid(12) + 1;
    let day_count = days_from_civil(norm_year, norm_month, 1) + (day as i64) - 1;
    let total_ms = day_count * 86_400_000
        + (hour as i64) * 3_600_000
        + (minute as i64) * 60_000
        + (second as i64) * 1000
        + (ms as i64);
    Some(total_ms as f64)
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
            _ => Value::String(JsString::from("Invalid Date")),
        });
    }
    let millis = millis_f as i64;
    // getTimezoneOffset is 0 (the sandbox runs in UTC).
    if method == "getTimezoneOffset" {
        return Some(Value::Int(0));
    }
    if matches!(method, "toJSON" | "toString" | "toDateString") {
        return Some(Value::String(JsString::from(unix_millis_to_iso(millis).as_str())));
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
        "toISOString" => Some(Value::String(JsString::from(
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

/// The UTF-16 code unit at index `i` of `s` as a single-unit JS string (a lone
/// surrogate becomes a `Wtf` string), or `undefined` if out of range.
fn unit_at(s: &JsString, i: usize) -> Value {
    s.units()
        .get(i)
        .map(|u| Value::String(JsString::from_units(&[*u])))
        .unwrap_or(Value::Undefined)
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
            | "toLocaleString"
            | "reverse"
            | "sort"
            | "toReversed"
            | "toSorted"
            | "toSpliced"
            | "with"
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
