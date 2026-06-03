use std::sync::Arc;

use js_sys::{Array, Object, Reflect};
use serde::Deserialize;
use wasm_bindgen::prelude::*;

use zapcode_core::heap::Heap;
use zapcode_core::{
    ExecutionTrace, ResourceLimits, RunResult, TraceSpan as CoreTraceSpan, TraceStatus, Value,
    VmState, ZapcodeError, ZapcodeSnapshot as CoreSnapshot,
};

// ---------------------------------------------------------------------------
// Value conversion: zapcode_core::Value <-> JsValue
// ---------------------------------------------------------------------------

/// Convert a `JsValue` to a `zapcode_core::Value`, allocating any array/object
/// into `heap` (array/object `Value`s carry a `Handle`, not inline contents).
fn js_to_value(js: &JsValue, heap: &mut Heap) -> Result<Value, JsError> {
    if js.is_undefined() {
        Ok(Value::Undefined)
    } else if js.is_null() {
        Ok(Value::Null)
    } else if let Some(b) = js.as_bool() {
        Ok(Value::Bool(b))
    } else if let Some(n) = js.as_f64() {
        // Represent whole numbers as Int for fidelity with the core VM.
        if n.fract() == 0.0 && n >= i64::MIN as f64 && n <= i64::MAX as f64 {
            Ok(Value::Int(n as i64))
        } else {
            Ok(Value::Float(n))
        }
    } else if let Some(s) = js.as_string() {
        Ok(Value::String(Arc::from(s.as_str())))
    } else if Array::is_array(js) {
        let arr = Array::from(js);
        let mut items = Vec::with_capacity(arr.length() as usize);
        for i in 0..arr.length() {
            items.push(js_to_value(&arr.get(i), heap)?);
        }
        Ok(Value::Array(heap.alloc_array(items)))
    } else if js.is_object() {
        let obj = Object::from(js.clone());
        let entries = Object::entries(&obj);
        let mut map = indexmap::IndexMap::new();
        for i in 0..entries.length() {
            let pair = Array::from(&entries.get(i));
            let key = pair
                .get(0)
                .as_string()
                .ok_or_else(|| JsError::new("object keys must be strings"))?;
            let val = js_to_value(&pair.get(1), heap)?;
            map.insert(Arc::from(key.as_str()), val);
        }
        Ok(Value::Object(heap.alloc_object(map)))
    } else {
        Err(JsError::new(&format!(
            "cannot convert JS value to Zapcode value: {:?}",
            js
        )))
    }
}

/// Convert a `zapcode_core::Value` to a `JsValue`, dereferencing array/object
/// handles through `heap`.
fn value_to_js(val: &Value, heap: &Heap) -> Result<JsValue, JsError> {
    match val {
        Value::Undefined => Ok(JsValue::undefined()),
        Value::Null => Ok(JsValue::null()),
        Value::Bool(b) => Ok(JsValue::from(*b)),
        Value::Int(n) => Ok(JsValue::from(*n as f64)),
        Value::Float(n) => Ok(JsValue::from(*n)),
        Value::String(s) => Ok(JsValue::from_str(s.as_ref())),
        Value::Array(h) => {
            let items = heap.array(*h);
            let js_arr = Array::new_with_length(items.len() as u32);
            for (i, item) in items.iter().enumerate() {
                js_arr.set(i as u32, value_to_js(item, heap)?);
            }
            Ok(js_arr.into())
        }
        Value::Object(h) => {
            let obj = Object::new();
            if let Some(map) = heap.object(*h) {
                for (k, v) in map {
                    Reflect::set(&obj, &JsValue::from_str(k.as_ref()), &value_to_js(v, heap)?)
                        .map_err(|_| JsError::new("failed to set object property"))?;
                }
            }
            Ok(obj.into())
        }
        Value::Function(_) | Value::BuiltinMethod { .. } => Ok(JsValue::from_str("<function>")),
        Value::Generator(_) => Ok(JsValue::from_str("<generator>")),
        // A deferred batch call never escapes to a result value.
        Value::Pending(_) => Ok(JsValue::null()),
    }
}

/// Convert a `ZapcodeError` to a `JsError`.
fn zapcode_err(e: ZapcodeError) -> JsError {
    JsError::new(&e.to_string())
}

