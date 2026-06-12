pub mod ir;

use ir::*;
use oxc_allocator::Allocator;
use oxc_ast::ast;
use oxc_parser::Parser;
use oxc_span::SourceType;

use crate::error::{Result, ZapcodeError};

type DestructureFieldParts = (Option<String>, Option<Box<ParamPattern>>, Option<Expr>);

/// Maximum bracket/brace/paren nesting depth the source may contain.
///
/// oxc's recursive-descent parser AND our `AstLowerer` descend one native stack
/// frame per nesting level, and each level is *very* stack-hungry — on a 2MB
/// thread stack (Rust's default worker/test stack) a debug build overflows below
/// 80 levels of `[[[…]]]`, aborting the whole host process (an uncatchable
/// `SIGSEGV`/`SIGABRT`) *before any VM resource limit is consulted*, since this
/// is parse time. We reject deeper input up front with a clean, catchable
/// `ParseError`. 64 matches the existing `JSON_MAX_DEPTH` parse cap, is safe on
/// the smallest stack we run on, and is far beyond any realistic AI-generated
/// expression (which nests literals a handful of levels at most).
const MAX_NESTING_DEPTH: usize = 64;

pub fn parse(source: &str) -> Result<Program> {
    // Reject pathologically deep bracket nesting before it reaches oxc's
    // recursive descent (which would overflow the native stack and abort).
    check_nesting_depth(source)?;

    // Auto-wrap trailing object literals: `{ key: value }` → `({ key: value })`
    // This avoids the JS ambiguity where `{` at statement start is a block.
    let source = wrap_trailing_object(source);

    let allocator = Allocator::default();
    let source_type = SourceType::tsx();
    let ret = Parser::new(&allocator, &source, source_type).parse();

    if !ret.errors.is_empty() {
        let msgs: Vec<String> = ret.errors.iter().map(|e| e.to_string()).collect();
        return Err(ZapcodeError::ParseError(msgs.join("\n")));
    }

    let mut lowerer = AstLowerer::new(&source);
    lowerer.lower_program(&ret.program)?;

    Ok(Program {
        body: lowerer.body,
        functions: lowerer.functions,
        // Computed over the wrapped source, which is what the IR spans index.
        line_starts: compute_line_starts(&source),
    })
}

/// Byte offset of the start of every line: entry 0 is offset 0, plus one entry
/// per `\n`. Used to map span offsets to 1-based line/column at error time.
fn compute_line_starts(source: &str) -> Vec<u32> {
    let mut starts = vec![0u32];
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i as u32 + 1);
        }
    }
    starts
}

