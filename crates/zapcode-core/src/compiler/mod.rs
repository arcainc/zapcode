pub mod instruction;

use std::collections::{HashMap, HashSet};

use crate::error::{Result, ZapcodeError};
use crate::parser::ir::*;
use instruction::*;

/// Compiled program ready for VM execution.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CompiledProgram {
    pub instructions: Vec<Instruction>,
    pub functions: Vec<CompiledFunction>,
    pub local_names: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TopLevelBindingKind {
    Const,
    Let,
    Var,
    Function,
    Class,
}

impl TopLevelBindingKind {
    fn from_var_kind(kind: VarKind) -> Self {
        match kind {
            VarKind::Const => Self::Const,
            VarKind::Let => Self::Let,
            VarKind::Var => Self::Var,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CompiledFunction {
    pub name: Option<String>,
    pub params: Vec<ParamPattern>,
    pub instructions: Vec<Instruction>,
    pub local_count: usize,
    pub local_names: Vec<String>,
    pub is_async: bool,
    pub is_generator: bool,
}

struct Compiler {
    instructions: Vec<Instruction>,
    locals: Vec<String>,
    local_indices: HashMap<String, usize>,
    functions: Vec<CompiledFunction>,
    loop_stack: Vec<LoopInfo>,
    external_functions: HashSet<String>,
    mode: CompilerMode,
    top_level_bindings: HashMap<String, TopLevelBindingKind>,
    /// Label attached to the next loop (from a `label:` statement), if any.
    pending_label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompilerMode {
    Standard,
    SessionChunk,
}

struct LoopInfo {
    break_patches: Vec<usize>,
    continue_patches: Vec<usize>,
    label: Option<String>,
}

impl Compiler {
    fn new(external_functions: HashSet<String>) -> Self {
        Self {
            instructions: Vec::new(),
            locals: Vec::new(),
            local_indices: HashMap::new(),
            functions: Vec::new(),
            loop_stack: Vec::new(),
            external_functions,
            mode: CompilerMode::Standard,
            top_level_bindings: HashMap::new(),
            pending_label: None,
        }
    }

    fn new_session_chunk(
        external_functions: HashSet<String>,
        top_level_bindings: HashMap<String, TopLevelBindingKind>,
    ) -> Self {
        Self {
            instructions: Vec::new(),
            locals: Vec::new(),
            local_indices: HashMap::new(),
            functions: Vec::new(),
            loop_stack: Vec::new(),
            external_functions,
            mode: CompilerMode::SessionChunk,
            top_level_bindings,
            pending_label: None,
        }
    }

    fn emit(&mut self, instr: Instruction) -> usize {
        let idx = self.instructions.len();
        self.instructions.push(instr);
        idx
    }

    fn current_offset(&self) -> usize {
        self.instructions.len()
    }

    fn patch_jump(&mut self, instr_idx: usize, target: usize) {
        match &mut self.instructions[instr_idx] {
            Instruction::Jump(t)
            | Instruction::JumpIfFalse(t)
            | Instruction::JumpIfTrue(t)
            | Instruction::JumpIfNullish(t) => {
                *t = target;
            }
            Instruction::SetupTry(catch_target, _) => {
                *catch_target = target;
            }
            _ => {}
        }
    }

    fn declare_local(&mut self, name: &str) -> usize {
        if let Some(&idx) = self.local_indices.get(name) {
            return idx;
        }
        let idx = self.locals.len();
        self.locals.push(name.to_string());
        self.local_indices.insert(name.to_string(), idx);
        idx
    }

    fn resolve_local(&self, name: &str) -> Option<usize> {
        self.local_indices.get(name).copied()
    }

    fn is_session_chunk(&self) -> bool {
        self.mode == CompilerMode::SessionChunk
    }

    fn record_top_level_binding(&mut self, name: &str, kind: TopLevelBindingKind) -> Result<()> {
        if !self.is_session_chunk() {
            return Ok(());
        }

        if self.top_level_bindings.contains_key(name) {
            return Err(ZapcodeError::CompileError(format!(
                "top-level binding '{}' has already been declared in this session",
                name
            )));
        }

        self.top_level_bindings.insert(name.to_string(), kind);
        Ok(())
    }

    fn top_level_store_instruction(&self, name: &str, idx: usize) -> Instruction {
        if self.is_session_chunk() {
            Instruction::StoreGlobal(name.to_string())
        } else {
            Instruction::StoreLocal(idx)
        }
    }

    fn compile_program(&mut self, program: &Program) -> Result<()> {
        // First pass: compile all function definitions
        for func_def in &program.functions {
            let compiled = self.compile_function_def(func_def)?;
            self.functions.push(compiled);
        }

        // Second pass: compile body
        // For the last statement, if it's an expression, keep the value on the stack
        let len = program.body.len();
        for (i, stmt) in program.body.iter().enumerate() {
            let is_last = i == len - 1;
            if is_last {
                if let Statement::Expression { expr, .. } = stmt {
                    self.compile_expr(expr)?;
                    // Don't pop — leave value on stack as program result
                } else {
                    self.compile_statement(stmt)?;
                }
            } else {
                self.compile_statement(stmt)?;
            }
        }

        Ok(())
    }

    fn compile_function_def(&mut self, func: &FunctionDef) -> Result<CompiledFunction> {
        let mut func_compiler = Compiler::new(self.external_functions.clone());

        // Set up parameters as locals
        for param in &func.params {
            match param {
                ParamPattern::Ident(name) => {
                    func_compiler.declare_local(name);
                }
                ParamPattern::Rest(name) => {
                    func_compiler.declare_local(name);
                }
                ParamPattern::DefaultValue { pattern, .. } => {
                    if let ParamPattern::Ident(name) = pattern.as_ref() {
                        func_compiler.declare_local(name);
                    }
                }
                ParamPattern::ObjectDestructure(fields) => {
                    func_compiler.declare_destructure_locals(fields);
                }
                ParamPattern::ArrayDestructure(elems) => {
                    for elem in elems.iter().flatten() {
                        if let ParamPattern::Ident(name) = elem {
                            func_compiler.declare_local(name);
                        }
                    }
                }
            }
        }

        // Apply default parameter values: `if (param === undefined) param = <default>`.
        for param in &func.params {
            match param {
                ParamPattern::DefaultValue { pattern, default } => match pattern.as_ref() {
                    ParamPattern::Ident(name) => {
                        if let Some(slot) = func_compiler.resolve_local(name) {
                            func_compiler.emit_slot_default(slot, default)?;
                        }
                    }
                    // `function f({a = 5} = {})`: a missing argument leaves the
                    // destructured fields undefined, so the field defaults below
                    // already cover it.
                    ParamPattern::ObjectDestructure(fields) => {
                        func_compiler.emit_object_param_defaults(fields)?;
                    }
                    _ => {}
                },
                ParamPattern::ObjectDestructure(fields) => {
                    func_compiler.emit_object_param_defaults(fields)?;
                }
                _ => {}
            }
        }

        for stmt in &func.body {
            func_compiler.compile_statement(stmt)?;
        }

        // Implicit return undefined
        func_compiler.emit(Instruction::Push(Constant::Undefined));
        func_compiler.emit(Instruction::Return);

        Ok(CompiledFunction {
            name: func.name.clone(),
            params: func.params.clone(),
            instructions: func_compiler.instructions,
            local_count: func_compiler.locals.len(),
            local_names: func_compiler.locals,
            is_async: func.is_async,
            is_generator: func.is_generator,
        })
    }

    fn compile_statement(&mut self, stmt: &Statement) -> Result<()> {
        match stmt {
            Statement::VariableDecl {
                kind, declarations, ..
            } => {
                for decl in declarations {
                    self.compile_var_declarator(decl, *kind)?;
                }
            }
            Statement::Expression { expr, .. } => {
                self.compile_expr(expr)?;
                self.emit(Instruction::Pop);
            }
            Statement::Return { value, .. } => {
                match value {
                    Some(expr) => self.compile_expr(expr)?,
                    None => {
                        self.emit(Instruction::Push(Constant::Undefined));
                    }
                }
                self.emit(Instruction::Return);
            }
            Statement::If {
                test,
                consequent,
                alternate,
                ..
            } => {
                self.compile_expr(test)?;
                let jump_else = self.emit(Instruction::JumpIfFalse(0));

                for s in consequent {
                    self.compile_statement(s)?;
                }

                if let Some(alt) = alternate {
                    let jump_end = self.emit(Instruction::Jump(0));
                    let else_target = self.current_offset();
                    self.patch_jump(jump_else, else_target);

                    for s in alt {
                        self.compile_statement(s)?;
                    }
                    let end_target = self.current_offset();
                    self.patch_jump(jump_end, end_target);
                } else {
                    let else_target = self.current_offset();
                    self.patch_jump(jump_else, else_target);
                }
            }
            Statement::While { test, body, .. } => {
                let loop_start = self.current_offset();
                self.loop_stack.push(LoopInfo {
                    break_patches: Vec::new(),
                    continue_patches: Vec::new(),
                    label: self.pending_label.take(),
                });

                self.compile_expr(test)?;
                let exit_jump = self.emit(Instruction::JumpIfFalse(0));

                for s in body {
                    self.compile_statement(s)?;
                }

                self.emit(Instruction::Jump(loop_start));
                let loop_end = self.current_offset();
                self.patch_jump(exit_jump, loop_end);

                let loop_info = self.loop_stack.pop().unwrap();
                for patch in loop_info.break_patches {
                    self.patch_jump(patch, loop_end);
                }
                for patch in loop_info.continue_patches {
                    self.patch_jump(patch, loop_start);
                }
            }
            Statement::DoWhile { body, test, .. } => {
                let loop_start = self.current_offset();
                self.loop_stack.push(LoopInfo {
                    break_patches: Vec::new(),
                    continue_patches: Vec::new(),
                    label: self.pending_label.take(),
                });

                for s in body {
                    self.compile_statement(s)?;
                }

                let continue_target = self.current_offset();
                self.compile_expr(test)?;
                self.emit(Instruction::JumpIfTrue(loop_start));

                let loop_end = self.current_offset();
                let loop_info = self.loop_stack.pop().unwrap();
                for patch in loop_info.break_patches {
                    self.patch_jump(patch, loop_end);
                }
                for patch in loop_info.continue_patches {
                    self.patch_jump(patch, continue_target);
                }
            }
            Statement::For {
                init,
                test,
                update,
                body,
                ..
            } => {
                if let Some(init) = init {
                    self.compile_statement(init)?;
                }

                // `for (let i ...)` gives each iteration a fresh binding of the
                // loop variables. Collect the let/const-declared slots so we can
                // re-bind any captured ones per iteration (see FreshenBinding).
                let per_iter_slots: Vec<usize> = match init.as_deref() {
                    Some(Statement::VariableDecl {
                        kind: VarKind::Let | VarKind::Const,
                        declarations,
                        ..
                    }) => declarations
                        .iter()
                        .filter_map(|d| match &d.pattern {
                            AssignTarget::Ident(name) => self.resolve_local(name),
                            _ => None,
                        })
                        .collect(),
                    _ => Vec::new(),
                };

                let loop_start = self.current_offset();
                self.loop_stack.push(LoopInfo {
                    break_patches: Vec::new(),
                    continue_patches: Vec::new(),
                    label: self.pending_label.take(),
                });

                let exit_jump = if let Some(test) = test {
                    self.compile_expr(test)?;
                    Some(self.emit(Instruction::JumpIfFalse(0)))
                } else {
                    None
                };

                for s in body {
                    self.compile_statement(s)?;
                }

                let continue_target = self.current_offset();
                // Freshen captured let-loop bindings before the update runs, so a
                // closure created this iteration keeps the value it saw rather
                // than sharing the updated binding with later iterations.
                for &slot in &per_iter_slots {
                    self.emit(Instruction::FreshenBinding(slot));
                }
                if let Some(update) = update {
                    self.compile_expr(update)?;
                    self.emit(Instruction::Pop);
                }

                self.emit(Instruction::Jump(loop_start));
                let loop_end = self.current_offset();

                if let Some(exit) = exit_jump {
                    self.patch_jump(exit, loop_end);
                }

                let loop_info = self.loop_stack.pop().unwrap();
                for patch in loop_info.break_patches {
                    self.patch_jump(patch, loop_end);
                }
                for patch in loop_info.continue_patches {
                    self.patch_jump(patch, continue_target);
                }
            }
            Statement::ForOf {
                binding,
                iterable,
                body,
                ..
            } => {
                self.compile_expr(iterable)?;
                self.emit(Instruction::GetIterator);

                let loop_start = self.current_offset();
                self.loop_stack.push(LoopInfo {
                    break_patches: Vec::new(),
                    continue_patches: Vec::new(),
                    label: self.pending_label.take(),
                });

                self.emit(Instruction::Dup);
                self.emit(Instruction::IteratorNext);
                self.emit(Instruction::IteratorDone);
                let exit_jump = self.emit(Instruction::JumpIfTrue(0));

                // Bind the value
                match binding {
                    ForBinding::Ident(name) => {
                        let idx = self.declare_local(name);
                        self.emit(Instruction::StoreLocal(idx));
                    }
                    ForBinding::Destructure(pattern) => {
                        // Destructure the iterated value into the bound names, then
                        // pop the value the pattern was read from.
                        self.compile_destructure_pattern(pattern, VarKind::Let)?;
                        self.emit(Instruction::Pop);
                    }
                }

                for s in body {
                    self.compile_statement(s)?;
                }

                self.emit(Instruction::Jump(loop_start));
                let loop_end = self.current_offset();
                self.patch_jump(exit_jump, loop_end);
                self.emit(Instruction::Pop); // pop iterator

                let loop_info = self.loop_stack.pop().unwrap();
                for patch in loop_info.break_patches {
                    self.patch_jump(patch, loop_end);
                }
                for patch in loop_info.continue_patches {
                    self.patch_jump(patch, loop_start);
                }
            }
            Statement::Block { body, .. } => {
                for s in body {
                    self.compile_statement(s)?;
                }
            }
            Statement::Throw { value, .. } => {
                self.compile_expr(value)?;
                self.emit(Instruction::Throw);
            }
            Statement::TryCatch {
                try_body,
                catch_param,
                catch_body,
                finally_body,
                ..
            } => {
                let setup = self.emit(Instruction::SetupTry(0, None));

                for s in try_body {
                    self.compile_statement(s)?;
                }
                self.emit(Instruction::EndTry);
                let jump_past_catch = self.emit(Instruction::Jump(0));

                // Catch block
                let catch_start = self.current_offset();
                self.patch_jump(setup, catch_start);

                if let Some(param) = catch_param {
                    let idx = self.declare_local(param);
                    self.emit(Instruction::StoreLocal(idx));
                } else {
                    self.emit(Instruction::Pop); // discard error
                }

                for s in catch_body {
                    self.compile_statement(s)?;
                }

                let after_catch = self.current_offset();
                self.patch_jump(jump_past_catch, after_catch);

                if let Some(finally) = finally_body {
                    for s in finally {
                        self.compile_statement(s)?;
                    }
                }
            }
            Statement::Break { label, .. } => {
                let idx = self.emit(Instruction::Jump(0));
                let target = match label {
                    Some(l) => self
                        .loop_stack
                        .iter_mut()
                        .rev()
                        .find(|li| li.label.as_deref() == Some(l.as_str())),
                    None => self.loop_stack.last_mut(),
                };
                if let Some(loop_info) = target {
                    loop_info.break_patches.push(idx);
                }
            }
            Statement::Continue { label, .. } => {
                let idx = self.emit(Instruction::Jump(0));
                let target = match label {
                    Some(l) => self
                        .loop_stack
                        .iter_mut()
                        .rev()
                        .find(|li| li.label.as_deref() == Some(l.as_str())),
                    None => self.loop_stack.last_mut(),
                };
                if let Some(loop_info) = target {
                    loop_info.continue_patches.push(idx);
                }
            }
            Statement::Labeled { label, body, .. } => {
                // The next loop/switch picks up this label; clear it afterward in
                // case the labeled statement wasn't a loop.
                self.pending_label = Some(label.clone());
                self.compile_statement(body)?;
                self.pending_label = None;
            }
            Statement::FunctionDecl { func_index, .. } => {
                self.emit(Instruction::CreateClosure(*func_index));
                let name = if *func_index < self.functions.len() {
                    self.functions[*func_index].name.clone()
                } else {
                    None
                };
                if let Some(name) = name {
                    if self.is_session_chunk() {
                        self.record_top_level_binding(&name, TopLevelBindingKind::Function)?;
                        self.emit(Instruction::StoreGlobal(name));
                    } else {
                        // Store as both local and global so recursion works
                        self.emit(Instruction::Dup);
                        let idx = self.declare_local(&name);
                        self.emit(Instruction::StoreLocal(idx));
                        self.emit(Instruction::StoreGlobal(name));
                    }
                } else {
                    self.emit(Instruction::Pop);
                }
            }
            Statement::ClassDecl {
                name,
                super_class,
                constructor,
                methods,
                static_methods,
                ..
            } => {
                self.compile_class(
                    Some(name),
                    super_class.as_deref(),
                    constructor.as_deref(),
                    methods,
                    static_methods,
                )?;
                if self.is_session_chunk() {
                    self.record_top_level_binding(name, TopLevelBindingKind::Class)?;
                    self.emit(Instruction::StoreGlobal(name.clone()));
                } else {
                    // Store the class as both local and global
                    self.emit(Instruction::Dup);
                    let idx = self.declare_local(name);
                    self.emit(Instruction::StoreLocal(idx));
                    self.emit(Instruction::StoreGlobal(name.clone()));
                }
            }
            Statement::Switch {
                discriminant,
                cases,
                ..
            } => {
                self.compile_expr(discriminant)?;
                let mut case_jumps = Vec::new();
                let mut default_jump = None;

                // Compile test expressions and jumps
                for case in cases {
                    if let Some(test) = &case.test {
                        self.emit(Instruction::Dup);
                        self.compile_expr(test)?;
                        self.emit(Instruction::StrictEq);
                        let jump = self.emit(Instruction::JumpIfTrue(0));
                        case_jumps.push(jump);
                    } else {
                        default_jump = Some(case_jumps.len());
                        case_jumps.push(0); // placeholder
                    }
                }

                let jump_end = self.emit(Instruction::Jump(0));

                // Establish a `break` target for the switch so `break;` jumps to
                // the end (an unpatched break would loop to instruction 0).
                self.loop_stack.push(LoopInfo {
                    break_patches: Vec::new(),
                    continue_patches: Vec::new(),
                    label: self.pending_label.take(),
                });

                // Compile case bodies
                let mut body_starts = Vec::new();
                for case in cases {
                    body_starts.push(self.current_offset());
                    for s in &case.consequent {
                        self.compile_statement(s)?;
                    }
                }

                let end = self.current_offset();
                self.emit(Instruction::Pop); // pop discriminant

                let switch_info = self.loop_stack.pop().unwrap();
                // `break` exits the switch.
                for patch in switch_info.break_patches {
                    self.patch_jump(patch, end);
                }
                // `continue` inside a switch targets the *enclosing* loop, not the
                // switch — forward it to the parent loop if there is one.
                if let Some(parent) = self.loop_stack.last_mut() {
                    parent.continue_patches.extend(switch_info.continue_patches);
                }

                // Patch jumps
                for (i, &jump) in case_jumps.iter().enumerate() {
                    if jump != 0 {
                        self.patch_jump(jump, body_starts[i]);
                    }
                }
                if let Some(default_idx) = default_jump {
                    // Jump to default case
                    self.patch_jump(jump_end, body_starts[default_idx]);
                } else {
                    self.patch_jump(jump_end, end);
                }
            }
        }
        Ok(())
    }

    fn compile_var_declarator(&mut self, decl: &VarDeclarator, kind: VarKind) -> Result<()> {
        match &decl.pattern {
            AssignTarget::Ident(name) => {
                let idx = if self.is_session_chunk() {
                    self.record_top_level_binding(name, TopLevelBindingKind::from_var_kind(kind))?;
                    None
                } else {
                    Some(self.declare_local(name))
                };
                match &decl.init {
                    Some(expr) => {
                        self.compile_expr(expr)?;
                        self.emit(match idx {
                            Some(idx) => self.top_level_store_instruction(name, idx),
                            None => Instruction::StoreGlobal(name.to_string()),
                        });
                    }
                    None => {
                        self.emit(Instruction::Push(Constant::Undefined));
                        self.emit(match idx {
                            Some(idx) => self.top_level_store_instruction(name, idx),
                            None => Instruction::StoreGlobal(name.to_string()),
                        });
                    }
                }
            }
            AssignTarget::ObjectDestructure(fields) => {
                if let Some(expr) = &decl.init {
                    self.compile_expr(expr)?;
                } else {
                    self.emit(Instruction::Push(Constant::Undefined));
                }
                self.compile_object_destructure(fields, kind)?;
                self.emit(Instruction::Pop); // pop source object
            }
            AssignTarget::ArrayDestructure(elems) => {
                if let Some(expr) = &decl.init {
                    self.compile_expr(expr)?;
                } else {
                    self.emit(Instruction::Push(Constant::Undefined));
                }
                for (i, elem) in elems.iter().enumerate() {
                    if let Some(target) = elem {
                        self.emit(Instruction::Dup);
                        self.emit(Instruction::Push(Constant::Int(i as i64)));
                        self.emit(Instruction::GetIndex);
                        match target {
                            AssignTarget::Ident(name) => {
                                self.store_binding(name, kind)?;
                            }
                            _ => {
                                self.emit(Instruction::Pop); // TODO: nested destructure
                            }
                        }
                    }
                }
                self.emit(Instruction::Pop); // pop source array
            }
        }
        Ok(())
    }

    fn declare_destructure_locals(&mut self, fields: &[DestructureField]) {
        for field in fields {
            if field.rest {
                let name = field.alias.as_ref().unwrap_or(&field.key);
                self.declare_local(name);
            } else if let Some(nested) = &field.nested {
                self.declare_destructure_locals(nested);
            } else {
                let name = field.alias.as_ref().unwrap_or(&field.key);
                self.declare_local(name);
            }
        }
    }

    fn store_binding(&mut self, name: &str, kind: VarKind) -> Result<()> {
        if self.is_session_chunk() {
            self.record_top_level_binding(name, TopLevelBindingKind::from_var_kind(kind))?;
            self.emit(Instruction::StoreGlobal(name.to_string()));
        } else {
            let idx = self.declare_local(name);
            self.emit(self.top_level_store_instruction(name, idx));
        }
        Ok(())
    }

    /// Build a single flattened arguments array on the stack from call args that
    /// include a spread (`f(a, ...xs, b)`), reusing the array-append instructions.
    fn compile_spread_args(&mut self, args: &[Expr]) -> Result<()> {
        self.emit(Instruction::CreateArray(0));
        for arg in args {
            if let Expr::Spread(inner) = arg {
                self.compile_expr(inner)?;
                self.emit(Instruction::ArraySpreadAppend);
            } else {
                self.compile_expr(arg)?;
                self.emit(Instruction::ArrayAppend);
            }
        }
        Ok(())
    }

    /// Destructure the value on top of the stack into the names of a parameter
    /// pattern (object or array, nested), storing each via `store_binding`. The
    /// source value is left on the stack for the caller to pop.
    fn compile_destructure_pattern(&mut self, pattern: &ParamPattern, kind: VarKind) -> Result<()> {
        match pattern {
            ParamPattern::ObjectDestructure(fields) => {
                self.compile_object_destructure(fields, kind)
            }
            ParamPattern::ArrayDestructure(elems) => {
                for (i, elem) in elems.iter().enumerate() {
                    let Some(p) = elem else { continue };
                    self.emit(Instruction::Dup);
                    self.emit(Instruction::Push(Constant::Int(i as i64)));
                    self.emit(Instruction::GetIndex);
                    match p {
                        ParamPattern::Ident(name) => self.store_binding(name, kind)?,
                        ParamPattern::ObjectDestructure(_) | ParamPattern::ArrayDestructure(_) => {
                            self.compile_destructure_pattern(p, kind)?;
                            self.emit(Instruction::Pop);
                        }
                        _ => {
                            self.emit(Instruction::Pop);
                        }
                    }
                }
                Ok(())
            }
            ParamPattern::Ident(name) => self.store_binding(name, kind),
            _ => {
                self.emit(Instruction::Pop);
                Ok(())
            }
        }
    }

    fn compile_object_destructure(
        &mut self,
        fields: &[DestructureField],
        kind: VarKind,
    ) -> Result<()> {
        let excluded_keys: Vec<String> = fields
            .iter()
            .filter(|field| !field.rest)
            .map(|field| field.key.clone())
            .collect();

        for field in fields {
            self.emit(Instruction::Dup);
            if field.rest {
                self.emit(Instruction::ObjectRest(excluded_keys.clone()));
                let name = field.alias.as_ref().unwrap_or(&field.key);
                self.store_binding(name, kind)?;
            } else {
                self.emit(Instruction::GetProperty(field.key.clone()));
                self.emit_apply_default(field.default.as_ref())?;
                if let Some(nested) = &field.nested {
                    self.compile_object_destructure(nested, kind)?;
                    self.emit(Instruction::Pop);
                } else {
                    let name = field.alias.as_ref().unwrap_or(&field.key);
                    self.store_binding(name, kind)?;
                }
            }
        }
        Ok(())
    }

    /// Emit `if (localslot === undefined) localslot = <default>`.
    fn emit_slot_default(&mut self, slot: usize, default: &Expr) -> Result<()> {
        self.emit(Instruction::LoadLocal(slot));
        self.emit(Instruction::Push(Constant::Undefined));
        self.emit(Instruction::StrictEq);
        let skip = self.emit(Instruction::JumpIfFalse(0));
        self.compile_expr(default)?;
        self.emit(Instruction::StoreLocal(slot));
        let after = self.current_offset();
        self.patch_jump(skip, after);
        Ok(())
    }

    /// Apply field defaults for a destructured object parameter (`function
    /// f({a = 5})`), whose fields were bound positionally by `bind_params`.
    fn emit_object_param_defaults(&mut self, fields: &[DestructureField]) -> Result<()> {
        for field in fields {
            if field.rest {
                continue;
            }
            let name = field.alias.as_ref().unwrap_or(&field.key);
            if let (Some(def), Some(slot)) = (&field.default, self.resolve_local(name)) {
                self.emit_slot_default(slot, def)?;
            }
        }
        Ok(())
    }

    /// If the top-of-stack value is `undefined`, replace it with the evaluated
    /// default expression. Used for destructuring defaults (`{a = 10}` / `[x = 1]`).
    fn emit_apply_default(&mut self, default: Option<&Expr>) -> Result<()> {
        let Some(default) = default else {
            return Ok(());
        };
        self.emit(Instruction::Dup);
        self.emit(Instruction::Push(Constant::Undefined));
        self.emit(Instruction::StrictEq);
        let skip = self.emit(Instruction::JumpIfFalse(0));
        self.emit(Instruction::Pop);
        self.compile_expr(default)?;
        let after = self.current_offset();
        self.patch_jump(skip, after);
        Ok(())
    }

    fn compile_expr(&mut self, expr: &Expr) -> Result<()> {
        match expr {
            Expr::NumberLit(n) => {
                if *n == (*n as i64) as f64 && !n.is_nan() && n.is_finite() {
                    self.emit(Instruction::Push(Constant::Int(*n as i64)));
                } else {
                    self.emit(Instruction::Push(Constant::Float(*n)));
                }
            }
            Expr::StringLit(s) => {
                self.emit(Instruction::Push(Constant::String(s.clone())));
            }
            Expr::BoolLit(b) => {
                self.emit(Instruction::Push(Constant::Bool(*b)));
            }
            Expr::NullLit => {
                self.emit(Instruction::Push(Constant::Null));
            }
            Expr::UndefinedLit => {
                self.emit(Instruction::Push(Constant::Undefined));
            }
            Expr::TemplateLit { quasis, exprs } => {
                let mut parts = 0;
                for (i, quasi) in quasis.iter().enumerate() {
                    if !quasi.is_empty() {
                        self.emit(Instruction::Push(Constant::String(quasi.clone())));
                        parts += 1;
                    }
                    if i < exprs.len() {
                        self.compile_expr(&exprs[i])?;
                        parts += 1;
                    }
                }
                if parts == 0 {
                    self.emit(Instruction::Push(Constant::String(String::new())));
                } else {
                    // Always concat (even a single interpolated expression) so the
                    // result is string-coerced: `${obj}` yields "[object Object]".
                    self.emit(Instruction::ConcatStrings(parts));
                }
            }
            Expr::RegExpLit { pattern, flags } => {
                self.emit(Instruction::Push(Constant::String(
                    "__regexp__".to_string(),
                )));
                self.emit(Instruction::Push(Constant::Bool(true)));
                self.emit(Instruction::Push(Constant::String("pattern".to_string())));
                self.emit(Instruction::Push(Constant::String(pattern.clone())));
                self.emit(Instruction::Push(Constant::String("flags".to_string())));
                self.emit(Instruction::Push(Constant::String(flags.clone())));
                self.emit(Instruction::CreateObject(3));
            }
            Expr::Ident(name) => {
                if name == "this" {
                    self.emit(Instruction::LoadThis);
                } else if let Some(idx) = self.resolve_local(name) {
                    self.emit(Instruction::LoadLocal(idx));
                } else {
                    self.emit(Instruction::LoadGlobal(name.clone()));
                }
            }
            Expr::Array(elements) => {
                let has_spread = elements.iter().any(|e| matches!(e, Some(Expr::Spread(_))));
                if !has_spread {
                    let mut count = 0;
                    for elem in elements {
                        match elem {
                            Some(e) => {
                                self.compile_expr(e)?;
                                count += 1;
                            }
                            None => {
                                self.emit(Instruction::Push(Constant::Undefined));
                                count += 1;
                            }
                        }
                    }
                    self.emit(Instruction::CreateArray(count));
                } else {
                    // Build incrementally so `[...a, x, ...b]` flattens correctly.
                    self.emit(Instruction::CreateArray(0));
                    for elem in elements {
                        match elem {
                            Some(Expr::Spread(inner)) => {
                                self.compile_expr(inner)?;
                                self.emit(Instruction::ArraySpreadAppend);
                            }
                            Some(e) => {
                                self.compile_expr(e)?;
                                self.emit(Instruction::ArrayAppend);
                            }
                            None => {
                                self.emit(Instruction::Push(Constant::Undefined));
                                self.emit(Instruction::ArrayAppend);
                            }
                        }
                    }
                }
            }
            Expr::Object(props) => {
                let has_spread = props.iter().any(|p| matches!(p.kind, PropKind::Spread));
                if !has_spread {
                    let mut count = 0;
                    for prop in props {
                        match &prop.key_expr {
                            Some(key_expr) => self.compile_expr(key_expr)?,
                            None => {
                                self.emit(Instruction::Push(Constant::String(prop.key.clone())));
                            }
                        }
                        self.compile_expr(&prop.value)?;
                        count += 1;
                    }
                    self.emit(Instruction::CreateObject(count));
                } else {
                    // Build incrementally so `{ ...a, k: v, ...b }` merges in order.
                    self.emit(Instruction::CreateObject(0));
                    for prop in props {
                        match prop.kind {
                            PropKind::Spread => {
                                self.compile_expr(&prop.value)?;
                                self.emit(Instruction::ObjectSpreadAssign);
                            }
                            _ => {
                                match &prop.key_expr {
                                    Some(key_expr) => self.compile_expr(key_expr)?,
                                    None => {
                                        self.emit(Instruction::Push(Constant::String(
                                            prop.key.clone(),
                                        )));
                                    }
                                }
                                self.compile_expr(&prop.value)?;
                                self.emit(Instruction::ObjectInsert);
                            }
                        }
                    }
                }
            }
            Expr::Spread(expr) => {
                self.compile_expr(expr)?;
                self.emit(Instruction::Spread);
            }
            Expr::Binary { op, left, right } => {
                self.compile_expr(left)?;
                self.compile_expr(right)?;
                let instr = match op {
                    BinOp::Add => Instruction::Add,
                    BinOp::Sub => Instruction::Sub,
                    BinOp::Mul => Instruction::Mul,
                    BinOp::Div => Instruction::Div,
                    BinOp::Rem => Instruction::Rem,
                    BinOp::Pow => Instruction::Pow,
                    BinOp::Eq => Instruction::Eq,
                    BinOp::Neq => Instruction::Neq,
                    BinOp::StrictEq => Instruction::StrictEq,
                    BinOp::StrictNeq => Instruction::StrictNeq,
                    BinOp::Lt => Instruction::Lt,
                    BinOp::Lte => Instruction::Lte,
                    BinOp::Gt => Instruction::Gt,
                    BinOp::Gte => Instruction::Gte,
                    BinOp::BitAnd => Instruction::BitAnd,
                    BinOp::BitOr => Instruction::BitOr,
                    BinOp::BitXor => Instruction::BitXor,
                    BinOp::Shl => Instruction::Shl,
                    BinOp::Shr => Instruction::Shr,
                    BinOp::Ushr => Instruction::Ushr,
                    BinOp::In => Instruction::In,
                    BinOp::InstanceOf => Instruction::InstanceOf,
                };
                self.emit(instr);
            }
            Expr::Unary { op, operand } => {
                self.compile_expr(operand)?;
                match op {
                    UnaryOp::Neg => {
                        self.emit(Instruction::Neg);
                    }
                    UnaryOp::Not => {
                        self.emit(Instruction::Not);
                    }
                    UnaryOp::BitNot => {
                        self.emit(Instruction::BitNot);
                    }
                    UnaryOp::Void => {
                        self.emit(Instruction::Void);
                    }
                }
            }
            Expr::Update {
                op,
                prefix,
                operand,
            } => {
                // Load current value
                self.compile_expr(operand)?;

                if !prefix {
                    self.emit(Instruction::Dup); // keep pre-value
                }

                match op {
                    UpdateOp::Increment => {
                        self.emit(Instruction::Increment);
                    }
                    UpdateOp::Decrement => {
                        self.emit(Instruction::Decrement);
                    }
                }

                if *prefix {
                    self.emit(Instruction::Dup); // keep post-value
                }

                // Store back
                self.compile_store(operand)?;

                if !prefix {
                    // Swap to get pre-value on top
                    // Actually the dup before increment already has it
                }
            }
            Expr::Logical { op, left, right } => match op {
                LogicalOp::And => {
                    self.compile_expr(left)?;
                    self.emit(Instruction::Dup);
                    let skip = self.emit(Instruction::JumpIfFalse(0));
                    self.emit(Instruction::Pop);
                    self.compile_expr(right)?;
                    let end = self.current_offset();
                    self.patch_jump(skip, end);
                }
                LogicalOp::Or => {
                    self.compile_expr(left)?;
                    self.emit(Instruction::Dup);
                    let skip = self.emit(Instruction::JumpIfTrue(0));
                    self.emit(Instruction::Pop);
                    self.compile_expr(right)?;
                    let end = self.current_offset();
                    self.patch_jump(skip, end);
                }
                LogicalOp::NullishCoalescing => {
                    // JumpIfNullish peeks (does not pop), so both branches must
                    // explicitly clear the duplicate to leave exactly one value.
                    self.compile_expr(left)?;
                    self.emit(Instruction::Dup);
                    let to_right = self.emit(Instruction::JumpIfNullish(0));
                    // Not nullish: keep the left value.
                    self.emit(Instruction::Pop);
                    let end = self.emit(Instruction::Jump(0));
                    let right_target = self.current_offset();
                    self.patch_jump(to_right, right_target);
                    self.emit(Instruction::Pop);
                    self.emit(Instruction::Pop);
                    self.compile_expr(right)?;
                    let after = self.current_offset();
                    self.patch_jump(end, after);
                }
            },
            Expr::Conditional {
                test,
                consequent,
                alternate,
            } => {
                self.compile_expr(test)?;
                let jump_else = self.emit(Instruction::JumpIfFalse(0));
                self.compile_expr(consequent)?;
                let jump_end = self.emit(Instruction::Jump(0));
                let else_target = self.current_offset();
                self.patch_jump(jump_else, else_target);
                self.compile_expr(alternate)?;
                let end = self.current_offset();
                self.patch_jump(jump_end, end);
            }
            Expr::Assignment { op, target, value } => {
                match op {
                    AssignOp::Assign => {
                        self.compile_expr(value)?;
                        self.emit(Instruction::Dup);
                        self.compile_store(target)?;
                    }
                    // Logical assignments short-circuit: the right-hand side is
                    // only evaluated (and stored) when the current value fails the
                    // keep condition. `a ||= b` / `a &&= b` test truthiness;
                    // `a ??= b` tests nullishness.
                    AssignOp::OrAssign | AssignOp::AndAssign => {
                        self.compile_expr(target)?;
                        self.emit(Instruction::Dup);
                        let assign_b = match op {
                            AssignOp::OrAssign => self.emit(Instruction::JumpIfFalse(0)),
                            _ => self.emit(Instruction::JumpIfTrue(0)),
                        };
                        // Keep path: leave the current value as the result.
                        let end = self.emit(Instruction::Jump(0));
                        let bpos = self.current_offset();
                        self.patch_jump(assign_b, bpos);
                        self.emit(Instruction::Pop);
                        self.compile_expr(value)?;
                        self.emit(Instruction::Dup);
                        self.compile_store(target)?;
                        let after = self.current_offset();
                        self.patch_jump(end, after);
                    }
                    AssignOp::NullishAssign => {
                        self.compile_expr(target)?;
                        self.emit(Instruction::Dup);
                        // JumpIfNullish peeks (does not pop) the tested value.
                        let assign_b = self.emit(Instruction::JumpIfNullish(0));
                        // Keep path: discard the duplicate, leave the current value.
                        self.emit(Instruction::Pop);
                        let end = self.emit(Instruction::Jump(0));
                        let bpos = self.current_offset();
                        self.patch_jump(assign_b, bpos);
                        self.emit(Instruction::Pop);
                        self.emit(Instruction::Pop);
                        self.compile_expr(value)?;
                        self.emit(Instruction::Dup);
                        self.compile_store(target)?;
                        let after = self.current_offset();
                        self.patch_jump(end, after);
                    }
                    _ => {
                        // Compound assignment: load, operate, store
                        self.compile_expr(target)?;
                        self.compile_expr(value)?;
                        match op {
                            AssignOp::AddAssign => {
                                self.emit(Instruction::Add);
                            }
                            AssignOp::SubAssign => {
                                self.emit(Instruction::Sub);
                            }
                            AssignOp::MulAssign => {
                                self.emit(Instruction::Mul);
                            }
                            AssignOp::DivAssign => {
                                self.emit(Instruction::Div);
                            }
                            AssignOp::RemAssign => {
                                self.emit(Instruction::Rem);
                            }
                            AssignOp::PowAssign => {
                                self.emit(Instruction::Pow);
                            }
                            AssignOp::BitAndAssign => {
                                self.emit(Instruction::BitAnd);
                            }
                            AssignOp::BitOrAssign => {
                                self.emit(Instruction::BitOr);
                            }
                            AssignOp::BitXorAssign => {
                                self.emit(Instruction::BitXor);
                            }
                            AssignOp::ShlAssign => {
                                self.emit(Instruction::Shl);
                            }
                            AssignOp::ShrAssign => {
                                self.emit(Instruction::Shr);
                            }
                            AssignOp::UshrAssign => {
                                self.emit(Instruction::Ushr);
                            }
                            _ => {}
                        }
                        self.emit(Instruction::Dup);
                        self.compile_store(target)?;
                    }
                }
            }
            Expr::Sequence(exprs) => {
                for (i, e) in exprs.iter().enumerate() {
                    self.compile_expr(e)?;
                    if i < exprs.len() - 1 {
                        self.emit(Instruction::Pop);
                    }
                }
            }
            Expr::Member {
                object,
                property,
                optional,
            } => {
                self.compile_expr(object)?;
                if *optional {
                    self.emit(Instruction::Dup);
                    let skip = self.emit(Instruction::JumpIfNullish(0));
                    self.emit(Instruction::GetProperty(property.clone()));
                    let end = self.emit(Instruction::Jump(0));
                    let nullish = self.current_offset();
                    self.patch_jump(skip, nullish);
                    self.emit(Instruction::Pop);
                    self.emit(Instruction::Push(Constant::Undefined));
                    let after = self.current_offset();
                    self.patch_jump(end, after);
                } else {
                    self.emit(Instruction::GetProperty(property.clone()));
                }
            }
            Expr::ComputedMember {
                object,
                property,
                optional,
            } => {
                self.compile_expr(object)?;
                if *optional {
                    self.emit(Instruction::Dup);
                    let skip = self.emit(Instruction::JumpIfNullish(0));
                    self.compile_expr(property)?;
                    self.emit(Instruction::GetIndex);
                    let end = self.emit(Instruction::Jump(0));
                    let nullish = self.current_offset();
                    self.patch_jump(skip, nullish);
                    self.emit(Instruction::Pop);
                    self.emit(Instruction::Push(Constant::Undefined));
                    let after = self.current_offset();
                    self.patch_jump(end, after);
                } else {
                    self.compile_expr(property)?;
                    self.emit(Instruction::GetIndex);
                }
            }
            Expr::Call { callee, args, .. } => {
                // Check for `Promise.all([ ...direct external calls... ])` — when
                // any element is a direct external call, compile the elements as
                // deferred calls so the host can run them in parallel.
                if self.try_compile_promise_all_batch(callee, args)? {
                    return Ok(());
                }
                // Check if this is a super() call
                if let Expr::Ident(name) = callee.as_ref() {
                    if name == "super" {
                        for arg in args {
                            self.compile_expr(arg)?;
                        }
                        self.emit(Instruction::CallSuper(args.len()));
                        return Ok(());
                    }
                }
                let has_spread = args.iter().any(|a| matches!(a, Expr::Spread(_)));
                // Check if this is a direct call to an external function
                if let Expr::Ident(name) = callee.as_ref() {
                    if self.external_functions.contains(name) {
                        if has_spread {
                            self.compile_spread_args(args)?;
                            self.emit(Instruction::CallExternalSpread(name.clone()));
                        } else {
                            for arg in args {
                                self.compile_expr(arg)?;
                            }
                            self.emit(Instruction::CallExternal(name.clone(), args.len()));
                        }
                        return Ok(());
                    }
                }
                self.compile_expr(callee)?;
                if has_spread {
                    // [callee, args_array] → CallSpread expands and dispatches.
                    self.compile_spread_args(args)?;
                    self.emit(Instruction::CallSpread);
                } else {
                    for arg in args {
                        self.compile_expr(arg)?;
                    }
                    self.emit(Instruction::Call(args.len()));
                }
            }
            Expr::New { callee, args } => {
                self.compile_expr(callee)?;
                for arg in args {
                    self.compile_expr(arg)?;
                }
                self.emit(Instruction::Construct(args.len()));
            }
            Expr::ArrowFunction { func_index } | Expr::FunctionExpr { func_index } => {
                self.emit(Instruction::CreateClosure(*func_index));
            }
            Expr::Await(expr) => {
                self.compile_expr(expr)?;
                // Emit Await instruction to unwrap Promise objects.
                // External call suspension is already handled by CallExternal
                // before this point — Await only handles internal promise values.
                self.emit(Instruction::Await);
            }
            Expr::Yield { value, delegate: _ } => {
                // Compile the yielded value (or undefined if none)
                match value {
                    Some(expr) => self.compile_expr(expr)?,
                    None => {
                        self.emit(Instruction::Push(Constant::Undefined));
                    }
                }
                // Yield instruction: suspends the generator, pops value, pushes received value on resume
                self.emit(Instruction::Yield);
            }
            Expr::TypeOf(operand) => {
                self.compile_expr(operand)?;
                self.emit(Instruction::TypeOf);
            }
            Expr::Delete(target) => {
                // `delete obj.prop` removes the key and writes the mutated object
                // back to its source place (value semantics), then yields `true`.
                match target.as_ref() {
                    Expr::Member {
                        object, property, ..
                    } if is_place_expr(object) => {
                        self.compile_expr(object)?;
                        self.emit(Instruction::DeleteProperty(property.clone()));
                        self.compile_store(object)?;
                        self.emit(Instruction::Push(Constant::Bool(true)));
                    }
                    Expr::ComputedMember {
                        object, property, ..
                    } if is_place_expr(object) => {
                        self.compile_expr(object)?;
                        self.compile_expr(property)?;
                        self.emit(Instruction::DeleteIndex);
                        self.compile_store(object)?;
                        self.emit(Instruction::Push(Constant::Bool(true)));
                    }
                    // `delete` on a non-reference (or a non-place object) is a
                    // no-op that evaluates to true in non-strict mode.
                    other => {
                        self.compile_expr(other)?;
                        self.emit(Instruction::Pop);
                        self.emit(Instruction::Push(Constant::Bool(true)));
                    }
                }
            }
            Expr::ClassExpr {
                name,
                super_class,
                constructor,
                methods,
                static_methods,
            } => {
                self.compile_class(
                    name.as_deref(),
                    super_class.as_deref(),
                    constructor.as_deref(),
                    methods,
                    static_methods,
                )?;
            }
        }
        Ok(())
    }

    /// If `callee(args)` is `Promise.all([...])` and at least one array element
    /// is a direct call to an external function, compile it as a parallel batch:
    /// each direct external-call element becomes a deferred call (no suspend),
    /// other elements compile normally, then `MakeBatchPromise` wraps them.
    /// Returns `true` if it handled the call.
    fn try_compile_promise_all_batch(&mut self, callee: &Expr, args: &[Expr]) -> Result<bool> {
        // callee must be `Promise.all`
        let is_promise_all = matches!(
            callee,
            Expr::Member { object, property, .. }
                if property == "all" && matches!(object.as_ref(), Expr::Ident(n) if n == "Promise")
        );
        if !is_promise_all || args.len() != 1 {
            return Ok(false);
        }
        let Expr::Array(elements) = &args[0] else {
            return Ok(false);
        };
        // Only take the batch path if there's at least one direct external call;
        // otherwise fall through to the normal Promise.all builtin so existing
        // behavior (resolved promises, plain values, rejection) is unchanged.
        let has_external_call = elements
            .iter()
            .flatten()
            .any(|el| self.external_call_target(el).is_some());
        if !has_external_call {
            return Ok(false);
        }

        for element in elements {
            match element {
                Some(expr) => {
                    if let Some((name, call_args)) = self.external_call_target(expr) {
                        let name = name.to_string();
                        let argc = call_args.len();
                        // clone args out before mutably borrowing self
                        let call_args: Vec<Expr> = call_args.to_vec();
                        for arg in &call_args {
                            self.compile_expr(arg)?;
                        }
                        self.emit(Instruction::CallExternalDeferred(name, argc));
                    } else {
                        self.compile_expr(expr)?;
                    }
                }
                None => {
                    self.emit(Instruction::Push(Constant::Undefined));
                }
            }
        }
        self.emit(Instruction::MakeBatchPromise(elements.len()));
        Ok(true)
    }

    /// If `expr` is a direct call to a registered external function, return its
    /// name and argument expressions.
    fn external_call_target<'e>(&self, expr: &'e Expr) -> Option<(&'e str, &'e [Expr])> {
        if let Expr::Call { callee, args, .. } = expr {
            if let Expr::Ident(name) = callee.as_ref() {
                if self.external_functions.contains(name) {
                    return Some((name.as_str(), args.as_slice()));
                }
            }
        }
        None
    }

    fn compile_class(
        &mut self,
        name: Option<&str>,
        super_class: Option<&str>,
        constructor: Option<&FunctionDef>,
        methods: &[ClassMethod],
        static_methods: &[ClassMethod],
    ) -> Result<()> {
        let class_name = name.unwrap_or("AnonymousClass").to_string();

        // Push super class if present
        if let Some(sc) = super_class {
            if let Some(idx) = self.resolve_local(sc) {
                self.emit(Instruction::LoadLocal(idx));
            } else {
                self.emit(Instruction::LoadGlobal(sc.to_string()));
            }
        }

        // Push static methods: name, closure pairs
        for sm in static_methods {
            self.emit(Instruction::Push(Constant::String(sm.name.clone())));
            let compiled = self.compile_function_def(&sm.func)?;
            let func_idx = self.functions.len();
            self.functions.push(compiled);
            self.emit(Instruction::CreateClosure(func_idx));
        }

        // Push instance methods: name, closure pairs
        for m in methods {
            self.emit(Instruction::Push(Constant::String(m.name.clone())));
            let compiled = self.compile_function_def(&m.func)?;
            let func_idx = self.functions.len();
            self.functions.push(compiled);
            self.emit(Instruction::CreateClosure(func_idx));
        }

        // Push constructor closure (or undefined if none)
        if let Some(ctor) = constructor {
            let compiled = self.compile_function_def(ctor)?;
            let func_idx = self.functions.len();
            self.functions.push(compiled);
            self.emit(Instruction::CreateClosure(func_idx));
        } else {
            self.emit(Instruction::Push(Constant::Undefined));
        }

        self.emit(Instruction::CreateClass {
            name: class_name,
            n_methods: methods.len(),
            n_statics: static_methods.len(),
            has_super: super_class.is_some(),
        });

        Ok(())
    }

    fn compile_store(&mut self, target: &Expr) -> Result<()> {
        match target {
            Expr::Ident(name) if name == "this" => {
                self.emit(Instruction::StoreThis);
            }
            Expr::Ident(name) => {
                if let Some(idx) = self.resolve_local(name) {
                    self.emit(Instruction::StoreLocal(idx));
                } else {
                    self.emit(Instruction::StoreGlobal(name.clone()));
                }
            }
            Expr::Member {
                object, property, ..
            } => {
                self.compile_expr(object)?;
                self.emit(Instruction::SetProperty(property.clone()));
                // SetProperty pushes the modified object back — store it to the parent
                self.compile_store(object)?;
            }
            Expr::ComputedMember {
                object, property, ..
            } => {
                self.compile_expr(object)?;
                self.compile_expr(property)?;
                self.emit(Instruction::SetIndex);
                // SetIndex pushes the modified object back — store it to the parent
                self.compile_store(object)?;
            }
            _ => {
                return Err(ZapcodeError::CompileError(
                    "invalid assignment target".to_string(),
                ));
            }
        }
        Ok(())
    }
}

/// Whether an expression denotes a storable location (so a mutated copy can be
/// written back). Used by `delete` to decide whether to persist the change.
fn is_place_expr(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Ident(_) | Expr::Member { .. } | Expr::ComputedMember { .. }
    )
}

pub fn compile(program: &Program) -> Result<CompiledProgram> {
    compile_with_externals(program, HashSet::new())
}

pub fn compile_with_externals(
    program: &Program,
    external_functions: HashSet<String>,
) -> Result<CompiledProgram> {
    let mut compiler = Compiler::new(external_functions);
    compiler.compile_program(program)?;

    Ok(CompiledProgram {
        instructions: compiler.instructions,
        functions: compiler.functions,
        local_names: compiler.locals,
    })
}

pub fn compile_session_chunk(
    program: &Program,
    external_functions: HashSet<String>,
    existing_bindings: HashMap<String, TopLevelBindingKind>,
) -> Result<(CompiledProgram, HashMap<String, TopLevelBindingKind>)> {
    let mut compiler = Compiler::new_session_chunk(external_functions, existing_bindings);
    compiler.compile_program(program)?;

    Ok((
        CompiledProgram {
            instructions: compiler.instructions,
            functions: compiler.functions,
            local_names: compiler.locals,
        },
        compiler.top_level_bindings,
    ))
}
