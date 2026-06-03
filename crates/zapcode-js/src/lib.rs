use std::collections::HashMap;
use std::sync::Arc;

use napi::bindgen_prelude::*;
use napi_derive::napi;

use zapcode_core::heap::Heap;
use zapcode_core::{
    ExecutionTrace, ResourceLimits, RunResult, TraceSpan, TraceStatus, Value, VmState, ZapcodeRun,
    ZapcodeSessionSnapshot, ZapcodeSessionState, ZapcodeSnapshot,
};

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

#[napi(object)]
pub struct ZapcodeOptions {
    /// Variable names injected at runtime.
    pub inputs: Option<Vec<String>>,
    /// Function names the sandbox may call.
    pub external_functions: Option<Vec<String>>,
    /// Memory limit in megabytes (default: 32).
    pub memory_limit_mb: Option<u32>,
    /// Execution time limit in milliseconds (default: 5000).
    pub time_limit_ms: Option<u32>,
}

#[napi(object)]
pub struct ZapcodeSessionOptions {
    /// Function names the sandbox may call.
    pub external_functions: Option<Vec<String>>,
    /// Memory limit in megabytes (default: 32).
    pub memory_limit_mb: Option<u32>,
    /// Execution time limit in milliseconds (default: 5000).
    pub time_limit_ms: Option<u32>,
}

// ---------------------------------------------------------------------------
// Result types exposed to JS
// ---------------------------------------------------------------------------

#[napi(object)]
pub struct JsTraceSpan {
    pub name: String,
    pub start_time_ms: f64,
    pub end_time_ms: f64,
    pub duration_us: f64,
    pub status: String,
    pub attributes: Vec<Vec<String>>,
    pub children: Vec<JsTraceSpan>,
}

#[napi(object)]
pub struct ZapcodeResult {
    /// Discriminant for agent-friendly result handling. Always "complete".
    pub kind: String,
    /// Whether execution completed. Always true for this type.
    pub completed: bool,
    /// The output value, converted to a JSON-compatible serde_json::Value.
    pub output: serde_json::Value,
    /// Captured stdout output.
    pub stdout: String,
    /// Execution trace (parse → compile → execute).
    pub trace: JsTraceSpan,
}

#[napi(object)]
pub struct ZapcodeSuspension {
    /// Discriminant for agent-friendly result handling. Always "suspended".
    pub kind: String,
    /// Whether execution completed. Always false for this type.
    pub completed: bool,
    /// Name of the external function that caused suspension.
    pub function_name: String,
    /// Arguments passed to the external function.
    pub args: Vec<serde_json::Value>,
    /// Opaque snapshot bytes -- pass to `ZapcodeSnapshotHandle.load()` to resume.
    pub snapshot: Buffer,
}

/// One external call in a parallel batch (`Promise.all([...])`).
#[napi(object)]
pub struct JsExternalCall {
    pub name: String,
    pub args: Vec<serde_json::Value>,
}

/// Suspension on a batch of external calls the host can run in parallel.
/// Resume with `resumeMany(results)` passing one result per call, in order.
#[napi(object)]
pub struct ZapcodeBatchSuspension {
    /// Discriminant. Always "suspended_many".
    pub kind: String,
    /// Whether execution completed. Always false for this type.
    pub completed: bool,
    /// Which `Promise` combinator produced this batch: one of "all", "race",
    /// "any", "allSettled". The host settles the calls with the matching
    /// `Promise.*` and resumes accordingly (one result per call for
    /// all/allSettled; a single result for race/any; resumeError on rejection).
    pub combinator: String,
    /// The batched external calls, in order.
    pub calls: Vec<JsExternalCall>,
    /// Opaque snapshot bytes -- pass to `ZapcodeSnapshotHandle.load()` to resume.
    pub snapshot: Buffer,
}

#[napi(object)]
pub struct ZapcodeSessionResult {
    /// Discriminant for agent-friendly result handling. Always "complete".
    pub kind: String,
    /// Whether execution completed. Always true for this type.
    pub completed: bool,
    /// The output value, converted to a JSON-compatible serde_json::Value.
    pub output: serde_json::Value,
    /// Captured stdout output for this chunk/resume step.
    pub stdout: String,
    /// Opaque session bytes -- pass to `ZapcodeSessionHandle.load()` to continue.
    pub session: Buffer,
}

