pub mod instruction;

use std::sync::Arc;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

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
    /// True iff the body references `arguments` and the function is non-arrow, so
    /// the VM must bind an array-like `arguments` local right after the params.
    pub needs_arguments: bool,
}

impl CompiledFunction {
    /// An empty stand-in used to reserve a slot in the shared function table
    /// before the real compiled function is written back by index (see
    /// `compile_program`). Never executed.
    fn placeholder() -> Self {
        CompiledFunction {
            name: None,
            params: Vec::new(),
            instructions: Vec::new(),
            local_count: 0,
            local_names: Vec::new(),
            is_async: false,
            is_generator: false,
            needs_arguments: false,
        }
    }
}

struct Compiler {
    instructions: Vec<Instruction>,
    locals: Vec<String>,
    /// Lexical scope stack of name→slot maps. `scopes[0]` is the function scope
    /// (`var`/params/`arguments`); each `{}` block / loop / catch pushes a scope
    /// so `let`/`const` bindings shadow and don't leak. Resolution searches
    /// innermost-out. Slots in `locals` are never reused, so a shadow gets its
    /// own slot.
    scopes: Vec<HashMap<String, usize>>,
    /// Local slots / top-level names bound by `const`. Assigning to one compiles
    /// to a runtime `TypeError` throw ("Assignment to constant variable"), like
    /// JS — caught by enclosing try/catch.
    const_slots: HashSet<usize>,
    const_globals: HashSet<String>,
    /// The program's flat function table, SHARED across the top-level compiler
    /// and every per-function sub-compiler (`compile_function_def`). Parser-
    /// assigned function indices occupy slots `0..program.functions.len()`;
    /// class member functions (constructors/methods/field-inits), which have no
    /// parser index, are appended beyond that. Sharing is essential: a class
    /// declared inside a function compiles in a sub-compiler, and its members
    /// must land in this one global table (else their CreateClosure indices
    /// dangle into the wrong functions and instantiating recurses forever).
    functions: Rc<RefCell<Vec<CompiledFunction>>>,
    loop_stack: Vec<LoopInfo>,
    external_functions: HashSet<String>,
    mode: CompilerMode,
    top_level_bindings: HashMap<String, TopLevelBindingKind>,
    /// Label attached to the next loop (from a `label:` statement), if any.
    pending_label: Option<String>,
    /// Active labeled NON-loop statements (e.g. `foo: { ... break foo; }`).
    /// Kept separate from `loop_stack` so that an *unlabeled* `break`/`continue`
    /// never targets a labeled block — only a labeled `break foo` falls back to
    /// this stack. Without it, `break foo` finds no loop, leaves its jump offset
    /// at 0, and runs away to instruction 0 (hits the allocation limit).
    labeled_blocks: Vec<LoopInfo>,
    /// Function-declaration indices already bound by the scope's hoist pass, so
    /// the in-source-order `FunctionDecl` statement is a no-op for them.
    hoisted_funcs: std::collections::HashSet<usize>,
    /// The lexically-enclosing class name while compiling a class
    /// method/constructor body. Lets `super`/`super.m()` resolve against the
    /// defining class's `__super__` regardless of the runtime receiver's class.
    current_class: Option<String>,
    /// Counter for the hidden locals `prepare_rmw_target` allocates (one per
    /// read-modify-write member target in this function).
    rmw_temp_counter: usize,
}