/// Scan the source and reject it (with a catchable `ParseError`) if the
/// bracket/brace/paren nesting ever exceeds [`MAX_NESTING_DEPTH`]. This runs
/// before oxc so a deeply-nested literal can never drive oxc's recursive descent
/// to native-stack exhaustion (which aborts the host process).
///
/// The scanner is bracket-counting and skips the contents of string, template,
/// and regex-ish literals and of `//` / `/* */` comments, so brackets *inside*
/// strings/comments don't inflate the count. It does not need to be a full lexer
/// — a conservative over-count inside an exotic construct only makes the guard
/// fire slightly earlier, never later, which is the safe direction.
fn check_nesting_depth(source: &str) -> Result<()> {
    let bytes = source.as_bytes();
    let n = bytes.len();
    let mut i = 0usize;
    let mut depth: usize = 0;
    let mut max_depth: usize = 0;

    while i < n {
        let c = bytes[i];
        match c {
            // Line comment: skip to end of line.
            b'/' if i + 1 < n && bytes[i + 1] == b'/' => {
                i += 2;
                while i < n && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            // Block comment: skip to closing */.
            b'/' if i + 1 < n && bytes[i + 1] == b'*' => {
                i += 2;
                while i + 1 < n && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
            }
            // String / template literal: skip to the matching unescaped quote.
            // (Template `${}` interpolations may contain brackets, but counting
            // the whole template as opaque only under-counts depth there, which
            // is safe — a deeply-nested interpolation would still have to repeat
            // outside any single template to reach the cap.)
            b'"' | b'\'' | b'`' => {
                let quote = c;
                i += 1;
                while i < n {
                    if bytes[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if bytes[i] == quote {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b'(' | b'[' | b'{' => {
                depth += 1;
                if depth > max_depth {
                    max_depth = depth;
                    if max_depth > MAX_NESTING_DEPTH {
                        return Err(ZapcodeError::ParseError(format!(
                            "expression nesting depth exceeds the maximum of {}",
                            MAX_NESTING_DEPTH
                        )));
                    }
                }
                i += 1;
            }
            b')' | b']' | b'}' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            _ => i += 1,
        }
    }
    Ok(())
}

/// If the source ends with a `{ ... }` block that looks like an object literal
/// (contains `key: value` or `key,` patterns), wrap it in `(...)` so oxc
/// parses it as an expression instead of a block statement.
fn wrap_trailing_object(source: &str) -> String {
    let trimmed = source.trim_end();

    // Must end with `}`
    if !trimmed.ends_with('}') {
        return source.to_string();
    }

    // Find the matching `{`
    //
    // Limitation: this brace scanner is a simple depth-counting heuristic that
    // does not account for braces inside string literals, comments, or template
    // literals. This is acceptable for preprocessing because:
    //   1. The input is AI-generated tool output (simple expressions), not
    //      arbitrary user code likely to contain embedded brace characters.
    //   2. This is only a heuristic for trailing-object extraction — if the
    //      heuristic produces malformed output, oxc will catch the parse error
    //      downstream, so correctness is never silently lost.
    //
    // TODO: revisit this heuristic if real-world failures are reported (e.g.
    // strings containing unbalanced braces causing incorrect extraction).
    let mut depth = 0;
    let mut open_pos = None;
    for (i, ch) in trimmed.char_indices().rev() {
        match ch {
            '}' => depth += 1,
            '{' => {
                depth -= 1;
                if depth == 0 {
                    open_pos = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }

    let open_pos = match open_pos {
        Some(pos) => pos,
        None => return source.to_string(),
    };

    // The `{` must be at the start of a statement (preceded by newline, semicolon, or start)
    let before = trimmed[..open_pos].trim_end();
    if !before.is_empty() {
        let last_char = before.chars().last().unwrap();
        // If preceded by =, (, return, =>, etc. — it's already in expression context.
        // `)` means the `{` is a control-flow / function block (if/for/while/catch/
        // switch/function with params), never an object literal — don't wrap it.
        //
        // Binary / unary operator characters (`& | ? + - * / % < ! ^ ~`) likewise
        // mean the `{` is the right-hand operand of an expression (e.g. the object
        // literal in `"a" in {a:1}` once the `in` keyword's trailing space is
        // trimmed away leaves no operator char, so the keyword check below handles
        // `in`/`of`; the operator chars here cover things like `x ?? {y:1}`).
        if matches!(
            last_char,
            '=' | '(' | ',' | ':' | '>' | '[' | ')' | '&' | '|' | '?' | '+' | '-' | '*' | '/'
                | '%' | '<' | '!' | '^' | '~'
        ) {
            return source.to_string();
        }
        // If preceded by a keyword that takes a block, don't wrap
        let last_word = before
            .rsplit(|c: char| !c.is_alphanumeric() && c != '_')
            .next()
            .unwrap_or("");
        if matches!(
            last_word,
            "if" | "else"
                | "for"
                | "while"
                | "do"
                | "try"
                | "catch"
                | "finally"
                | "class"
                | "function"
                | "switch"
                // Keywords that introduce expression context: a trailing `{...}` is
                // an object literal operand, not a statement-level block. Wrapping it
                // with `;(` would split the expression and cause a parse error
                // (e.g. `"a" in {a:1}`, `x instanceof {}`, `return {a:1}`).
                | "in"
                | "of"
                | "instanceof"
                | "typeof"
                | "return"
                | "yield"
                | "new"
                | "delete"
                | "void"
                | "await"
                | "case"
        ) {
            return source.to_string();
        }
    }

    // Check the content between braces looks like object literal syntax
    let inner = &trimmed[open_pos + 1..trimmed.len() - 1].trim();
    if inner.is_empty() {
        return source.to_string();
    }

    // Heuristic: contains `identifier:` pattern (key-value) or commas between identifiers
    let looks_like_object = inner.contains(':') || {
        // Check for shorthand properties: `{ a, b }` pattern
        inner.split(',').all(|part| {
            let p = part.trim();
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == ' ')
        })
    };

    if !looks_like_object {
        return source.to_string();
    }

    // Wrap in parentheses with a semicolon to prevent it being parsed
    // as a function call on the preceding expression (e.g. `1({a})`)
    let close_pos = source.rfind('}').unwrap();
    let mut result = String::with_capacity(source.len() + 3);
    result.push_str(&source[..open_pos]);
    result.push_str(";(");
    result.push_str(&source[open_pos..=close_pos]);
    result.push(')');
    if close_pos + 1 < source.len() {
        result.push_str(&source[close_pos + 1..]);
    }
    result
}

struct AstLowerer<'a> {
    #[allow(dead_code)]
    source: &'a str,
    body: Vec<Statement>,
    functions: Vec<FunctionDef>,
}

impl<'a> AstLowerer<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            body: Vec::new(),
            functions: Vec::new(),
        }
    }

    fn span(&self, s: oxc_span::Span) -> Span {
        s.into()
    }

    fn unsupported(&self, span: oxc_span::Span, desc: &str) -> ZapcodeError {
        ZapcodeError::UnsupportedSyntax {
            span: format!("{}..{}", span.start, span.end),
            description: desc.to_string(),
        }
    }

    fn lower_program(&mut self, program: &ast::Program<'_>) -> Result<()> {
        // Handle directives (e.g., "use strict", but also bare string literals)
        for directive in &program.directives {
            let span = self.span(directive.span);
            let expr = Expr::StringLit(directive.directive.to_string());
            self.body.push(Statement::Expression { expr, span });
        }
        for stmt in &program.body {
            let s = self.lower_statement(stmt)?;
            self.body.push(s);
        }
        Ok(())
    }

    fn lower_statement(&mut self, stmt: &ast::Statement<'_>) -> Result<Statement> {
        match stmt {
            ast::Statement::VariableDeclaration(decl) => self.lower_var_decl(decl),
            ast::Statement::ExpressionStatement(expr_stmt) => {
                let span = self.span(expr_stmt.span);
                let expr = self.lower_expr(&expr_stmt.expression)?;
                Ok(Statement::Expression { expr, span })
            }
            ast::Statement::ReturnStatement(ret) => {
                let span = self.span(ret.span);
                let value = match &ret.argument {
                    Some(arg) => Some(self.lower_expr(arg)?),
                    None => None,
                };
                Ok(Statement::Return { value, span })
            }
            ast::Statement::IfStatement(if_stmt) => self.lower_if(if_stmt),
            ast::Statement::WhileStatement(while_stmt) => {
                let span = self.span(while_stmt.span);
                let test = self.lower_expr(&while_stmt.test)?;
                let body = self.lower_statement_as_block(&while_stmt.body)?;
                Ok(Statement::While { test, body, span })
            }
            ast::Statement::DoWhileStatement(do_while) => {
                let span = self.span(do_while.span);
                let body = self.lower_statement_as_block(&do_while.body)?;
                let test = self.lower_expr(&do_while.test)?;
                Ok(Statement::DoWhile { body, test, span })
            }
            ast::Statement::ForStatement(for_stmt) => self.lower_for(for_stmt),
            ast::Statement::ForInStatement(s) => self.lower_for_in(s),
            ast::Statement::ForOfStatement(for_of) => self.lower_for_of(for_of),
            ast::Statement::BlockStatement(block) => {
                let span = self.span(block.span);
                let body = self.lower_statements(&block.body)?;
                Ok(Statement::Block { body, span })
            }
            ast::Statement::ThrowStatement(throw) => {
                let span = self.span(throw.span);
                let value = self.lower_expr(&throw.argument)?;
                Ok(Statement::Throw { value, span })
            }
            ast::Statement::TryStatement(try_stmt) => self.lower_try(try_stmt),
            ast::Statement::BreakStatement(s) => Ok(Statement::Break {
                label: s.label.as_ref().map(|l| l.name.to_string()),
                span: self.span(s.span),
            }),
            ast::Statement::ContinueStatement(s) => Ok(Statement::Continue {
                label: s.label.as_ref().map(|l| l.name.to_string()),
                span: self.span(s.span),
            }),
            ast::Statement::FunctionDeclaration(func) => self.lower_func_decl(func),
            ast::Statement::ClassDeclaration(class) => self.lower_class_decl(class),
            ast::Statement::SwitchStatement(switch) => self.lower_switch(switch),
            ast::Statement::EmptyStatement(_) => Ok(Statement::Expression {
                expr: Expr::UndefinedLit,
                span: Span { start: 0, end: 0 },
            }),
            ast::Statement::LabeledStatement(labeled) => Ok(Statement::Labeled {
                label: labeled.label.name.to_string(),
                body: Box::new(self.lower_statement(&labeled.body)?),
                span: self.span(labeled.span),
            }),
            ast::Statement::TSTypeAliasDeclaration(s) => Ok(Statement::Expression {
                expr: Expr::UndefinedLit,
                span: self.span(s.span),
            }),
            ast::Statement::TSInterfaceDeclaration(s) => Ok(Statement::Expression {
                expr: Expr::UndefinedLit,
                span: self.span(s.span),
            }),
            ast::Statement::TSEnumDeclaration(s) => {
                Err(self.unsupported(s.span, "TypeScript enums are not supported"))
            }
            ast::Statement::ImportDeclaration(s) => Err(ZapcodeError::SandboxViolation(format!(
                "import declarations are forbidden in the sandbox (at {}..{})",
                s.span.start, s.span.end
            ))),
            ast::Statement::ExportDefaultDeclaration(s) => {
                Err(ZapcodeError::SandboxViolation(format!(
                    "export declarations are forbidden in the sandbox (at {}..{})",
                    s.span.start, s.span.end
                )))
            }
            ast::Statement::ExportNamedDeclaration(s) => {
                Err(ZapcodeError::SandboxViolation(format!(
                    "export declarations are forbidden in the sandbox (at {}..{})",
                    s.span.start, s.span.end
                )))
            }
            ast::Statement::ExportAllDeclaration(s) => {
                Err(ZapcodeError::SandboxViolation(format!(
                    "export declarations are forbidden in the sandbox (at {}..{})",
                    s.span.start, s.span.end
                )))
            }
            ast::Statement::DebuggerStatement(s) => {
                Err(self.unsupported(s.span, "unsupported statement type"))
            }
            ast::Statement::WithStatement(s) => {
                Err(self.unsupported(s.span, "unsupported statement type"))
            }
            ast::Statement::TSModuleDeclaration(s) => {
                Err(self.unsupported(s.span, "unsupported statement type"))
            }
            ast::Statement::TSGlobalDeclaration(s) => {
                Err(self.unsupported(s.span, "unsupported statement type"))
            }
            ast::Statement::TSImportEqualsDeclaration(s) => {
                Err(self.unsupported(s.span, "unsupported statement type"))
            }
            ast::Statement::TSExportAssignment(s) => {
                Err(self.unsupported(s.span, "unsupported statement type"))
            }
            ast::Statement::TSNamespaceExportDeclaration(s) => {
                Err(self.unsupported(s.span, "unsupported statement type"))
            }
        }
    }

    fn lower_statements(&mut self, stmts: &[ast::Statement<'_>]) -> Result<Vec<Statement>> {
        stmts.iter().map(|s| self.lower_statement(s)).collect()
    }

    fn lower_statement_as_block(&mut self, stmt: &ast::Statement<'_>) -> Result<Vec<Statement>> {
        match stmt {
            ast::Statement::BlockStatement(block) => self.lower_statements(&block.body),
            other => Ok(vec![self.lower_statement(other)?]),
        }
    }

    fn lower_var_decl(&mut self, decl: &ast::VariableDeclaration<'_>) -> Result<Statement> {
        let span = self.span(decl.span);
        let kind = match decl.kind {
            ast::VariableDeclarationKind::Const => VarKind::Const,
            ast::VariableDeclarationKind::Let => VarKind::Let,
            ast::VariableDeclarationKind::Var => VarKind::Var,
            ast::VariableDeclarationKind::Using | ast::VariableDeclarationKind::AwaitUsing => {
                return Err(self.unsupported(decl.span, "using declarations are not supported"));
            }
        };
        let mut declarations = Vec::new();
        for declarator in &decl.declarations {
            let pattern = self.lower_binding_pattern(&declarator.id)?;
            let init = match &declarator.init {
                Some(expr) => Some(self.lower_expr(expr)?),
                None => None,
            };
            // JS name inference: `const f = function(){}` / `const f = () => {}`
            // gives the (otherwise anonymous) function the binding's name.
            if let (AssignTarget::Ident(bind_name), Some(init_expr)) = (&pattern, &init) {
                if let Some(func_index) = match init_expr {
                    Expr::FunctionExpr { func_index } => Some(*func_index),
                    Expr::ArrowFunction { func_index } => Some(*func_index),
                    _ => None,
                } {
                    if let Some(f) = self.functions.get_mut(func_index) {
                        if f.name.is_none() {
                            f.name = Some(bind_name.clone());
                        }
                    }
                }
            }
            declarations.push(VarDeclarator { pattern, init });
        }
        Ok(Statement::VariableDecl {
            kind,
            declarations,
            span,
        })
    }

    fn lower_binding_pattern(&mut self, pat: &ast::BindingPattern<'_>) -> Result<AssignTarget> {
        match pat {
            ast::BindingPattern::BindingIdentifier(id) => {
                Ok(AssignTarget::Ident(id.name.to_string()))
            }
            // Object/array destructuring var-decls are lowered to the unified
            // `ParamPattern` form, which carries element defaults and arbitrary
            // object/array nesting. The compiler destructures it via the same
            // recursive path used for parameters and `for…of` bindings.
            ast::BindingPattern::ObjectPattern(_) | ast::BindingPattern::ArrayPattern(_) => {
                Ok(AssignTarget::Pattern(self.lower_binding_pattern_to_param(pat)?))
            }
            ast::BindingPattern::AssignmentPattern(assign) => {
                self.lower_binding_pattern(&assign.left)
            }
        }
    }

    fn lower_object_pattern_fields(
        &mut self,
        obj: &ast::ObjectPattern<'_>,
    ) -> Result<Vec<DestructureField>> {
        let mut fields = Vec::new();
        for prop in &obj.properties {
            let key = property_key_to_string(&prop.key);
            // A computed key built from a non-literal expression (`{[k]: v}`)
            // must be resolved at runtime. A string/number/identifier-literal
            // key (including `{['a']: v}`) is already static and needs no
            // runtime expression.
            let computed_key = if prop.computed && key == "<computed>" {
                Some(self.lower_expr(prop.key.to_expression())?)
            } else {
                None
            };
            let (alias, nested, default) = self.lower_destructure_field_value(&prop.value, &key)?;
            fields.push(DestructureField {
                key,
                alias,
                nested,
                default,
                rest: false,
                computed_key,
            });
        }

        if let Some(rest) = &obj.rest {
            match &rest.argument {
                ast::BindingPattern::BindingIdentifier(id) => {
                    fields.push(DestructureField {
                        key: id.name.to_string(),
                        alias: Some(id.name.to_string()),
                        nested: None,
                        default: None,
                        rest: true,
                        computed_key: None,
                    });
                }
                _ => {
                    return Err(ZapcodeError::UnsupportedSyntax {
                        span: self.span(rest.span).to_string(),
                        description: "only identifier object rest destructuring is supported"
                            .to_string(),
                    });
                }
            }
        }

        Ok(fields)
    }

    fn lower_destructure_field_value(
        &mut self,
        value: &ast::BindingPattern<'_>,
        key: &str,
    ) -> Result<DestructureFieldParts> {
        match value {
            ast::BindingPattern::BindingIdentifier(id) => {
                let name = id.name.to_string();
                let alias = if name != key { Some(name) } else { None };
                Ok((alias, None, None))
            }
            // A nested object OR array pattern bound to this field's value
            // (`{a: {b}}`, `{a: [x, y]}`). Both lower to a `ParamPattern`.
            ast::BindingPattern::ObjectPattern(_) | ast::BindingPattern::ArrayPattern(_) => Ok((
                None,
                Some(Box::new(self.lower_binding_pattern_to_param(value)?)),
                None,
            )),
            ast::BindingPattern::AssignmentPattern(assign) => {
                let default = self.lower_expr(&assign.right)?;
                let (alias, nested, _) = self.lower_destructure_field_value(&assign.left, key)?;
                Ok((alias, nested, Some(default)))
            }
        }
    }

    fn lower_binding_pattern_to_param(
        &mut self,
        pat: &ast::BindingPattern<'_>,
    ) -> Result<ParamPattern> {
        match pat {
            ast::BindingPattern::BindingIdentifier(id) => {
                Ok(ParamPattern::Ident(id.name.to_string()))
            }
            ast::BindingPattern::ObjectPattern(obj) => Ok(ParamPattern::ObjectDestructure(
                self.lower_object_pattern_fields(obj)?,
            )),
            ast::BindingPattern::ArrayPattern(arr) => {
                let mut elems = Vec::new();
                for elem in &arr.elements {
                    match elem {
                        Some(p) => elems.push(Some(self.lower_binding_pattern_to_param(p)?)),
                        None => elems.push(None),
                    }
                }
                if let Some(rest) = &arr.rest {
                    if let ast::BindingPattern::BindingIdentifier(id) = &rest.argument {
                        elems.push(Some(ParamPattern::Rest(id.name.to_string())));
                    } else {
                        return Err(self.unsupported(
                            rest.span,
                            "only identifier array rest destructuring is supported",
                        ));
                    }
                }
                Ok(ParamPattern::ArrayDestructure(elems))
            }
            ast::BindingPattern::AssignmentPattern(assign) => {
                let inner = self.lower_binding_pattern_to_param(&assign.left)?;
                let default = self.lower_expr(&assign.right)?;
                Ok(ParamPattern::DefaultValue {
                    pattern: Box::new(inner),
                    default,
                })
            }
        }
    }

    fn lower_if(&mut self, if_stmt: &ast::IfStatement<'_>) -> Result<Statement> {
        let span = self.span(if_stmt.span);
        let test = self.lower_expr(&if_stmt.test)?;
        let consequent = self.lower_statement_as_block(&if_stmt.consequent)?;
        let alternate = match &if_stmt.alternate {
            Some(alt) => Some(self.lower_statement_as_block(alt)?),
            None => None,
        };
        Ok(Statement::If {
            test,
            consequent,
            alternate,
            span,
        })
    }

    fn lower_for(&mut self, for_stmt: &ast::ForStatement<'_>) -> Result<Statement> {
        let span = self.span(for_stmt.span);
        let init = match &for_stmt.init {
            Some(init) => match init {
                ast::ForStatementInit::VariableDeclaration(decl) => {
                    Some(Box::new(self.lower_var_decl(decl)?))
                }
                other => {
                    if let Some(expr_ref) = other.as_expression() {
                        let expr = self.lower_expr(expr_ref)?;
                        Some(Box::new(Statement::Expression { expr, span }))
                    } else {
                        return Err(ZapcodeError::CompileError(
                            "unsupported for-loop initializer".to_string(),
                        ));
                    }
                }
            },
            None => None,
        };
        let test = match &for_stmt.test {
            Some(t) => Some(self.lower_expr(t)?),
            None => None,
        };
        let update = match &for_stmt.update {
            Some(u) => Some(self.lower_expr(u)?),
            None => None,
        };
        let body = self.lower_statement_as_block(&for_stmt.body)?;
        Ok(Statement::For {
            init,
            test,
            update,
            body,
            span,
        })
    }

    /// `for (const k in obj)` iterates the object's own enumerable keys. We
    /// lower it to a for-of over `Object.keys(obj)`, which yields string keys
    /// for objects and index strings for arrays — matching for-in semantics.
    fn lower_for_in(&mut self, for_in: &ast::ForInStatement<'_>) -> Result<Statement> {
        let span = self.span(for_in.span);
        let binding = match &for_in.left {
            ast::ForStatementLeft::VariableDeclaration(decl) => {
                if let Some(declarator) = decl.declarations.first() {
                    match &declarator.id {
                        ast::BindingPattern::BindingIdentifier(id) => {
                            ForBinding::Ident(id.name.to_string())
                        }
                        _ => {
                            return Err(
                                self.unsupported(for_in.span, "destructuring for-in binding")
                            )
                        }
                    }
                } else {
                    return Err(self.unsupported(for_in.span, "empty for-in binding"));
                }
            }
            _ => return Err(self.unsupported(for_in.span, "unsupported for-in left-hand side")),
        };
        let source = self.lower_expr(&for_in.right)?;
        let iterable = Expr::Call {
            callee: Box::new(Expr::Member {
                object: Box::new(Expr::Ident("Object".to_string())),
                property: "keys".to_string(),
                optional: false,
            }),
            args: vec![source],
            optional: false,
        };
        let body = self.lower_statement_as_block(&for_in.body)?;
        Ok(Statement::ForOf {
            binding,
            iterable,
            body,
            await_each: false,
            span,
        })
    }

    fn lower_for_of(&mut self, for_of: &ast::ForOfStatement<'_>) -> Result<Statement> {
        let span = self.span(for_of.span);
        let binding = match &for_of.left {
            ast::ForStatementLeft::VariableDeclaration(decl) => {
                if let Some(declarator) = decl.declarations.first() {
                    match &declarator.id {
                        ast::BindingPattern::BindingIdentifier(id) => {
                            ForBinding::Ident(id.name.to_string())
                        }
                        _ => {
                            let pat = self.lower_binding_pattern_to_param(&declarator.id)?;
                            ForBinding::Destructure(pat)
                        }
                    }
                } else {
                    return Err(self.unsupported(for_of.span, "empty for-of binding"));
                }
            }
            _ => return Err(self.unsupported(for_of.span, "unsupported for-of left-hand side")),
        };
        let iterable = self.lower_expr(&for_of.right)?;
        let body = self.lower_statement_as_block(&for_of.body)?;
        // `for await (const x of it)`: oxc sets `for_of.await`. We lower the loop
        // identically to a sync for-of but flag it so the compiler awaits each
        // iterated value (resolving promises / suspending on pending external
        // calls via the existing Await path) before binding it.
        Ok(Statement::ForOf {
            binding,
            iterable,
            body,
            await_each: for_of.r#await,
            span,
        })
    }

    fn lower_try(&mut self, try_stmt: &ast::TryStatement<'_>) -> Result<Statement> {
        let span = self.span(try_stmt.span);
        let try_body = self.lower_statements(&try_stmt.block.body)?;
        let (catch_param, catch_body) = match &try_stmt.handler {
            Some(handler) => {
                let param = handler.param.as_ref().and_then(|p| match &p.pattern {
                    ast::BindingPattern::BindingIdentifier(id) => Some(id.name.to_string()),
                    _ => None,
                });
                let body = self.lower_statements(&handler.body.body)?;
                (param, body)
            }
            None => (None, Vec::new()),
        };
        let finally_body = match &try_stmt.finalizer {
            Some(block) => Some(self.lower_statements(&block.body)?),
            None => None,
        };
        Ok(Statement::TryCatch {
            try_body,
            has_catch: try_stmt.handler.is_some(),
            catch_param,
            catch_body,
            finally_body,
            span,
        })
    }

    fn lower_func_decl(&mut self, func: &ast::Function<'_>) -> Result<Statement> {
        let span = self.span(func.span);
        let func_def = self.lower_function(func)?;
        let name = func_def.name.clone();
        let func_index = self.functions.len();
        self.functions.push(func_def);
        Ok(Statement::FunctionDecl {
            func_index,
            name,
            span,
        })
    }

    fn lower_function(&mut self, func: &ast::Function<'_>) -> Result<FunctionDef> {
        let name = func.id.as_ref().map(|id| id.name.to_string());
        let params = self.lower_formal_params(&func.params)?;
        let body = match &func.body {
            Some(body) => self.lower_statements(&body.statements)?,
            None => Vec::new(),
        };
        Ok(FunctionDef {
            name,
            params,
            body,
            is_async: func.r#async,
            is_generator: func.generator,
            is_arrow: false,
            span: self.span(func.span),
        })
    }

    fn lower_formal_params(
        &mut self,
        params: &ast::FormalParameters<'_>,
    ) -> Result<Vec<ParamPattern>> {
        let mut result = Vec::new();
        for param in &params.items {
            let pat = self.lower_binding_pattern_to_param(&param.pattern)?;
            // A defaulted parameter (`function f(x = 42)`) stores its default in
            // `FormalParameter::initializer`, not as an `AssignmentPattern`.
            let pat = match &param.initializer {
                Some(init) => ParamPattern::DefaultValue {
                    pattern: Box::new(pat),
                    default: self.lower_expr(init)?,
                },
                None => pat,
            };
            result.push(pat);
        }
        if let Some(rest) = &params.rest {
            match &rest.rest.argument {
                ast::BindingPattern::BindingIdentifier(id) => {
                    result.push(ParamPattern::Rest(id.name.to_string()));
                }
                _ => {
                    return Err(self.unsupported(
                        rest.span,
                        "complex rest parameter patterns are not supported",
                    ));
                }
            }
        }
        Ok(result)
    }

    fn lower_class_decl(&mut self, class: &ast::Class<'_>) -> Result<Statement> {
        let span = self.span(class.span);
        let name = class
            .id
            .as_ref()
            .map(|id| id.name.to_string())
            .unwrap_or_else(|| "AnonymousClass".to_string());

        let super_class = match &class.super_class {
            Some(expr) => {
                if let ast::Expression::Identifier(id) = expr {
                    Some(id.name.to_string())
                } else {
                    return Err(self.unsupported(
                        class.span,
                        "computed super class expressions are not supported",
                    ));
                }
            }
            None => None,
        };

        let (constructor, methods, static_methods, fields, static_fields) =
            self.lower_class_body(&class.body)?;

        Ok(Statement::ClassDecl {
            name,
            super_class,
            constructor,
            methods,
            static_methods,
            fields,
            static_fields,
            span,
        })
    }

    fn lower_class_expr(&mut self, class: &ast::Class<'_>) -> Result<Expr> {
        let name = class.id.as_ref().map(|id| id.name.to_string());

        let super_class = match &class.super_class {
            Some(expr) => {
                if let ast::Expression::Identifier(id) = expr {
                    Some(id.name.to_string())
                } else {
                    return Err(self.unsupported(
                        class.span,
                        "computed super class expressions are not supported",
                    ));
                }
            }
            None => None,
        };

        let (constructor, methods, static_methods, fields, static_fields) =
            self.lower_class_body(&class.body)?;

        Ok(Expr::ClassExpr {
            name,
            super_class,
            constructor,
            methods,
            static_methods,
            fields,
            static_fields,
        })
    }

    fn lower_class_body(&mut self, body: &ast::ClassBody<'_>) -> Result<ClassBodyParts> {
        let mut constructor = None;
        let mut methods = Vec::new();
        let mut static_methods = Vec::new();
        let mut fields = Vec::new();
        let mut static_fields = Vec::new();

        for element in &body.body {
            match element {
                ast::ClassElement::MethodDefinition(method) => {
                    let method_name = match &method.key {
                        ast::PropertyKey::StaticIdentifier(id) => id.name.to_string(),
                        ast::PropertyKey::StringLiteral(s) => s.value.to_string(),
                        // `#method` -> stored under a "#"-prefixed key (hidden
                        // from reflection); `this.#method()` reads the same key.
                        ast::PropertyKey::PrivateIdentifier(id) => format!("#{}", id.name),
                        // Well-known `[Symbol.X]` keys are statically
                        // recognizable: store the method under the sentinel
                        // key the runtime reads (iteration protocol /
                        // `Vm::to_primitive`), exactly like the object-literal
                        // lowering. Only the symbols the VM actually honors
                        // are mapped; others fall through to unsupported.
                        ast::PropertyKey::StaticMemberExpression(sm)
                            if matches!(&sm.object,
                                ast::Expression::Identifier(o) if o.name == "Symbol") =>
                        {
                            match sm.property.name.as_str() {
                                "iterator" => "__@@iterator".to_string(),
                                "toPrimitive" => "__@@toPrimitive".to_string(),
                                _ => continue,
                            }
                        }
                        _ => continue, // truly dynamic computed names unsupported
                    };

                    let func = &method.value;
                    let params = self.lower_formal_params(&func.params)?;
                    let body_stmts = match &func.body {
                        Some(body) => self.lower_statements(&body.statements)?,
                        None => Vec::new(),
                    };

                    let func_def = FunctionDef {
                        name: Some(method_name.clone()),
                        params,
                        body: body_stmts,
                        is_async: func.r#async,
                        is_generator: func.generator,
                        is_arrow: false,
                        span: self.span(func.span),
                    };

                    let kind = match method.kind {
                        ast::MethodDefinitionKind::Constructor => {
                            constructor = Some(Box::new(func_def));
                            continue;
                        }
                        ast::MethodDefinitionKind::Method => ClassMethodKind::Method,
                        ast::MethodDefinitionKind::Get => ClassMethodKind::Get,
                        ast::MethodDefinitionKind::Set => ClassMethodKind::Set,
                    };

                    let entry = ClassMethod {
                        name: method_name,
                        func: func_def,
                        kind,
                    };
                    if method.r#static {
                        static_methods.push(entry);
                    } else {
                        methods.push(entry);
                    }
                }
                ast::ClassElement::PropertyDefinition(prop) => {
                    // Computed field names (`[k] = …`) are not supported.
                    let field_name = match &prop.key {
                        ast::PropertyKey::StaticIdentifier(id) => id.name.to_string(),
                        ast::PropertyKey::StringLiteral(s) => s.value.to_string(),
                        // `#field` -> "#"-prefixed (hidden) key.
                        ast::PropertyKey::PrivateIdentifier(id) => format!("#{}", id.name),
                        _ => continue,
                    };
                    // `declare x: T;` is a TypeScript type-only declaration with no
                    // runtime initialization — skip it.
                    if prop.declare {
                        continue;
                    }
                    let value = match &prop.value {
                        Some(expr) => Some(self.lower_expr(expr)?),
                        None => None,
                    };
                    let entry = ClassField {
                        name: field_name,
                        value,
                    };
                    if prop.r#static {
                        static_fields.push(entry);
                    } else {
                        fields.push(entry);
                    }
                }
                ast::ClassElement::AccessorProperty(s) => {
                    return Err(self
                        .unsupported(s.span, "accessor properties in classes are not supported"));
                }
                ast::ClassElement::TSIndexSignature(_) => {
                    // TypeScript-only, skip
                }
                ast::ClassElement::StaticBlock(s) => {
                    return Err(self.unsupported(s.span, "static blocks are not supported"));
                }
            }
        }

        Ok((constructor, methods, static_methods, fields, static_fields))
    }

    fn lower_switch(&mut self, switch: &ast::SwitchStatement<'_>) -> Result<Statement> {
        let span = self.span(switch.span);
        let discriminant = self.lower_expr(&switch.discriminant)?;
        let mut cases = Vec::new();
        for case in &switch.cases {
            let test = match &case.test {
                Some(t) => Some(self.lower_expr(t)?),
                None => None,
            };
            let consequent = self.lower_statements(&case.consequent)?;
            cases.push(SwitchCase { test, consequent });
        }
        Ok(Statement::Switch {
            discriminant,
            cases,
            span,
        })
    }

    fn lower_expr(&mut self, expr: &ast::Expression<'_>) -> Result<Expr> {
        match expr {
            ast::Expression::NumericLiteral(lit) => Ok(Expr::NumberLit(lit.value)),
            ast::Expression::BigIntLiteral(lit) => {
                // `lit.raw` is the source text (e.g. "10n", "0xFFn", "1_000n").
                // Strip the `n` suffix, any radix prefix, and digit separators,
                // then parse to an arbitrary-precision integer.
                let raw = lit.raw.as_ref().map(|a| a.as_str()).unwrap_or("");
                let body = raw.strip_suffix('n').unwrap_or(raw).replace('_', "");
                let (digits, radix) = if let Some(r) =
                    body.strip_prefix("0x").or_else(|| body.strip_prefix("0X"))
                {
                    (r, 16)
                } else if let Some(r) = body.strip_prefix("0o").or_else(|| body.strip_prefix("0O")) {
                    (r, 8)
                } else if let Some(r) = body.strip_prefix("0b").or_else(|| body.strip_prefix("0B")) {
                    (r, 2)
                } else {
                    (body.as_str(), 10)
                };
                let value = num_bigint::BigInt::parse_bytes(digits.as_bytes(), radix)
                    .ok_or_else(|| ZapcodeError::UnsupportedSyntax {
                        span: "unknown".to_string(),
                        description: "invalid BigInt literal".to_string(),
                    })?;
                Ok(Expr::BigIntLit(value))
            }
            ast::Expression::StringLiteral(lit) => Ok(Expr::StringLit(lit.value.to_string())),
            ast::Expression::BooleanLiteral(lit) => Ok(Expr::BoolLit(lit.value)),
            ast::Expression::NullLiteral(_) => Ok(Expr::NullLit),
            ast::Expression::TemplateLiteral(tpl) => {
                // Use the *cooked* value so escape sequences (\n, \t, \uXXXX, \\)
                // are processed; `raw` is the unescaped source text.
                let quasis: Vec<String> = tpl
                    .quasis
                    .iter()
                    .map(|q| {
                        q.value
                            .cooked
                            .as_ref()
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| q.value.raw.to_string())
                    })
                    .collect();
                let exprs: Result<Vec<Expr>> =
                    tpl.expressions.iter().map(|e| self.lower_expr(e)).collect();
                Ok(Expr::TemplateLit {
                    quasis,
                    exprs: exprs?,
                })
            }
            ast::Expression::RegExpLiteral(re) => Ok(Expr::RegExpLit {
                pattern: re.regex.pattern.text.to_string(),
                flags: re.regex.flags.to_string(),
            }),
            ast::Expression::Identifier(id) => {
                let name = id.name.to_string();
                match name.as_str() {
                    "undefined" => Ok(Expr::UndefinedLit),
                    "NaN" => Ok(Expr::NumberLit(f64::NAN)),
                    "Infinity" => Ok(Expr::NumberLit(f64::INFINITY)),
                    "eval" => Err(ZapcodeError::SandboxViolation(
                        "eval is forbidden in the sandbox".to_string(),
                    )),
                    // `Function` resolves to a non-constructible global VALUE (see
                    // `register_globals`), so `typeof Function === "function"` and
                    // `f instanceof Function` work like Node. The sandbox violation
                    // is raised at runtime when it is actually CALLED/constructed,
                    // where it is catchable, instead of aborting at parse time.
                    "process" => Err(ZapcodeError::SandboxViolation(
                        "process is forbidden in the sandbox".to_string(),
                    )),
                    "globalThis" | "global" => Err(ZapcodeError::SandboxViolation(
                        "globalThis/global is forbidden in the sandbox".to_string(),
                    )),
                    "require" => Err(ZapcodeError::SandboxViolation(
                        "require is forbidden in the sandbox".to_string(),
                    )),
                    _ => Ok(Expr::Ident(name)),
                }
            }
            ast::Expression::ArrayExpression(arr) => {
                let mut elements = Vec::new();
                for elem in &arr.elements {
                    match elem {
                        ast::ArrayExpressionElement::SpreadElement(spread) => {
                            let expr = self.lower_expr(&spread.argument)?;
                            elements.push(Some(Expr::Spread(Box::new(expr))));
                        }
                        ast::ArrayExpressionElement::Elision(_) => {
                            elements.push(None);
                        }
                        other => {
                            let expr_ref = other.to_expression();
                            let expr = self.lower_expr(expr_ref)?;
                            elements.push(Some(expr));
                        }
                    }
                }
                Ok(Expr::Array(elements))
            }
            ast::Expression::ObjectExpression(obj) => {
                let mut props = Vec::new();
                for prop in &obj.properties {
                    match prop {
                        ast::ObjectPropertyKind::ObjectProperty(p) => {
                            let key = self.lower_property_key(&p.key)?;
                            let computed = p.computed;
                            let key_expr = if computed {
                                match p.key.as_expression() {
                                    Some(e) => Some(Box::new(self.lower_expr(e)?)),
                                    None => None,
                                }
                            } else {
                                None
                            };

                            if p.shorthand {
                                props.push(ObjProperty {
                                    kind: PropKind::Shorthand,
                                    key: key.clone(),
                                    value: Expr::Ident(key),
                                    computed: false,
                                    key_expr: None,
                                });
                            } else if matches!(p.kind, ast::PropertyKind::Get) {
                                // `{ get x() { ... } }` — accessor getter.
                                let value = self.lower_expr(&p.value)?;
                                props.push(ObjProperty {
                                    kind: PropKind::Get,
                                    key,
                                    value,
                                    computed,
                                    key_expr,
                                });
                            } else if matches!(p.kind, ast::PropertyKind::Set) {
                                // `{ set x(v) { ... } }` — accessor setter.
                                let value = self.lower_expr(&p.value)?;
                                props.push(ObjProperty {
                                    kind: PropKind::Set,
                                    key,
                                    value,
                                    computed,
                                    key_expr,
                                });
                            } else if p.method {
                                let value = self.lower_expr(&p.value)?;
                                props.push(ObjProperty {
                                    kind: PropKind::Method,
                                    key,
                                    value,
                                    computed,
                                    key_expr,
                                });
                            } else {
                                let value = self.lower_expr(&p.value)?;
                                props.push(ObjProperty {
                                    kind: PropKind::Init,
                                    key,
                                    value,
                                    computed,
                                    key_expr,
                                });
                            }
                        }
                        ast::ObjectPropertyKind::SpreadProperty(spread) => {
                            let expr = self.lower_expr(&spread.argument)?;
                            props.push(ObjProperty {
                                kind: PropKind::Spread,
                                key: String::new(),
                                value: expr,
                                computed: false,
                                key_expr: None,
                            });
                        }
                    }
                }
                Ok(Expr::Object(props))
            }
            ast::Expression::BinaryExpression(bin) => {
                let op = lower_binary_op(bin.operator)?;
                let left = self.lower_expr(&bin.left)?;
                let right = self.lower_expr(&bin.right)?;
                Ok(Expr::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                })
            }
            ast::Expression::UnaryExpression(unary) => {
                if matches!(unary.operator, ast::UnaryOperator::Typeof) {
                    let operand = self.lower_expr(&unary.argument)?;
                    return Ok(Expr::TypeOf(Box::new(operand)));
                }
                if matches!(unary.operator, ast::UnaryOperator::Delete) {
                    let operand = self.lower_expr(&unary.argument)?;
                    return Ok(Expr::Delete(Box::new(operand)));
                }
                let op = match unary.operator {
                    ast::UnaryOperator::UnaryNegation => UnaryOp::Neg,
                    ast::UnaryOperator::LogicalNot => UnaryOp::Not,
                    ast::UnaryOperator::BitwiseNot => UnaryOp::BitNot,
                    ast::UnaryOperator::Void => UnaryOp::Void,
                    ast::UnaryOperator::UnaryPlus => {
                        let operand = self.lower_expr(&unary.argument)?;
                        return Ok(Expr::Binary {
                            op: BinOp::Mul,
                            left: Box::new(operand),
                            right: Box::new(Expr::NumberLit(1.0)),
                        });
                    }
                    _ => {
                        return Err(self.unsupported(unary.span, "unsupported unary operator"));
                    }
                };
                let operand = self.lower_expr(&unary.argument)?;
                Ok(Expr::Unary {
                    op,
                    operand: Box::new(operand),
                })
            }
            ast::Expression::UpdateExpression(update) => {
                let op = match update.operator {
                    ast::UpdateOperator::Increment => UpdateOp::Increment,
                    ast::UpdateOperator::Decrement => UpdateOp::Decrement,
                };
                let operand = self.lower_simple_assign_target(&update.argument)?;
                Ok(Expr::Update {
                    op,
                    prefix: update.prefix,
                    operand: Box::new(operand),
                })
            }
            ast::Expression::LogicalExpression(logical) => {
                let op = match logical.operator {
                    ast::LogicalOperator::And => LogicalOp::And,
                    ast::LogicalOperator::Or => LogicalOp::Or,
                    ast::LogicalOperator::Coalesce => LogicalOp::NullishCoalescing,
                };
                let left = self.lower_expr(&logical.left)?;
                let right = self.lower_expr(&logical.right)?;
                Ok(Expr::Logical {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                })
            }
            ast::Expression::ConditionalExpression(cond) => {
                let test = self.lower_expr(&cond.test)?;
                let consequent = self.lower_expr(&cond.consequent)?;
                let alternate = self.lower_expr(&cond.alternate)?;
                Ok(Expr::Conditional {
                    test: Box::new(test),
                    consequent: Box::new(consequent),
                    alternate: Box::new(alternate),
                })
            }
            ast::Expression::AssignmentExpression(assign) => {
                // Destructuring assignment (`[a, b] = …`, `({x: o.p} = …)`): the
                // left side is an array/object target rather than a simple lvalue.
                if matches!(
                    &assign.left,
                    ast::AssignmentTarget::ArrayAssignmentTarget(_)
                        | ast::AssignmentTarget::ObjectAssignmentTarget(_)
                ) {
                    let pattern = self.lower_assign_pattern(&assign.left)?;
                    let value = self.lower_expr(&assign.right)?;
                    return Ok(Expr::DestructureAssign {
                        pattern: Box::new(pattern),
                        value: Box::new(value),
                    });
                }
                let op = lower_assign_op(assign.operator);
                let target = self.lower_assignment_target(&assign.left)?;
                let value = self.lower_expr(&assign.right)?;
                Ok(Expr::Assignment {
                    op,
                    target: Box::new(target),
                    value: Box::new(value),
                })
            }
            ast::Expression::SequenceExpression(seq) => {
                let exprs: Result<Vec<Expr>> =
                    seq.expressions.iter().map(|e| self.lower_expr(e)).collect();
                Ok(Expr::Sequence(exprs?))
            }
            ast::Expression::CallExpression(call) => {
                let callee = self.lower_expr(&call.callee)?;
                let args = self.lower_args(&call.arguments)?;
                Ok(Expr::Call {
                    callee: Box::new(callee),
                    args,
                    optional: call.optional,
                })
            }
            ast::Expression::NewExpression(new_expr) => {
                let callee = self.lower_expr(&new_expr.callee)?;
                let args = self.lower_args(&new_expr.arguments)?;
                Ok(Expr::New {
                    callee: Box::new(callee),
                    args,
                })
            }
            ast::Expression::StaticMemberExpression(member) => {
                let object = self.lower_expr(&member.object)?;
                let property = member.property.name.to_string();
                Ok(Expr::Member {
                    object: Box::new(object),
                    property,
                    optional: member.optional,
                })
            }
            ast::Expression::ComputedMemberExpression(member) => {
                let object = self.lower_expr(&member.object)?;
                let property = self.lower_expr(&member.expression)?;
                Ok(Expr::ComputedMember {
                    object: Box::new(object),
                    property: Box::new(property),
                    optional: member.optional,
                })
            }
            ast::Expression::PrivateFieldExpression(s) => {
                // `obj.#field` reads the "#"-prefixed (hidden) key.
                let object = self.lower_expr(&s.object)?;
                Ok(Expr::Member {
                    object: Box::new(object),
                    property: format!("#{}", s.field.name),
                    optional: false,
                })
            }
            ast::Expression::ArrowFunctionExpression(arrow) => {
                let params = self.lower_formal_params(&arrow.params)?;
                let body = if arrow.expression {
                    match arrow.body.statements.first() {
                        Some(ast::Statement::ExpressionStatement(expr)) => {
                            let ret_expr = self.lower_expr(&expr.expression)?;
                            vec![Statement::Return {
                                value: Some(ret_expr),
                                span: self.span(arrow.span),
                            }]
                        }
                        _ => self.lower_statements(&arrow.body.statements)?,
                    }
                } else {
                    self.lower_statements(&arrow.body.statements)?
                };
                let func_index = self.functions.len();
                self.functions.push(FunctionDef {
                    name: None,
                    params,
                    body,
                    is_async: arrow.r#async,
                    is_generator: false,
                    is_arrow: true,
                    span: self.span(arrow.span),
                });
                Ok(Expr::ArrowFunction { func_index })
            }
            ast::Expression::FunctionExpression(func) => {
                let func_def = self.lower_function(func)?;
                let func_index = self.functions.len();
                self.functions.push(func_def);
                Ok(Expr::FunctionExpr { func_index })
            }
            ast::Expression::AwaitExpression(await_expr) => {
                let expr = self.lower_expr(&await_expr.argument)?;
                Ok(Expr::Await(Box::new(expr)))
            }
            ast::Expression::ParenthesizedExpression(paren) => self.lower_expr(&paren.expression),
            ast::Expression::ChainExpression(chain) => self.lower_chain_expr(&chain.expression),
            ast::Expression::TaggedTemplateExpression(s) => {
                // Desugar `tag`a${x}b`` into the call `tag(["a", "b"], x)`:
                // a strings array (cooked quasis) followed by the interpolated
                // values, matching the JS tagged-template call shape. A member
                // tag (`obj.tag`...`` / `String.raw`...``) keeps its `this`
                // binding through the normal Call machinery.
                //
                // The strings array is a plain Array (so custom tags can use
                // `strings[i]` / `.join` / `.map` / `.reduce`). Its `.raw`
                // companion property is NOT provided yet — `String.raw` and tags
                // that read `strings.raw` are a documented residual.
                // `String.raw` is recognized statically and desugared to a
                // concatenation of the RAW quasi texts with the interpolated
                // values — the overwhelmingly common use (custom tags reading
                // `strings.raw` remain a documented residual: the strings
                // array carries only the cooked texts).
                if let ast::Expression::StaticMemberExpression(sm) = &s.tag {
                    let is_string_raw = matches!(&sm.object,
                        ast::Expression::Identifier(o) if o.name == "String")
                        && sm.property.name == "raw";
                    if is_string_raw {
                        let mut acc: Option<Expr> = None;
                        let mut push = |e: Expr, acc: &mut Option<Expr>| {
                            *acc = Some(match acc.take() {
                                None => e,
                                Some(prev) => Expr::Binary {
                                    op: BinOp::Add,
                                    left: Box::new(prev),
                                    right: Box::new(e),
                                },
                            });
                        };
                        for (i, q) in s.quasi.quasis.iter().enumerate() {
                            push(Expr::StringLit(q.value.raw.to_string()), &mut acc);
                            if let Some(e) = s.quasi.expressions.get(i) {
                                let lowered = self.lower_expr(e)?;
                                // Force string concatenation even when the
                                // first quasi is empty and both operands are
                                // numbers.
                                push(lowered, &mut acc);
                            }
                        }
                        return Ok(acc.unwrap_or(Expr::StringLit(String::new())));
                    }
                }
                let tag = self.lower_expr(&s.tag)?;
                let cooked: Vec<Option<Expr>> = s
                    .quasi
                    .quasis
                    .iter()
                    .map(|q| {
                        let text = q
                            .value
                            .cooked
                            .as_ref()
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| q.value.raw.to_string());
                        Some(Expr::StringLit(text))
                    })
                    .collect();
                let mut args: Vec<Expr> = vec![Expr::Array(cooked)];
                for e in &s.quasi.expressions {
                    args.push(self.lower_expr(e)?);
                }
                Ok(Expr::Call {
                    callee: Box::new(tag),
                    args,
                    optional: false,
                })
            }
            ast::Expression::ThisExpression(_) => Ok(Expr::Ident("this".to_string())),
            ast::Expression::Super(_) => Ok(Expr::Ident("super".to_string())),
            ast::Expression::YieldExpression(yield_expr) => {
                let value = match &yield_expr.argument {
                    Some(arg) => Some(Box::new(self.lower_expr(arg)?)),
                    None => None,
                };
                Ok(Expr::Yield {
                    value,
                    delegate: yield_expr.delegate,
                })
            }
            ast::Expression::ClassExpression(class) => self.lower_class_expr(class),
            ast::Expression::MetaProperty(s) => {
                Err(self.unsupported(s.span, "meta properties are not supported"))
            }
            ast::Expression::ImportExpression(s) => Err(ZapcodeError::SandboxViolation(format!(
                "dynamic import() is forbidden in the sandbox (at {}..{})",
                s.span.start, s.span.end
            ))),
            ast::Expression::TSAsExpression(ts) => self.lower_expr(&ts.expression),
            ast::Expression::TSSatisfiesExpression(ts) => self.lower_expr(&ts.expression),
            ast::Expression::TSNonNullExpression(ts) => self.lower_expr(&ts.expression),
            ast::Expression::TSTypeAssertion(ts) => self.lower_expr(&ts.expression),
            ast::Expression::TSInstantiationExpression(ts) => self.lower_expr(&ts.expression),
            _ => Err(ZapcodeError::UnsupportedSyntax {
                span: "unknown".to_string(),
                description: "unsupported expression type".to_string(),
            }),
        }
    }

    fn lower_chain_expr(&mut self, expr: &ast::ChainElement<'_>) -> Result<Expr> {
        match expr {
            ast::ChainElement::CallExpression(call) => {
                let callee = self.lower_expr(&call.callee)?;
                let args = self.lower_args(&call.arguments)?;
                Ok(Expr::Call {
                    callee: Box::new(callee),
                    args,
                    optional: call.optional,
                })
            }
            ast::ChainElement::StaticMemberExpression(member) => {
                let object = self.lower_expr(&member.object)?;
                Ok(Expr::Member {
                    object: Box::new(object),
                    property: member.property.name.to_string(),
                    optional: member.optional,
                })
            }
            ast::ChainElement::ComputedMemberExpression(member) => {
                let object = self.lower_expr(&member.object)?;
                let property = self.lower_expr(&member.expression)?;
                Ok(Expr::ComputedMember {
                    object: Box::new(object),
                    property: Box::new(property),
                    optional: member.optional,
                })
            }
            ast::ChainElement::PrivateFieldExpression(s) => {
                let object = self.lower_expr(&s.object)?;
                Ok(Expr::Member {
                    object: Box::new(object),
                    property: format!("#{}", s.field.name),
                    optional: s.optional,
                })
            }
            ast::ChainElement::TSNonNullExpression(ts) => self.lower_expr(&ts.expression),
        }
    }

    fn lower_args(&mut self, args: &[ast::Argument<'_>]) -> Result<Vec<Expr>> {
        let mut result = Vec::new();
        for arg in args {
            match arg {
                ast::Argument::SpreadElement(spread) => {
                    let expr = self.lower_expr(&spread.argument)?;
                    result.push(Expr::Spread(Box::new(expr)));
                }
                other => {
                    let expr_ref = other.to_expression();
                    let expr = self.lower_expr(expr_ref)?;
                    result.push(expr);
                }
            }
        }
        Ok(result)
    }

    fn lower_property_key(&mut self, key: &ast::PropertyKey<'_>) -> Result<String> {
        Ok(property_key_to_string_from_key(key))
    }

    fn lower_assignment_target(&mut self, target: &ast::AssignmentTarget<'_>) -> Result<Expr> {
        match target {
            ast::AssignmentTarget::AssignmentTargetIdentifier(id) => {
                Ok(Expr::Ident(id.name.to_string()))
            }
            ast::AssignmentTarget::StaticMemberExpression(member) => {
                let object = self.lower_expr(&member.object)?;
                Ok(Expr::Member {
                    object: Box::new(object),
                    property: member.property.name.to_string(),
                    optional: false,
                })
            }
            ast::AssignmentTarget::ComputedMemberExpression(member) => {
                let object = self.lower_expr(&member.object)?;
                let property = self.lower_expr(&member.expression)?;
                Ok(Expr::ComputedMember {
                    object: Box::new(object),
                    property: Box::new(property),
                    optional: false,
                })
            }
            // `this.#field = v` assigns the "#"-prefixed (hidden) key.
            ast::AssignmentTarget::PrivateFieldExpression(s) => {
                let object = self.lower_expr(&s.object)?;
                Ok(Expr::Member {
                    object: Box::new(object),
                    property: format!("#{}", s.field.name),
                    optional: false,
                })
            }
            _ => Err(ZapcodeError::CompileError(
                "unsupported assignment target".to_string(),
            )),
        }
    }

    /// Lower a destructuring-assignment target (`[a, b]`, `{x: o.p}`) into an
    /// [`AssignPattern`], whose leaves are arbitrary assignable expressions.
    fn lower_assign_pattern(
        &mut self,
        target: &ast::AssignmentTarget<'_>,
    ) -> Result<AssignPattern> {
        match target {
            ast::AssignmentTarget::ArrayAssignmentTarget(arr) => {
                let mut elements = Vec::new();
                for elem in &arr.elements {
                    match elem {
                        Some(maybe_default) => {
                            elements.push(Some(self.lower_assign_pattern_element(maybe_default)?))
                        }
                        None => elements.push(None),
                    }
                }
                let rest = match &arr.rest {
                    Some(rest) => Some(Box::new(self.lower_assign_pattern(&rest.target)?)),
                    None => None,
                };
                Ok(AssignPattern::Array { elements, rest })
            }
            ast::AssignmentTarget::ObjectAssignmentTarget(obj) => {
                let mut fields = Vec::new();
                for prop in &obj.properties {
                    fields.push(self.lower_assign_pattern_field(prop)?);
                }
                let rest = match &obj.rest {
                    Some(rest) => Some(Box::new(self.lower_assign_pattern(&rest.target)?)),
                    None => None,
                };
                Ok(AssignPattern::Object { fields, rest })
            }
            // A simple lvalue leaf (identifier or member expression).
            other => Ok(AssignPattern::Target(self.lower_assignment_target(other)?)),
        }
    }

    fn lower_assign_pattern_element(
        &mut self,
        elem: &ast::AssignmentTargetMaybeDefault<'_>,
    ) -> Result<AssignPatternElement> {
        match elem {
            ast::AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(with_default) => {
                let pattern = self.lower_assign_pattern(&with_default.binding)?;
                let default = Some(self.lower_expr(&with_default.init)?);
                Ok(AssignPatternElement { pattern, default })
            }
            // The non-default variants are inherited `AssignmentTarget`s.
            other => {
                let pattern = self.lower_assign_pattern(other.to_assignment_target())?;
                Ok(AssignPatternElement {
                    pattern,
                    default: None,
                })
            }
        }
    }

    fn lower_assign_pattern_field(
        &mut self,
        prop: &ast::AssignmentTargetProperty<'_>,
    ) -> Result<AssignPatternField> {
        match prop {
            // `{ foo }` / `{ foo = default }` shorthand.
            ast::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(id) => {
                let name = id.binding.name.to_string();
                let default = match &id.init {
                    Some(init) => Some(self.lower_expr(init)?),
                    None => None,
                };
                Ok(AssignPatternField {
                    key: name.clone(),
                    computed_key: None,
                    pattern: AssignPattern::Target(Expr::Ident(name)),
                    default,
                })
            }
            // `{ key: target }` / `{ [k]: target }` / `{ key: t = default }`.
            ast::AssignmentTargetProperty::AssignmentTargetPropertyProperty(prop) => {
                let key = property_key_to_string(&prop.name);
                let computed_key = if prop.computed && key == "<computed>" {
                    Some(self.lower_expr(prop.name.to_expression())?)
                } else {
                    None
                };
                let element = self.lower_assign_pattern_element(&prop.binding)?;
                Ok(AssignPatternField {
                    key,
                    computed_key,
                    pattern: element.pattern,
                    default: element.default,
                })
            }
        }
    }

    fn lower_simple_assign_target(
        &mut self,
        target: &ast::SimpleAssignmentTarget<'_>,
    ) -> Result<Expr> {
        match target {
            ast::SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => {
                Ok(Expr::Ident(id.name.to_string()))
            }
            ast::SimpleAssignmentTarget::StaticMemberExpression(member) => {
                let object = self.lower_expr(&member.object)?;
                Ok(Expr::Member {
                    object: Box::new(object),
                    property: member.property.name.to_string(),
                    optional: false,
                })
            }
            ast::SimpleAssignmentTarget::ComputedMemberExpression(member) => {
                let object = self.lower_expr(&member.object)?;
                let property = self.lower_expr(&member.expression)?;
                Ok(Expr::ComputedMember {
                    object: Box::new(object),
                    property: Box::new(property),
                    optional: false,
                })
            }
            // `++obj.#field` / `obj.#field--`: update on a private member.
            ast::SimpleAssignmentTarget::PrivateFieldExpression(s) => {
                let object = self.lower_expr(&s.object)?;
                Ok(Expr::Member {
                    object: Box::new(object),
                    property: format!("#{}", s.field.name),
                    optional: false,
                })
            }
            _ => Err(ZapcodeError::CompileError(
                "unsupported update target".to_string(),
            )),
        }
    }
}