#[napi(object)]
pub struct ZapcodeSessionSuspension {
    /// Discriminant for agent-friendly result handling. Always "suspended".
    pub kind: String,
    /// Whether execution completed. Always false for this type.
    pub completed: bool,
    /// Name of the external function that caused suspension.
    pub function_name: String,
    /// Arguments passed to the external function.
    pub args: Vec<serde_json::Value>,
    /// Captured stdout output for this chunk/resume step.
    pub stdout: String,
    /// Opaque session bytes -- pass to `ZapcodeSessionHandle.load()` to continue.
    pub session: Buffer,
}

/// Session suspended on a batch of external calls
/// (`Promise.{all,race,any,allSettled}([...])`). Resume with
/// `resumeMany(results)` (or `resumeError` on rejection).
#[napi(object)]
pub struct ZapcodeSessionBatchSuspension {
    /// Discriminant. Always "suspended_many".
    pub kind: String,
    /// Whether execution completed. Always false for this type.
    pub completed: bool,
    /// Which `Promise` combinator produced this batch: "all", "race", "any", or
    /// "allSettled".
    pub combinator: String,
    /// The batched external calls, in order.
    pub calls: Vec<JsExternalCall>,
    /// Captured stdout output for this chunk/resume step.
    pub stdout: String,
    /// Opaque session bytes -- pass to `ZapcodeSessionHandle.load()` to continue.
    pub session: Buffer,
}

// ---------------------------------------------------------------------------
// Snapshot handle
// ---------------------------------------------------------------------------

#[napi]
pub struct ZapcodeSnapshotHandle {
    inner: ZapcodeSnapshot,
}

#[napi]
impl ZapcodeSnapshotHandle {
    /// Serialize the snapshot to bytes for storage or transport.
    #[napi]
    pub fn dump(&self) -> napi::Result<Buffer> {
        let bytes = self
            .inner
            .dump()
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        Ok(Buffer::from(bytes))
    }

    /// Load a snapshot from bytes previously obtained via `dump()`.
    #[napi(factory)]
    pub fn load(bytes: Buffer) -> napi::Result<Self> {
        let snapshot =
            ZapcodeSnapshot::load(&bytes).map_err(|e| napi::Error::from_reason(e.to_string()))?;
        Ok(Self { inner: snapshot })
    }

    /// Resume execution with the return value from the external function.
    ///
    /// Returns either a `ZapcodeResult` (complete) or a `ZapcodeSuspension`
    /// (suspended again on another external call).
    #[napi(ts_return_type = "ZapcodeResult | ZapcodeSuspension | ZapcodeBatchSuspension")]
    pub fn resume(
        &self,
        return_value: serde_json::Value,
    ) -> napi::Result<Either3<ZapcodeResult, ZapcodeSuspension, ZapcodeBatchSuspension>> {
        // Allocate any array/object in the return value into the snapshot's own
        // heap so its handles are valid when the snapshot is restored on resume.
        let mut snapshot = self.inner.clone();
        let value = json_to_value(&return_value, snapshot.heap_mut());
        let result = snapshot
            .resume(value)
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        // resume() doesn't produce a full trace yet — use an empty one
        let trace = ExecutionTrace {
            root: TraceSpan {
                name: "resume".to_string(),
                start_time_ms: 0,
                end_time_ms: 0,
                duration_us: 0,
                status: TraceStatus::Ok,
                attributes: Vec::new(),
                children: Vec::new(),
            },
        };
        run_result_to_either(result, trace)
    }

    /// Resume execution by *raising* an error at the suspended external call,
    /// instead of returning a value. Use when the host tool / activity failed.
    /// The error is catchable by a surrounding `try`/`catch` in the guest;
    /// otherwise it propagates out as an execution error.
    #[napi(ts_return_type = "ZapcodeResult | ZapcodeSuspension | ZapcodeBatchSuspension")]
    pub fn resume_error(
        &self,
        error: serde_json::Value,
    ) -> napi::Result<Either3<ZapcodeResult, ZapcodeSuspension, ZapcodeBatchSuspension>> {
        let mut snapshot = self.inner.clone();
        let value = json_to_value(&error, snapshot.heap_mut());
        let result = snapshot
            .resume_with_error(value)
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        let trace = ExecutionTrace {
            root: TraceSpan {
                name: "resume_error".to_string(),
                start_time_ms: 0,
                end_time_ms: 0,
                duration_us: 0,
                status: TraceStatus::Ok,
                attributes: Vec::new(),
                children: Vec::new(),
            },
        };
        run_result_to_either(result, trace)
    }