// ---------------------------------------------------------------------------
// Options structs (deserialized from JsValue via serde-wasm-bindgen)
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ZapcodeOptions {
    #[serde(default)]
    inputs: Vec<String>,
    #[serde(default)]
    external_functions: Vec<String>,
    #[serde(default)]
    memory_limit_bytes: Option<usize>,
    #[serde(default)]
    time_limit_ms: Option<u64>,
    #[serde(default)]
    max_stack_depth: Option<usize>,
    #[serde(default)]
    max_allocations: Option<usize>,
}

// ---------------------------------------------------------------------------
// Zapcode — main entry point
// ---------------------------------------------------------------------------

#[wasm_bindgen]
pub struct Zapcode {
    inner: zapcode_core::ZapcodeRun,
}

#[wasm_bindgen]
impl Zapcode {
    /// Create a new Zapcode instance.
    ///
    /// @param code - TypeScript source code to execute.
    /// @param options - Optional configuration object with fields:
    ///   - inputs: string[] - Variable names injected at runtime.
    ///   - externalFunctions: string[] - Function names the sandbox may call.
    ///   - memoryLimitBytes: number - Maximum memory in bytes.
    ///   - timeLimitMs: number - Maximum execution time in milliseconds.
    ///   - maxStackDepth: number - Maximum call stack depth.
    ///   - maxAllocations: number - Maximum heap allocations.
    #[wasm_bindgen(constructor)]
    pub fn new(code: &str, options: JsValue) -> Result<Zapcode, JsError> {
        let opts: ZapcodeOptions = if options.is_undefined() || options.is_null() {
            ZapcodeOptions::default()
        } else {
            serde_wasm_bindgen::from_value(options)
                .map_err(|e| JsError::new(&format!("invalid options: {}", e)))?
        };

        let defaults = ResourceLimits::default();
        let limits = ResourceLimits {
            memory_limit_bytes: opts
                .memory_limit_bytes
                .unwrap_or(defaults.memory_limit_bytes),
            time_limit_ms: opts.time_limit_ms.unwrap_or(defaults.time_limit_ms),
            max_stack_depth: opts.max_stack_depth.unwrap_or(defaults.max_stack_depth),
            max_allocations: opts.max_allocations.unwrap_or(defaults.max_allocations),
            max_snapshot_bytes: defaults.max_snapshot_bytes,
        };

        let inner = zapcode_core::ZapcodeRun::new(
            code.to_string(),
            opts.inputs,
            opts.external_functions,
            limits,
        )
        .map_err(zapcode_err)?;

        Ok(Self { inner })
    }

    /// Run the program to completion.
    ///
    /// @param inputs - Optional object mapping input names to values.
    /// @returns An object with `output` and `stdout` keys on completion,
    ///          or `suspended`, `functionName`, `args`, and `snapshot` keys on suspension.
    pub fn run(&self, inputs: JsValue) -> Result<JsValue, JsError> {
        let (input_values, input_heap) = extract_inputs(&inputs)?;
        let result = self
            .inner
            .run_with_input_heap(input_values, input_heap)
            .map_err(zapcode_err)?;
        run_result_to_js(result, true)
    }

    /// Start execution, returning raw state (for suspension / snapshot handling).
    ///
    /// @param inputs - Optional object mapping input names to values.
    /// @returns Same shape as `run()`.
    pub fn start(&self, inputs: JsValue) -> Result<JsValue, JsError> {
        let (input_values, input_heap) = extract_inputs(&inputs)?;
        let result = self
            .inner
            .run_with_input_heap(input_values, input_heap)
            .map_err(zapcode_err)?;
        run_result_to_js(result, true)
    }
}

/// Extract input key-value pairs from a JsValue (expected to be an object or
/// undefined/null), allocating any array/object inputs into a fresh `Heap`. The
/// heap is returned alongside so the caller can pass it to the heap-seeding core
/// entry point (`run_with_input_heap`); the input handles are valid there.
fn extract_inputs(inputs: &JsValue) -> Result<(Vec<(String, Value)>, Heap), JsError> {
    let mut heap = Heap::new();
    if inputs.is_undefined() || inputs.is_null() {
        return Ok((Vec::new(), heap));
    }
    let obj = Object::from(inputs.clone());
    let entries = Object::entries(&obj);
    let mut out = Vec::with_capacity(entries.length() as usize);
    for i in 0..entries.length() {
        let pair = Array::from(&entries.get(i));
        let key = pair
            .get(0)
            .as_string()
            .ok_or_else(|| JsError::new("input keys must be strings"))?;
        let val = js_to_value(&pair.get(1), &mut heap)?;
        out.push((key, val));
    }
    Ok((out, heap))
}