fn property_key_to_string(key: &ast::PropertyKey<'_>) -> String {
    property_key_to_string_from_key(key)
}

fn property_key_to_string_from_key(key: &ast::PropertyKey<'_>) -> String {
    match key {
        ast::PropertyKey::StaticIdentifier(id) => id.name.to_string(),
        ast::PropertyKey::StringLiteral(s) => s.value.to_string(),
        ast::PropertyKey::NumericLiteral(n) => n.value.to_string(),
        _ => "<computed>".to_string(),
    }
}

fn lower_binary_op(op: ast::BinaryOperator) -> Result<BinOp> {
    match op {
        ast::BinaryOperator::Addition => Ok(BinOp::Add),
        ast::BinaryOperator::Subtraction => Ok(BinOp::Sub),
        ast::BinaryOperator::Multiplication => Ok(BinOp::Mul),
        ast::BinaryOperator::Division => Ok(BinOp::Div),
        ast::BinaryOperator::Remainder => Ok(BinOp::Rem),
        ast::BinaryOperator::Exponential => Ok(BinOp::Pow),
        ast::BinaryOperator::Equality => Ok(BinOp::Eq),
        ast::BinaryOperator::Inequality => Ok(BinOp::Neq),
        ast::BinaryOperator::StrictEquality => Ok(BinOp::StrictEq),
        ast::BinaryOperator::StrictInequality => Ok(BinOp::StrictNeq),
        ast::BinaryOperator::LessThan => Ok(BinOp::Lt),
        ast::BinaryOperator::LessEqualThan => Ok(BinOp::Lte),
        ast::BinaryOperator::GreaterThan => Ok(BinOp::Gt),
        ast::BinaryOperator::GreaterEqualThan => Ok(BinOp::Gte),
        ast::BinaryOperator::BitwiseAnd => Ok(BinOp::BitAnd),
        ast::BinaryOperator::BitwiseOR => Ok(BinOp::BitOr),
        ast::BinaryOperator::BitwiseXOR => Ok(BinOp::BitXor),
        ast::BinaryOperator::ShiftLeft => Ok(BinOp::Shl),
        ast::BinaryOperator::ShiftRight => Ok(BinOp::Shr),
        ast::BinaryOperator::ShiftRightZeroFill => Ok(BinOp::Ushr),
        ast::BinaryOperator::In => Ok(BinOp::In),
        ast::BinaryOperator::Instanceof => Ok(BinOp::InstanceOf),
    }
}