    /// Resume a batch suspension (`ZapcodeBatchSuspension`) with one result per
    /// call, in the order the calls were presented. The host can run the calls
    /// in parallel and pass back all results at once.
    #[napi(ts_return_type = "ZapcodeResult | ZapcodeSuspension | ZapcodeBatchSuspension")]
    pub fn resume_many(
        &self,
        results: Vec<serde_json::Value>,
    ) -> napi::Result<Either3<ZapcodeResult, ZapcodeSuspension, ZapcodeBatchSuspension>> {
        let mut snapshot = self.inner.clone();
        let values: Vec<Value> = results
            .iter()
            .map(|v| json_to_value(v, snapshot.heap_mut()))
            .collect();
        let result = snapshot
            .resume_many(values)
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        let trace = ExecutionTrace {
            root: TraceSpan {
                name: "resume_many".to_string(),
                start_time_ms: 0,
                end_time_ms: 0,
                duration_us: 0,
                status: TraceStatus::Ok,
                attributes: Vec::new(),
                children: Vec::new(),
            },
        };
        run_result_to_either(result, trace)
    }
}

// ---------------------------------------------------------------------------
// Ongoing session handle
// ---------------------------------------------------------------------------

#[napi]
pub struct ZapcodeSessionHandle {
    inner: ZapcodeSessionSnapshot,
}

#[napi]
impl ZapcodeSessionHandle {
    #[napi(factory)]
    pub fn create(options: Option<ZapcodeSessionOptions>) -> napi::Result<Self> {
        let opts = options.unwrap_or(ZapcodeSessionOptions {
            external_functions: None,
            memory_limit_mb: None,
            time_limit_ms: None,
        });

        let limits = resource_limits_from_session_options(&opts);
        let inner =
            ZapcodeSessionSnapshot::new(opts.external_functions.unwrap_or_default(), limits)
                .map_err(|e| napi::Error::from_reason(e.to_string()))?;

        Ok(Self { inner })
    }

    #[napi(factory)]
    pub fn load(bytes: Buffer) -> napi::Result<Self> {
        let inner = ZapcodeSessionSnapshot::load(&bytes)
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        Ok(Self { inner })
    }

    #[napi]
    pub fn dump(&self) -> napi::Result<Buffer> {
        let bytes = self
            .inner
            .dump()
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        Ok(Buffer::from(bytes))
    }

    #[napi(
        ts_return_type = "ZapcodeSessionResult | ZapcodeSessionSuspension | ZapcodeSessionBatchSuspension"
    )]
    pub fn run_chunk(
        &self,
        code: String,
        inputs: Option<HashMap<String, serde_json::Value>>,
    ) -> napi::Result<SessionEither> {
        let (input_values, input_heap) = inputs_to_vec(inputs);
        let state = self
            .inner
            .run_chunk_with_input_heap(code, input_values, input_heap)
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;

        session_state_to_either(state)
    }

    #[napi(
        ts_return_type = "ZapcodeSessionResult | ZapcodeSessionSuspension | ZapcodeSessionBatchSuspension"
    )]
    pub fn resume(&self, return_value: serde_json::Value) -> napi::Result<SessionEither> {
        let (value, value_heap) = json_value_with_heap(&return_value);
        let state = self
            .inner
            .resume_with_input_heap(value, value_heap)
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;

        session_state_to_either(state)
    }

    /// Resume a suspended session by *raising* an error at the external call
    /// site instead of returning a value (a failed tool / activity). Catchable
    /// by a surrounding `try`/`catch` in the chunk; otherwise it propagates.
    #[napi(
        ts_return_type = "ZapcodeSessionResult | ZapcodeSessionSuspension | ZapcodeSessionBatchSuspension"
    )]
    pub fn resume_error(&self, error: serde_json::Value) -> napi::Result<SessionEither> {
        let (value, value_heap) = json_value_with_heap(&error);
        let state = self
            .inner
            .resume_with_error_in_heap(value, value_heap)
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;

        session_state_to_either(state)
    }

    /// Resume a session suspended on a `Promise.all` batch with one result per
    /// call, in order. Use when the host ran the batched calls in parallel.
    #[napi(
        ts_return_type = "ZapcodeSessionResult | ZapcodeSessionSuspension | ZapcodeSessionBatchSuspension"
    )]
    pub fn resume_many(&self, results: Vec<serde_json::Value>) -> napi::Result<SessionEither> {
        let (values, value_heap) = json_values_with_heap(&results);
        let state = self
            .inner
            .resume_many_with_input_heap(values, value_heap)
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;

        session_state_to_either(state)
    }
}

