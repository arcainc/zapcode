use crate::heap::{Handle, Heap};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Value {
    Undefined,
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(Arc<str>),
    /// Reference to an array slot in the [`Heap`]. Cloning shares the handle.
    Array(Handle),
    /// Reference to an object slot in the [`Heap`].
    Object(Handle),
    Function(Closure),
    /// A generator object — calling function* creates one of these.
    Generator(GeneratorObject),
    /// A deferred external call result, used to batch parallel calls inside a
    /// `Promise.all([...])`. Holds the call id; resolved by the host on resume.
    /// Only ever lives transiently inside a batch — never escapes to user code.
    Pending(u64),
    /// Internal: a bound method on a built-in object (e.g., console.log, Math.floor).
    /// Not visible to user code — used to dispatch builtin calls. These handles
    /// must be serializable because argument evaluation can suspend after a
    /// method is loaded but before it is called.
    BuiltinMethod {
        object_name: Arc<str>,
        method_name: Arc<str>,
        /// The receiver this method is bound to, captured at property-load time
        /// so argument evaluation can't clobber it. `None` for unbound markers.
        #[serde(default)]
        recv: Option<Box<Value>>,
        /// Where to write the receiver back after a mutating method (push, etc.),
        /// supporting nested paths like `obj.items` or `rows[i].tags`.
        #[serde(default)]
        place: Option<Place>,
    },
}

/// The root variable a [`Place`] resolves from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PlaceRoot {
    Global(String),
    Local {
        frame_index: usize,
        slot: usize,
    },
    /// A shared upvalue cell (captured variable) by arena id.
    Cell(u64),
    /// The nearest enclosing `this` (for `this.items.push(...)` in a method).
    This,
}

/// One step in a [`Place`] path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PlaceSeg {
    Prop(String),
    Index(usize),
}

/// A write-back location: a root variable plus a path of property/index steps,
/// e.g. `obj.items` is `{ root: obj, path: [Prop("items")] }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Place {
    pub root: PlaceRoot,
    pub path: Vec<PlaceSeg>,
}

/// Identifies a function in the compiled program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FunctionRef {
    pub program_id: usize,
    pub function_id: usize,
}

/// A closure captures the enclosing scope's variables.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Closure {
    pub func_ref: FunctionRef,
    /// Free variables captured by value (functions, top-level user globals).
    pub captured: Vec<(String, Value)>,
    /// Free variables captured by reference: name -> shared upvalue cell id.
    /// Mutations through the cell are visible to every scope sharing it, which
    /// is what makes accumulators, factory state, and callback side effects work.
    #[serde(default)]
    pub env: Vec<(String, u64)>,
}

/// The state of a generator object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratorObject {
    /// Unique ID for this generator instance (used as key in VM generator registry).
    pub id: u64,
    /// The function this generator was created from.
    pub func_ref: FunctionRef,
    /// Captured closure variables.
    pub captured: Vec<(String, Value)>,
    /// Free variables captured by reference (shared upvalue cells).
    #[serde(default)]
    pub env: Vec<(String, u64)>,
    /// Suspended execution state. None = not yet started.
    pub suspended: Option<SuspendedFrame>,
    /// Whether the generator has completed.
    pub done: bool,
}

