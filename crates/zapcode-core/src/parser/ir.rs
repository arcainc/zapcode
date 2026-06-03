use serde::{Deserialize, Serialize};

/// Result type for parsing a class body: (constructor, instance methods, static methods).
pub type ClassBodyParts = (Option<Box<FunctionDef>>, Vec<ClassMethod>, Vec<ClassMethod>);

/// Span information for error reporting.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl std::fmt::Display for Span {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

impl From<oxc_span::Span> for Span {
    fn from(s: oxc_span::Span) -> Self {
        Span {
            start: s.start,
            end: s.end,
        }
    }
}

/// A complete program — a list of statements.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Program {
    pub body: Vec<Statement>,
    pub functions: Vec<FunctionDef>,
}

/// Function definition stored separately (hoisted).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDef {
    pub name: Option<String>,
    pub params: Vec<ParamPattern>,
    pub body: Vec<Statement>,
    pub is_async: bool,
    pub is_generator: bool,
    pub is_arrow: bool,
    pub span: Span,
}

/// Parameter pattern (simple name or destructuring).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParamPattern {
    Ident(String),
    ObjectDestructure(Vec<DestructureField>),
    ArrayDestructure(Vec<Option<ParamPattern>>),
    Rest(String),
    DefaultValue {
        pattern: Box<ParamPattern>,
        default: Expr,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DestructureField {
    pub key: String,
    pub alias: Option<String>,
    /// A nested pattern bound to this field's value. May be an object pattern
    /// (`{a: {b}}`) or an array pattern (`{a: [x, y]}`); both nest arbitrarily.
    pub nested: Option<Box<ParamPattern>>,
    pub default: Option<Expr>,
    pub rest: bool,
    /// For a computed key (`{[k]: v}`), the runtime key expression to evaluate.
    /// When set, `key` is a placeholder and the actual property name comes from
    /// evaluating this expression and coercing to a string.
    pub computed_key: Option<Expr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Statement {
    VariableDecl {
        kind: VarKind,
        declarations: Vec<VarDeclarator>,
        span: Span,
    },
    Expression {
        expr: Expr,
        span: Span,
    },
    Return {
        value: Option<Expr>,
        span: Span,
    },
    If {
        test: Expr,
        consequent: Vec<Statement>,
        alternate: Option<Vec<Statement>>,
        span: Span,
    },
    While {
        test: Expr,
        body: Vec<Statement>,
        span: Span,
    },
    ForOf {
        binding: ForBinding,
        iterable: Expr,
        body: Vec<Statement>,
        /// `for await (const x of it)` — await each iterated value before binding.
        await_each: bool,
        span: Span,
    },
    For {
        init: Option<Box<Statement>>,
        test: Option<Expr>,
        update: Option<Expr>,
        body: Vec<Statement>,
        span: Span,
    },
    Block {
        body: Vec<Statement>,
        span: Span,
    },
    Throw {
        value: Expr,
        span: Span,
    },
    TryCatch {
        try_body: Vec<Statement>,
        catch_param: Option<String>,
        catch_body: Vec<Statement>,
        finally_body: Option<Vec<Statement>>,
        span: Span,
    },
    Break {
        label: Option<String>,
        span: Span,
    },
    Continue {
        label: Option<String>,
        span: Span,
    },
    /// A labeled statement, e.g. `outer: for (...) { ... }`.
    Labeled {
        label: String,
        body: Box<Statement>,
        span: Span,
    },
    FunctionDecl {
        func_index: usize,
        /// The declared name, carried here so the compiler can bind it without a
        /// lookup into a (per-function-scope) compiled-function table.
        name: Option<String>,
        span: Span,
    },
    Switch {
        discriminant: Expr,
        cases: Vec<SwitchCase>,
        span: Span,
    },
    DoWhile {
        body: Vec<Statement>,
        test: Expr,
        span: Span,
    },
    ClassDecl {
        name: String,
        super_class: Option<String>,
        constructor: Option<Box<FunctionDef>>,
        methods: Vec<ClassMethod>,
        static_methods: Vec<ClassMethod>,
        span: Span,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassMethod {
    pub name: String,
    pub func: FunctionDef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwitchCase {
    pub test: Option<Expr>,
    pub consequent: Vec<Statement>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ForBinding {
    Ident(String),
    Destructure(ParamPattern),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum VarKind {
    Const,
    Let,
    Var,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VarDeclarator {
    pub pattern: AssignTarget,
    pub init: Option<Expr>,
}

/// A destructuring-ASSIGNMENT pattern (left-hand side of `pattern = value` with
/// no declaration keyword). Unlike [`AssignTarget`], its leaves are arbitrary
/// assignable expressions (`a`, `o.p`, `arr[i]`), each stored via the normal
/// assignment path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AssignPattern {
    /// A leaf assignment target: an identifier or member expression.
    Target(Expr),
    /// `[a, b, ...rest]` — elements may be `None` (elision/hole).
    Array {
        elements: Vec<Option<AssignPatternElement>>,
        rest: Option<Box<AssignPattern>>,
    },
    /// `{a, b: x, [k]: y, ...rest}`.
    Object {
        fields: Vec<AssignPatternField>,
        rest: Option<Box<AssignPattern>>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignPatternElement {
    pub pattern: AssignPattern,
    pub default: Option<Expr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignPatternField {
    pub key: String,
    pub computed_key: Option<Expr>,
    pub pattern: AssignPattern,
    pub default: Option<Expr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AssignTarget {
    Ident(String),
    ObjectDestructure(Vec<DestructureField>),
    ArrayDestructure(Vec<Option<AssignTarget>>),
    /// Array-rest binding `...name`; only valid as the last element of an
    /// `ArrayDestructure`.
    Rest(String),
    /// A destructuring binding lowered to the unified `ParamPattern` form, which
    /// supports element defaults and arbitrary object/array nesting. Used for
    /// var-decl destructuring so `const [[a],[b]] = …`, `const [a = 1] = []`, and
    /// `const {arr: [x]} = …` all bind correctly.
    Pattern(ParamPattern),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Expr {
    // Literals
    NumberLit(f64),
    StringLit(String),
    BoolLit(bool),
    NullLit,
    UndefinedLit,
    TemplateLit {
        quasis: Vec<String>,
        exprs: Vec<Expr>,
    },
    RegExpLit {
        pattern: String,
        flags: String,
    },

    // Identifiers
    Ident(String),

    // Compound
    Array(Vec<Option<Expr>>),
    Object(Vec<ObjProperty>),
    Spread(Box<Expr>),

    // Operations
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Update {
        op: UpdateOp,
        prefix: bool,
        operand: Box<Expr>,
    },
    Logical {
        op: LogicalOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Conditional {
        test: Box<Expr>,
        consequent: Box<Expr>,
        alternate: Box<Expr>,
    },
    Assignment {
        op: AssignOp,
        target: Box<Expr>,
        value: Box<Expr>,
    },
    /// Destructuring ASSIGNMENT (no declaration keyword): `[a, b] = …`,
    /// `({x: o.p} = …)`, `[arr[0], y] = …`. The leaves are arbitrary assignable
    /// expressions (identifiers or member accesses), unlike a binding pattern.
    /// Evaluates to the right-hand value.
    DestructureAssign {
        pattern: Box<AssignPattern>,
        value: Box<Expr>,
    },
    Sequence(Vec<Expr>),

    // Access
    Member {
        object: Box<Expr>,
        property: String,
        optional: bool,
    },
    ComputedMember {
        object: Box<Expr>,
        property: Box<Expr>,
        optional: bool,
    },

    // Calls
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        optional: bool,
    },
    New {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },

    // Functions
    ArrowFunction {
        func_index: usize,
    },
    FunctionExpr {
        func_index: usize,
    },

    // Async
    Await(Box<Expr>),

    // Generators
    Yield {
        value: Option<Box<Expr>>,
        delegate: bool,
    },

    // Typeof
    TypeOf(Box<Expr>),

    // `delete obj.prop` / `delete obj[key]`. Yields a boolean.
    Delete(Box<Expr>),

    // Classes
    ClassExpr {
        name: Option<String>,
        super_class: Option<String>,
        constructor: Option<Box<FunctionDef>>,
        methods: Vec<ClassMethod>,
        static_methods: Vec<ClassMethod>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjProperty {
    pub kind: PropKind,
    pub key: String,
    pub value: Expr,
    pub computed: bool,
    /// For computed keys (`{[expr]: v}`), the key expression to evaluate at runtime.
    pub key_expr: Option<Box<Expr>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum PropKind {
    Init,
    Get,
    Set,
    Method,
    Shorthand,
    Spread,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
    Eq,
    Neq,
    StrictEq,
    StrictNeq,
    Lt,
    Lte,
    Gt,
    Gte,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Ushr,
    In,
    InstanceOf,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum UnaryOp {
    Neg,
    Not,
    BitNot,
    Void,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum UpdateOp {
    Increment,
    Decrement,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum LogicalOp {
    And,
    Or,
    NullishCoalescing,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum AssignOp {
    Assign,
    AddAssign,
    SubAssign,
    MulAssign,
    DivAssign,
    RemAssign,
    PowAssign,
    BitAndAssign,
    BitOrAssign,
    BitXorAssign,
    ShlAssign,
    ShrAssign,
    UshrAssign,
    NullishAssign,
    AndAssign,
    OrAssign,
}