// ---------------------------------------------------------------------------
// Main Zapcode class
// ---------------------------------------------------------------------------

#[napi]
pub struct Zapcode {
    inner: ZapcodeRun,
}

#[napi]
impl Zapcode {
    #[napi(constructor)]
    pub fn new(code: String, options: Option<ZapcodeOptions>) -> napi::Result<Self> {
        let opts = options.unwrap_or(ZapcodeOptions {
            inputs: None,
            external_functions: None,
            memory_limit_mb: None,
            time_limit_ms: None,
        });

        let limits = resource_limits_from_options(&opts);

        let inner = ZapcodeRun::new(
            code,
            opts.inputs.unwrap_or_default(),
            opts.external_functions.unwrap_or_default(),
            limits,
        )
        .map_err(|e| napi::Error::from_reason(e.to_string()))?;

        Ok(Self { inner })
    }

    /// Run the code to completion. Returns the output value and captured stdout.
    ///
    /// If the code calls an external function, this will return an error.
    /// Use `start()` for code that may suspend.
    #[napi]
    pub fn run(
        &self,
        inputs: Option<HashMap<String, serde_json::Value>>,
    ) -> napi::Result<ZapcodeResult> {
        let (input_values, input_heap) = inputs_to_vec(inputs);
        let result = self
            .inner
            .run_with_input_heap(input_values, input_heap)
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;

        let RunResult {
            state,
            heap,
            stdout,
            trace,
        } = result;
        match state {
            VmState::Complete(v) => Ok(ZapcodeResult {
                kind: "complete".to_string(),
                completed: true,
                output: value_to_json(&v, &heap)?,
                stdout,
                trace: trace_to_js(&trace),
            }),
            VmState::Suspended { function_name, .. } => Err(napi::Error::from_reason(format!(
                "execution suspended on external function '{}' -- use start() instead",
                function_name
            ))),
            VmState::SuspendedMany { .. } => Err(napi::Error::from_reason(
                "execution suspended on a Promise.all batch -- use start() instead".to_string(),
            )),
        }
    }

