use serde::{Deserialize, Serialize};

/// Which `Promise` combinator a deferred batch represents. Determines how the
/// host settles the batched calls and how the VM assembles the final promise
/// value when the batch resumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BatchKind {
    /// `Promise.all` — array of results in call order; reject on first failure.
    All,
    /// `Promise.race` — the single first-settled value (resolve or reject).
    Race,
    /// `Promise.any` — the first fulfilled value; reject (AggregateError) only
    /// when every call rejects.
    Any,
    /// `Promise.allSettled` — array of `{status, value|reason}` objects; never
    /// rejects.
    AllSettled,
}

impl BatchKind {
    /// Stable lowercase tag exposed to the host bridge.
    pub fn as_str(self) -> &'static str {
        match self {
            BatchKind::All => "all",
            BatchKind::Race => "race",
            BatchKind::Any => "any",
            BatchKind::AllSettled => "allSettled",
        }
    }
}

/// Bytecode instructions for the Zapcode VM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Instruction {
    // Stack
    Push(Constant),
    Pop,
    Dup,

    // Variables
    LoadLocal(usize),
    StoreLocal(usize),
    LoadGlobal(String),
    StoreGlobal(String),
    DeclareLocal(String),

    // Arithmetic
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
    Neg,
    BitNot,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Ushr,

    // Comparison
    Eq,
    Neq,
    StrictEq,
    StrictNeq,
    Lt,
    Lte,
    Gt,
    Gte,

    // Logical
    Not,

    // Objects & Arrays
    CreateArray(usize),
    CreateObject(usize),
    ObjectRest(Vec<String>),
    GetProperty(String),
    SetProperty(String),
    GetIndex,
    SetIndex,
    /// Remove a property by name from the object on the stack: `[obj] -> [obj']`.
    DeleteProperty(String),
    /// Remove a property by computed key: `[obj, key] -> [obj']`.
    DeleteIndex,
    /// Per-iteration rebinding for `for (let i ...)`: if the local slot has been
    /// captured (boxed) into a shared cell, copy its value into a fresh cell so
    /// closures created in the just-finished iteration keep their own binding.
    FreshenBinding(usize),
    Spread,
    /// Append one value to an accumulator array on the stack: `[acc, value] -> [acc']`.
    ArrayAppend,
    /// Spread an iterable into an accumulator array: `[acc, iterable] -> [acc']`.
    ArraySpreadAppend,
    /// Replace the array on top of the stack with a new array of its elements
    /// from the given index onward (array-rest destructuring `[a, ...rest]`).
    ArrayRestFrom(usize),
    /// Insert a key/value into an accumulator object: `[acc, key, value] -> [acc']`.
    ObjectInsert,
    /// Merge a source object's entries into an accumulator object: `[acc, src] -> [acc']`.
    ObjectSpreadAssign,
    In,
    InstanceOf,

    // Functions
    CreateClosure(usize),
    Call(usize),
    Return,
    CallExternal(String, usize),
    /// Call with spread args: stack is `[callee, args_array]`. The flattened
    /// args array (built like an array literal) is expanded and the call runs.
    CallSpread,
    /// External call with spread args: stack is `[args_array]`.
    CallExternalSpread(String),
    /// Like `CallExternal` but does not suspend: pops the args, registers a
    /// deferred external call, and pushes a `Value::Pending`. Emitted only for
    /// direct external calls that are elements of a `Promise.all([...])` literal,
    /// so the calls can be batched and run in parallel by the host.
    CallExternalDeferred(String, usize),
    /// Pops `n` items (some may be `Value::Pending`) and builds a batch promise
    /// tagged with the combinator kind. When awaited it suspends once with all
    /// of its pending calls; the host settles them per the combinator and the
    /// VM assembles the final value accordingly.
    MakeBatchPromise(BatchKind, usize),
    /// Pops a single `Value::Pending(id)` (just produced by `CallExternalDeferred`)
    /// and wraps it in a deferred single-call Promise object (`status:
    /// "pending_call"`). The host call is not made until the promise is awaited
    /// or driven by `.then`/`.catch`/`.finally`. Emitted for a bare (un-awaited)
    /// tool-call expression so `const p = tool(); typeof p === "object"` and
    /// `p.then(...)` behave like a real Promise (N5).
    MakeCallPromise,

    // Control flow
    Jump(usize),
    JumpIfFalse(usize),
    JumpIfTrue(usize),
    JumpIfNullish(usize),

    // Loops
    SetupLoop,
    /// Transfer control to `target` (a loop's exit / next-iteration ip) the way a
    /// `break`/`continue` does, but first run any `finally` blocks the transfer
    /// escapes (try-statements that enclose this jump but are enclosed by the
    /// loop). Carries the resolved jump target.
    Break(usize),
    Continue(usize),

    // Iterators
    GetIterator,
    IteratorNext,
    IteratorDone,
    /// Pop the top value and push a freshly materialized array of its iterated
    /// elements. Drives generators, iterates strings (by char), Sets, Maps (as
    /// [k,v] pairs) and copies arrays. Used by spread (`[...x]`) and array
    /// destructuring (`const [a,b] = x`) so non-array iterables are consumed.
    IterableToArray,

    // Error handling
    //
    // `SetupTry { catch_ip, finally_ip, region_end }` protects the following try
    // body. On a throw the VM transfers to `catch_ip` if a catch handler exists,
    // otherwise straight to the `finally` body (recording a Throw completion).
    // `finally_ip` is the start of the finally body when the statement has one.
    // `region_end` is the ip just past the whole try/catch/finally statement,
    // used to decide whether a `break`/`continue` escapes this try (so its
    // finally must run) or stays inside it.
    SetupTry {
        catch_ip: usize,
        finally_ip: Option<usize>,
        region_end: usize,
    },
    Throw,
    EndTry,
    /// Record a normal completion for the active try/finally and jump to its
    /// finally body (compiler emits this on the normal fall-through and end-of-
    /// catch paths so the finally always runs). No-op if the active try has no
    /// finally. Operand is the finally body's start ip.
    EnterFinallyNormal(usize),
    /// Marks the end of a finally body. Pops the active try frame's pending
    /// completion and resumes it (re-throw / re-return / re-break / re-continue /
    /// fall through). An abrupt completion *inside* the finally body supersedes
    /// the pending one and is handled before this instruction is reached.
    EndFinally,

    // Typeof
    TypeOf,

    // Void
    Void,

    // Update
    Increment,
    Decrement,

    // Template literals
    ConcatStrings(usize),

    // Destructuring
    DestructureObject(Vec<String>),
    DestructureArray(usize),

    // Classes
    /// Create a class. The compiler pushes the following groups onto the stack, in
    /// this order (so they pop in reverse — constructor first):
    ///   [optional super class]
    ///   n_static_fields  * (name, init_closure) pairs
    ///   n_fields         * (name, init_closure) pairs
    ///   n_static_setters * (name, closure) pairs
    ///   n_static_getters * (name, closure) pairs
    ///   n_setters        * (name, closure) pairs
    ///   n_getters        * (name, closure) pairs
    ///   n_statics        * (name, closure) pairs   (static methods)
    ///   n_methods        * (name, closure) pairs   (instance methods)
    ///   constructor closure (or undefined)   <- top of stack
    /// Field init closures take no args and run with `this` bound to the instance;
    /// getter/setter closures are installed as accessor descriptors. Pushes the
    /// class object (an Object with __constructor__, __prototype__, __class_name__,
    /// and any __getters__/__setters__/__field_inits__/__static_field_inits__).
    CreateClass {
        name: String,
        n_methods: usize,
        n_statics: usize,
        n_getters: usize,
        n_setters: usize,
        n_static_getters: usize,
        n_static_setters: usize,
        n_fields: usize,
        n_static_fields: usize,
        has_super: bool,
    },
    /// Construct: pops class object + args, creates instance, calls constructor, pushes instance.
    Construct(usize),
    /// Load `this` from the current call frame.
    LoadThis,
    /// Store a value as the current `this` (used for this.prop = val).
    StoreThis,
    /// Call super constructor with n args. Pops args, looks up __super__.__constructor__,
    /// calls it with current `this`.
    ///
    /// `class` is the lexically-defining class name (the class whose method/constructor
    /// the `super(...)` appears in), so we resolve `class.__super__.__constructor__`
    /// reliably even with multiple subclasses. `None` falls back to the legacy
    /// global-scan heuristic for any bytecode compiled before this field existed.
    CallSuper {
        arg_count: usize,
        class: Option<String>,
    },
    /// Resolve a parent-class method for `super.m(...)`: look up the lexically-defining
    /// `class`'s `__super__.__prototype__`, fetch method `method`, bind the current
    /// `this` as receiver, and push the resulting `Value::Function` so a following
    /// `Call` invokes the parent method with the current instance.
    LoadSuperMethod { class: String, method: String },
    /// Read a parent-class property for `super.prop` (non-call): look up the
    /// lexically-defining `class`'s `__super__.__prototype__` and fetch `prop`.
    /// Bare super-prototype data is rare (instance fields live on `this`), so this
    /// yields `undefined` when absent, matching JS prototype-chain reads.
    LoadSuperProp { class: String, prop: String },

    // Generators
    /// Create a generator object from a function index (like CreateClosure but for generators).
    CreateGenerator(usize),
    /// Yield a value from a generator. Pops the value, suspends execution.
    Yield,

    /// Await: if the top-of-stack is a resolved Promise object, unwrap its value.
    /// If it's a regular value, leave it as-is. External call suspension is handled
    /// by CallExternal before Await is reached.
    Await,

    // Misc
    Nop,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Constant {
    Undefined,
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
}