/// Saved execution state of a suspended generator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuspendedFrame {
    pub ip: usize,
    pub locals: Vec<Value>,
    pub stack: Vec<Value>,
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Undefined => "undefined",
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Int(_) | Value::Float(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "object",
            Value::Object(_) => "object",
            Value::Function(_) | Value::BuiltinMethod { .. } => "function",
            Value::Generator(_) => "object",
            Value::Pending(_) => "object",
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Undefined | Value::Null => false,
            Value::Bool(b) => *b,
            Value::Int(n) => *n != 0,
            Value::Float(n) => *n != 0.0 && !n.is_nan(),
            Value::String(s) => !s.is_empty(),
            Value::Array(_)
            | Value::Object(_)
            | Value::Function(_)
            | Value::BuiltinMethod { .. }
            | Value::Generator(_)
            | Value::Pending(_) => true,
        }
    }

    /// Numeric coercion for primitives. Reference values (array/object) need the
    /// heap to inspect their contents — use [`Value::to_number_heap`] for those;
    /// here they coerce to NaN.
    pub fn to_number(&self) -> f64 {
        match self {
            Value::Undefined => f64::NAN,
            Value::Null => 0.0,
            Value::Bool(true) => 1.0,
            Value::Bool(false) => 0.0,
            Value::Int(n) => *n as f64,
            Value::Float(n) => *n,
            Value::String(s) => Self::parse_number_str(s),
            _ => f64::NAN,
        }
    }

    /// Full JS numeric coercion, including reference values:
    /// `ToNumber(array)` via its `toString` ([] -> 0, [5] -> 5, [1,2] -> NaN),
    /// and a Date to its epoch-millis (so `d2 - d1`, `+d`, `<`/`>` work).
    pub fn to_number_heap(&self, heap: &Heap) -> f64 {
        match self {
            Value::Array(_) => Self::parse_number_str(&self.to_js_string(heap)),
            Value::Object(h) => match heap.object(*h).and_then(|m| m.get("__date_ms__")) {
                Some(Value::Int(ms)) => *ms as f64,
                Some(Value::Float(ms)) => *ms,
                _ => f64::NAN,
            },
            _ => self.to_number(),
        }
    }

    /// JS string-to-number coercion (`Number("...")`), supporting the numeric
    /// forms Node accepts: empty/whitespace -> 0, decimal/float, hex `0x`,
    /// binary `0b`, octal `0o`, and `Infinity`. Anything else -> NaN.
    fn parse_number_str(s: &str) -> f64 {
        let t = s.trim();
        if t.is_empty() {
            return 0.0;
        }
        match t {
            "Infinity" | "+Infinity" => return f64::INFINITY,
            "-Infinity" => return f64::NEG_INFINITY,
            _ => {}
        }
        // Radix prefixes (no sign allowed by spec for these forms).
        let radix = if let Some(rest) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
            Some((rest, 16))
        } else if let Some(rest) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) {
            Some((rest, 2))
        } else if let Some(rest) = t.strip_prefix("0o").or_else(|| t.strip_prefix("0O")) {
            Some((rest, 8))
        } else {
            None
        };
        if let Some((digits, base)) = radix {
            return match u64::from_str_radix(digits, base) {
                Ok(n) => n as f64,
                Err(_) => f64::NAN,
            };
        }
        t.parse::<f64>().unwrap_or(f64::NAN)
    }

    pub fn to_js_string(&self, heap: &Heap) -> String {
        match self {
            Value::Undefined => "undefined".to_string(),
            Value::Null => "null".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Int(n) => n.to_string(),
            Value::Float(n) => {
                if n.is_infinite() {
                    if *n > 0.0 {
                        "Infinity".to_string()
                    } else {
                        "-Infinity".to_string()
                    }
                } else if n.is_nan() {
                    "NaN".to_string()
                } else {
                    // Remove trailing ".0" for whole numbers
                    n.to_string()
                }
            }
            Value::String(s) => s.to_string(),
            Value::Array(h) => {
                // JS Array.prototype.toString renders null/undefined (and holes)
                // as the empty string, not the literal "null"/"undefined".
                let items: Vec<String> = heap
                    .array(*h)
                    .iter()
                    .map(|v| match v {
                        Value::Null | Value::Undefined => String::new(),
                        _ => v.to_js_string(heap),
                    })
                    .collect();
                items.join(",")
            }
            Value::Object(h) => {
                let Some(map) = heap.object(*h) else {
                    return "[object Object]".to_string();
                };
                // A Date stringifies to its ISO form (rather than [object Object]).
                if let Some(ms) = map.get("__date_ms__") {
                    let ms = match ms {
                        Value::Int(n) => *n as f64,
                        Value::Float(n) => *n,
                        _ => f64::NAN,
                    };
                    if ms.is_nan() {
                        return "Invalid Date".to_string();
                    }
                    return crate::vm::unix_millis_to_iso(ms as i64);
                }
                // Error objects stringify as "Name: message" (like JS).
                if matches!(map.get("__error__"), Some(Value::Bool(true))) {
                    let name = map
                        .get("name")
                        .map(|v| v.to_js_string(heap))
                        .unwrap_or_else(|| "Error".to_string());
                    let message = map
                        .get("message")
                        .map(|v| v.to_js_string(heap))
                        .unwrap_or_default();
                    if message.is_empty() {
                        name
                    } else {
                        format!("{}: {}", name, message)
                    }
                } else {
                    "[object Object]".to_string()
                }
            }
            Value::Function(_) | Value::BuiltinMethod { .. } => "function".to_string(),
            Value::Generator(_) => "[object Generator]".to_string(),
            Value::Pending(_) => "[object Promise]".to_string(),
        }
    }

    /// Strict equality (===). Arrays/objects compare by identity (handle).
    pub fn strict_eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Undefined, Value::Undefined) | (Value::Null, Value::Null) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Int(a), Value::Float(b)) => (*a as f64) == *b,
            (Value::Float(a), Value::Int(b)) => *a == (*b as f64),
            (Value::String(a), Value::String(b)) => a == b,
            // Reference identity for arrays/objects (same heap slot).
            (Value::Array(a), Value::Array(b)) => a == b,
            (Value::Object(a), Value::Object(b)) => a == b,
            _ => false,
        }
    }

    /// JS abstract (loose) equality (`==`). Performs type coercion between
    /// numbers, strings, and booleans; `null`/`undefined` are loosely equal to
    /// each other and to nothing else. Object/array operands fall back to
    /// reference identity (no `toPrimitive` coercion).
    pub fn loose_eq(&self, other: &Value) -> bool {
        match (self, other) {
            // null and undefined are loosely equal to each other only.
            (Value::Null | Value::Undefined, Value::Null | Value::Undefined) => true,
            (Value::Null | Value::Undefined, _) | (_, Value::Null | Value::Undefined) => false,
            // Same-type primitives.
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Int(_) | Value::Float(_), Value::Int(_) | Value::Float(_)) => {
                self.strict_eq(other)
            }
            // number == string: compare numerically (NaN never equal).
            (Value::Int(_) | Value::Float(_), Value::String(_))
            | (Value::String(_), Value::Int(_) | Value::Float(_)) => {
                let a = self.to_number();
                let b = other.to_number();
                !a.is_nan() && !b.is_nan() && a == b
            }
            // boolean coerces to number, then compare.
            (Value::Bool(_), _) => Value::Float(self.to_number()).loose_eq(other),
            (_, Value::Bool(_)) => self.loose_eq(&Value::Float(other.to_number())),
            // Arrays/objects: reference identity only.
            _ => self.strict_eq(other),
        }
    }
}

impl fmt::Display for Value {
    /// Heap-free best-effort rendering for diagnostics. Array/object contents
    /// aren't available without the heap; use [`Value::to_js_string`] for real
    /// string coercion of those.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Array(_) => write!(f, "[object Array]"),
            Value::Object(_) => write!(f, "[object Object]"),
            other => write!(f, "{}", other.to_js_string(&Heap::new())),
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        self.strict_eq(other)
    }
}