    /// Start execution. Returns either a completed result or a suspension.
    ///
    /// Check the `completed` field to determine which type was returned.
    #[napi(ts_return_type = "ZapcodeResult | ZapcodeSuspension | ZapcodeBatchSuspension")]
    pub fn start(
        &self,
        inputs: Option<HashMap<String, serde_json::Value>>,
    ) -> napi::Result<Either3<ZapcodeResult, ZapcodeSuspension, ZapcodeBatchSuspension>> {
        let (input_values, input_heap) = inputs_to_vec(inputs);
        let result = self
            .inner
            .run_with_input_heap(input_values, input_heap)
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;

        run_result_to_either_with_stdout(result)
    }
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

/// Convert a JS inputs map to the `Vec<(String, Value)>` that zapcode-core
/// expects, allocating any array/object inputs into a fresh `Heap`. The heap is
/// returned alongside so callers can hand it to the heap-seeding core entry
/// points (`run_with_input_heap` / `run_chunk_with_input_heap`); the input
/// handles are valid in that heap.
fn inputs_to_vec(
    inputs: Option<HashMap<String, serde_json::Value>>,
) -> (Vec<(String, Value)>, Heap) {
    let mut heap = Heap::new();
    let values = inputs
        .unwrap_or_default()
        .into_iter()
        .map(|(k, v)| (k, json_to_value(&v, &mut heap)))
        .collect();
    (values, heap)
}

/// Convert a single host JSON value into a `Value`, allocating into a fresh heap
/// that the caller then merges into the target VM heap.
fn json_value_with_heap(json: &serde_json::Value) -> (Value, Heap) {
    let mut heap = Heap::new();
    let value = json_to_value(json, &mut heap);
    (value, heap)
}

/// Convert a list of host JSON values into `Value`s sharing one fresh heap.
fn json_values_with_heap(values: &[serde_json::Value]) -> (Vec<Value>, Heap) {
    let mut heap = Heap::new();
    let converted = values.iter().map(|v| json_to_value(v, &mut heap)).collect();
    (converted, heap)
}

fn resource_limits_from_options(opts: &ZapcodeOptions) -> ResourceLimits {
    let mut limits = ResourceLimits::default();
    if let Some(mb) = opts.memory_limit_mb {
        limits.memory_limit_bytes = (mb as usize) * 1024 * 1024;
    }
    if let Some(ms) = opts.time_limit_ms {
        limits.time_limit_ms = ms as u64;
    }
    limits
}

fn resource_limits_from_session_options(opts: &ZapcodeSessionOptions) -> ResourceLimits {
    let mut limits = ResourceLimits::default();
    if let Some(mb) = opts.memory_limit_mb {
        limits.memory_limit_bytes = (mb as usize) * 1024 * 1024;
    }
    if let Some(ms) = opts.time_limit_ms {
        limits.time_limit_ms = ms as u64;
    }
    limits
}

/// Convert a `serde_json::Value` to a `zapcode_core::Value`, allocating any
/// array/object into `heap`. Array/object `Value`s carry a `Handle` into the
/// heap rather than owning their contents, so a heap is required at the host
/// boundary.
fn json_to_value(json: &serde_json::Value, heap: &mut Heap) -> Value {
    match json {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else if let Some(f) = n.as_f64() {
                Value::Float(f)
            } else {
                Value::Undefined
            }
        }
        serde_json::Value::String(s) => Value::String(Arc::from(s.as_str())),
        serde_json::Value::Array(arr) => {
            let items: Vec<Value> = arr.iter().map(|v| json_to_value(v, heap)).collect();
            Value::Array(heap.alloc_array(items))
        }
        serde_json::Value::Object(obj) => {
            let map = obj
                .iter()
                .map(|(k, v)| (Arc::from(k.as_str()), json_to_value(v, heap)))
                .collect();
            Value::Object(heap.alloc_object(map))
        }
    }
}

/// Maximum nesting depth when marshalling a guest `Value` out across the napi
/// boundary. Guest reference values can form cycles (`const a = []; a.push(a)`)
/// or be nested arbitrarily deep; an unbounded native recursion here would
/// overflow the OS stack and *abort the entire host Node process* (an
/// uncatchable `SIGSEGV` — a panic/abort across napi kills the host). Capping
/// the recursion turns "cyclic / very deep value handed back or passed to a
/// tool" into a catchable `napi::Error` the host can handle. Kept in lockstep
/// with the core's `MAX_RENDER_DEPTH`.
const MAX_MARSHAL_DEPTH: usize = 256;

/// Convert a `zapcode_core::Value` to a `serde_json::Value`, dereferencing
/// array/object handles through `heap`.
///
/// Fallible: a reference cycle or an over-deep structure surfaces a catchable
/// `napi::Error` instead of recursing to native-stack exhaustion (which would
/// abort the host process). `seen` tracks the chain of currently-open
/// Array/Object handles to detect cycles; `depth` is the hard backstop.
fn value_to_json(value: &Value, heap: &Heap) -> napi::Result<serde_json::Value> {
    let mut seen: Vec<zapcode_core::heap::Handle> = Vec::new();
    value_to_json_inner(value, heap, &mut seen, 0)
}

