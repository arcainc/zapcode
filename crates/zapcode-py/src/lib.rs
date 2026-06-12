use std::sync::Arc;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyList, PyString};

use zapcode_core::heap::Heap;
use zapcode_core::{
    ExecutionTrace, ResourceLimits, RunResult, TraceSpan as CoreTraceSpan, TraceStatus, Value,
    VmState, ZapcodeError, ZapcodeSnapshot as CoreSnapshot,
};

// ---------------------------------------------------------------------------
// Value conversion: zapcode_core::Value <-> Python object
// ---------------------------------------------------------------------------

/// Convert a Python object to a `zapcode_core::Value`, allocating any
/// list/dict into `heap` (array/object `Value`s carry a `Handle`, not inline
/// contents).
fn py_to_value(obj: &Bound<'_, PyAny>, heap: &mut Heap) -> PyResult<Value> {
    if obj.is_none() {
        Ok(Value::Null)
    } else if let Ok(b) = obj.downcast::<PyBool>() {
        Ok(Value::Bool(b.is_true()))
    } else if let Ok(i) = obj.downcast::<PyInt>() {
        let val: i64 = i.extract()?;
        Ok(Value::Int(val))
    } else if let Ok(f) = obj.downcast::<PyFloat>() {
        let val: f64 = f.extract()?;
        Ok(Value::Float(val))
    } else if let Ok(s) = obj.downcast::<PyString>() {
        let val: String = s.extract()?;
        Ok(Value::String(zapcode_core::JsString::from(val.as_str())))
    } else if let Ok(list) = obj.downcast::<PyList>() {
        let mut items = Vec::with_capacity(list.len());
        for item in list.iter() {
            items.push(py_to_value(&item, heap)?);
        }
        Ok(Value::Array(heap.alloc_array(items)))
    } else if let Ok(dict) = obj.downcast::<PyDict>() {
        let mut map = indexmap::IndexMap::new();
        for (k, v) in dict.iter() {
            let key: String = k.extract()?;
            let val = py_to_value(&v, heap)?;
            map.insert(Arc::from(key.as_str()), val);
        }
        Ok(Value::Object(heap.alloc_object(map)))
    } else {
        Err(PyRuntimeError::new_err(format!(
            "cannot convert Python type '{}' to Zapcode value",
            obj.get_type().name()?
        )))
    }
}

/// Convert a `zapcode_core::Value` to a Python object, dereferencing
/// array/object handles through `heap`.
/// Max nesting depth when marshalling a guest value out to Python. Guest
/// reference values can form cycles or be nested arbitrarily deep; unbounded
/// native recursion here would overflow the OS stack and abort the host. Capped
/// (and cycle-checked) so a cyclic/deep value surfaces a catchable error.
const MAX_MARSHAL_DEPTH: usize = 256;

fn value_to_py(py: Python<'_>, val: &Value, heap: &Heap) -> PyResult<PyObject> {
    let mut seen: Vec<zapcode_core::heap::Handle> = Vec::new();
    value_to_py_inner(py, val, heap, &mut seen, 0)
}