/// A read-modify-write member reference whose object/index live in hidden
/// locals — see [`Compiler::prepare_rmw_target`].
struct RmwRef {
    obj_slot: usize,
    idx_slot: Option<usize>,
    property: Option<Arc<str>>,
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
            scopes: vec![HashMap::new()],
            const_slots: HashSet::new(),
            const_globals: HashSet::new(),
            functions: Rc::new(RefCell::new(Vec::new())),
            loop_stack: Vec::new(),
            external_functions,
            mode: CompilerMode::Standard,
            top_level_bindings: HashMap::new(),
            pending_label: None,
            labeled_blocks: Vec::new(),
            hoisted_funcs: std::collections::HashSet::new(),
            current_class: None,
            rmw_temp_counter: 0,
        }
    }

    fn new_session_chunk(
        external_functions: HashSet<String>,
        top_level_bindings: HashMap<String, TopLevelBindingKind>,
    ) -> Self {
        Self {
            instructions: Vec::new(),
            locals: Vec::new(),
            scopes: vec![HashMap::new()],
            const_slots: HashSet::new(),
            const_globals: HashSet::new(),
            functions: Rc::new(RefCell::new(Vec::new())),
            loop_stack: Vec::new(),
            external_functions,
            mode: CompilerMode::SessionChunk,
            top_level_bindings,
            pending_label: None,
            labeled_blocks: Vec::new(),
            hoisted_funcs: std::collections::HashSet::new(),
            current_class: None,
            rmw_temp_counter: 0,
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
            | Instruction::JumpIfNullish(t)
            | Instruction::Break(t)
            | Instruction::Continue(t)
            | Instruction::EnterFinallyNormal(t) => {
                *t = target;
            }
            Instruction::SetupTry { catch_ip, .. } => {
                *catch_ip = target;
            }
            _ => {}
        }
    }

    /// Allocate a brand-new slot for `name` (always; never reused).
    fn alloc_slot(&mut self, name: &str) -> usize {
        let idx = self.locals.len();
        self.locals.push(name.to_string());
        idx
    }

    /// Declare a FUNCTION-scoped binding (`var`, params, `arguments`,
    /// hoisted names): lives in the outermost scope and is reused if already
    /// present there.
    fn declare_local(&mut self, name: &str) -> usize {
        if let Some(&idx) = self.scopes[0].get(name) {
            return idx;
        }
        let idx = self.alloc_slot(name);
        self.scopes[0].insert(name.to_string(), idx);
        idx
    }

    /// Declare a BLOCK-scoped binding (`let`/`const`, catch param): a fresh slot
    /// in the CURRENT (innermost) scope, shadowing any same-named outer binding
    /// and disappearing when the block ends.
    fn declare_block_local(&mut self, name: &str) -> usize {
        let idx = self.alloc_slot(name);
        self.scopes
            .last_mut()
            .expect("at least one scope")
            .insert(name.to_string(), idx);
        idx
    }

    /// True if `name` is already declared in the CURRENT scope (a duplicate
    /// `let`/`const`, which JS rejects as a SyntaxError).
    fn declared_in_current_scope(&self, name: &str) -> bool {
        self.scopes.last().is_some_and(|s| s.contains_key(name))
    }

    /// Declare a binding by its kind: `var` is function-scoped, `let`/`const`
    /// are block-scoped (a duplicate `let`/`const` in the same scope is a
    /// SyntaxError, surfaced here as a compile error).
    fn declare_binding(&mut self, name: &str, kind: VarKind) -> Result<usize> {
        match kind {
            VarKind::Var => Ok(self.declare_local(name)),
            VarKind::Let | VarKind::Const => {
                if self.declared_in_current_scope(name) {
                    return Err(ZapcodeError::CompileError(format!(
                        "Identifier '{}' has already been declared",
                        name
                    )));
                }
                Ok(self.declare_block_local(name))
            }
        }
    }

    /// Resolve `name` to a slot, searching innermost scope outward.
    fn resolve_local(&self, name: &str) -> Option<usize> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    fn enter_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn exit_scope(&mut self) {
        self.scopes.pop();
    }

    /// Compile a statement list inside a fresh lexical block scope so its
    /// `let`/`const` bindings don't leak out (or clobber outer bindings).
    fn compile_block_scoped(&mut self, stmts: &[Statement]) -> Result<()> {
        self.enter_scope();
        let r = (|| {
            for s in stmts {
                self.compile_statement(s)?;
            }
            Ok(())
        })();
        self.exit_scope();
        r
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
            Instruction::StoreGlobal(Arc::from(name))
        } else {
            Instruction::StoreLocal(idx)
        }
    }

    fn compile_program(&mut self, program: &Program) -> Result<()> {
        // First pass: compile all parser-indexed function definitions. Reserve
        // their slots up front and assign by index, because compiling a function
        // whose body declares a class appends that class's member functions to
        // the shared table mid-pass — reserving keeps parser indices stable
        // (regular functions own 0..N; class members land at N and beyond).
        let n = program.functions.len();
        self.functions
            .borrow_mut()
            .resize_with(n, CompiledFunction::placeholder);
        for (i, func_def) in program.functions.iter().enumerate() {
            let compiled = self.compile_function_def(func_def)?;
            self.functions.borrow_mut()[i] = compiled;
        }

        // Second pass: compile body. Hoist top-level function declarations first
        // so forward references and mutual recursion resolve.
        self.hoist_function_decls(&program.body)?;
        // For the last statement, if it's an expression, keep the value on the stack
        let len = program.body.len();
        for (i, stmt) in program.body.iter().enumerate() {
            let is_last = i == len - 1;
            if is_last {
                // Leave the trailing statement's completion value on the stack as
                // the program result (so a script ending in try/catch, if, or a
                // block yields that block's value, not null).
                self.compile_completion_statement(stmt)?;
            } else {
                self.compile_statement(stmt)?;
            }
        }

        Ok(())
    }

    /// True iff `arguments` is referenced directly in this function body. Nested
    /// function/arrow bodies live in the separate `Program.functions` table
    /// (referenced here only by index), so this scan stops at the current
    /// function boundary — exactly the lexical scope `arguments` belongs to.
    fn body_uses_arguments(body: &[Statement]) -> bool {
        body.iter().any(Self::stmt_uses_arguments)
    }

    /// Whether a labeled statement's body is itself a loop/switch that will
    /// adopt the pending label into its own `LoopInfo` (so `break label` /
    /// `continue label` resolve against it). Anything else (notably a plain
    /// block) is a labeled non-loop and needs its own break frame.
    fn body_consumes_label(stmt: &Statement) -> bool {
        matches!(
            stmt,
            Statement::While { .. }
                | Statement::DoWhile { .. }
                | Statement::For { .. }
                | Statement::ForOf { .. }
                | Statement::Switch { .. }
        )
    }

    fn stmt_uses_arguments(stmt: &Statement) -> bool {
        match stmt {
            Statement::VariableDecl { declarations, .. } => declarations
                .iter()
                .any(|d| d.init.as_ref().is_some_and(Self::expr_uses_arguments)),
            Statement::Expression { expr, .. } => Self::expr_uses_arguments(expr),
            Statement::Return { value, .. } => value.as_ref().is_some_and(Self::expr_uses_arguments),
            Statement::Throw { value, .. } => Self::expr_uses_arguments(value),
            Statement::If {
                test,
                consequent,
                alternate,
                ..
            } => {
                Self::expr_uses_arguments(test)
                    || Self::body_uses_arguments(consequent)
                    || alternate.as_deref().is_some_and(Self::body_uses_arguments)
            }
            Statement::While { test, body, .. } | Statement::DoWhile { body, test, .. } => {
                Self::expr_uses_arguments(test) || Self::body_uses_arguments(body)
            }
            Statement::For {
                init,
                test,
                update,
                body,
                ..
            } => {
                init.as_deref().is_some_and(Self::stmt_uses_arguments)
                    || test.as_ref().is_some_and(Self::expr_uses_arguments)
                    || update.as_ref().is_some_and(Self::expr_uses_arguments)
                    || Self::body_uses_arguments(body)
            }
            Statement::ForOf { iterable, body, .. } => {
                Self::expr_uses_arguments(iterable) || Self::body_uses_arguments(body)
            }
            Statement::Block { body, .. } => Self::body_uses_arguments(body),
            Statement::TryCatch {
                try_body,
                catch_body,
                finally_body,
                ..
            } => {
                Self::body_uses_arguments(try_body)
                    || Self::body_uses_arguments(catch_body)
                    || finally_body.as_deref().is_some_and(Self::body_uses_arguments)
            }
            Statement::Labeled { body, .. } => Self::stmt_uses_arguments(body),
            Statement::Switch {
                discriminant,
                cases,
                ..
            } => {
                Self::expr_uses_arguments(discriminant)
                    || cases.iter().any(|c| {
                        c.test.as_ref().is_some_and(Self::expr_uses_arguments)
                            || Self::body_uses_arguments(&c.consequent)
                    })
            }
            _ => false,
        }
    }

    fn expr_uses_arguments(expr: &Expr) -> bool {
        match expr {
            Expr::Ident(name) => name == "arguments",
            Expr::TemplateLit { exprs, .. } => exprs.iter().any(Self::expr_uses_arguments),
            Expr::Array(items) => items
                .iter()
                .any(|e| e.as_ref().is_some_and(Self::expr_uses_arguments)),
            Expr::Object(props) => props.iter().any(|p| Self::expr_uses_arguments(&p.value)),
            Expr::Spread(e)
            | Expr::Unary { operand: e, .. }
            | Expr::Update { operand: e, .. }
            | Expr::Await(e)
            | Expr::TypeOf(e)
            | Expr::Delete(e) => Self::expr_uses_arguments(e),
            Expr::Binary { left, right, .. } | Expr::Logical { left, right, .. } => {
                Self::expr_uses_arguments(left) || Self::expr_uses_arguments(right)
            }
            Expr::Conditional {
                test,
                consequent,
                alternate,
            } => {
                Self::expr_uses_arguments(test)
                    || Self::expr_uses_arguments(consequent)
                    || Self::expr_uses_arguments(alternate)
            }
            Expr::Assignment { target, value, .. } => {
                Self::expr_uses_arguments(target) || Self::expr_uses_arguments(value)
            }
            Expr::Sequence(items) => items.iter().any(Self::expr_uses_arguments),
            Expr::Member { object, .. } => Self::expr_uses_arguments(object),
            Expr::ComputedMember {
                object, property, ..
            } => Self::expr_uses_arguments(object) || Self::expr_uses_arguments(property),
            Expr::Call { callee, args, .. } | Expr::New { callee, args } => {
                Self::expr_uses_arguments(callee) || args.iter().any(Self::expr_uses_arguments)
            }
            Expr::Yield { value, .. } => value.as_deref().is_some_and(Self::expr_uses_arguments),
            _ => false,
        }
    }

    fn compile_function_def(&mut self, func: &FunctionDef) -> Result<CompiledFunction> {
        let mut func_compiler = Compiler::new(self.external_functions.clone());
        // Share the one global function table so class members declared in this
        // body register globally (see the `functions` field doc).
        func_compiler.functions = Rc::clone(&self.functions);
        // Inherit the enclosing class context so `super` inside a method/constructor
        // body (which compiles into this fresh sub-compiler) resolves to the right
        // defining class. Nested non-method closures inside a method keep the same
        // class context, matching JS lexical `super` scoping.
        func_compiler.current_class = self.current_class.clone();

        // Set up parameters as locals. The slot layout MUST match the order the
        // VM's `bind_params` pushes values (see vm::bind_params).
        for (i, param) in func.params.iter().enumerate() {
            match param {
                ParamPattern::Ident(name) => {
                    func_compiler.declare_local(name);
                }
                ParamPattern::Rest(name) => {
                    func_compiler.declare_local(name);
                }
                ParamPattern::DefaultValue { pattern, .. } => match pattern.as_ref() {
                    ParamPattern::Ident(name) | ParamPattern::Rest(name) => {
                        func_compiler.declare_local(name);
                    }
                    // `function f({a} = {})` / `function f([a] = […])`: a hidden
                    // temp holds the raw argument (so the whole-pattern default can
                    // be re-destructured when it is undefined), then the inner
                    // pattern's leaf locals, matching the VM's push order.
                    nested => {
                        func_compiler.declare_local(&Self::param_temp_name(i));
                        func_compiler.declare_param_pattern_locals(nested);
                    }
                },
                ParamPattern::ObjectDestructure(fields) => {
                    // A field-level default (`{b: {c} = {…}}`) needs the raw
                    // argument in a hidden temp (see has_field_level_default).
                    if param.has_field_level_default() {
                        func_compiler.declare_local(&Self::param_temp_name(i));
                    }
                    func_compiler.declare_destructure_locals(fields);
                }
                ParamPattern::ArrayDestructure(elems) => {
                    for elem in elems.iter().flatten() {
                        func_compiler.declare_param_pattern_locals(elem);
                    }
                }
            }
        }

        // `arguments`: an ordinary (non-arrow) function that references
        // `arguments` gets an array-like bound right after the params. The VM
        // (`bind_params`) pushes the full argument list into this slot, so it must
        // be declared immediately after the param-derived locals. Arrows inherit
        // the enclosing `arguments` lexically (handled by global/closure lookup),
        // so they do NOT declare their own.
        let needs_arguments = !func.is_arrow && Self::body_uses_arguments(&func.body);
        if needs_arguments {
            func_compiler.declare_local("arguments");
        }

        // Apply default parameter values: `if (param === undefined) param = <default>`.
        for (i, param) in func.params.iter().enumerate() {
            match param {
                ParamPattern::DefaultValue { pattern, default } => match pattern.as_ref() {
                    ParamPattern::Ident(name) | ParamPattern::Rest(name) => {
                        if let Some(slot) = func_compiler.resolve_local(name) {
                            func_compiler.emit_slot_default(slot, default)?;
                        }
                    }
                    // `function f({a = 5} = {})` / `function f([a, b] = [7, 8])`:
                    // when the argument is undefined, evaluate the whole-pattern
                    // default and destructure it into the leaf slots; then apply
                    // any inner field/element defaults.
                    nested => {
                        func_compiler.emit_pattern_param_default(&Self::param_temp_name(i), nested, default)?;
                    }
                },
                ParamPattern::ObjectDestructure(_) | ParamPattern::ArrayDestructure(_) => {
                    if param.has_field_level_default() {
                        func_compiler
                            .emit_field_level_defaults(&Self::param_temp_name(i), param)?;
                    }
                    func_compiler
                        .emit_pattern_inner_defaults(param, param.has_field_level_default())?;
                }
                _ => {}
            }
        }

        // Hoist this function body's own nested function declarations.
        func_compiler.hoist_function_decls(&func.body)?;
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
            needs_arguments,
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

                self.compile_block_scoped(consequent)?;

                if let Some(alt) = alternate {
                    let jump_end = self.emit(Instruction::Jump(0));
                    let else_target = self.current_offset();
                    self.patch_jump(jump_else, else_target);

                    self.compile_block_scoped(alt)?;
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

                self.compile_block_scoped(body)?;

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

                self.compile_block_scoped(body)?;

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
                // A loop scope holds the `for (let i …)` head bindings so they
                // don't leak after the loop.
                self.enter_scope();
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

                self.compile_block_scoped(body)?;

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
                self.exit_scope();
            }
            Statement::ForOf {
                binding,
                iterable,
                body,
                await_each,
                ..
            } => {
                self.compile_expr(iterable)?;
                self.emit(Instruction::GetIterator);

                // Loop scope for the `for (const x of …)` binding so it doesn't
                // leak after the loop.
                self.enter_scope();
                let loop_start = self.current_offset();
                self.loop_stack.push(LoopInfo {
                    break_patches: Vec::new(),
                    continue_patches: Vec::new(),
                    label: self.pending_label.take(),
                });

                // IteratorNext consumes the iterator and pushes the *advanced*
                // iterator plus the next value, so the single iterator is
                // threaded through each iteration. (A `Dup` here would leak one
                // iterator per iteration onto the stack; harmless for a single
                // loop, but a nested loop would then leave exhausted inner
                // iterators sitting on top of the outer one — making the outer
                // loop read the wrong iterator and exit after one pass.)
                self.emit(Instruction::IteratorNext);
                self.emit(Instruction::IteratorDone);
                let exit_jump = self.emit(Instruction::JumpIfTrue(0));

                // `for await`: the iterated value (now on top of the stack, with
                // the threaded iterator beneath it) must be awaited before being
                // bound. Await unwraps a resolved promise, throws a rejected one,
                // suspends on a pending external call, and passes non-promises
                // through — exactly the per-element semantics we want.
                if *await_each {
                    self.emit(Instruction::Await);
                }

                // Bind the value (block-scoped to the loop)
                match binding {
                    ForBinding::Ident(name) => {
                        let idx = self.declare_block_local(name);
                        self.emit(Instruction::StoreLocal(idx));
                    }
                    ForBinding::Destructure(pattern) => {
                        // Destructure the iterated value into the bound names, then
                        // pop the value the pattern was read from.
                        self.compile_destructure_pattern(pattern, VarKind::Let)?;
                        self.emit(Instruction::Pop);
                    }
                }
                // `for (const x of …)` gives each iteration a FRESH binding, so a
                // closure created this iteration keeps the value it saw rather than
                // sharing one slot (the loop scope holds only the binding slots).
                let per_iter_slots: Vec<usize> = self
                    .scopes
                    .last()
                    .map(|s| s.values().copied().collect())
                    .unwrap_or_default();

                self.compile_block_scoped(body)?;

                for &slot in &per_iter_slots {
                    self.emit(Instruction::FreshenBinding(slot));
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
                self.exit_scope();
            }
            Statement::Block { body, .. } => {
                self.compile_block_scoped(body)?;
            }
            Statement::Throw { value, .. } => {
                self.compile_expr(value)?;
                self.emit(Instruction::Throw);
            }
            Statement::TryCatch {
                try_body,
                has_catch,
                catch_param,
                catch_body,
                finally_body,
                ..
            } => {
                self.compile_try_catch_finally(
                    try_body,
                    *has_catch,
                    catch_param.as_deref(),
                    catch_body,
                    finally_body.as_deref(),
                    None,
                )?;
            }
            Statement::Break { label, .. } => {
                let idx = self.emit(Instruction::Break(0));
                let target = match label {
                    // A labeled break targets the matching loop/switch, or — if
                    // none matches — a labeled non-loop block (e.g. `foo: {...}`).
                    Some(l) => self
                        .loop_stack
                        .iter_mut()
                        .rev()
                        .find(|li| li.label.as_deref() == Some(l.as_str()))
                        .or_else(|| {
                            self.labeled_blocks
                                .iter_mut()
                                .rev()
                                .find(|li| li.label.as_deref() == Some(l.as_str()))
                        }),
                    // An unlabeled break targets the innermost loop/switch only,
                    // never an enclosing labeled block.
                    None => self.loop_stack.last_mut(),
                };
                if let Some(loop_info) = target {
                    loop_info.break_patches.push(idx);
                }
            }
            Statement::Continue { label, .. } => {
                let idx = self.emit(Instruction::Continue(0));
                let target = match label {
                    // `continue label` only legally targets a loop, but a labeled
                    // *block* may match in malformed input; fall back to it so the
                    // jump is still patched (to the block end) rather than left at
                    // 0 and running away.
                    Some(l) => self
                        .loop_stack
                        .iter_mut()
                        .rev()
                        .find(|li| li.label.as_deref() == Some(l.as_str()))
                        .or_else(|| {
                            self.labeled_blocks
                                .iter_mut()
                                .rev()
                                .find(|li| li.label.as_deref() == Some(l.as_str()))
                        }),
                    None => self.loop_stack.last_mut(),
                };
                if let Some(loop_info) = target {
                    loop_info.continue_patches.push(idx);
                }
            }
            Statement::Labeled { label, body, .. } => {
                if Self::body_consumes_label(body) {
                    // The loop/switch picks up this label into its own LoopInfo;
                    // clear it afterward in case the body turned out not to.
                    self.pending_label = Some(label.clone());
                    self.compile_statement(body)?;
                    self.pending_label = None;
                } else {
                    // Labeled non-loop statement (commonly a block). Register a
                    // break frame so `break label` jumps past the body instead of
                    // emitting an unpatched Break(0) that loops to instruction 0.
                    self.labeled_blocks.push(LoopInfo {
                        break_patches: Vec::new(),
                        continue_patches: Vec::new(),
                        label: Some(label.clone()),
                    });
                    self.compile_statement(body)?;
                    let end = self.current_offset();
                    let info = self.labeled_blocks.pop().unwrap();
                    for patch in info.break_patches {
                        self.patch_jump(patch, end);
                    }
                    // `continue` is invalid on a block; if one slipped through,
                    // patch it to the end too (behaves like break) so it can't
                    // run away.
                    for patch in info.continue_patches {
                        self.patch_jump(patch, end);
                    }
                }
            }
            Statement::FunctionDecl {
                func_index, name, ..
            } => {
                // Already bound by the enclosing scope's hoist pass — no-op.
                if self.hoisted_funcs.contains(func_index) {
                    return Ok(());
                }
                self.emit_function_decl_binding(*func_index, name)?;
            }
            Statement::ClassDecl {
                name,
                super_class,
                constructor,
                methods,
                static_methods,
                fields,
                static_fields,
                ..
            } => {
                self.compile_class(
                    Some(name),
                    super_class.as_deref(),
                    constructor.as_deref(),
                    methods,
                    static_methods,
                    fields,
                    static_fields,
                )?;
                if self.is_session_chunk() {
                    self.record_top_level_binding(name, TopLevelBindingKind::Class)?;
                    self.emit(Instruction::StoreGlobal(Arc::from(name.as_str())));
                } else {
                    // Store the class as both local and global
                    self.emit(Instruction::Dup);
                    let idx = self.declare_local(name);
                    self.emit(Instruction::StoreLocal(idx));
                    self.emit(Instruction::StoreGlobal(Arc::from(name.as_str())));
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

    /// Compile a statement in "completion-value position": its value is left on
    /// the stack as the result (used for the program's trailing statement and,
    /// recursively, the trailing statement of a block/if/try it contains).
    /// Expression/Block/If/TryCatch produce a value; anything else keeps its
    /// normal (value-less) compilation.
    fn compile_completion_statement(&mut self, stmt: &Statement) -> Result<()> {
        match stmt {
            Statement::Expression { expr, .. } => {
                self.compile_expr(expr)?; // leave value, no Pop
            }
            Statement::Block { body, .. } => {
                self.compile_completion_block(body)?;
            }
            Statement::If {
                test,
                consequent,
                alternate,
                ..
            } => {
                self.compile_expr(test)?;
                let jump_else = self.emit(Instruction::JumpIfFalse(0));
                self.compile_completion_block(consequent)?;
                let jump_end = self.emit(Instruction::Jump(0));
                let else_target = self.current_offset();
                self.patch_jump(jump_else, else_target);
                match alternate {
                    Some(alt) => self.compile_completion_block(alt)?,
                    None => {
                        self.emit(Instruction::Push(Constant::Undefined));
                    }
                }
                let end_target = self.current_offset();
                self.patch_jump(jump_end, end_target);
            }
            Statement::TryCatch {
                try_body,
                has_catch,
                catch_param,
                catch_body,
                finally_body,
                ..
            } => {
                // In completion-value position the try and catch arms each leave a
                // value on the stack. The finally body runs for effects only; its
                // value is discarded (it pops the completion value), and a normal
                // finally completion lets the try/catch value through. An abrupt
                // completion inside finally still supersedes via EndFinally.
                self.compile_try_catch_finally(
                    try_body,
                    *has_catch,
                    catch_param.as_deref(),
                    catch_body,
                    finally_body.as_deref(),
                    Some(()),
                )?;
            }
            other => {
                self.compile_statement(other)?;
            }
        }
        Ok(())
    }

    /// Compile a statement list so exactly one value (its completion value) is
    /// left on the stack.
    fn compile_completion_block(&mut self, stmts: &[Statement]) -> Result<()> {
        let Some((last, init)) = stmts.split_last() else {
            self.emit(Instruction::Push(Constant::Undefined));
            return Ok(());
        };
        for s in init {
            self.compile_statement(s)?;
        }
        match last {
            Statement::Expression { .. }
            | Statement::Block { .. }
            | Statement::If { .. }
            | Statement::TryCatch { .. } => self.compile_completion_statement(last)?,
            other => {
                // A non-value statement contributes no completion value; compile
                // it normally and default the block's value to undefined.
                self.compile_statement(other)?;
                self.emit(Instruction::Push(Constant::Undefined));
            }
        }
        Ok(())
    }

    /// Lower a `try { … } catch (e) { … } finally { … }` statement. When
    /// `completion` is `Some`, the try and catch arms are compiled in
    /// completion-value position (each leaves its value on the stack); otherwise
    /// they are compiled as plain statements.
    ///
    /// The finally body — when present — is compiled once at a known ip. Every
    /// way of leaving the try/catch (normal fall-through, caught/uncaught throw,
    /// `return`, `break`, `continue`) routes through that finally at runtime via
    /// the try frame's pending-completion slot; an abrupt completion inside the
    /// finally supersedes the pending one (`EndFinally`). Without a finally the
    /// lowering is the classic catch-only shape.
    fn compile_try_catch_finally(
        &mut self,
        try_body: &[Statement],
        has_catch: bool,
        catch_param: Option<&str>,
        catch_body: &[Statement],
        finally_body: Option<&[Statement]>,
        completion: Option<()>,
    ) -> Result<()> {

        // ---- No finally: the classic catch-only lowering. ----
        let Some(finally) = finally_body else {
            let setup = self.emit(Instruction::SetupTry {
                catch_ip: 0,
                finally_ip: None,
                region_end: 0,
            });
            self.compile_body(try_body, completion)?;
            self.emit(Instruction::EndTry);
            let jump_past_catch = self.emit(Instruction::Jump(0));

            let catch_start = self.current_offset();
            self.patch_jump(setup, catch_start);
            // The catch param is block-scoped to the catch clause (shadows an
            // outer binding of the same name, gone after the clause).
            self.enter_scope();
            if let Some(param) = catch_param {
                let idx = self.declare_block_local(param);
                self.emit(Instruction::StoreLocal(idx));
            } else {
                self.emit(Instruction::Pop);
            }
            self.compile_body(catch_body, completion)?;
            self.exit_scope();

            let after_catch = self.current_offset();
            self.patch_jump(jump_past_catch, after_catch);
            if let Instruction::SetupTry { region_end, .. } = &mut self.instructions[setup] {
                *region_end = after_catch;
            }
            return Ok(());
        };

        // ---- With finally: route every exit through the finally body. ----
        //
        // SetupTry's catch_ip is patched to the catch block when there is one,
        // otherwise to the finally entry (so an uncaught throw runs the finally,
        // carrying the in-flight exception). finally_ip is filled in once the
        // finally body is emitted; the runtime reads it when an abrupt completion
        // (return/break/continue) or a throw must run this finally.
        let setup = self.emit(Instruction::SetupTry {
            catch_ip: 0,
            finally_ip: None,
            region_end: 0,
        });

        // Try body. On normal completion, route to the finally with a Normal
        // completion. (EnterFinallyNormal's operand is patched to finally_ip.)
        // No EndTry here: the try frame must persist until EndFinally so the
        // finally body can run and resume the pending completion. EnterFinallyNormal
        // transitions the frame into its finally phase.
        self.compile_body(try_body, completion)?;
        let try_to_finally = self.emit(Instruction::EnterFinallyNormal(0));

        // Catch block (optional). The catch param is block-scoped to the clause.
        let catch_start = self.current_offset();
        if has_catch {
            self.enter_scope();
            if let Some(param) = catch_param {
                let idx = self.declare_block_local(param);
                self.emit(Instruction::StoreLocal(idx));
            } else {
                self.emit(Instruction::Pop);
            }
            self.compile_body(catch_body, completion)?;
            self.exit_scope();
        }
        // After the catch body (or, when there is no catch, this point is never
        // reached by fall-through) route to the finally with a Normal completion.
        let catch_to_finally = self.emit(Instruction::EnterFinallyNormal(0));

        // Finally body.
        let finally_ip = self.current_offset();
        self.patch_jump(try_to_finally, finally_ip);
        self.patch_jump(catch_to_finally, finally_ip);

        // In completion-value position the (try/catch) value sits on the stack
        // beneath the finally body's temporaries; the finally runs for effects.
        // The finally body is compiled as statements (stack-neutral), so the
        // underlying value is preserved automatically — no extra work needed.
        for s in finally {
            self.compile_statement(s)?;
        }
        self.emit(Instruction::EndFinally);
        let region_end = self.current_offset();

        // Patch SetupTry now that catch/finally/region ips are known. Throws go
        // to the catch block if present, otherwise straight to the finally entry
        // (with the exception pending). finally_ip lets return/break/continue
        // find this finally; region_end bounds the statement for break/continue.
        if let Instruction::SetupTry {
            catch_ip,
            finally_ip: fin,
            region_end: end,
        } = &mut self.instructions[setup]
        {
            *catch_ip = if has_catch { catch_start } else { finally_ip };
            *fin = Some(finally_ip);
            *end = region_end;
        }

        Ok(())
    }

    /// Compile a statement list either as plain statements or (when
    /// `completion` is `Some`) in completion-value position.
    fn compile_body(&mut self, body: &[Statement], completion: Option<()>) -> Result<()> {
        if completion.is_some() {
            self.compile_completion_block(body)
        } else {
            for s in body {
                self.compile_statement(s)?;
            }
            Ok(())
        }
    }

    /// Compile a complete optional chain (`a?.b.c`, `x?.f()`, `arr?.[i]`, …).
    /// Every link is evaluated left-to-right; an optional link that sees a
    /// nullish receiver jumps to a single landing that yields `undefined`,
    /// skipping all remaining links (later non-optional accesses and calls).
    fn compile_optional_chain(&mut self, top: &Expr) -> Result<()> {
        let mut short_circuits: Vec<usize> = Vec::new();
        self.compile_chain_link(top, &mut short_circuits)?;
        let done = self.emit(Instruction::Jump(0));
        // Short-circuit landing: the guard left [.., obj, obj] on the stack
        // (Dup + peeked-nullish), so drop both and yield undefined.
        let sc_target = self.current_offset();
        for j in &short_circuits {
            self.patch_jump(*j, sc_target);
        }
        self.emit(Instruction::Pop);
        self.emit(Instruction::Pop);
        self.emit(Instruction::Push(Constant::Undefined));
        let end = self.current_offset();
        self.patch_jump(done, end);
        Ok(())
    }

    /// Compile one link of an optional chain, recursing into its object/callee
    /// first. A non-chain head is compiled normally. Each link keeps exactly one
    /// value on the stack; an optional link guards its receiver and records a
    /// short-circuit jump (taken with `[.., obj, obj]` on the stack).
    fn compile_chain_link(&mut self, expr: &Expr, sc: &mut Vec<usize>) -> Result<()> {
        match expr {
            Expr::Member {
                object,
                property,
                optional,
            } => {
                self.compile_chain_link(object, sc)?;
                if *optional {
                    self.emit(Instruction::Dup);
                    sc.push(self.emit(Instruction::JumpIfNullish(0)));
                    self.emit(Instruction::Pop);
                }
                self.emit(Instruction::GetProperty(Arc::from(property.as_str())));
            }
            Expr::ComputedMember {
                object,
                property,
                optional,
            } => {
                self.compile_chain_link(object, sc)?;
                if *optional {
                    self.emit(Instruction::Dup);
                    sc.push(self.emit(Instruction::JumpIfNullish(0)));
                    self.emit(Instruction::Pop);
                }
                self.compile_expr(property)?;
                self.emit(Instruction::GetIndex);
            }
            Expr::Call {
                callee,
                args,
                optional,
            } => {
                self.compile_chain_link(callee, sc)?;
                if *optional {
                    self.emit(Instruction::Dup);
                    sc.push(self.emit(Instruction::JumpIfNullish(0)));
                    self.emit(Instruction::Pop);
                }
                if args.iter().any(|a| matches!(a, Expr::Spread(_))) {
                    self.compile_spread_args(args)?;
                    self.emit(Instruction::CallSpread);
                } else {
                    for arg in args {
                        self.compile_expr(arg)?;
                    }
                    self.emit(Instruction::Call(args.len()));
                }
            }
            // The head of the chain (e.g. an identifier) — compile normally.
            other => self.compile_expr(other)?,
        }
        Ok(())
    }

    /// Emit the closure creation + name binding for a function declaration.
    fn emit_function_decl_binding(
        &mut self,
        func_index: usize,
        name: &Option<String>,
    ) -> Result<()> {
        self.emit(Instruction::CreateClosure(func_index));
        if let Some(name) = name {
            if self.is_session_chunk() {
                self.record_top_level_binding(name, TopLevelBindingKind::Function)?;
                self.emit(Instruction::StoreGlobal(Arc::from(name.as_str())));
            } else {
                // Store as both local and global so recursion + globals resolve.
                self.emit(Instruction::Dup);
                let idx = self.declare_local(name);
                self.emit(Instruction::StoreLocal(idx));
                self.emit(Instruction::StoreGlobal(Arc::from(name.as_str())));
            }
        } else {
            self.emit(Instruction::Pop);
        }
        Ok(())
    }

    /// Hoist top-level function declarations of a body: bind each before the
    /// body's other statements run, so forward references and mutual recursion
    /// resolve (JS function-declaration hoisting).
    fn hoist_function_decls(&mut self, stmts: &[Statement]) -> Result<()> {
        for stmt in stmts {
            if let Statement::FunctionDecl {
                func_index, name, ..
            } = stmt
            {
                if self.hoisted_funcs.insert(*func_index) {
                    self.emit_function_decl_binding(*func_index, name)?;
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
                    Some(self.declare_binding(name, kind)?)
                };
                if matches!(kind, VarKind::Const) {
                    match idx {
                        Some(i) => {
                            self.const_slots.insert(i);
                        }
                        None => {
                            self.const_globals.insert(name.to_string());
                        }
                    }
                }
                match &decl.init {
                    Some(expr) => {
                        self.compile_expr(expr)?;
                        self.emit(match idx {
                            Some(idx) => self.top_level_store_instruction(name, idx),
                            None => Instruction::StoreGlobal(Arc::from(name.as_str())),
                        });
                    }
                    None => {
                        self.emit(Instruction::Push(Constant::Undefined));
                        self.emit(match idx {
                            Some(idx) => self.top_level_store_instruction(name, idx),
                            None => Instruction::StoreGlobal(Arc::from(name.as_str())),
                        });
                    }
                }
            }
            // Object/array destructuring var-decls share the recursive pattern
            // path used for parameters and `for…of` (handles element defaults and
            // arbitrary object/array nesting).
            AssignTarget::Pattern(pattern) => {
                if let Some(expr) = &decl.init {
                    self.compile_expr(expr)?;
                } else {
                    self.emit(Instruction::Push(Constant::Undefined));
                }
                self.compile_destructure_pattern(pattern, kind)?;
                self.emit(Instruction::Pop); // pop source value
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
                        if let AssignTarget::Rest(name) = target {
                            // `...rest`: bind the remaining elements as an array.
                            self.emit(Instruction::Dup);
                            self.emit(Instruction::ArrayRestFrom(i));
                            self.store_binding(name, kind)?;
                            continue;
                        }
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
            // A bare `...rest` is only valid inside an array pattern, never as a
            // top-level declaration target.
            AssignTarget::Rest(_) => {}
        }
        Ok(())
    }

    fn declare_destructure_locals(&mut self, fields: &[DestructureField]) {
        for field in fields {
            if field.rest {
                let name = field.alias.as_ref().unwrap_or(&field.key);
                self.declare_local(name);
            } else if let Some(nested) = &field.nested {
                self.declare_param_pattern_locals(nested);
            } else {
                let name = field.alias.as_ref().unwrap_or(&field.key);
                self.declare_local(name);
            }
        }
    }

    /// Declare the locals a parameter pattern binds, in the exact order the VM's
    /// `extract_pattern` pushes them (so positionally-bound destructured params
    /// land in the matching slots).
    fn declare_param_pattern_locals(&mut self, pattern: &ParamPattern) {
        match pattern {
            ParamPattern::Ident(name) | ParamPattern::Rest(name) => {
                self.declare_local(name);
            }
            ParamPattern::DefaultValue { pattern, .. } => {
                self.declare_param_pattern_locals(pattern);
            }
            ParamPattern::ObjectDestructure(fields) => {
                self.declare_destructure_locals(fields);
            }
            ParamPattern::ArrayDestructure(elems) => {
                for elem in elems.iter().flatten() {
                    self.declare_param_pattern_locals(elem);
                }
            }
        }
    }

    fn store_binding(&mut self, name: &str, kind: VarKind) -> Result<()> {
        if self.is_session_chunk() {
            self.record_top_level_binding(name, TopLevelBindingKind::from_var_kind(kind))?;
            if matches!(kind, VarKind::Const) {
                self.const_globals.insert(name.to_string());
            }
            self.emit(Instruction::StoreGlobal(Arc::from(name)));
        } else {
            let idx = self.declare_binding(name, kind)?;
            if matches!(kind, VarKind::Const) {
                self.const_slots.insert(idx);
            }
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
                // The source must be iterated, not merely indexed: a generator or
                // an object with a custom `[Symbol.iterator]` is materialized into
                // an array first (arrays/strings/etc. pass through unchanged).
                self.emit(Instruction::IterableToArray);
                for (i, elem) in elems.iter().enumerate() {
                    let Some(p) = elem else { continue };
                    if let ParamPattern::Rest(name) = p {
                        self.emit(Instruction::Dup);
                        self.emit(Instruction::ArrayRestFrom(i));
                        self.store_binding(name, kind)?;
                        continue;
                    }
                    // Read element `i` of the source array onto the stack.
                    self.emit(Instruction::Dup);
                    self.emit(Instruction::Push(Constant::Int(i as i64)));
                    self.emit(Instruction::GetIndex);
                    // `[a = expr]`: a missing/undefined element takes its default;
                    // the default may reference earlier-bound names in the pattern.
                    let inner = if let ParamPattern::DefaultValue { pattern, default } = p {
                        self.emit_apply_default(Some(default))?;
                        pattern.as_ref()
                    } else {
                        p
                    };
                    self.bind_element_pattern(inner, kind)?;
                }
                Ok(())
            }
            ParamPattern::Ident(name) => self.store_binding(name, kind),
            ParamPattern::DefaultValue { pattern, .. } => {
                // A whole-pattern default (`function f([a, b] = [7, 8])`) is
                // applied by the parameter-default path before this runs; here we
                // just destructure the (possibly defaulted) value.
                self.compile_destructure_pattern(pattern, kind)
            }
            _ => {
                self.emit(Instruction::Pop);
                Ok(())
            }
        }
    }

    /// Bind a single array-element pattern whose value is already on top of the
    /// stack (after any default has been applied). Consumes that value.
    fn bind_element_pattern(&mut self, p: &ParamPattern, kind: VarKind) -> Result<()> {
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
        Ok(())
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
                // Read the field's value: a computed key (`{[k]: v}`) evaluates
                // the key expression at runtime; a static key uses GetProperty.
                if let Some(key_expr) = &field.computed_key {
                    self.compile_expr(key_expr)?;
                    self.emit(Instruction::GetIndex);
                } else {
                    self.emit(Instruction::GetProperty(Arc::from(field.key.as_str())));
                }
                self.emit_apply_default(field.default.as_ref())?;
                if let Some(nested) = &field.nested {
                    // The nested pattern (object or array) destructures the
                    // field's value, which sits on top of the stack.
                    self.compile_destructure_pattern(nested, kind)?;
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

    /// A reserved, un-typable local name for the hidden temp that holds a
    /// destructured parameter's raw argument (parameter index `i`).
    fn param_temp_name(i: usize) -> String {
        format!("<param${i}>")
    }

    /// `function f({a} = {})`: when the raw argument (held in `temp`) is
    /// undefined, evaluate the whole-pattern default and destructure it into the
    /// pattern's already-declared leaf slots.
    fn emit_pattern_param_default(
        &mut self,
        temp: &str,
        pattern: &ParamPattern,
        default: &Expr,
    ) -> Result<()> {
        let Some(slot) = self.resolve_local(temp) else {
            return Ok(());
        };
        self.emit(Instruction::LoadLocal(slot));
        self.emit(Instruction::Push(Constant::Undefined));
        self.emit(Instruction::StrictEq);
        let to_else = self.emit(Instruction::JumpIfFalse(0));
        self.compile_expr(default)?;
        // Store the default's destructured pieces into the leaf slots. They were
        // already declared up-front as params (function-scoped), so store with
        // Var semantics (reuse the existing slot) rather than re-declaring. The
        // destructure applies every inner default itself, so this branch needs
        // no repair pass (running one anyway would evaluate defaults twice).
        self.compile_destructure_pattern(pattern, VarKind::Var)?;
        self.emit(Instruction::Pop);
        let to_end = self.emit(Instruction::Jump(0));
        let else_target = self.current_offset();
        self.patch_jump(to_else, else_target);
        // The argument was supplied: the VM bound the leaves positionally with
        // no defaults applied, so repair them here — exactly like a destructure
        // param without a whole-pattern default.
        if pattern.has_field_level_default() {
            self.emit_field_level_defaults(temp, pattern)?;
        }
        self.emit_pattern_inner_defaults(pattern, pattern.has_field_level_default())?;
        let end = self.current_offset();
        self.patch_jump(to_end, end);
        Ok(())
    }

    /// `function f({b: {c} = {c: 9}})`: for each top-level field carrying BOTH
    /// a nested pattern and a default, read the raw field off the hidden temp,
    /// apply the default if it is undefined, and re-destructure into the
    /// already-declared leaf slots (Var semantics reuse them). The VM's flat
    /// extraction bound those leaves from `undefined` when the field was
    /// missing; this prologue pass repairs them. (Deeper-than-top-level field
    /// defaults inside params remain unsupported.)
    fn emit_field_level_defaults(&mut self, temp: &str, pattern: &ParamPattern) -> Result<()> {
        let Some(slot) = self.resolve_local(temp) else {
            return Ok(());
        };
        let ParamPattern::ObjectDestructure(fields) = pattern else {
            return Ok(());
        };
        for field in fields {
            let (Some(nested), Some(default)) = (&field.nested, &field.default) else {
                continue;
            };
            self.emit(Instruction::LoadLocal(slot));
            if let Some(key_expr) = &field.computed_key {
                self.compile_expr(key_expr)?;
                self.emit(Instruction::GetIndex);
            } else {
                self.emit(Instruction::GetProperty(Arc::from(field.key.as_str())));
            }
            self.emit_apply_default(Some(default))?;
            self.compile_destructure_pattern(nested, VarKind::Var)?;
            self.emit(Instruction::Pop);
        }
        Ok(())
    }

    /// Apply the inner field/element defaults of a destructured parameter
    /// pattern, whose leaves were bound positionally by `bind_params`. Walks
    /// object fields (`{a = 5}`), array elements (`[a = 1]`), and nesting.
    ///
    /// `skip_repaired`: when the field-level repair prologue
    /// (`emit_field_level_defaults`) already ran for this pattern, its
    /// re-destructure applied every default inside the repaired subtrees —
    /// descending into them again would evaluate those defaults twice.
    fn emit_pattern_inner_defaults(
        &mut self,
        pattern: &ParamPattern,
        skip_repaired: bool,
    ) -> Result<()> {
        match pattern {
            ParamPattern::DefaultValue { pattern, .. } => {
                self.emit_pattern_inner_defaults(pattern, skip_repaired)
            }
            ParamPattern::ObjectDestructure(fields) => {
                for field in fields {
                    if field.rest {
                        continue;
                    }
                    if let Some(nested) = &field.nested {
                        if skip_repaired && field.default.is_some() {
                            continue;
                        }
                        // Defaults applied directly to the field (`{a: [x] = []}`)
                        // are handled at the destructure site; here we only descend
                        // into the nested pattern's own leaf defaults.
                        self.emit_pattern_inner_defaults(nested, false)?;
                    } else {
                        let name = field.alias.as_ref().unwrap_or(&field.key);
                        if let (Some(def), Some(slot)) = (&field.default, self.resolve_local(name)) {
                            self.emit_slot_default(slot, def)?;
                        }
                    }
                }
                Ok(())
            }
            ParamPattern::ArrayDestructure(elems) => {
                for elem in elems.iter().flatten() {
                    match elem {
                        ParamPattern::DefaultValue { pattern, default } => {
                            if let ParamPattern::Ident(name) = pattern.as_ref() {
                                if let Some(slot) = self.resolve_local(name) {
                                    self.emit_slot_default(slot, default)?;
                                }
                            } else {
                                self.emit_pattern_inner_defaults(pattern, false)?;
                            }
                        }
                        ParamPattern::ObjectDestructure(_) | ParamPattern::ArrayDestructure(_) => {
                            self.emit_pattern_inner_defaults(elem, false)?;
                        }
                        _ => {}
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
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
            Expr::BigIntLit(v) => {
                self.emit(Instruction::Push(Constant::BigInt(v.clone())));
            }
            Expr::StringLit(s) => {
                self.emit(Instruction::Push(Constant::String(Arc::from(s.as_str()))));
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
                        self.emit(Instruction::Push(Constant::String(Arc::from(quasi.as_str()))));
                        parts += 1;
                    }
                    if i < exprs.len() {
                        self.compile_expr(&exprs[i])?;
                        parts += 1;
                    }
                }
                if parts == 0 {
                    self.emit(Instruction::Push(Constant::String(Arc::from(String::new().as_str()))));
                } else {
                    // Always concat (even a single interpolated expression) so the
                    // result is string-coerced: `${obj}` yields "[object Object]".
                    self.emit(Instruction::ConcatStrings(parts));
                }
            }
            Expr::RegExpLit { pattern, flags } => {
                self.emit(Instruction::Push(Constant::String(
                    Arc::from("__regexp__"),
                )));
                self.emit(Instruction::Push(Constant::Bool(true)));
                self.emit(Instruction::Push(Constant::String(Arc::from("pattern"))));
                self.emit(Instruction::Push(Constant::String(Arc::from(pattern.as_str()))));
                self.emit(Instruction::Push(Constant::String(Arc::from("flags"))));
                self.emit(Instruction::Push(Constant::String(Arc::from(flags.as_str()))));
                // `lastIndex` is the mutable cursor maintained by `exec`/`test` on
                // /g (and /y) regexes so that repeated calls advance through the
                // subject string and eventually terminate (G3).
                self.emit(Instruction::Push(Constant::String(Arc::from("lastIndex"))));
                self.emit(Instruction::Push(Constant::Int(0)));
                self.emit(Instruction::CreateObject(4));
            }
            Expr::Ident(name) => {
                if name == "this" {
                    self.emit(Instruction::LoadThis);
                } else if let Some(idx) = self.resolve_local(name) {
                    self.emit(Instruction::LoadLocal(idx));
                } else {
                    self.emit(Instruction::LoadGlobal(Arc::from(name.as_str())));
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
                let has_accessor = props
                    .iter()
                    .any(|p| matches!(p.kind, PropKind::Get | PropKind::Set));
                if has_accessor {
                    // Object literal with getters/setters. Build the base object
                    // (data + spread props) incrementally, then attach the
                    // `__getters__`/`__setters__` accessor tables the runtime
                    // already consults on property read/write. No new bytecode —
                    // the tables are plain reserved-key sub-objects.
                    self.emit(Instruction::CreateObject(0));
                    let emit_key = |c: &mut Self, prop: &ObjProperty| -> Result<()> {
                        match &prop.key_expr {
                            Some(key_expr) => c.compile_expr(key_expr),
                            None => {
                                c.emit(Instruction::Push(Constant::String(Arc::from(prop.key.as_str()))));
                                Ok(())
                            }
                        }
                    };
                    for prop in props {
                        match prop.kind {
                            PropKind::Spread => {
                                self.compile_expr(&prop.value)?;
                                self.emit(Instruction::ObjectSpreadAssign);
                            }
                            // Accessors are ALSO stored as a data prop (key -> fn)
                            // so the key enumerates in source order for free —
                            // Object.keys / for-in need no special-casing. The
                            // __getters__/__setters__ tables below drive the actual
                            // read/write; value-producing surfaces (JSON, spread,
                            // Object.values/entries) invoke the getter rather than
                            // serializing the stored function.
                            _ => {
                                emit_key(self, prop)?;
                                self.compile_expr(&prop.value)?;
                                self.emit(Instruction::ObjectInsert);
                            }
                        }
                    }
                    // Attach the accessor tables: `obj.__getters__ = { name: fn }`.
                    for (table, want_get) in [("__getters__", true), ("__setters__", false)] {
                        let accessors: Vec<&ObjProperty> = props
                            .iter()
                            .filter(|p| {
                                matches!(p.kind, PropKind::Get if want_get)
                                    || matches!(p.kind, PropKind::Set if !want_get)
                            })
                            .collect();
                        if accessors.is_empty() {
                            continue;
                        }
                        self.emit(Instruction::Push(Constant::String(Arc::from(table))));
                        self.emit(Instruction::CreateObject(0));
                        for acc in accessors {
                            emit_key(self, acc)?;
                            self.compile_expr(&acc.value)?;
                            self.emit(Instruction::ObjectInsert);
                        }
                        self.emit(Instruction::ObjectInsert);
                    }
                } else if !has_spread {
                    let mut count = 0;
                    for prop in props {
                        match &prop.key_expr {
                            Some(key_expr) => self.compile_expr(key_expr)?,
                            None => {
                                self.emit(Instruction::Push(Constant::String(Arc::from(prop.key.as_str()))));
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
                                            Arc::from(prop.key.as_str()),
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
                // A member operand's object/index evaluate ONCE (hidden
                // temps): `f().n++` calls `f` once, like JS.
                let rmw = self.prepare_rmw_target(operand)?;
                match &rmw {
                    Some(r) => self.emit_rmw_load(r),
                    None => self.compile_expr(operand)?,
                }

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

                // Store back. The RMW store leaves its value on the stack
                // (post-value for prefix); the dup'd pre-value sits below it
                // for the postfix result — pop the stored value to expose it.
                match &rmw {
                    Some(r) => {
                        self.emit_rmw_store(r);
                        self.emit(Instruction::Pop);
                    }
                    None => {
                        self.compile_store(operand)?;
                    }
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
                        let rmw = self.prepare_rmw_target(target)?;
                        match &rmw {
                            Some(r) => self.emit_rmw_load(r),
                            None => self.compile_expr(target)?,
                        }
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
                        match &rmw {
                            Some(r) => self.emit_rmw_store(r),
                            None => {
                                self.emit(Instruction::Dup);
                                self.compile_store(target)?;
                            }
                        }
                        let after = self.current_offset();
                        self.patch_jump(end, after);
                    }
                    AssignOp::NullishAssign => {
                        let rmw = self.prepare_rmw_target(target)?;
                        match &rmw {
                            Some(r) => self.emit_rmw_load(r),
                            None => self.compile_expr(target)?,
                        }
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
                        match &rmw {
                            Some(r) => self.emit_rmw_store(r),
                            None => {
                                self.emit(Instruction::Dup);
                                self.compile_store(target)?;
                            }
                        }
                        let after = self.current_offset();
                        self.patch_jump(end, after);
                    }
                    _ => {
                        // Compound assignment: load, operate, store. A member
                        // target's object/index evaluate ONCE (hidden temps) —
                        // `f().x += 1` calls `f` once, like JS.
                        let rmw = self.prepare_rmw_target(target)?;
                        match &rmw {
                            Some(r) => self.emit_rmw_load(r),
                            None => self.compile_expr(target)?,
                        }
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
                        match &rmw {
                            Some(r) => self.emit_rmw_store(r),
                            None => {
                                self.emit(Instruction::Dup);
                                self.compile_store(target)?;
                            }
                        }
                    }
                }
            }
            Expr::DestructureAssign { pattern, value } => {
                self.compile_destructure_assign(pattern, value)?;
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
                // An optional chain short-circuits the *whole* chain (incl. later
                // non-optional accesses/calls) to undefined when a `?.` link is
                // nullish, so it's compiled as a unit.
                if expr_is_optional_chain(expr) {
                    return self.compile_optional_chain(expr);
                }
                // `super.prop` (not a call): read from the defining class's super
                // prototype. Note a *bare* super-method reference (without an
                // immediate call) loses its `this` binding here; that mirrors plain
                // method references and is acceptable for C3.
                if matches!(object.as_ref(), Expr::Ident(n) if n == "super") {
                    let class = self.current_class.clone().ok_or_else(|| {
                        ZapcodeError::CompileError(
                            "'super' keyword unexpected here (outside a class method)".to_string(),
                        )
                    })?;
                    self.emit(Instruction::LoadSuperProp {
                        class: Arc::from(class.as_str()),
                        prop: Arc::from(property.as_str()),
                    });
                    return Ok(());
                }
                self.compile_expr(object)?;
                if *optional {
                    self.emit(Instruction::Dup);
                    let skip = self.emit(Instruction::JumpIfNullish(0));
                    self.emit(Instruction::Pop);
                    self.emit(Instruction::GetProperty(Arc::from(property.as_str())));
                    let end = self.emit(Instruction::Jump(0));
                    let nullish = self.current_offset();
                    self.patch_jump(skip, nullish);
                    self.emit(Instruction::Pop);
                    self.emit(Instruction::Pop);
                    self.emit(Instruction::Push(Constant::Undefined));
                    let after = self.current_offset();
                    self.patch_jump(end, after);
                } else {
                    self.emit(Instruction::GetProperty(Arc::from(property.as_str())));
                }
            }
            Expr::ComputedMember {
                object,
                property,
                optional,
            } => {
                if expr_is_optional_chain(expr) {
                    return self.compile_optional_chain(expr);
                }
                self.compile_expr(object)?;
                if *optional {
                    self.emit(Instruction::Dup);
                    let skip = self.emit(Instruction::JumpIfNullish(0));
                    self.emit(Instruction::Pop);
                    self.compile_expr(property)?;
                    self.emit(Instruction::GetIndex);
                    let end = self.emit(Instruction::Jump(0));
                    let nullish = self.current_offset();
                    self.patch_jump(skip, nullish);
                    self.emit(Instruction::Pop);
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
                if expr_is_optional_chain(expr) {
                    return self.compile_optional_chain(expr);
                }
                // Check for `Promise.{all,race,any,allSettled}([ ...direct
                // external calls... ])` — when any element is a direct external
                // call, compile the elements as deferred calls so the host can
                // run them in parallel and settle them per the combinator.
                if self.try_compile_promise_batch(callee, args)? {
                    return Ok(());
                }
                // Check if this is a super() call
                if let Expr::Ident(name) = callee.as_ref() {
                    if name == "super" {
                        for arg in args {
                            self.compile_expr(arg)?;
                        }
                        self.emit(Instruction::CallSuper {
                            arg_count: args.len(),
                            class: self
                                .current_class
                                .as_deref()
                                .map(Arc::from),
                        });
                        return Ok(());
                    }
                }
                // Check if this is a `super.method(...)` call: resolve the parent
                // method against the defining class's super prototype and call it
                // with the current `this` bound as receiver.
                if let Expr::Member {
                    object, property, ..
                } = callee.as_ref()
                {
                    if matches!(object.as_ref(), Expr::Ident(n) if n == "super") {
                        let class = self.current_class.clone().ok_or_else(|| {
                            ZapcodeError::CompileError(
                                "'super' keyword unexpected here (outside a class method)"
                                    .to_string(),
                            )
                        })?;
                        self.emit(Instruction::LoadSuperMethod {
                            class: Arc::from(class.as_str()),
                            method: Arc::from(property.as_str()),
                        });
                        for arg in args {
                            self.compile_expr(arg)?;
                        }
                        self.emit(Instruction::Call(args.len()));
                        return Ok(());
                    }
                }
                let has_spread = args.iter().any(|a| matches!(a, Expr::Spread(_)));
                // Check if this is a direct call to an external function. A bare
                // (un-awaited) tool call evaluates to a deferred single-call
                // Promise object (N5): the host call is registered but not made
                // until the promise is awaited or driven by .then/.catch/.finally.
                // A *directly awaited* tool call (`await tool()`) is special-cased
                // in `Expr::Await` to use the eager-suspend `CallExternal` path,
                // so it never reaches here — keeping that hot path unchanged.
                if let Expr::Ident(name) = callee.as_ref() {
                    if self.external_functions.contains(name) {
                        if has_spread {
                            self.compile_spread_args(args)?;
                            self.emit(Instruction::CallExternalSpread(Arc::from(name.as_str())));
                        } else {
                            for arg in args {
                                self.compile_expr(arg)?;
                            }
                            self.emit(Instruction::CallExternalDeferred(Arc::from(name.as_str()), args.len()));
                            self.emit(Instruction::MakeCallPromise);
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
                // Fast path: `await tool(args)` where `tool` is a direct external
                // call (no spread). Compile to the eager-suspend `CallExternal`,
                // exactly as before N5 — the call suspends here and resumes with
                // the result, and the following `Await` passes the (non-promise)
                // result through. This keeps the overwhelmingly common
                // await-a-tool-call path byte-for-byte unchanged and off the new
                // deferred single-call-promise machinery.
                let mut handled = false;
                if let Expr::Call { callee, args, .. } = expr.as_ref() {
                    if !expr_is_optional_chain(expr)
                        && !args.iter().any(|a| matches!(a, Expr::Spread(_)))
                    {
                        if let Expr::Ident(name) = callee.as_ref() {
                            if self.external_functions.contains(name) {
                                for arg in args {
                                    self.compile_expr(arg)?;
                                }
                                self.emit(Instruction::CallExternal(Arc::from(name.as_str()), args.len()));
                                handled = true;
                            }
                        }
                    }
                }
                if !handled {
                    self.compile_expr(expr)?;
                }
                // Emit Await instruction to unwrap Promise objects (including a
                // deferred single-call promise, which suspends here on its host
                // call). External call suspension for the direct `await tool()`
                // form is handled by CallExternal above.
                self.emit(Instruction::Await);
            }
            Expr::Yield { value, delegate } => {
                if *delegate {
                    // `yield* X` delegates to an iterable: iterate X and yield each
                    // element individually (flattening), then leave the delegate's
                    // completion value on the stack as the value of the `yield*`
                    // expression. We reuse the iterator protocol so arrays,
                    // strings, generators, Sets and Maps all delegate correctly.
                    let expr = value
                        .as_deref()
                        .expect("yield* always carries an operand expression");
                    self.compile_expr(expr)?;
                    self.emit(Instruction::GetIterator);
                    // loop:
                    let loop_start = self.current_offset();
                    self.emit(Instruction::IteratorNext); // -> [advanced_iter, value]
                    self.emit(Instruction::IteratorDone); // -> [iter, value(if !done), done]
                    let exit_jump = self.emit(Instruction::JumpIfTrue(0));
                    // Stack: [iter, value]. Yield pops the value, suspends, and on
                    // resume pushes the value sent into `.next(v)`. We discard that
                    // resumed value (sent values are not threaded into the delegate).
                    self.emit(Instruction::Yield);
                    self.emit(Instruction::Pop);
                    self.emit(Instruction::Jump(loop_start));
                    // exit: stack is [iter] (IteratorDone did not push a value when done).
                    let exit = self.current_offset();
                    self.patch_jump(exit_jump, exit);
                    self.emit(Instruction::Pop); // pop the exhausted iterator
                    // Completion value of `yield* X` (the iterator's return value).
                    // For array/string/Set/Map iterators this is `undefined`, which
                    // matches JS. (A delegated generator's explicit return value is
                    // not propagated here.)
                    self.emit(Instruction::Push(Constant::Undefined));
                } else {
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
                    } => {
                        self.compile_expr(object)?;
                        self.emit(Instruction::DeleteProperty(Arc::from(property.as_str())));
                        // The mutation hits the shared heap slot in place; a
                        // nameable parent gets the formal write-back, while an
                        // arbitrary object expression (`delete f().x`) just
                        // drops the pushed-back reference — same rule as
                        // member stores.
                        if Self::is_store_target(object) {
                            self.compile_store_inner(object, false)?;
                        } else {
                            self.emit(Instruction::Pop);
                        }
                        self.emit(Instruction::Push(Constant::Bool(true)));
                    }
                    Expr::ComputedMember {
                        object, property, ..
                    } => {
                        self.compile_expr(object)?;
                        self.compile_expr(property)?;
                        self.emit(Instruction::DeleteIndex);
                        if Self::is_store_target(object) {
                            self.compile_store_inner(object, false)?;
                        } else {
                            self.emit(Instruction::Pop);
                        }
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
                fields,
                static_fields,
            } => {
                self.compile_class(
                    name.as_deref(),
                    super_class.as_deref(),
                    constructor.as_deref(),
                    methods,
                    static_methods,
                    fields,
                    static_fields,
                )?;
            }
        }
        Ok(())
    }

    /// If `callee(args)` is `Promise.{all,race,any,allSettled}([...])` and at
    /// least one array element is a direct call to an external function, compile
    /// it as a parallel batch tagged with the combinator kind: each direct
    /// external-call element becomes a deferred call (no suspend), other
    /// elements compile normally, then `MakeBatchPromise(kind, n)` wraps them.
    /// Returns `true` if it handled the call.
    fn try_compile_promise_batch(&mut self, callee: &Expr, args: &[Expr]) -> Result<bool> {
        // callee must be `Promise.<combinator>`
        let Expr::Member { object, property, .. } = callee else {
            return Ok(false);
        };
        if !matches!(object.as_ref(), Expr::Ident(n) if n == "Promise") {
            return Ok(false);
        }
        let kind = match property.as_str() {
            "all" => BatchKind::All,
            "race" => BatchKind::Race,
            "any" => BatchKind::Any,
            "allSettled" => BatchKind::AllSettled,
            _ => return Ok(false),
        };
        if args.len() != 1 {
            return Ok(false);
        }
        let Expr::Array(elements) = &args[0] else {
            return Ok(false);
        };
        // Only take the batch path if there's at least one direct external call;
        // otherwise fall through to the normal Promise.* builtin so existing
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
                        self.emit(Instruction::CallExternalDeferred(Arc::from(name.as_str()), argc));
                    } else {
                        self.compile_expr(expr)?;
                    }
                }
                None => {
                    self.emit(Instruction::Push(Constant::Undefined));
                }
            }
        }
        self.emit(Instruction::MakeBatchPromise(kind, elements.len()));
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

    #[allow(clippy::too_many_arguments)]
    fn compile_class(
        &mut self,
        name: Option<&str>,
        super_class: Option<&str>,
        constructor: Option<&FunctionDef>,
        methods: &[ClassMethod],
        static_methods: &[ClassMethod],
        fields: &[ClassField],
        static_fields: &[ClassField],
    ) -> Result<()> {
        let class_name = name.unwrap_or("AnonymousClass").to_string();

        // Push super class if present. Resolve the super reference under the OUTER
        // class context (it names a sibling/ancestor binding, not this class).
        if let Some(sc) = super_class {
            if let Some(idx) = self.resolve_local(sc) {
                self.emit(Instruction::LoadLocal(idx));
            } else {
                self.emit(Instruction::LoadGlobal(Arc::from(sc)));
            }
        }

        // Method/constructor bodies are compiled with this class as the lexical
        // `super` context; restore the previous context afterwards so nested or
        // sibling classes don't leak it.
        let prev_class = self.current_class.take();
        self.current_class = Some(class_name.clone());

        // Split methods by kind so getters/setters install accessor descriptors.
        let plain = |ms: &[ClassMethod], k: ClassMethodKind| -> Vec<ClassMethod> {
            ms.iter().filter(|m| m.kind == k).cloned().collect()
        };
        let inst_methods = plain(methods, ClassMethodKind::Method);
        let inst_getters = plain(methods, ClassMethodKind::Get);
        let inst_setters = plain(methods, ClassMethodKind::Set);
        let stat_methods = plain(static_methods, ClassMethodKind::Method);
        let stat_getters = plain(static_methods, ClassMethodKind::Get);
        let stat_setters = plain(static_methods, ClassMethodKind::Set);

        // Push, in CreateClass's documented order (popped in reverse):
        //   static_fields, fields, static setters, static getters,
        //   instance setters, instance getters, statics, methods, constructor.
        self.push_class_field_pairs(static_fields)?;
        self.push_class_field_pairs(fields)?;
        self.push_class_method_pairs(&stat_setters)?;
        self.push_class_method_pairs(&stat_getters)?;
        self.push_class_method_pairs(&inst_setters)?;
        self.push_class_method_pairs(&inst_getters)?;
        self.push_class_method_pairs(&stat_methods)?;
        self.push_class_method_pairs(&inst_methods)?;

        // Push constructor closure (or undefined if none)
        if let Some(ctor) = constructor {
            let compiled = self.compile_function_def(ctor)?;
            let func_idx = self.functions.borrow().len();
            self.functions.borrow_mut().push(compiled);
            self.emit(Instruction::CreateClosure(func_idx));
        } else {
            self.emit(Instruction::Push(Constant::Undefined));
        }

        self.current_class = prev_class;

        self.emit(Instruction::CreateClass(Box::new(
            crate::compiler::instruction::CreateClassSpec {
                name: Arc::from(class_name.as_str()),
                n_methods: inst_methods.len(),
                n_statics: stat_methods.len(),
                n_getters: inst_getters.len(),
                n_setters: inst_setters.len(),
                n_static_getters: stat_getters.len(),
                n_static_setters: stat_setters.len(),
                n_fields: fields.len(),
                n_static_fields: static_fields.len(),
                has_super: super_class.is_some(),
            },
        )));

        Ok(())
    }

    /// Emit `(name_string, closure)` pairs for a group of class methods/accessors.
    fn push_class_method_pairs(&mut self, ms: &[ClassMethod]) -> Result<()> {
        for m in ms {
            self.emit(Instruction::Push(Constant::String(Arc::from(m.name.as_str()))));
            let compiled = self.compile_function_def(&m.func)?;
            let func_idx = self.functions.borrow().len();
            self.functions.borrow_mut().push(compiled);
            self.emit(Instruction::CreateClosure(func_idx));
        }
        Ok(())
    }

    /// Emit `(name_string, init_closure)` pairs for class field declarations. Each
    /// initializer is a zero-arg function whose body `return`s the field value
    /// (or `undefined` for a bare `x;`), run later with `this` bound to the
    /// instance/class so `this.other` references resolve.
    fn push_class_field_pairs(&mut self, fields: &[ClassField]) -> Result<()> {
        for f in fields {
            self.emit(Instruction::Push(Constant::String(Arc::from(f.name.as_str()))));
            let init = FunctionDef {
                name: None,
                params: Vec::new(),
                body: vec![Statement::Return {
                    value: f.value.clone(),
                    span: Span { start: 0, end: 0 },
                }],
                is_async: false,
                is_generator: false,
                is_arrow: false,
                span: Span { start: 0, end: 0 },
            };
            let compiled = self.compile_function_def(&init)?;
            let func_idx = self.functions.borrow().len();
            self.functions.borrow_mut().push(compiled);
            self.emit(Instruction::CreateClosure(func_idx));
        }
        Ok(())
    }

    /// Compile a destructuring ASSIGNMENT expression (`[a, b] = …`,
    /// `({x: o.p} = …)`). The whole expression evaluates to the right-hand value
    /// (left on the stack), matching JS.
    fn compile_destructure_assign(
        &mut self,
        pattern: &AssignPattern,
        value: &Expr,
    ) -> Result<()> {
        self.compile_expr(value)?;
        // Keep one copy of the source as the expression's result; destructure a
        // duplicate into the targets.
        self.emit(Instruction::Dup);
        self.compile_assign_pattern_value(pattern)?;
        Ok(())
    }

    /// Destructure the value on top of the stack into `pattern`, consuming it.
    fn compile_assign_pattern_value(&mut self, pattern: &AssignPattern) -> Result<()> {
        match pattern {
            // A leaf lvalue: store the (already-on-stack) value into it.
            AssignPattern::Target(target) => self.compile_store(target),
            AssignPattern::Array { elements, rest } => {
                for (i, elem) in elements.iter().enumerate() {
                    if let Some(elem) = elem {
                        self.emit(Instruction::Dup);
                        self.emit(Instruction::Push(Constant::Int(i as i64)));
                        self.emit(Instruction::GetIndex);
                        self.emit_apply_default(elem.default.as_ref())?;
                        self.compile_assign_pattern_value(&elem.pattern)?;
                    }
                }
                if let Some(rest) = rest {
                    self.emit(Instruction::Dup);
                    self.emit(Instruction::ArrayRestFrom(elements.len()));
                    self.compile_assign_pattern_value(rest)?;
                }
                // Drop the source value.
                self.emit(Instruction::Pop);
                Ok(())
            }
            AssignPattern::Object { fields, rest } => {
                let excluded: Vec<String> =
                    fields.iter().map(|f| f.key.clone()).collect();
                for field in fields {
                    self.emit(Instruction::Dup);
                    if let Some(key_expr) = &field.computed_key {
                        self.compile_expr(key_expr)?;
                        self.emit(Instruction::GetIndex);
                    } else {
                        self.emit(Instruction::GetProperty(Arc::from(field.key.as_str())));
                    }
                    self.emit_apply_default(field.default.as_ref())?;
                    self.compile_assign_pattern_value(&field.pattern)?;
                }
                if let Some(rest) = rest {
                    self.emit(Instruction::Dup);
                    self.emit(Instruction::ObjectRest(excluded));
                    self.compile_assign_pattern_value(rest)?;
                }
                // Drop the source value.
                self.emit(Instruction::Pop);
                Ok(())
            }
        }
    }

    /// Emit code that throws `new TypeError(msg)` at runtime (catchable). Used
    /// for errors JS surfaces at runtime rather than parse time, e.g. assigning
    /// to a `const`. The bytecode appends to whatever is on the stack; the throw
    /// unwinds it.
    fn emit_throw_type_error(&mut self, msg: &str) {
        self.emit(Instruction::LoadGlobal(Arc::from("TypeError")));
        self.emit(Instruction::Push(Constant::String(Arc::from(msg))));
        self.emit(Instruction::Construct(1));
        self.emit(Instruction::Throw);
    }

    fn compile_store(&mut self, target: &Expr) -> Result<()> {
        self.compile_store_inner(target, true)
    }

    /// Prepare a read-modify-write reference to `target` whose object (and
    /// computed index) expressions are evaluated EXACTLY ONCE into hidden
    /// locals. The whole `op=`/`++`/`||=` family must not re-run the target's
    /// side effects for its store leg (`f().x += 1` calls `f` once in JS);
    /// compiling the target expression twice did. Returns `None` for plain
    /// identifier targets (loads of those are pure — the simple lowering is
    /// already correct).
    fn prepare_rmw_target(&mut self, target: &Expr) -> Result<Option<RmwRef>> {
        match target {
            Expr::Member {
                object, property, ..
            } => {
                let obj_slot = self.declare_rmw_temp();
                self.compile_expr(object)?;
                self.emit(Instruction::StoreLocal(obj_slot));
                Ok(Some(RmwRef {
                    obj_slot,
                    idx_slot: None,
                    property: Some(Arc::from(property.as_str())),
                }))
            }
            Expr::ComputedMember {
                object, property, ..
            } => {
                let obj_slot = self.declare_rmw_temp();
                self.compile_expr(object)?;
                self.emit(Instruction::StoreLocal(obj_slot));
                let idx_slot = self.declare_rmw_temp();
                self.compile_expr(property)?;
                self.emit(Instruction::StoreLocal(idx_slot));
                Ok(Some(RmwRef {
                    obj_slot,
                    idx_slot: Some(idx_slot),
                    property: None,
                }))
            }
            _ => Ok(None),
        }
    }

    fn declare_rmw_temp(&mut self) -> usize {
        let n = self.rmw_temp_counter;
        self.rmw_temp_counter += 1;
        self.declare_local(&format!("__rmw_tmp_{n}"))
    }

    /// Push the referenced member's current value.
    fn emit_rmw_load(&mut self, r: &RmwRef) {
        self.emit(Instruction::LoadLocal(r.obj_slot));
        match (&r.property, r.idx_slot) {
            (Some(p), _) => {
                self.emit(Instruction::GetProperty(p.clone()));
            }
            (None, Some(idx)) => {
                self.emit(Instruction::LoadLocal(idx));
                self.emit(Instruction::GetIndex);
            }
            (None, None) => unreachable!("RmwRef without property or index"),
        }
    }

    /// Store the top-of-stack value into the referenced member, leaving the
    /// value on the stack as the expression result. The receiver is a shared
    /// heap handle, so the in-place mutation needs no write-back to the
    /// object's source place.
    fn emit_rmw_store(&mut self, r: &RmwRef) {
        self.emit(Instruction::Dup);
        self.emit(Instruction::LoadLocal(r.obj_slot));
        match (&r.property, r.idx_slot) {
            (Some(p), _) => {
                self.emit(Instruction::SetProperty(p.clone()));
            }
            (None, Some(idx)) => {
                self.emit(Instruction::LoadLocal(idx));
                self.emit(Instruction::SetIndex);
            }
            (None, None) => unreachable!("RmwRef without property or index"),
        }
        // SetProperty/SetIndex push the (same) object reference back; the
        // duplicated value below it is the expression result.
        self.emit(Instruction::Pop);
    }

    /// True for expressions that name a storable place (an identifier or a
    /// member/index chain rooted in one) — the targets `compile_store_inner`
    /// accepts. The chain must be walked to its root: `foo().bar` is a Member
    /// node but names no storable place, and write-back would re-evaluate
    /// `foo()`, duplicating its side effects.
    fn is_store_target(expr: &Expr) -> bool {
        match expr {
            Expr::Ident(_) => true,
            Expr::Member { object, .. } | Expr::ComputedMember { object, .. } => {
                Self::is_store_target(object)
            }
            _ => false,
        }
    }

    /// Compile a store to `target`. When `check_const` is true (a direct
    /// assignment), assigning to a `const` binding throws. The member/index
    /// write-back paths pass `false`: `const o = {}; o.x = 1` mutates the object
    /// in place (the binding still holds the same reference), which JS allows.
    fn compile_store_inner(&mut self, target: &Expr, check_const: bool) -> Result<()> {
        match target {
            Expr::Ident(name) if name == "this" => {
                self.emit(Instruction::StoreThis);
            }
            Expr::Ident(name) => {
                if let Some(idx) = self.resolve_local(name) {
                    if check_const && self.const_slots.contains(&idx) {
                        // Assigning to a const binding is a runtime TypeError in JS.
                        self.emit_throw_type_error("Assignment to constant variable.");
                    } else {
                        self.emit(Instruction::StoreLocal(idx));
                    }
                } else if check_const && self.const_globals.contains(name) {
                    self.emit_throw_type_error("Assignment to constant variable.");
                } else {
                    self.emit(Instruction::StoreGlobal(Arc::from(name.as_str())));
                }
            }
            Expr::Member {
                object, property, ..
            } => {
                self.compile_expr(object)?;
                self.emit(Instruction::SetProperty(Arc::from(property.as_str())));
                // SetProperty mutates the shared heap slot in place and pushes
                // the same reference back. A nameable parent gets the formal
                // write-back; an arbitrary object expression — a call result,
                // a parenthesized assignment (`(o[k] = o[k] || {}).x = v`), a
                // ternary — has no place to write to, and the mutation is
                // already visible through every alias: drop the reference.
                if Self::is_store_target(object) {
                    self.compile_store_inner(object, false)?;
                } else {
                    self.emit(Instruction::Pop);
                }
            }
            Expr::ComputedMember {
                object, property, ..
            } => {
                self.compile_expr(object)?;
                self.compile_expr(property)?;
                self.emit(Instruction::SetIndex);
                // Same write-back rule as `Expr::Member` above.
                if Self::is_store_target(object) {
                    self.compile_store_inner(object, false)?;
                } else {
                    self.emit(Instruction::Pop);
                }
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


/// Whether an expression's access/call spine contains an optional (`?.`) link,
/// i.e. it is the top of an optional chain and must short-circuit as a whole.
fn expr_is_optional_chain(expr: &Expr) -> bool {
    match expr {
        Expr::Member {
            object, optional, ..
        }
        | Expr::ComputedMember {
            object, optional, ..
        } => *optional || expr_is_optional_chain(object),
        Expr::Call {
            callee, optional, ..
        } => *optional || expr_is_optional_chain(callee),
        _ => false,
    }
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

    let functions = compiler.functions.borrow().clone();
    Ok(CompiledProgram {
        instructions: compiler.instructions,
        functions,
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

    let functions = compiler.functions.borrow().clone();
    Ok((
        CompiledProgram {
            instructions: compiler.instructions,
            functions,
            local_names: compiler.locals,
        },
        compiler.top_level_bindings,
    ))
}