fn value_to_json_inner(
    value: &Value,
    heap: &Heap,
    seen: &mut Vec<zapcode_core::heap::Handle>,
    depth: usize,
) -> napi::Result<serde_json::Value> {
    if depth > MAX_MARSHAL_DEPTH {
        return Err(napi::Error::from_reason(
            "value nesting depth exceeded while marshalling result (max 256)".to_string(),
        ));
    }
    Ok(match value {
        Value::Undefined | Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Int(n) => serde_json::json!(*n),
        Value::Float(n) => {
            if n.is_finite() {
                serde_json::json!(*n)
            } else {
                // JSON cannot represent Infinity/NaN -- use null like JSON.stringify does.
                serde_json::Value::Null
            }
        }
        Value::String(s) => serde_json::Value::String(s.to_string()),
        Value::Array(h) => {
            if seen.contains(h) {
                return Err(napi::Error::from_reason(
                    "Converting circular structure to JSON".to_string(),
                ));
            }
            seen.push(*h);
            let mut items = Vec::new();
            for v in heap.array(*h).iter() {
                items.push(value_to_json_inner(v, heap, seen, depth + 1)?);
            }
            seen.pop();
            serde_json::Value::Array(items)
        }
        Value::Object(h) => {
            let mut map = serde_json::Map::new();
            if let Some(obj) = heap.object(*h) {
                if seen.contains(h) {
                    return Err(napi::Error::from_reason(
                        "Converting circular structure to JSON".to_string(),
                    ));
                }
                seen.push(*h);
                for (k, v) in obj.iter() {
                    // Internal brand keys (`__error__`, `__class__`,
                    // `__frozen__`, …) are VM bookkeeping, not guest-visible
                    // properties — never marshal them across the boundary.
                    if k.starts_with("__") {
                        continue;
                    }
                    map.insert(k.to_string(), value_to_json_inner(v, heap, seen, depth + 1)?);
                }
                seen.pop();
            }
            serde_json::Value::Object(map)
        }
        Value::Function(_) | Value::BuiltinMethod { .. } => {
            // Functions are not serializable to JSON.
            serde_json::Value::Null
        }
        Value::Generator(_) => serde_json::Value::Null,
        // A deferred batch call never escapes to JS as a result value.
        Value::Pending(_) => serde_json::Value::Null,
    })
}

fn trace_span_to_js(span: &TraceSpan) -> JsTraceSpan {
    JsTraceSpan {
        name: span.name.clone(),
        start_time_ms: span.start_time_ms as f64,
        end_time_ms: span.end_time_ms as f64,
        duration_us: span.duration_us as f64,
        status: match span.status {
            TraceStatus::Ok => "ok".to_string(),
            TraceStatus::Error => "error".to_string(),
        },
        attributes: span
            .attributes
            .iter()
            .map(|(k, v)| vec![k.clone(), v.clone()])
            .collect(),
        children: span.children.iter().map(trace_span_to_js).collect(),
    }
}

fn trace_to_js(trace: &ExecutionTrace) -> JsTraceSpan {
    trace_span_to_js(&trace.root)
}

/// Package a [`RunResult`] into a complete / single-suspension / batch-suspension
/// result. The result's `heap` resolves the handles in `state` (the completed
/// output, or a suspension's args/calls), so it is threaded through every
/// value-to-JSON conversion.
fn run_result_to_either(
    result: RunResult,
    trace: ExecutionTrace,
) -> napi::Result<Either3<ZapcodeResult, ZapcodeSuspension, ZapcodeBatchSuspension>> {
    let RunResult { state, heap, .. } = result;
    match state {
        VmState::Complete(v) => Ok(Either3::A(ZapcodeResult {
            kind: "complete".to_string(),
            completed: true,
            output: value_to_json(&v, &heap)?,
            stdout: String::new(),
            trace: trace_to_js(&trace),
        })),
        VmState::Suspended {
            function_name,
            args,
            snapshot,
        } => {
            let js_args = values_to_json(&args, &heap)?;
            let snap_bytes = snapshot
                .dump()
                .map_err(|e| napi::Error::from_reason(e.to_string()))?;
            Ok(Either3::B(ZapcodeSuspension {
                kind: "suspended".to_string(),
                completed: false,
                function_name,
                args: js_args,
                snapshot: Buffer::from(snap_bytes),
            }))
        }
        VmState::SuspendedMany {
            calls,
            combinator,
            snapshot,
        } => {
            let js_calls = external_calls_to_js(&calls, &heap)?;
            let snap_bytes = snapshot
                .dump()
                .map_err(|e| napi::Error::from_reason(e.to_string()))?;
            Ok(Either3::C(ZapcodeBatchSuspension {
                kind: "suspended_many".to_string(),
                completed: false,
                combinator: combinator.as_str().to_string(),
                calls: js_calls,
                snapshot: Buffer::from(snap_bytes),
            }))
        }
    }
}