fn value_to_py_inner(
    py: Python<'_>,
    val: &Value,
    heap: &Heap,
    seen: &mut Vec<zapcode_core::heap::Handle>,
    depth: usize,
) -> PyResult<PyObject> {
    if depth > MAX_MARSHAL_DEPTH {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "value nesting depth exceeded while marshalling result (max 256)",
        ));
    }
    match val {
        Value::Undefined | Value::Null => Ok(py.None()),
        Value::Bool(b) => Ok(b.into_pyobject(py)?.to_owned().into_any().unbind()),
        Value::Int(n) => Ok(n.into_pyobject(py)?.into_any().unbind()),
        Value::Float(n) => Ok(n.into_pyobject(py)?.into_any().unbind()),
        // A BigInt marshals to a native Python int (arbitrary precision on
        // both sides; round-trips via the decimal string).
        Value::BigInt(n) => {
            let int_mod = py.import("builtins")?;
            let v = int_mod.getattr("int")?.call1((n.to_string(),))?;
            Ok(v.unbind())
        }
        Value::String(s) => Ok(s.as_ref().into_pyobject(py)?.into_any().unbind()),
        Value::Array(h) => {
            if seen.contains(h) {
                return Err(pyo3::exceptions::PyValueError::new_err(
                    "Converting circular structure to JSON",
                ));
            }
            seen.push(*h);
            let list = PyList::empty(py);
            for item in heap.array(*h) {
                list.append(value_to_py_inner(py, item, heap, seen, depth + 1)?)?;
            }
            seen.pop();
            Ok(list.into_pyobject(py)?.into_any().unbind())
        }
        Value::Object(h) => {
            let dict = PyDict::new(py);
            if let Some(map) = heap.object(*h) {
                if seen.contains(h) {
                    return Err(pyo3::exceptions::PyValueError::new_err(
                        "Converting circular structure to JSON",
                    ));
                }
                seen.push(*h);
                for (k, v) in map {
                    dict.set_item(k.as_ref(), value_to_py_inner(py, v, heap, seen, depth + 1)?)?;
                }
                seen.pop();
            }
            Ok(dict.into_pyobject(py)?.into_any().unbind())
        }
        Value::Function(_) | Value::BuiltinMethod { .. } => {
            // Functions cannot be meaningfully represented in Python.
            Ok("<function>".into_pyobject(py)?.into_any().unbind())
        }
        Value::Generator(_) => Ok("<generator>".into_pyobject(py)?.into_any().unbind()),
        // A deferred batch call never escapes to a result value.
        Value::Pending(_) => Ok(py.None()),
    }
}

/// Convert a `ZapcodeError` to a Python `RuntimeError`.
fn zapcode_err(e: ZapcodeError) -> PyErr {
    PyRuntimeError::new_err(e.to_string())
}

/// Extract input key-value pairs from an optional Python dict into
/// `Vec<(String, Value)>`, allocating any list/dict inputs into a fresh `Heap`.
/// The heap is returned alongside so the caller can pass it to the heap-seeding
/// core entry point (`run_with_input_heap`); the input handles are valid there.
fn extract_inputs(inputs: Option<&Bound<'_, PyDict>>) -> PyResult<(Vec<(String, Value)>, Heap)> {
    let mut heap = Heap::new();
    let mut out = Vec::new();
    if let Some(dict) = inputs {
        for (k, v) in dict.iter() {
            let key: String = k.extract()?;
            let val = py_to_value(&v, &mut heap)?;
            out.push((key, val));
        }
    }
    Ok((out, heap))
}

// ---------------------------------------------------------------------------
// Zapcode — main entry point
// ---------------------------------------------------------------------------

#[pyclass]
struct Zapcode {
    inner: zapcode_core::ZapcodeRun,
}

#[pymethods]
impl Zapcode {
    /// Create a new Zapcode instance.
    ///
    /// Args:
    ///     code: TypeScript source code to execute.
    ///     inputs: List of input variable names that will be injected at runtime.
    ///     external_functions: List of external function names the sandbox may call.
    ///     memory_limit_bytes: Maximum memory in bytes (default 32MB).
    ///     time_limit_ms: Maximum execution time in milliseconds (default 5000).
    ///     max_stack_depth: Maximum call stack depth (default 512).
    ///     max_allocations: Maximum number of heap allocations (default 100000).
    #[new]
    #[pyo3(signature = (code, inputs=None, external_functions=None, memory_limit_bytes=None, time_limit_ms=None, max_stack_depth=None, max_allocations=None))]
    fn new(
        code: String,
        inputs: Option<Vec<String>>,
        external_functions: Option<Vec<String>>,
        memory_limit_bytes: Option<usize>,
        time_limit_ms: Option<u64>,
        max_stack_depth: Option<usize>,
        max_allocations: Option<usize>,
    ) -> PyResult<Self> {
        let defaults = ResourceLimits::default();
        let limits = ResourceLimits {
            memory_limit_bytes: memory_limit_bytes.unwrap_or(defaults.memory_limit_bytes),
            time_limit_ms: time_limit_ms.unwrap_or(defaults.time_limit_ms),
            max_stack_depth: max_stack_depth.unwrap_or(defaults.max_stack_depth),
            max_allocations: max_allocations.unwrap_or(defaults.max_allocations),
            max_snapshot_bytes: defaults.max_snapshot_bytes,
        };
        let inner = zapcode_core::ZapcodeRun::new(
            code,
            inputs.unwrap_or_default(),
            external_functions.unwrap_or_default(),
            limits,
        )
        .map_err(zapcode_err)?;
        Ok(Self { inner })
    }