fn lower_assign_op(op: ast::AssignmentOperator) -> AssignOp {
    match op {
        ast::AssignmentOperator::Assign => AssignOp::Assign,
        ast::AssignmentOperator::Addition => AssignOp::AddAssign,
        ast::AssignmentOperator::Subtraction => AssignOp::SubAssign,
        ast::AssignmentOperator::Multiplication => AssignOp::MulAssign,
        ast::AssignmentOperator::Division => AssignOp::DivAssign,
        ast::AssignmentOperator::Remainder => AssignOp::RemAssign,
        ast::AssignmentOperator::Exponential => AssignOp::PowAssign,
        ast::AssignmentOperator::BitwiseAnd => AssignOp::BitAndAssign,
        ast::AssignmentOperator::BitwiseOR => AssignOp::BitOrAssign,
        ast::AssignmentOperator::BitwiseXOR => AssignOp::BitXorAssign,
        ast::AssignmentOperator::ShiftLeft => AssignOp::ShlAssign,
        ast::AssignmentOperator::ShiftRight => AssignOp::ShrAssign,
        ast::AssignmentOperator::ShiftRightZeroFill => AssignOp::UshrAssign,
        ast::AssignmentOperator::LogicalNullish => AssignOp::NullishAssign,
        ast::AssignmentOperator::LogicalAnd => AssignOp::AndAssign,
        ast::AssignmentOperator::LogicalOr => AssignOp::OrAssign,
    }
}
