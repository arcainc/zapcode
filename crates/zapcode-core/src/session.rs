use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::compiler::instruction::BatchKind;
use crate::jsstring::JsString;
use crate::compiler::{compile_session_chunk, CompiledProgram, TopLevelBindingKind};
use crate::error::{Result, ZapcodeError};
use crate::sandbox::ResourceLimits;
use crate::snapshot::VmSnapshot;
use crate::value::Value;
use crate::vm::{ExternalCall, Vm, VmState};
use crate::wire::FrameKind;

const RESERVED_SESSION_GLOBALS: &[&str] = Vm::BUILTIN_GLOBAL_NAMES;

const RESERVED_JS_WORDS: &[&str] = &[
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "let",
    "new",
    "null",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "undefined",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

#[derive(Debug)]
pub enum ZapcodeSessionState {
    Complete {
        output: Value,
        stdout: String,
        stderr: String,
        session: ZapcodeSessionSnapshot,
    },
    Suspended {
        function_name: String,
        args: Vec<Value>,
        stdout: String,
        stderr: String,
        session: ZapcodeSessionSnapshot,
    },
    /// Suspended on a batch of external calls (`Promise.{all,race,any,
    /// allSettled}([...])`) the host can run in parallel. `combinator` selects
    /// the `Promise.*` settle semantics. Resume with `resume_many` (or
    /// `resume_with_error` on rejection).
    SuspendedMany {
        calls: Vec<ExternalCall>,
        combinator: BatchKind,
        stdout: String,
        stderr: String,
        session: ZapcodeSessionSnapshot,
    },
}

impl ZapcodeSessionState {
    /// Borrow the object heap that resolves the array/object handles in this
    /// state's `output` / `args` / `calls`. Hosts need it to marshal those
    /// results out to JSON.
    pub fn heap(&mut self) -> &crate::heap::Heap {
        match self {
            ZapcodeSessionState::Complete { session, .. }
            | ZapcodeSessionState::Suspended { session, .. }
            | ZapcodeSessionState::SuspendedMany { session, .. } => session.heap(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZapcodeSessionSnapshot {
    data: SessionSnapshotData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum SessionSnapshotData {
    Idle(IdleSessionState),
    Suspended(Box<SuspendedSessionState>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IdleSessionState {
    /// Compiled chunks, shared behind `Arc` so an idle session captured between
    /// chunks bumps refcounts instead of deep-copying every chunk's bytecode.
    /// `Arc<T>` serializes identically to `T`, so the wire format is unchanged.
    programs: Vec<std::sync::Arc<CompiledProgram>>,
    globals: Vec<(String, Value)>,
    top_level_bindings: Vec<(String, TopLevelBindingKind)>,
    limits: ResourceLimits,
    external_functions: Vec<String>,
    next_generator_id: u64,
    /// Cumulative allocation count across the whole session, so a long sequence
    /// of chunks can't evade `max_allocations` by resetting per chunk.
    #[serde(default)]
    allocations: usize,
    /// Deterministic PRNG state for `Math.random`, carried across chunks.
    #[serde(default)]
    rng_state: u64,
    /// The object heap backing the persisted `globals`. Array/object globals
    /// hold `Handle`s into this heap, so it must travel with the idle state or
    /// those handles dangle when the next chunk runs.
    #[serde(default)]
    heap: crate::heap::Heap,
    /// Shared upvalue cells (captured function-local variables) backing any
    /// persisted closure's `env`. A closure RETURNED from a nested call keeps
    /// its captured locals in cells whose ids its `env` references; without
    /// carrying the arena forward, the next chunk starts with an empty `cells`
    /// and those captures read `undefined`. Indices double as ids, so preserving
    /// the `Vec` keeps every closure's `env` ids aligned across reload.
    #[serde(default)]
    cells: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SuspendedSessionState {
    vm: VmSnapshot,
    stdout_len: usize,
    /// Length of the cumulative `stderr` at the start of this chunk, so the
    /// next resume reports only the stderr this chunk/resume step produced
    /// (mirrors `stdout_len`). Added at wire v16.
    #[serde(default)]
    stderr_len: usize,
    top_level_bindings: Vec<(String, TopLevelBindingKind)>,
    transient_input_names: Vec<String>,
}

impl ZapcodeSessionSnapshot {
    pub fn new(external_functions: Vec<String>, limits: ResourceLimits) -> Result<Self> {
        validate_external_functions(&external_functions)?;
        Ok(Self {
            data: SessionSnapshotData::Idle(IdleSessionState {
                programs: Vec::new(),
                globals: Vec::new(),
                top_level_bindings: Vec::new(),
                limits,
                external_functions,
                next_generator_id: 0,
                allocations: 0,
                rng_state: 0,
                heap: crate::heap::Heap::new(),
                cells: Vec::new(),
            }),
        })
    }

    pub fn dump(&self) -> Result<Vec<u8>> {
        let payload = postcard::to_allocvec(self)
            .map_err(|e| ZapcodeError::SnapshotError(format!("dump failed: {}", e)))?;
        crate::wire::check_state_size(payload.len(), self.max_snapshot_bytes())?;
        Ok(crate::wire::encode_frame(FrameKind::Session, &payload))
    }

    /// Borrow the object heap backing this session. Array/object `Value`s held in
    /// the session's globals — or returned in a [`ZapcodeSessionState`]'s
    /// `output`/`args`/`calls` — carry `Handle`s into this heap, so a host needs
    /// it to read their elements / fields when marshalling results out.
    pub fn heap(&mut self) -> &crate::heap::Heap {
        match &mut self.data {
            SessionSnapshotData::Idle(idle) => &idle.heap,
            SessionSnapshotData::Suspended(s) => {
                // The captured VM heap is template-elided; hand out the
                // materialized layout the suspension's args/handles index.
                s.vm.materialize_heap();
                &s.vm.heap
            }
        }
    }

    fn max_snapshot_bytes(&self) -> usize {
        match &self.data {
            SessionSnapshotData::Idle(idle) => idle.limits.max_snapshot_bytes,
            SessionSnapshotData::Suspended(s) => s.vm.limits.max_snapshot_bytes,
        }
    }

    pub fn load(bytes: &[u8]) -> Result<Self> {
        let payload = crate::wire::decode_frame(
            FrameKind::Session,
            bytes,
            crate::wire::MAX_LOAD_DECOMPRESSED_BYTES,
        )?;
        let mut snapshot: Self = postcard::from_bytes(&payload)
            .map_err(|e| ZapcodeError::SnapshotError(format!("load failed: {}", e)))?;
        // Clamp untrusted, blob-embedded resource limits down to safe defaults so
        // a forged/tampered session can't raise its own limits to bypass sandbox
        // enforcement when the next chunk runs (the wire SHA is keyless — it
        // detects corruption, not forgery).
        match &mut snapshot.data {
            SessionSnapshotData::Idle(idle) => idle.limits.clamp_to_default(),
            SessionSnapshotData::Suspended(s) => s.vm.limits.clamp_to_default(),
        }
        Ok(snapshot)
    }

    pub fn run_chunk(
        &self,
        source: String,
        input_values: Vec<(String, Value)>,
    ) -> Result<ZapcodeSessionState> {
        self.run_chunk_with_input_heap(source, input_values, crate::heap::Heap::new())
    }

    /// Like [`run_chunk`], but `input_heap` backs any array/object `Value`s in
    /// `input_values`. The input heap is merged into the session's live heap and
    /// the input handles are rebased so they stay valid. Host bindings allocate
    /// compound inputs into a fresh heap and pass it here.
    pub fn run_chunk_with_input_heap(
        &self,
        source: String,
        mut input_values: Vec<(String, Value)>,
        input_heap: crate::heap::Heap,
    ) -> Result<ZapcodeSessionState> {
        let mut idle = match self.data.clone() {
            SessionSnapshotData::Idle(idle) => idle,
            SessionSnapshotData::Suspended(_) => {
                return Err(ZapcodeError::RuntimeError(
                    "session is suspended on an external function; resume it before running a new chunk"
                        .to_string(),
                ))
            }
        };

        // Merge the host-supplied input heap into the session's live heap and
        // rebase the input handles so any array/object inputs stay valid.
        if !input_heap.is_empty() {
            let offset = idle.heap.absorb(input_heap);
            for (_, value) in input_values.iter_mut() {
                *value = crate::heap::Heap::rebase_handles(value.clone(), offset);
            }
        }

        let transient_input_names = validate_input_values(&idle, &input_values)?;

        let parsed = crate::parser::parse(&source)?;
        let ext_set: HashSet<String> = idle.external_functions.iter().cloned().collect();
        let existing_bindings: HashMap<String, TopLevelBindingKind> =
            idle.top_level_bindings.iter().cloned().collect();
        let (compiled, top_level_bindings) =
            compile_session_chunk(&parsed, ext_set.clone(), existing_bindings)?;
        validate_new_top_level_bindings(&idle, &top_level_bindings)?;

        let mut programs = idle.programs;
        programs.push(std::sync::Arc::new(compiled));
        let program_index = programs.len() - 1;

        let mut vm =
            Vm::with_programs_and_heap(programs, idle.limits.clone(), ext_set, idle.heap);
        for (name, value) in idle.globals {
            vm.globals.insert(name, value);
        }
        for (name, value) in input_values {
            vm.globals.insert(name, value);
        }
        vm.next_generator_id = idle.next_generator_id;
        // Carry the cumulative allocation budget and PRNG state forward.
        vm.tracker.allocations = idle.allocations;
        vm.rng_state = idle.rng_state;
        // Restore the shared upvalue cells so closures persisted from earlier
        // chunks (including ones returned from a nested call) keep their captured
        // function-local state. New cells allocated by this chunk append past
        // these, so the persisted closures' `env` ids stay valid.
        vm.cells = idle.cells;

        let state = vm.run_program(program_index)?;
        build_session_state(state, vm, top_level_bindings, 0, 0, transient_input_names)
    }

    pub fn resume(&self, return_value: Value) -> Result<ZapcodeSessionState> {
        self.resume_with_input_heap(return_value, crate::heap::Heap::new())
    }

    /// Like [`resume`], but `value_heap` backs any array/object handles in
    /// `return_value`. It is merged into the suspended VM's heap and the return
    /// value rebased so the handles stay valid. Host bindings allocate compound
    /// resume values into a fresh heap and pass it here.
    pub fn resume_with_input_heap(
        &self,
        return_value: Value,
        value_heap: crate::heap::Heap,
    ) -> Result<ZapcodeSessionState> {
        self.drive_resume(|vm| {
            let value = absorb_into_vm(vm, return_value, value_heap);
            vm.stack.push(value);
            vm.resume_execution()
        })
    }

    /// Resume a suspended session by raising `error` at the external call site
    /// instead of returning a value. Catchable by a surrounding `try`/`catch`
    /// in the chunk; otherwise it propagates to the host. Use when a tool /
    /// Temporal activity failed.
    pub fn resume_with_error(&self, error: Value) -> Result<ZapcodeSessionState> {
        self.resume_with_error_in_heap(error, crate::heap::Heap::new())
    }

    /// Like [`resume_with_error`], but `value_heap` backs any array/object
    /// handles in `error`.
    pub fn resume_with_error_in_heap(
        &self,
        error: Value,
        value_heap: crate::heap::Heap,
    ) -> Result<ZapcodeSessionState> {
        self.drive_resume(|vm| {
            let value = absorb_into_vm(vm, error, value_heap);
            vm.resume_with_error(value)
        })
    }

    /// Resume a session suspended on a `Promise.all` batch with one result per
    /// call, in order. Use when the host ran the batched calls in parallel.
    pub fn resume_many(&self, results: Vec<Value>) -> Result<ZapcodeSessionState> {
        self.resume_many_with_input_heap(results, crate::heap::Heap::new())
    }

    /// Like [`resume_many`], but `value_heap` backs any array/object handles in
    /// `results`.
    pub fn resume_many_with_input_heap(
        &self,
        results: Vec<Value>,
        value_heap: crate::heap::Heap,
    ) -> Result<ZapcodeSessionState> {
        self.drive_resume(|vm| {
            let values = absorb_many_into_vm(vm, results, value_heap);
            vm.resume_many(values)
        })
    }

    fn drive_resume(
        &self,
        run: impl FnOnce(&mut Vm) -> Result<VmState>,
    ) -> Result<ZapcodeSessionState> {
        let suspended = match self.data.clone() {
            SessionSnapshotData::Suspended(suspended) => *suspended,
            SessionSnapshotData::Idle(_) => {
                return Err(ZapcodeError::RuntimeError(
                    "session is idle; run a chunk before calling resume".to_string(),
                ))
            }
        };

        let mut vm = suspended.vm.restore_vm()?;
        let state = run(&mut vm)?;
        let top_level_bindings: HashMap<String, TopLevelBindingKind> =
            suspended.top_level_bindings.into_iter().collect();
        build_session_state(
            state,
            vm,
            top_level_bindings,
            suspended.stdout_len,
            suspended.stderr_len,
            suspended.transient_input_names,
        )
    }
}

/// Merge a host-supplied `value_heap` into the live VM heap and rebase `value`'s
/// top-level array/object handle so it points into the merged heap. A no-op for
/// primitives or an empty heap.
fn absorb_into_vm(vm: &mut Vm, value: Value, value_heap: crate::heap::Heap) -> Value {
    if value_heap.is_empty() {
        return value;
    }
    let offset = vm.heap.absorb(value_heap);
    crate::heap::Heap::rebase_handles(value, offset)
}

/// Like [`absorb_into_vm`] for a batch of resume values sharing one heap.
fn absorb_many_into_vm(
    vm: &mut Vm,
    values: Vec<Value>,
    value_heap: crate::heap::Heap,
) -> Vec<Value> {
    if value_heap.is_empty() {
        return values;
    }
    let offset = vm.heap.absorb(value_heap);
    values
        .into_iter()
        .map(|v| crate::heap::Heap::rebase_handles(v, offset))
        .collect()
}

fn build_session_state(
    state: VmState,
    vm: Vm,
    top_level_bindings: HashMap<String, TopLevelBindingKind>,
    stdout_prefix_len: usize,
    stderr_prefix_len: usize,
    transient_input_names: Vec<String>,
) -> Result<ZapcodeSessionState> {
    let stdout = vm.stdout.get(stdout_prefix_len..).unwrap_or("").to_string();
    let stderr = vm.stderr.get(stderr_prefix_len..).unwrap_or("").to_string();
    ensure_serializable_globals(&vm.globals, &vm.heap)?;

    match state {
        VmState::Complete(output) => Ok(ZapcodeSessionState::Complete {
            output,
            stdout,
            stderr,
            session: ZapcodeSessionSnapshot {
                data: SessionSnapshotData::Idle(IdleSessionState {
                    programs: vm.programs.clone(),
                    globals: user_globals_from_vm(&vm, &top_level_bindings, &transient_input_names),
                    external_functions: sorted_external_functions(&vm),
                    top_level_bindings: sorted_bindings(top_level_bindings),
                    limits: vm.limits.clone(),
                    next_generator_id: vm.next_generator_id,
                    allocations: vm.tracker.allocations,
                    rng_state: vm.rng_state,
                    // Carry the heap so persisted array/object globals stay valid
                    // for the next chunk.
                    heap: vm.heap.clone(),
                    // Carry the upvalue-cell arena so persisted closures keep
                    // their captured function-local state across the reload.
                    cells: vm.cells.clone(),
                }),
            },
        }),
        VmState::Suspended {
            function_name,
            args,
            snapshot,
        } => Ok(ZapcodeSessionState::Suspended {
            session: ZapcodeSessionSnapshot {
                data: SessionSnapshotData::Suspended(Box::new(SuspendedSessionState {
                    // REUSE the capture taken at suspension time: the
                    // host-facing `args` are already remapped against its
                    // compacted heap, and the VM has not executed since.
                    // Re-capturing here would compact AGAIN and leave the
                    // args pointing into the wrong layout.
                    vm: snapshot.into_vm_snapshot(),
                    stdout_len: vm.stdout.len(),
                    stderr_len: vm.stderr.len(),
                    top_level_bindings: sorted_bindings(top_level_bindings),
                    transient_input_names,
                })),
            },
            function_name,
            args,
            stdout,
            stderr,
        }),
        VmState::SuspendedMany {
            calls,
            combinator,
            snapshot,
        } => Ok(ZapcodeSessionState::SuspendedMany {
            session: ZapcodeSessionSnapshot {
                data: SessionSnapshotData::Suspended(Box::new(SuspendedSessionState {
                    vm: snapshot.into_vm_snapshot(),
                    stdout_len: vm.stdout.len(),
                    stderr_len: vm.stderr.len(),
                    top_level_bindings: sorted_bindings(top_level_bindings),
                    transient_input_names,
                })),
            },
            calls,
            combinator,
            stdout,
            stderr,
        }),
    }
}

fn validate_external_functions(external_functions: &[String]) -> Result<()> {
    let mut seen = HashSet::new();
    for name in external_functions {
        validate_identifier("external function", name)?;
        if !seen.insert(name.as_str()) {
            return Err(ZapcodeError::RuntimeError(format!(
                "duplicate external function '{}'",
                name
            )));
        }
        if RESERVED_SESSION_GLOBALS.contains(&name.as_str()) {
            return Err(ZapcodeError::RuntimeError(format!(
                "external function '{}' conflicts with reserved global '{}'",
                name, name
            )));
        }
    }
    Ok(())
}

fn validate_new_top_level_bindings(
    idle: &IdleSessionState,
    top_level_bindings: &HashMap<String, TopLevelBindingKind>,
) -> Result<()> {
    let existing_bindings: HashSet<&str> = idle
        .top_level_bindings
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    let external_functions: HashSet<&str> =
        idle.external_functions.iter().map(String::as_str).collect();

    for name in top_level_bindings.keys() {
        if existing_bindings.contains(name.as_str()) {
            continue;
        }
        if RESERVED_SESSION_GLOBALS.contains(&name.as_str()) {
            return Err(ZapcodeError::CompileError(format!(
                "top-level binding '{}' conflicts with reserved global '{}'",
                name, name
            )));
        }
        if external_functions.contains(name.as_str()) {
            return Err(ZapcodeError::CompileError(format!(
                "top-level binding '{}' conflicts with external function '{}'",
                name, name
            )));
        }
    }
    Ok(())
}

fn user_globals_from_vm(
    vm: &Vm,
    top_level_bindings: &HashMap<String, TopLevelBindingKind>,
    transient_input_names: &[String],
) -> Vec<(String, Value)> {
    let builtin_names: HashSet<&str> = Vm::BUILTIN_GLOBAL_NAMES.iter().copied().collect();
    let transient_inputs: HashSet<&str> =
        transient_input_names.iter().map(String::as_str).collect();
    let mut globals: Vec<(String, Value)> = vm
        .globals
        .iter()
        .filter(|(name, _)| !builtin_names.contains(name.as_str()))
        .filter(|(name, _)| {
            !transient_inputs.contains(name.as_str())
                || top_level_bindings.contains_key(name.as_str())
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    // Sort for deterministic bytes — see VmSnapshot::capture.
    globals.sort_by(|a, b| a.0.cmp(&b.0));
    globals
}

/// Collect persisted top-level bindings in a stable order so identical session
/// state serializes to identical bytes.
fn sorted_bindings(
    top_level_bindings: HashMap<String, TopLevelBindingKind>,
) -> Vec<(String, TopLevelBindingKind)> {
    let mut bindings: Vec<(String, TopLevelBindingKind)> = top_level_bindings.into_iter().collect();
    bindings.sort_by(|a, b| a.0.cmp(&b.0));
    bindings
}

/// External-function names in a stable order (the live set is a HashSet).
fn sorted_external_functions(vm: &Vm) -> Vec<String> {
    let mut names: Vec<String> = vm.external_functions.iter().cloned().collect();
    names.sort();
    names
}

fn validate_input_values(
    idle: &IdleSessionState,
    input_values: &[(String, Value)],
) -> Result<Vec<String>> {
    let persisted_bindings: HashSet<&str> = idle
        .top_level_bindings
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    let persisted_globals: HashSet<&str> =
        idle.globals.iter().map(|(name, _)| name.as_str()).collect();
    let reserved_builtins: HashSet<&str> = Vm::BUILTIN_GLOBAL_NAMES.iter().copied().collect();
    let external_functions: HashSet<&str> =
        idle.external_functions.iter().map(String::as_str).collect();
    let mut seen = HashSet::new();
    let mut names = Vec::with_capacity(input_values.len());

    for (name, _) in input_values {
        validate_identifier("chunk input", name)?;

        if !seen.insert(name.as_str()) {
            return Err(ZapcodeError::RuntimeError(format!(
                "duplicate chunk input '{}'",
                name
            )));
        }

        if persisted_bindings.contains(name.as_str()) || persisted_globals.contains(name.as_str()) {
            return Err(ZapcodeError::RuntimeError(format!(
                "chunk input '{}' conflicts with existing session binding '{}'",
                name, name
            )));
        }
        if reserved_builtins.contains(name.as_str()) {
            return Err(ZapcodeError::RuntimeError(format!(
                "chunk input '{}' conflicts with reserved global '{}'",
                name, name
            )));
        }
        if external_functions.contains(name.as_str()) {
            return Err(ZapcodeError::RuntimeError(format!(
                "chunk input '{}' conflicts with external function '{}'",
                name, name
            )));
        }

        names.push(name.clone());
    }

    Ok(names)
}

fn validate_identifier(label: &str, name: &str) -> Result<()> {
    if is_valid_identifier(name) {
        return Ok(());
    }
    Err(ZapcodeError::RuntimeError(format!(
        "{} '{}' is not a valid JavaScript identifier",
        label, name
    )))
}

fn is_valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first == '$' || first.is_ascii_alphabetic()) {
        return false;
    }
    if !chars.all(|c| c == '_' || c == '$' || c.is_ascii_alphanumeric()) {
        return false;
    }
    !RESERVED_JS_WORDS.contains(&name)
}

fn ensure_serializable_globals(
    globals: &HashMap<String, Value>,
    heap: &crate::heap::Heap,
) -> Result<()> {
    let builtin_names: HashSet<&str> = Vm::BUILTIN_GLOBAL_NAMES.iter().copied().collect();
    for (name, value) in globals {
        // Builtins (including the String/Number/Boolean BuiltinMethod globals)
        // are re-registered on resume, never persisted — don't validate them.
        if builtin_names.contains(name.as_str()) {
            continue;
        }
        ensure_serializable_value(value, heap).map_err(|err| {
            ZapcodeError::SnapshotError(format!(
                "cannot persist session global '{}': {}",
                name, err
            ))
        })?;
    }
    Ok(())
}

fn ensure_serializable_value(value: &Value, heap: &crate::heap::Heap) -> Result<()> {
    // Under reference semantics, array/object handles can form cycles (e.g. a
    // closure that captures the very object holding it, as in a step registry of
    // arrow functions). Track visited handles so the walk terminates instead of
    // overflowing the stack.
    let mut seen: HashSet<crate::heap::Handle> = HashSet::new();
    ensure_serializable_value_inner(value, heap, &mut seen)
}

fn ensure_serializable_value_inner(
    value: &Value,
    heap: &crate::heap::Heap,
    seen: &mut HashSet<crate::heap::Handle>,
) -> Result<()> {
    match value {
        Value::Undefined
        | Value::Null
        | Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::BigInt(_)
        | Value::String(_) => Ok(()),
        Value::Array(h) => {
            if !seen.insert(*h) {
                return Ok(());
            }
            for item in heap.array(*h) {
                ensure_serializable_value_inner(item, heap, seen)?;
            }
            Ok(())
        }
        Value::Object(h) => {
            if !seen.insert(*h) {
                return Ok(());
            }
            if let Some(map) = heap.object(*h) {
                for value in map.values() {
                    ensure_serializable_value_inner(value, heap, seen)?;
                }
            }
            Ok(())
        }
        Value::Function(closure) => {
            for (_, captured) in &closure.captured {
                ensure_serializable_value_inner(captured, heap, seen)?;
            }
            Ok(())
        }
        Value::Generator(_) => Err(ZapcodeError::SnapshotError(
            "generators cannot be persisted in ongoing sessions".to_string(),
        )),
        // A deferred call only exists transiently inside an in-flight batch; it
        // never lands in a persisted global.
        Value::Pending(_) => Ok(()),
        Value::BuiltinMethod { .. } => Err(ZapcodeError::SnapshotError(
            "builtin methods cannot be persisted in ongoing sessions".to_string(),
        )),
    }
}