/// Convert a `TraceSpan` to a JS object.
fn trace_span_to_js(span: &CoreTraceSpan) -> Result<JsValue, JsError> {
    let obj = Object::new();
    Reflect::set(&obj, &"name".into(), &JsValue::from_str(&span.name))
        .map_err(|_| JsError::new("failed to set trace field"))?;
    Reflect::set(
        &obj,
        &"startTimeMs".into(),
        &JsValue::from(span.start_time_ms as f64),
    )
    .map_err(|_| JsError::new("failed to set trace field"))?;
    Reflect::set(
        &obj,
        &"endTimeMs".into(),
        &JsValue::from(span.end_time_ms as f64),
    )
    .map_err(|_| JsError::new("failed to set trace field"))?;
    Reflect::set(
        &obj,
        &"durationUs".into(),
        &JsValue::from(span.duration_us as f64),
    )
    .map_err(|_| JsError::new("failed to set trace field"))?;
    Reflect::set(
        &obj,
        &"status".into(),
        &JsValue::from_str(match span.status {
            TraceStatus::Ok => "ok",
            TraceStatus::Error => "error",
        }),
    )
    .map_err(|_| JsError::new("failed to set trace field"))?;

    let attrs = Object::new();
    for (k, v) in &span.attributes {
        Reflect::set(&attrs, &JsValue::from_str(k), &JsValue::from_str(v))
            .map_err(|_| JsError::new("failed to set trace attribute"))?;
    }
    Reflect::set(&obj, &"attributes".into(), &attrs.into())
        .map_err(|_| JsError::new("failed to set trace field"))?;

    let children = Array::new_with_length(span.children.len() as u32);
    for (i, child) in span.children.iter().enumerate() {
        children.set(i as u32, trace_span_to_js(child)?);
    }
    Reflect::set(&obj, &"children".into(), &children.into())
        .map_err(|_| JsError::new("failed to set trace field"))?;

    Ok(obj.into())
}

/// Marshal a whole [`RunResult`] (state + heap + stdout + optional trace) into a
/// JS object.
fn run_result_to_js(result: RunResult, include_trace: bool) -> Result<JsValue, JsError> {
    let RunResult {
        state,
        heap,
        stdout,
        trace,
    } = result;
    let trace_ref = if include_trace { Some(&trace) } else { None };
    vm_state_to_js(state, &heap, &stdout, trace_ref)
}