    /// Run the program to completion.
    ///
    /// Args:
    ///     inputs: Optional dict of input name -> value mappings.
    ///
    /// Returns:
    ///     A dict with keys "output" (the final value) and "stdout" (captured output).
    ///     If execution suspends on an external function, returns a dict with
    ///     "suspended", "function_name", "args", and "snapshot" keys instead.
    #[pyo3(signature = (inputs=None))]
    fn run(&self, py: Python<'_>, inputs: Option<&Bound<'_, PyDict>>) -> PyResult<PyObject> {
        let (input_values, input_heap) = extract_inputs(inputs)?;
        let result = self
            .inner
            .run_with_input_heap(input_values, input_heap)
            .map_err(zapcode_err)?;
        full_run_result_to_py(py, result, true)
    }

    /// Start execution, returning raw state (for suspension / snapshot handling).
    ///
    /// Args:
    ///     inputs: Optional dict of input name -> value mappings.
    ///
    /// Returns:
    ///     Same shape as `run()`.
    #[pyo3(signature = (inputs=None))]
    fn start(&self, py: Python<'_>, inputs: Option<&Bound<'_, PyDict>>) -> PyResult<PyObject> {
        let (input_values, input_heap) = extract_inputs(inputs)?;
        let result = self
            .inner
            .run_with_input_heap(input_values, input_heap)
            .map_err(zapcode_err)?;
        full_run_result_to_py(py, result, true)
    }
}

/// Convert a `TraceSpan` to a Python dict.
fn trace_span_to_py(py: Python<'_>, span: &CoreTraceSpan) -> PyResult<PyObject> {
    let dict = PyDict::new(py);
    dict.set_item("name", &span.name)?;
    dict.set_item("start_time_ms", span.start_time_ms)?;
    dict.set_item("end_time_ms", span.end_time_ms)?;
    dict.set_item("duration_us", span.duration_us)?;
    dict.set_item(
        "status",
        match span.status {
            TraceStatus::Ok => "ok",
            TraceStatus::Error => "error",
        },
    )?;
    let attrs = PyDict::new(py);
    for (k, v) in &span.attributes {
        attrs.set_item(k, v)?;
    }
    dict.set_item("attributes", attrs)?;
    let children = PyList::empty(py);
    for child in &span.children {
        children.append(trace_span_to_py(py, child)?)?;
    }
    dict.set_item("children", children)?;
    Ok(dict.into_pyobject(py)?.into_any().unbind())
}

/// Convert a `VmState` (+ optional stdout + trace) to a Python dict. `heap`
/// resolves the array/object handles in the completed output or a suspension's
/// args/calls.
fn run_result_to_py(
    py: Python<'_>,
    state: VmState,
    heap: &Heap,
    stdout: &str,
    trace: Option<&ExecutionTrace>,
) -> PyResult<PyObject> {
    let dict = PyDict::new(py);
    match state {
        VmState::Complete(value) => {
            dict.set_item("output", value_to_py(py, &value, heap)?)?;
            dict.set_item("stdout", stdout)?;
        }
        VmState::Suspended {
            function_name,
            args,
            mut snapshot,
        } => {
            dict.set_item("suspended", true)?;
            dict.set_item("function_name", &function_name)?;
            let py_args = PyList::empty(py);
            // Suspension args index the SNAPSHOT's heap (compacted at
            // capture), not the live VM heap — marshal against it.
            for arg in &args {
                py_args.append(value_to_py(py, arg, snapshot.heap())?)?;
            }
            dict.set_item("args", py_args)?;
            dict.set_item("snapshot", ZapcodeSnapshot { inner: snapshot })?;
            dict.set_item("stdout", stdout)?;
        }
        VmState::SuspendedMany {
            calls,
            combinator,
            mut snapshot,
        } => {
            dict.set_item("suspended", true)?;
            dict.set_item("suspended_many", true)?;
            dict.set_item("combinator", combinator.as_str())?;
            let py_calls = PyList::empty(py);
            for call in &calls {
                let call_dict = PyDict::new(py);
                call_dict.set_item("name", &call.name)?;
                let py_args = PyList::empty(py);
                for arg in &call.args {
                    py_args.append(value_to_py(py, arg, snapshot.heap())?)?;
                }
                call_dict.set_item("args", py_args)?;
                py_calls.append(call_dict)?;
            }
            dict.set_item("calls", py_calls)?;
            dict.set_item("snapshot", ZapcodeSnapshot { inner: snapshot })?;
            dict.set_item("stdout", stdout)?;
        }
    }
    if let Some(t) = trace {
        dict.set_item("trace", trace_span_to_py(py, &t.root)?)?;
    }
    Ok(dict.into_pyobject(py)?.into_any().unbind())
}

