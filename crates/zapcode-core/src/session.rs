use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

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
        session: ZapcodeSessionSnapshot,
    },
    Suspended {
        function_name: String,
        args: Vec<Value>,
        stdout: String,
        session: ZapcodeSessionSnapshot,
    },
    /// Suspended on a batch of external calls (`Promise.all([...])`) the host
    /// can run in parallel. Resume with `resume_many`.
    SuspendedMany {
        calls: Vec<ExternalCall>,
        stdout: String,
        session: ZapcodeSessionSnapshot,
    },
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
    programs: Vec<CompiledProgram>,
    globals: Vec<(String, Value)>,
    top_level_bindings: Vec<(String, TopLevelBindingKind)>,
    limits: ResourceLimits,
    external_functions: Vec<String>,
    next_generator_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SuspendedSessionState {
    vm: VmSnapshot,
    stdout_len: usize,
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
            }),
        })
    }

    pub fn dump(&self) -> Result<Vec<u8>> {
        let payload = postcard::to_allocvec(self)
            .map_err(|e| ZapcodeError::SnapshotError(format!("dump failed: {}", e)))?;
        Ok(crate::wire::encode_frame(FrameKind::Session, &payload))
    }

    pub fn load(bytes: &[u8]) -> Result<Self> {
        let payload = crate::wire::decode_frame(FrameKind::Session, bytes)?;
        postcard::from_bytes(&payload)
            .map_err(|e| ZapcodeError::SnapshotError(format!("load failed: {}", e)))
    }

    pub fn run_chunk(
        &self,
        source: String,
        input_values: Vec<(String, Value)>,
    ) -> Result<ZapcodeSessionState> {
        let idle = match self.data.clone() {
            SessionSnapshotData::Idle(idle) => idle,
            SessionSnapshotData::Suspended(_) => {
                return Err(ZapcodeError::RuntimeError(
                    "session is suspended on an external function; resume it before running a new chunk"
                        .to_string(),
                ))
            }
        };

        let transient_input_names = validate_input_values(&idle, &input_values)?;

        let parsed = crate::parser::parse(&source)?;
        let ext_set: HashSet<String> = idle.external_functions.iter().cloned().collect();
        let existing_bindings: HashMap<String, TopLevelBindingKind> =
            idle.top_level_bindings.iter().cloned().collect();
        let (compiled, top_level_bindings) =
            compile_session_chunk(&parsed, ext_set.clone(), existing_bindings)?;
        validate_new_top_level_bindings(&idle, &top_level_bindings)?;

        let mut programs = idle.programs;
        programs.push(compiled);
        let program_index = programs.len() - 1;

        let mut vm = Vm::with_programs(programs, idle.limits.clone(), ext_set);
        for (name, value) in idle.globals {
            vm.globals.insert(name, value);
        }
        for (name, value) in input_values {
            vm.globals.insert(name, value);
        }
        vm.next_generator_id = idle.next_generator_id;

        let state = vm.run_program(program_index)?;
        build_session_state(state, vm, top_level_bindings, 0, transient_input_names)
    }

    pub fn resume(&self, return_value: Value) -> Result<ZapcodeSessionState> {
        self.drive_resume(|vm| {
            vm.stack.push(return_value);
            vm.resume_execution()
        })
    }

    /// Resume a suspended session by raising `error` at the external call site
    /// instead of returning a value. Catchable by a surrounding `try`/`catch`
    /// in the chunk; otherwise it propagates to the host. Use when a tool /
    /// Temporal activity failed.
    pub fn resume_with_error(&self, error: Value) -> Result<ZapcodeSessionState> {
        self.drive_resume(|vm| vm.resume_with_error(error))
    }

    /// Resume a session suspended on a `Promise.all` batch with one result per
    /// call, in order. Use when the host ran the batched calls in parallel.
    pub fn resume_many(&self, results: Vec<Value>) -> Result<ZapcodeSessionState> {
        self.drive_resume(|vm| vm.resume_many(results))
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

        let mut vm = suspended.vm.restore_vm();
        let state = run(&mut vm)?;
        let top_level_bindings: HashMap<String, TopLevelBindingKind> =
            suspended.top_level_bindings.into_iter().collect();
        build_session_state(
            state,
            vm,
            top_level_bindings,
            suspended.stdout_len,
            suspended.transient_input_names,
        )
    }
}

fn build_session_state(
    state: VmState,
    vm: Vm,
    top_level_bindings: HashMap<String, TopLevelBindingKind>,
    stdout_prefix_len: usize,
    transient_input_names: Vec<String>,
) -> Result<ZapcodeSessionState> {
    let stdout = vm.stdout.get(stdout_prefix_len..).unwrap_or("").to_string();
    ensure_serializable_globals(&vm.globals)?;

    match state {
        VmState::Complete(output) => Ok(ZapcodeSessionState::Complete {
            output,
            stdout,
            session: ZapcodeSessionSnapshot {
                data: SessionSnapshotData::Idle(IdleSessionState {
                    programs: vm.programs.clone(),
                    globals: user_globals_from_vm(&vm, &top_level_bindings, &transient_input_names),
                    external_functions: sorted_external_functions(&vm),
                    top_level_bindings: sorted_bindings(top_level_bindings),
                    limits: vm.limits.clone(),
                    next_generator_id: vm.next_generator_id,
                }),
            },
        }),
        VmState::Suspended {
            function_name,
            args,
            snapshot: _,
        } => Ok(ZapcodeSessionState::Suspended {
            function_name,
            args,
            stdout,
            session: ZapcodeSessionSnapshot {
                data: SessionSnapshotData::Suspended(Box::new(SuspendedSessionState {
                    vm: VmSnapshot::capture(&vm),
                    stdout_len: vm.stdout.len(),
                    top_level_bindings: sorted_bindings(top_level_bindings),
                    transient_input_names,
                })),
            },
        }),
        VmState::SuspendedMany { calls, snapshot: _ } => Ok(ZapcodeSessionState::SuspendedMany {
            calls,
            stdout,
            session: ZapcodeSessionSnapshot {
                data: SessionSnapshotData::Suspended(Box::new(SuspendedSessionState {
                    vm: VmSnapshot::capture(&vm),
                    stdout_len: vm.stdout.len(),
                    top_level_bindings: sorted_bindings(top_level_bindings),
                    transient_input_names,
                })),
            },
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

fn ensure_serializable_globals(globals: &HashMap<String, Value>) -> Result<()> {
    for (name, value) in globals {
        ensure_serializable_value(value).map_err(|err| {
            ZapcodeError::SnapshotError(format!(
                "cannot persist session global '{}': {}",
                name, err
            ))
        })?;
    }
    Ok(())
}

fn ensure_serializable_value(value: &Value) -> Result<()> {
    match value {
        Value::Undefined
        | Value::Null
        | Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::String(_) => Ok(()),
        Value::Array(items) => {
            for item in items {
                ensure_serializable_value(item)?;
            }
            Ok(())
        }
        Value::Object(map) => {
            for value in map.values() {
                ensure_serializable_value(value)?;
            }
            Ok(())
        }
        Value::Function(closure) => {
            for (_, captured) in &closure.captured {
                ensure_serializable_value(captured)?;
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