/// Convert a `VmState` (+ heap + optional stdout + trace) to a JS object. `heap`
/// resolves the array/object handles in the completed output or a suspension's
/// args/calls.
fn vm_state_to_js(
    state: VmState,
    heap: &Heap,
    stdout: &str,
    trace: Option<&ExecutionTrace>,
) -> Result<JsValue, JsError> {
    let obj = Object::new();
    match state {
        VmState::Complete(value) => {
            Reflect::set(&obj, &JsValue::from_str("output"), &value_to_js(&value, heap)?)
                .map_err(|_| JsError::new("failed to set output"))?;
            Reflect::set(
                &obj,
                &JsValue::from_str("stdout"),
                &JsValue::from_str(stdout),
            )
            .map_err(|_| JsError::new("failed to set stdout"))?;
        }
        VmState::Suspended {
            function_name,
            args,
            snapshot,
        } => {
            Reflect::set(&obj, &JsValue::from_str("suspended"), &JsValue::from(true))
                .map_err(|_| JsError::new("failed to set suspended"))?;
            Reflect::set(
                &obj,
                &JsValue::from_str("functionName"),
                &JsValue::from_str(&function_name),
            )
            .map_err(|_| JsError::new("failed to set functionName"))?;
            let js_args = Array::new_with_length(args.len() as u32);
            for (i, arg) in args.iter().enumerate() {
                js_args.set(i as u32, value_to_js(arg, heap)?);
            }
            Reflect::set(&obj, &JsValue::from_str("args"), &js_args.into())
                .map_err(|_| JsError::new("failed to set args"))?;
            let snap = ZapcodeSnapshot { inner: snapshot };
            Reflect::set(&obj, &JsValue::from_str("snapshot"), &snap.into_js()?)
                .map_err(|_| JsError::new("failed to set snapshot"))?;
            Reflect::set(
                &obj,
                &JsValue::from_str("stdout"),
                &JsValue::from_str(stdout),
            )
            .map_err(|_| JsError::new("failed to set stdout"))?;
        }
        VmState::SuspendedMany {
            calls,
            combinator,
            snapshot,
        } => {
            Reflect::set(&obj, &JsValue::from_str("suspended"), &JsValue::from(true))
                .map_err(|_| JsError::new("failed to set suspended"))?;
            Reflect::set(
                &obj,
                &JsValue::from_str("suspendedMany"),
                &JsValue::from(true),
            )
            .map_err(|_| JsError::new("failed to set suspendedMany"))?;
            Reflect::set(
                &obj,
                &JsValue::from_str("combinator"),
                &JsValue::from_str(combinator.as_str()),
            )
            .map_err(|_| JsError::new("failed to set combinator"))?;
            let js_calls = Array::new_with_length(calls.len() as u32);
            for (i, call) in calls.iter().enumerate() {
                let call_obj = Object::new();
                Reflect::set(
                    &call_obj,
                    &JsValue::from_str("name"),
                    &JsValue::from_str(&call.name),
                )
                .map_err(|_| JsError::new("failed to set call name"))?;
                let js_args = Array::new_with_length(call.args.len() as u32);
                for (j, arg) in call.args.iter().enumerate() {
                    js_args.set(j as u32, value_to_js(arg, heap)?);
                }
                Reflect::set(&call_obj, &JsValue::from_str("args"), &js_args.into())
                    .map_err(|_| JsError::new("failed to set call args"))?;
                js_calls.set(i as u32, call_obj.into());
            }
            Reflect::set(&obj, &JsValue::from_str("calls"), &js_calls.into())
                .map_err(|_| JsError::new("failed to set calls"))?;
            let snap = ZapcodeSnapshot { inner: snapshot };
            Reflect::set(&obj, &JsValue::from_str("snapshot"), &snap.into_js()?)
                .map_err(|_| JsError::new("failed to set snapshot"))?;
            Reflect::set(
                &obj,
                &JsValue::from_str("stdout"),
                &JsValue::from_str(stdout),
            )
            .map_err(|_| JsError::new("failed to set stdout"))?;
        }
    }
    if let Some(t) = trace {
        Reflect::set(&obj, &"trace".into(), &trace_span_to_js(&t.root)?)
            .map_err(|_| JsError::new("failed to set trace"))?;
    }
    Ok(obj.into())
}

// ---------------------------------------------------------------------------
// ZapcodeSnapshot — snapshot / resume
// ---------------------------------------------------------------------------

#[wasm_bindgen]
pub struct ZapcodeSnapshot {
    inner: CoreSnapshot,
}

impl ZapcodeSnapshot {
    /// Convert snapshot to a JsValue (for embedding in result objects).
    fn into_js(self) -> Result<JsValue, JsError> {
        // Return as a wasm_bindgen class instance.
        Ok(JsValue::from(self))
    }
}

#[wasm_bindgen]
impl ZapcodeSnapshot {
    /// Serialize the snapshot to bytes (Uint8Array).
    pub fn dump(&self) -> Result<Vec<u8>, JsError> {
        self.inner.dump().map_err(zapcode_err)
    }

    /// Deserialize a snapshot from bytes.
    pub fn load(bytes: &[u8]) -> Result<ZapcodeSnapshot, JsError> {
        let inner = CoreSnapshot::load(bytes).map_err(zapcode_err)?;
        Ok(Self { inner })
    }

    /// Resume execution with a return value from the external function.
    ///
    /// @param return_value - The value to return to the suspended external call.
    /// @returns Same shape as `Zapcode.run()`.
    pub fn resume(&self, return_value: JsValue) -> Result<JsValue, JsError> {
        // Allocate any array/object in the return value into the snapshot's own
        // heap so its handles are valid when the snapshot is restored on resume.
        let mut snapshot = self.inner.clone();
        let val = js_to_value(&return_value, snapshot.heap_mut())?;
        let result = snapshot.resume(val).map_err(zapcode_err)?;
        run_result_to_js(result, false)
    }
}