/// Like [`run_result_to_either`] but carries the captured stdout into the
/// completed result (used by `start()`, which produces a full trace + stdout).
fn run_result_to_either_with_stdout(
    result: RunResult,
) -> napi::Result<Either3<ZapcodeResult, ZapcodeSuspension, ZapcodeBatchSuspension>> {
    let RunResult {
        state,
        heap,
        stdout,
        trace,
    } = result;
    match state {
        VmState::Complete(v) => Ok(Either3::A(ZapcodeResult {
            kind: "complete".to_string(),
            completed: true,
            output: value_to_json(&v, &heap)?,
            stdout,
            trace: trace_to_js(&trace),
        })),
        VmState::Suspended {
            function_name,
            args,
            snapshot,
        } => {
            let js_args = values_to_json(&args, &heap)?;
            let snap_bytes = snapshot
                .dump()
                .map_err(|e| napi::Error::from_reason(e.to_string()))?;
            Ok(Either3::B(ZapcodeSuspension {
                kind: "suspended".to_string(),
                completed: false,
                function_name,
                args: js_args,
                snapshot: Buffer::from(snap_bytes),
            }))
        }
        VmState::SuspendedMany {
            calls,
            combinator,
            snapshot,
        } => {
            let js_calls = external_calls_to_js(&calls, &heap)?;
            let snap_bytes = snapshot
                .dump()
                .map_err(|e| napi::Error::from_reason(e.to_string()))?;
            Ok(Either3::C(ZapcodeBatchSuspension {
                kind: "suspended_many".to_string(),
                completed: false,
                combinator: combinator.as_str().to_string(),
                calls: js_calls,
                snapshot: Buffer::from(snap_bytes),
            }))
        }
    }
}

fn external_calls_to_js(
    calls: &[zapcode_core::ExternalCall],
    heap: &Heap,
) -> napi::Result<Vec<JsExternalCall>> {
    let mut out = Vec::with_capacity(calls.len());
    for c in calls {
        let mut args = Vec::with_capacity(c.args.len());
        for v in c.args.iter() {
            args.push(value_to_json(v, heap)?);
        }
        out.push(JsExternalCall {
            name: c.name.clone(),
            args,
        });
    }
    Ok(out)
}

/// Marshal a list of `Value`s to JSON, surfacing a catchable error if any
/// contains a cycle / is too deeply nested (instead of aborting the host).
fn values_to_json(values: &[Value], heap: &Heap) -> napi::Result<Vec<serde_json::Value>> {
    let mut out = Vec::with_capacity(values.len());
    for v in values {
        out.push(value_to_json(v, heap)?);
    }
    Ok(out)
}

type SessionEither =
    Either3<ZapcodeSessionResult, ZapcodeSessionSuspension, ZapcodeSessionBatchSuspension>;

fn session_state_to_either(state: ZapcodeSessionState) -> napi::Result<SessionEither> {
    // Clone the session's heap up front: it resolves the handles in
    // `output`/`args`/`calls` and is moved into the dumped session below.
    let heap = state.heap().clone();
    match state {
        ZapcodeSessionState::Complete {
            output,
            stdout,
            session,
        } => {
            let js_output = value_to_json(&output, &heap)?;
            let bytes = session
                .dump()
                .map_err(|e| napi::Error::from_reason(e.to_string()))?;
            Ok(Either3::A(ZapcodeSessionResult {
                kind: "complete".to_string(),
                completed: true,
                output: js_output,
                stdout,
                session: Buffer::from(bytes),
            }))
        }
        ZapcodeSessionState::Suspended {
            function_name,
            args,
            stdout,
            session,
        } => {
            let js_args = values_to_json(&args, &heap)?;
            let bytes = session
                .dump()
                .map_err(|e| napi::Error::from_reason(e.to_string()))?;
            Ok(Either3::B(ZapcodeSessionSuspension {
                kind: "suspended".to_string(),
                completed: false,
                function_name,
                args: js_args,
                stdout,
                session: Buffer::from(bytes),
            }))
        }
        ZapcodeSessionState::SuspendedMany {
            calls,
            combinator,
            stdout,
            session,
        } => {
            let js_calls = external_calls_to_js(&calls, &heap)?;
            let bytes = session
                .dump()
                .map_err(|e| napi::Error::from_reason(e.to_string()))?;
            Ok(Either3::C(ZapcodeSessionBatchSuspension {
                kind: "suspended_many".to_string(),
                completed: false,
                combinator: combinator.as_str().to_string(),
                calls: js_calls,
                stdout,
                session: Buffer::from(bytes),
            }))
        }
    }
}