/// Marshal a whole [`RunResult`] (state + heap + stdout + optional trace) into a
/// Python dict.
fn full_run_result_to_py(
    py: Python<'_>,
    result: RunResult,
    include_trace: bool,
) -> PyResult<PyObject> {
    let RunResult {
        state,
        heap,
        stdout,
        trace,
    } = result;
    let trace_ref = if include_trace { Some(&trace) } else { None };
    run_result_to_py(py, state, &heap, &stdout, trace_ref)
}

// ---------------------------------------------------------------------------
// ZapcodeSnapshot — snapshot / resume
// ---------------------------------------------------------------------------

#[pyclass]
struct ZapcodeSnapshot {
    inner: CoreSnapshot,
}

#[pymethods]
impl ZapcodeSnapshot {
    /// Serialize the snapshot to bytes.
    fn dump(&self) -> PyResult<Vec<u8>> {
        self.inner.dump().map_err(zapcode_err)
    }

    /// Deserialize a snapshot from bytes.
    #[staticmethod]
    fn load(bytes: Vec<u8>) -> PyResult<Self> {
        let inner = CoreSnapshot::load(&bytes).map_err(zapcode_err)?;
        Ok(Self { inner })
    }

    /// Resume execution with a return value from the external function.
    ///
    /// Args:
    ///     return_value: The value to return to the suspended external call.
    ///
    /// Returns:
    ///     A dict with either "output" or "suspended" keys (same shape as Zapcode.run()).
    fn resume(&self, py: Python<'_>, return_value: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        // Allocate any list/dict in the return value into the snapshot's own heap
        // so its handles are valid when the snapshot is restored on resume.
        let mut snapshot = self.inner.clone();
        let val = py_to_value(return_value, snapshot.heap_mut())?;
        let result = snapshot.resume(val).map_err(zapcode_err)?;
        full_run_result_to_py(py, result, false)
    }

    /// Resume by raising an error at the suspended external call (a failed
    /// tool). Catchable by a surrounding try/catch in the guest.
    fn resume_error(&self, py: Python<'_>, error: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let mut snapshot = self.inner.clone();
        let val = py_to_value(error, snapshot.heap_mut())?;
        let result = snapshot.resume_with_error(val).map_err(zapcode_err)?;
        full_run_result_to_py(py, result, false)
    }

    /// Resume a `Promise.all` batch suspension with one result per call, in the
    /// order the calls were presented.
    fn resume_many(&self, py: Python<'_>, results: &Bound<'_, PyList>) -> PyResult<PyObject> {
        let mut snapshot = self.inner.clone();
        let mut values = Vec::with_capacity(results.len());
        for item in results.iter() {
            values.push(py_to_value(&item, snapshot.heap_mut())?);
        }
        let result = snapshot.resume_many(values).map_err(zapcode_err)?;
        full_run_result_to_py(py, result, false)
    }
}

// ---------------------------------------------------------------------------
// Module registration
// ---------------------------------------------------------------------------

#[pymodule]
fn zapcode(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Zapcode>()?;
    m.add_class::<ZapcodeSnapshot>()?;
    Ok(())
}
