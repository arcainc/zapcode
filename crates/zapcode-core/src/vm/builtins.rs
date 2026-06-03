use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;

use crate::error::{Result, ZapcodeError};
use crate::heap::{Handle, Heap};
use crate::sandbox::ResourceLimits;
use crate::value::Value;

/// Register built-in global objects and functions, allocating their backing
/// objects in `heap`.
pub fn register_globals(globals: &mut HashMap<String, Value>, heap: &mut Heap) {
    // Register known globals as objects — method calls are intercepted by the VM.
    let empty = heap.alloc_object(IndexMap::new());
    globals.insert("console".to_string(), Value::Object(empty));
    let empty = heap.alloc_object(IndexMap::new());
    globals.insert("JSON".to_string(), Value::Object(empty));
    globals.insert("Object".to_string(), builtin_constructor("Object", heap));
    globals.insert("Array".to_string(), builtin_constructor("Array", heap));
    let empty = heap.alloc_object(IndexMap::new());
    globals.insert("Promise".to_string(), Value::Object(empty));
    globals.insert("Map".to_string(), builtin_constructor("Map", heap));
    globals.insert("Set".to_string(), builtin_constructor("Set", heap));
    globals.insert("Date".to_string(), {
        let mut d = IndexMap::new();
        d.insert(
            Arc::from("__builtin_constructor__"),
            Value::String(Arc::from("Date")),
        );
        // Static methods Date.now()/Date.parse()/Date.UTC() (callable globals).
        for (prop, target) in [("now", "Date.now"), ("parse", "Date.parse"), ("UTC", "Date.UTC")] {
            d.insert(Arc::from(prop), global_fn_marker(target, heap));
        }
        Value::Object(heap.alloc_object(d))
    });
    for err in [
        "Error",
        "TypeError",
        "RangeError",
        "SyntaxError",
        "ReferenceError",
        "AggregateError",
    ] {
        globals.insert(err.to_string(), builtin_constructor(err, heap));
    }

    // Callable bare globals (type conversions + numeric parsing/predicates),
    // dispatched by the VM's Call instruction (object marker "__global_fn__").
    for name in [
        "String",
        "Number",
        "Boolean",
        "parseInt",
        "parseFloat",
        "isNaN",
        "isFinite",
        "structuredClone",
        // Minimal Symbol factory (O8): callable, typeof === "function", and
        // `Symbol()` yields a unique marker value.
        "Symbol",
    ] {
        let v = global_fn(name, heap);
        globals.insert(name.to_string(), v);
    }

    // Math gets its constants as real properties
    let mut math = IndexMap::new();
    math.insert(Arc::from("PI"), Value::Float(std::f64::consts::PI));
    math.insert(Arc::from("E"), Value::Float(std::f64::consts::E));
    math.insert(Arc::from("LN2"), Value::Float(std::f64::consts::LN_2));
    math.insert(Arc::from("LN10"), Value::Float(std::f64::consts::LN_10));
    math.insert(Arc::from("LOG2E"), Value::Float(std::f64::consts::LOG2_E));
    math.insert(Arc::from("LOG10E"), Value::Float(std::f64::consts::LOG10_E));
    math.insert(Arc::from("SQRT2"), Value::Float(std::f64::consts::SQRT_2));
    math.insert(
        Arc::from("SQRT1_2"),
        Value::Float(1.0 / std::f64::consts::SQRT_2),
    );
    globals.insert("Math".to_string(), Value::Object(heap.alloc_object(math)));
}

/// A `{ __global_fn__: target }` marker object (a callable static), heap-allocated.
fn global_fn_marker(target: &str, heap: &mut Heap) -> Value {
    let mut s = IndexMap::new();
    s.insert(Arc::from("__global_fn__"), Value::String(Arc::from(target)));
    Value::Object(heap.alloc_object(s))
}

fn builtin_constructor(name: &str, heap: &mut Heap) -> Value {
    let mut obj = IndexMap::new();
    obj.insert(
        Arc::from("__builtin_constructor__"),
        Value::String(Arc::from(name)),
    );
    Value::Object(heap.alloc_object(obj))
}

/// A bare type-conversion function (`String`/`Number`/`Boolean`), represented as
/// an object so it can be both *called* (via the `__global_fn__` marker, handled
/// in the VM's Call instruction) and carry static properties (e.g.
/// `Number.MAX_SAFE_INTEGER`).
fn global_fn(name: &str, heap: &mut Heap) -> Value {
    let mut obj = IndexMap::new();
    obj.insert(Arc::from("__global_fn__"), Value::String(Arc::from(name)));
    if name == "String" {
        for (prop, target) in [
            ("fromCharCode", "String.fromCharCode"),
            ("fromCodePoint", "String.fromCodePoint"),
        ] {
            obj.insert(Arc::from(prop), global_fn_marker(target, heap));
        }
    }
    if name == "Number" {
        obj.insert(
            Arc::from("MAX_SAFE_INTEGER"),
            Value::Int(9_007_199_254_740_991),
        );
        obj.insert(
            Arc::from("MIN_SAFE_INTEGER"),
            Value::Int(-9_007_199_254_740_991),
        );
        obj.insert(Arc::from("MAX_VALUE"), Value::Float(f64::MAX));
        obj.insert(Arc::from("MIN_VALUE"), Value::Float(f64::MIN_POSITIVE));
        obj.insert(Arc::from("EPSILON"), Value::Float(f64::EPSILON));
        obj.insert(Arc::from("POSITIVE_INFINITY"), Value::Float(f64::INFINITY));
        obj.insert(
            Arc::from("NEGATIVE_INFINITY"),
            Value::Float(f64::NEG_INFINITY),
        );
        obj.insert(Arc::from("NaN"), Value::Float(f64::NAN));
        // Static methods (callable): Number.isInteger(x), Number.parseInt(s), …
        for m in [
            "isInteger",
            "isSafeInteger",
            "isNaN",
            "isFinite",
            "parseInt",
            "parseFloat",
        ] {
            obj.insert(Arc::from(m), global_fn_marker(&format!("Number.{m}"), heap));
        }
    }
    Value::Object(heap.alloc_object(obj))
}

/// Parse the leading integer of a string (JS `parseInt` semantics, base 10 or a
/// given radix). Returns NaN if no digits lead.
fn js_parse_int(s: &str, radix: u32) -> f64 {
    let t = s.trim();
    let (neg, rest) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    // radix 0 = auto-detect: a `0x`/`0X` prefix means hex, otherwise base 10.
    let (radix, rest) = if radix == 0 {
        match rest.strip_prefix("0x").or_else(|| rest.strip_prefix("0X")) {
            Some(hex) => (16, hex),
            None => (10, rest),
        }
    } else if radix == 16 {
        // An explicit radix 16 still tolerates a leading 0x.
        (
            16,
            rest.strip_prefix("0x")
                .or_else(|| rest.strip_prefix("0X"))
                .unwrap_or(rest),
        )
    } else {
        (radix, rest)
    };
    let digits: String = rest.chars().take_while(|c| c.is_digit(radix)).collect();
    if digits.is_empty() {
        return f64::NAN;
    }
    match i64::from_str_radix(&digits, radix) {
        Ok(n) => {
            let n = n as f64;
            if neg {
                -n
            } else {
                n
            }
        }
        Err(_) => f64::NAN,
    }
}

/// Parse the leading float of a string (JS `parseFloat` semantics).
fn js_parse_float(s: &str) -> f64 {
    let t = s.trim();
    // Take the longest valid float prefix.
    let mut end = 0;
    let bytes = t.as_bytes();
    let mut seen_dot = false;
    let mut seen_e = false;
    for (i, &b) in bytes.iter().enumerate() {
        let c = b as char;
        let ok = c.is_ascii_digit()
            || (c == '-' || c == '+') && (i == 0 || bytes[i - 1] == b'e' || bytes[i - 1] == b'E')
            || (c == '.' && !seen_dot && !seen_e)
            || ((c == 'e' || c == 'E') && !seen_e && i > 0);
        if !ok {
            break;
        }
        if c == '.' {
            seen_dot = true;
        }
        if c == 'e' || c == 'E' {
            seen_e = true;
        }
        end = i + 1;
    }
    t[..end].parse::<f64>().unwrap_or(f64::NAN)
}

fn finite_number(n: f64) -> Value {
    if n.is_finite() && n.fract() == 0.0 {
        Value::Int(n as i64)
    } else {
        Value::Float(n)
    }
}

pub fn is_number_method(name: &str) -> bool {
    matches!(
        name,
        "toFixed" | "toString" | "toPrecision" | "toExponential" | "valueOf"
    )
}

/// Methods on number primitives: `(3.14159).toFixed(2)`, `(255).toString(16)`, …
pub fn call_number_method(n: f64, method: &str, args: &[Value]) -> Result<Option<Value>> {
    let result = match method {
        "toFixed" => {
            let digits = match args.first() {
                Some(v) => v.to_number().clamp(0.0, 100.0) as usize,
                None => 0,
            };
            Value::String(Arc::from(js_to_fixed(n, digits).as_str()))
        }
        "toPrecision" => match args.first() {
            None | Some(Value::Undefined) => Value::String(Arc::from(format_number(n).as_str())),
            Some(v) => {
                let p = (v.to_number() as usize).clamp(1, 100);
                Value::String(Arc::from(js_to_precision(n, p).as_str()))
            }
        },
        "toExponential" => {
            let digits = match args.first() {
                None | Some(Value::Undefined) => None,
                Some(v) => Some((v.to_number() as usize).min(100)),
            };
            Value::String(Arc::from(js_to_exponential(n, digits).as_str()))
        }
        "toString" => {
            let radix = match args.first() {
                Some(v) => v.to_number() as u32,
                None => 10,
            };
            if radix == 10 || !(2..=36).contains(&radix) {
                Value::String(Arc::from(format_number(n).as_str()))
            } else {
                Value::String(Arc::from(radix_to_string(n, radix).as_str()))
            }
        }
        "valueOf" => finite_number(n),
        _ => return Ok(None),
    };
    Ok(Some(result))
}

/// `Number.prototype.toFixed`: round half away from zero (JS semantics, unlike
/// Rust's `{:.*}` which rounds half-to-even), then format with `digits` decimals.
fn js_to_fixed(n: f64, digits: usize) -> String {
    if n.is_nan() {
        return "NaN".to_string();
    }
    if n.is_infinite() {
        return if n > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    // JS switches to ToString for magnitudes >= 1e21.
    if n.abs() >= 1e21 {
        return format_number(n);
    }
    let neg = n < 0.0;
    // Round the *exact* decimal expansion of the f64 (Rust's high-precision
    // format is exact), half-away-from-zero. Scaling by 10^digits and rounding
    // would reintroduce exact halves (e.g. 0.15*10 rounds up to 1.5), giving the
    // wrong answer; V8 rounds the true decimal value, so we do the same.
    let extra = 40usize;
    let full = format!("{:.*}", digits + extra, n.abs());
    let (int_part, frac) = full.split_once('.').unwrap_or((full.as_str(), ""));
    let frac_bytes = frac.as_bytes();
    let decider = frac_bytes.get(digits).map(|b| b - b'0').unwrap_or(0);

    let mut digs: Vec<u8> = int_part
        .bytes()
        .chain(frac.bytes().take(digits))
        .map(|b| b - b'0')
        .collect();
    let mut int_len = int_part.len();
    if decider >= 5 {
        let mut carry = 1u8;
        for d in digs.iter_mut().rev() {
            let v = *d + carry;
            *d = v % 10;
            carry = v / 10;
            if carry == 0 {
                break;
            }
        }
        if carry > 0 {
            digs.insert(0, carry);
            int_len += 1;
        }
    }
    let int_s: String = digs[..int_len].iter().map(|&d| (d + b'0') as char).collect();
    let s = if digits == 0 {
        int_s
    } else {
        let frac_s: String = digs[int_len..].iter().map(|&d| (d + b'0') as char).collect();
        format!("{}.{}", int_s, frac_s)
    };
    if neg {
        format!("-{}", s)
    } else {
        s
    }
}

/// Append an explicit sign to the exponent of a Rust `{:e}`-formatted string
/// (Rust omits `+` for positive exponents; JS requires `e+N`/`e-N`).
fn with_exp_sign(s: &str) -> String {
    match s.find('e') {
        Some(pos) => {
            let (mantissa, exp) = s.split_at(pos);
            let exp = &exp[1..];
            if exp.starts_with('-') {
                format!("{}e{}", mantissa, exp)
            } else {
                format!("{}e+{}", mantissa, exp)
            }
        }
        None => s.to_string(),
    }
}

/// `Number.prototype.toExponential`.
fn js_to_exponential(n: f64, digits: Option<usize>) -> String {
    if !n.is_finite() {
        return format_number(n);
    }
    let formatted = match digits {
        Some(d) => format!("{:.*e}", d, n),
        None => format!("{:e}", n),
    };
    with_exp_sign(&formatted)
}

/// `Number.prototype.toPrecision` with a significant-digit count.
fn js_to_precision(n: f64, p: usize) -> String {
    if !n.is_finite() {
        return format_number(n);
    }
    if n == 0.0 {
        return if p == 1 {
            "0".to_string()
        } else {
            format!("0.{}", "0".repeat(p - 1))
        };
    }
    let neg = n < 0.0;
    let x = n.abs();
    // Decompose into `p` significant digits and a base-10 exponent.
    let sci = format!("{:.*e}", p - 1, x);
    let (mantissa, exp_str) = sci.split_once('e').unwrap_or((sci.as_str(), "0"));
    let e: i32 = exp_str.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(|c| c.is_ascii_digit()).collect();

    let body = if e < -6 || e >= p as i32 {
        // Exponential form.
        let mut m = String::new();
        m.push_str(&digits[..1]);
        if p > 1 {
            m.push('.');
            m.push_str(&digits[1..]);
        }
        with_exp_sign(&format!("{}e{}", m, e))
    } else if e >= 0 {
        let int_len = (e + 1) as usize;
        if int_len >= p {
            format!("{}{}", digits, "0".repeat(int_len - p))
        } else {
            format!("{}.{}", &digits[..int_len], &digits[int_len..])
        }
    } else {
        format!("0.{}{}", "0".repeat((-e - 1) as usize), digits)
    };
    if neg {
        format!("-{}", body)
    } else {
        body
    }
}

/// Radix conversion for `Number.prototype.toString(radix)`, including the
/// fractional part (e.g. `(3.5).toString(2)` -> "11.1").
fn radix_to_string(n: f64, radix: u32) -> String {
    if n == 0.0 {
        return "0".to_string();
    }
    if !n.is_finite() {
        return format_number(n);
    }
    let neg = n < 0.0;
    let x = n.abs();
    let mut int_part = x.trunc() as i128;
    let mut frac = x.fract();

    let mut idigits = Vec::new();
    if int_part == 0 {
        idigits.push('0');
    }
    while int_part > 0 {
        let d = (int_part % radix as i128) as u32;
        idigits.push(std::char::from_digit(d, radix).unwrap());
        int_part /= radix as i128;
    }
    idigits.reverse();
    let mut s: String = idigits.into_iter().collect();

    if frac > 0.0 {
        s.push('.');
        let mut count = 0;
        while frac > 0.0 && count < 52 {
            frac *= radix as f64;
            let d = frac.trunc() as u32;
            s.push(std::char::from_digit(d, radix).unwrap());
            frac -= d as f64;
            count += 1;
        }
    }
    if neg {
        format!("-{}", s)
    } else {
        s
    }
}

/// Format a number the way the VM's `to_js_string` does (no trailing `.0`).
fn format_number(n: f64) -> String {
    if n.is_nan() {
        "NaN".to_string()
    } else if n.is_infinite() {
        if n > 0.0 {
            "Infinity".to_string()
        } else {
            "-Infinity".to_string()
        }
    } else if n.fract() == 0.0 && n.abs() < 1e15 {
        (n as i64).to_string()
    } else {
        n.to_string()
    }
}

/// Dispatch a callable bare global / Number static (see `global_fn`).
pub fn call_global_fn(kind: &str, args: &[Value], heap: &mut Heap) -> Result<Value> {
    let arg = args.first().cloned().unwrap_or(Value::Undefined);
    Ok(match kind {
        "String" => Value::String(Arc::from(arg.to_js_string(heap).as_str())),
        "Number" => finite_number(arg.to_number_heap(heap)),
        "Boolean" => Value::Bool(arg.is_truthy()),
        "parseInt" | "Number.parseInt" => {
            // radix 0 means "auto-detect": js_parse_int infers hex from a 0x
            // prefix, else base 10.
            let radix = match args.get(1) {
                Some(Value::Int(r)) if (2..=36).contains(r) => *r as u32,
                Some(Value::Float(r)) if (2.0..=36.0).contains(r) => *r as u32,
                _ => 0,
            };
            let s = match &arg {
                Value::String(s) => s.to_string(),
                other => other.to_js_string(heap),
            };
            Value::Float(js_parse_int(&s, radix))
        }
        "parseFloat" | "Number.parseFloat" => {
            let s = match &arg {
                Value::String(s) => s.to_string(),
                other => other.to_js_string(heap),
            };
            Value::Float(js_parse_float(&s))
        }
        "isNaN" => Value::Bool(arg.to_number_heap(heap).is_nan()),
        "isFinite" => Value::Bool(arg.to_number_heap(heap).is_finite()),
        // Deep-copy so the result is independent of the original (reference semantics).
        "structuredClone" => heap.deep_clone(&arg),
        "String.fromCharCode" => {
            let s: String = args
                .iter()
                .filter_map(|v| char::from_u32(v.to_number() as u32))
                .collect();
            Value::String(Arc::from(s.as_str()))
        }
        "String.fromCodePoint" => {
            let s: String = args
                .iter()
                .filter_map(|v| char::from_u32(v.to_number() as u32))
                .collect();
            Value::String(Arc::from(s.as_str()))
        }
        // Date statics. now() is 0 — the sandbox has no wall clock (deterministic
        // replay); inject the current time via a host tool when needed.
        "Date.now" => Value::Int(0),
        "Date.parse" => {
            let s = arg.to_js_string(heap);
            match crate::vm::parse_date_string(&s) {
                Some(ms) => Value::Int(ms),
                None => Value::Float(f64::NAN),
            }
        }
        "Date.UTC" => finite_number(crate::vm::date_utc_millis(args)),
        "Number.isNaN" => Value::Bool(matches!(arg, Value::Float(n) if n.is_nan())),
        "Number.isFinite" => Value::Bool(
            matches!(arg, Value::Int(_)) || matches!(arg, Value::Float(n) if n.is_finite()),
        ),
        "Number.isInteger" => Value::Bool(match arg {
            Value::Int(_) => true,
            Value::Float(n) => n.is_finite() && n.fract() == 0.0,
            _ => false,
        }),
        "Number.isSafeInteger" => Value::Bool(match arg {
            Value::Int(n) => n.unsigned_abs() <= 9_007_199_254_740_991,
            Value::Float(n) => {
                n.is_finite() && n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_991.0
            }
            _ => false,
        }),
        // Minimal Symbol(): returns a fresh, unique primitive-ish marker so that
        // feature-detection (`typeof Symbol === "function"`) and simple use don't
        // throw (O8). Uniqueness comes from heap-handle identity — each call
        // allocates a new object, so `Symbol() !== Symbol()` under `strict_eq`
        // (reference equality), while a symbol equals itself. This is NOT full
        // Symbol semantics (no global registry, no well-known symbols, no
        // Symbol.toPrimitive dispatch).
        "Symbol" => {
            let mut s = IndexMap::new();
            s.insert(Arc::from("__symbol__"), Value::Bool(true));
            // Optional description, coerced to string per spec (undefined stays absent).
            if !matches!(arg, Value::Undefined) {
                s.insert(
                    Arc::from("description"),
                    Value::String(Arc::from(arg.to_js_string(heap).as_str())),
                );
            }
            Value::Object(heap.alloc_object(s))
        }
        other => {
            return Err(ZapcodeError::TypeError(format!(
                "{} is not a function",
                other
            )))
        }
    })
}

/// Execute a built-in method call. Returns Some(value) if handled, None if not a builtin.
pub fn call_builtin(
    object: &Value,
    method: &str,
    args: &[Value],
    limits: &ResourceLimits,
    _stdout: &mut String,
    heap: &mut Heap,
) -> Result<Option<Value>> {
    match object {
        Value::String(s) => call_string_method(&s.clone(), method, args, limits, heap),
        Value::Array(h) => call_array_method(*h, method, args, limits, heap),
        _ => Ok(None),
    }
}

/// Execute a global builtin function/method like console.log, Math.floor, JSON.parse.
pub fn call_global_method(
    global_name: &str,
    method: &str,
    args: &[Value],
    stdout: &mut String,
    heap: &mut Heap,
) -> Result<Option<Value>> {
    match global_name {
        "console" => call_console_method(method, args, stdout, heap),
        "Math" => call_math_method(method, args),
        "JSON" => call_json_method(method, args, heap),
        "Object" => call_object_method(method, args, heap),
        "Array" => call_array_static_method(method, args, heap),
        "Promise" => call_promise_method(method, args, heap),
        _ => Ok(None),
    }
}

// ── Console ──────────────────────────────────────────────────────────

fn call_console_method(method: &str, args: &[Value], stdout: &mut String, heap: &Heap) -> Result<Option<Value>> {
    match method {
        "log" | "info" | "warn" | "error" | "debug" => {
            let output: Vec<String> = args.iter().map(|v| v.to_js_string(heap)).collect();
            let line = output.join(" ");
            stdout.push_str(&line);
            stdout.push('\n');
            Ok(Some(Value::Undefined))
        }
        _ => Ok(None),
    }
}

// ── Math ─────────────────────────────────────────────────────────────

fn call_math_method(method: &str, args: &[Value]) -> Result<Option<Value>> {
    let result = match method {
        "abs" => {
            let n = arg_num(args, 0);
            Value::Float(n.abs())
        }
        "floor" => {
            let n = arg_num(args, 0);
            Value::Float(n.floor())
        }
        "ceil" => {
            let n = arg_num(args, 0);
            Value::Float(n.ceil())
        }
        "round" => {
            // JS rounds halves toward +Infinity (Math.round(-2.5) === -2),
            // unlike Rust's round-half-away-from-zero.
            let n = arg_num(args, 0);
            Value::Float((n + 0.5).floor())
        }
        "trunc" => {
            let n = arg_num(args, 0);
            Value::Float(n.trunc())
        }
        "sqrt" => {
            let n = arg_num(args, 0);
            Value::Float(n.sqrt())
        }
        "cbrt" => {
            let n = arg_num(args, 0);
            Value::Float(n.cbrt())
        }
        "pow" => {
            let base = arg_num(args, 0);
            let exp = arg_num(args, 1);
            Value::Float(base.powf(exp))
        }
        "log" => {
            let n = arg_num(args, 0);
            Value::Float(n.ln())
        }
        "log2" => {
            let n = arg_num(args, 0);
            Value::Float(n.log2())
        }
        "log10" => {
            let n = arg_num(args, 0);
            Value::Float(n.log10())
        }
        "exp" => {
            let n = arg_num(args, 0);
            Value::Float(n.exp())
        }
        "sin" => {
            let n = arg_num(args, 0);
            Value::Float(n.sin())
        }
        "cos" => {
            let n = arg_num(args, 0);
            Value::Float(n.cos())
        }
        "tan" => {
            let n = arg_num(args, 0);
            Value::Float(n.tan())
        }
        "asin" => {
            let n = arg_num(args, 0);
            Value::Float(n.asin())
        }
        "acos" => {
            let n = arg_num(args, 0);
            Value::Float(n.acos())
        }
        "atan" => {
            let n = arg_num(args, 0);
            Value::Float(n.atan())
        }
        "atan2" => {
            let y = arg_num(args, 0);
            let x = arg_num(args, 1);
            Value::Float(y.atan2(x))
        }
        "max" => {
            if args.is_empty() {
                Value::Float(f64::NEG_INFINITY)
            } else {
                let mut max = arg_num(args, 0);
                for arg in &args[1..] {
                    let n = arg.to_number();
                    if n > max {
                        max = n;
                    }
                }
                Value::Float(max)
            }
        }
        "min" => {
            if args.is_empty() {
                Value::Float(f64::INFINITY)
            } else {
                let mut min = arg_num(args, 0);
                for arg in &args[1..] {
                    let n = arg.to_number();
                    if n < min {
                        min = n;
                    }
                }
                Value::Float(min)
            }
        }
        "sign" => {
            let n = arg_num(args, 0);
            if n > 0.0 {
                Value::Float(1.0)
            } else if n < 0.0 {
                Value::Float(-1.0)
            } else {
                Value::Float(0.0)
            }
        }
        "hypot" => {
            let sum_sq: f64 = args.iter().map(|a| a.to_number().powi(2)).sum();
            Value::Float(sum_sq.sqrt())
        }
        "expm1" => Value::Float(arg_num(args, 0).exp_m1()),
        "log1p" => Value::Float(arg_num(args, 0).ln_1p()),
        "sinh" => Value::Float(arg_num(args, 0).sinh()),
        "cosh" => Value::Float(arg_num(args, 0).cosh()),
        "tanh" => Value::Float(arg_num(args, 0).tanh()),
        "random" => {
            // Math.random is served by the VM's seeded PRNG (see Vm::next_random)
            // so the sequence is deterministic across replay yet varied. This
            // stateless fallback is only reached if called without a VM.
            Value::Float(0.5)
        }
        "PI" => Value::Float(std::f64::consts::PI),
        "E" => Value::Float(std::f64::consts::E),
        _ => return Ok(None),
    };
    Ok(Some(result))
}

// ── JSON ─────────────────────────────────────────────────────────────

fn call_json_method(method: &str, args: &[Value], heap: &mut Heap) -> Result<Option<Value>> {
    match method {
        "stringify" => {
            let val = args.first().unwrap_or(&Value::Undefined);
            // Second arg may be an array replacer (whitelist of keys to keep).
            let whitelist: Option<Vec<String>> = match args.get(1) {
                Some(Value::Array(h)) => Some(
                    heap.array(*h)
                        .iter()
                        .filter_map(|v| match v {
                            Value::String(s) => Some(s.to_string()),
                            Value::Int(n) => Some(n.to_string()),
                            _ => None,
                        })
                        .collect(),
                ),
                _ => None,
            };
            // Third arg `space` enables pretty-printing (number of spaces or a string).
            let indent = match args.get(2) {
                Some(Value::Int(n)) if *n > 0 => Some(" ".repeat((*n).min(10) as usize)),
                Some(Value::Float(n)) if *n >= 1.0 => Some(" ".repeat((*n as usize).min(10))),
                Some(Value::String(s)) if !s.is_empty() => Some(s.to_string()),
                _ => None,
            };
            match serialize_json(val, whitelist.as_deref(), indent.as_deref(), 0, heap) {
                // JSON.stringify(undefined) / of a function returns the value undefined.
                Some(s) => Ok(Some(Value::String(Arc::from(s.as_str())))),
                None => Ok(Some(Value::Undefined)),
            }
        }
        "parse" => {
            let s = match args.first() {
                Some(Value::String(s)) => s.to_string(),
                _ => {
                    return Err(ZapcodeError::TypeError(
                        "JSON.parse requires a string argument".to_string(),
                    ))
                }
            };
            let val = json_to_value(&s, heap)?;
            Ok(Some(val))
        }
        _ => Ok(None),
    }
}

/// JSON-escape a string, including control characters (`\n`, `\t`, …) which
/// must not appear raw in valid JSON.
fn json_escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Serialize a value to JSON. Returns `None` for values JSON omits (undefined,
/// functions) so callers can drop object properties / emit `null` in arrays.
/// `whitelist` is the array-replacer key filter; `indent` enables pretty output.
fn serialize_json(
    val: &Value,
    whitelist: Option<&[String]>,
    indent: Option<&str>,
    depth: usize,
    heap: &Heap,
) -> Option<String> {
    match val {
        Value::Undefined
        | Value::Function(_)
        | Value::BuiltinMethod { .. }
        | Value::Generator(_)
        | Value::Pending(_) => None,
        Value::Null => Some("null".to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Int(n) => Some(n.to_string()),
        Value::Float(n) => Some(if n.is_finite() {
            format_number(*n)
        } else {
            "null".to_string()
        }),
        Value::String(s) => Some(json_escape_string(s)),
        Value::Array(h) => {
            let items: Vec<String> = heap
                .array(*h)
                .iter()
                .map(|v| {
                    serialize_json(v, whitelist, indent, depth + 1, heap)
                        .unwrap_or_else(|| "null".to_string())
                })
                .collect();
            Some(join_json_array(&items, indent, depth))
        }
        Value::Object(h) => {
            let map = heap.object(*h)?;
            // Date -> ISO string (matches Date.prototype.toJSON).
            if map.contains_key("__date_ms__") {
                let ms = map.get("__date_ms__").map(|v| v.to_number() as i64).unwrap_or(0);
                return Some(json_escape_string(&crate::vm::unix_millis_to_iso(ms)));
            }
            // Map/Set/RegExp/Error have no enumerable own data properties in JS.
            if map.contains_key("__map__")
                || map.contains_key("__set__")
                || map.contains_key("__regexp__")
                || map.contains_key("__error__")
            {
                return Some("{}".to_string());
            }
            let pairs: Vec<(String, String)> = map
                .iter()
                .filter(|(k, _)| !k.starts_with("__"))
                .filter(|(k, _)| whitelist.map_or(true, |w| w.iter().any(|x| x == k.as_ref())))
                .filter_map(|(k, v)| {
                    serialize_json(v, whitelist, indent, depth + 1, heap).map(|s| (k.to_string(), s))
                })
                .collect();
            Some(join_json_object(&pairs, indent, depth))
        }
    }
}

fn join_json_array(items: &[String], indent: Option<&str>, depth: usize) -> String {
    if items.is_empty() {
        return "[]".to_string();
    }
    match indent {
        None => format!("[{}]", items.join(",")),
        Some(unit) => {
            let pad = unit.repeat(depth + 1);
            let close = unit.repeat(depth);
            let body = items
                .iter()
                .map(|i| format!("{}{}", pad, i))
                .collect::<Vec<_>>()
                .join(",\n");
            format!("[\n{}\n{}]", body, close)
        }
    }
}

fn join_json_object(pairs: &[(String, String)], indent: Option<&str>, depth: usize) -> String {
    if pairs.is_empty() {
        return "{}".to_string();
    }
    match indent {
        None => format!(
            "{{{}}}",
            pairs
                .iter()
                .map(|(k, v)| format!("{}:{}", json_escape_string(k), v))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Some(unit) => {
            let pad = unit.repeat(depth + 1);
            let close = unit.repeat(depth);
            let body = pairs
                .iter()
                .map(|(k, v)| format!("{}{}: {}", pad, json_escape_string(k), v))
                .collect::<Vec<_>>()
                .join(",\n");
            format!("{{\n{}\n{}}}", body, close)
        }
    }
}

/// Maximum nesting depth for JSON parsing to prevent stack overflow.
const JSON_MAX_DEPTH: usize = 64;

fn json_to_value(s: &str, heap: &mut Heap) -> Result<Value> {
    json_to_value_depth(s, 0, heap)
}

fn json_to_value_depth(s: &str, depth: usize, heap: &mut Heap) -> Result<Value> {
    if depth > JSON_MAX_DEPTH {
        return Err(ZapcodeError::RuntimeError(
            "JSON nesting depth exceeded (max 64)".to_string(),
        ));
    }
    let s = s.trim();
    if s == "null" {
        return Ok(Value::Null);
    }
    if s == "true" {
        return Ok(Value::Bool(true));
    }
    if s == "false" {
        return Ok(Value::Bool(false));
    }
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        let inner = &s[1..s.len() - 1];
        let unescaped = inner
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
            .replace("\\n", "\n")
            .replace("\\t", "\t");
        return Ok(Value::String(Arc::from(unescaped.as_str())));
    }
    if let Ok(n) = s.parse::<i64>() {
        return Ok(Value::Int(n));
    }
    if let Ok(n) = s.parse::<f64>() {
        return Ok(Value::Float(n));
    }
    if s.starts_with('[') {
        return parse_json_array(s, depth, heap);
    }
    if s.starts_with('{') {
        return parse_json_object(s, depth, heap);
    }
    Err(ZapcodeError::RuntimeError(format!("Invalid JSON: {}", s)))
}

fn parse_json_array(s: &str, depth: usize, heap: &mut Heap) -> Result<Value> {
    let inner = &s[1..s.len() - 1].trim();
    if inner.is_empty() {
        return Ok(Value::Array(heap.alloc_array(Vec::new())));
    }
    let mut items = Vec::new();
    for part in split_json_top_level(inner) {
        items.push(json_to_value_depth(part.trim(), depth + 1, heap)?);
    }
    Ok(Value::Array(heap.alloc_array(items)))
}

fn parse_json_object(s: &str, depth: usize, heap: &mut Heap) -> Result<Value> {
    let inner = &s[1..s.len() - 1].trim();
    if inner.is_empty() {
        return Ok(Value::Object(heap.alloc_object(IndexMap::new())));
    }
    let mut map = IndexMap::new();
    for part in split_json_top_level(inner) {
        let part = part.trim();
        if let Some(colon_pos) = find_json_colon(part) {
            let key = part[..colon_pos].trim();
            let val = part[colon_pos + 1..].trim();
            let key = if key.starts_with('"') && key.ends_with('"') {
                &key[1..key.len() - 1]
            } else {
                key
            };
            map.insert(Arc::from(key), json_to_value_depth(val, depth + 1, heap)?);
        }
    }
    Ok(Value::Object(heap.alloc_object(map)))
}

/// Count consecutive backslashes preceding position `i` in `bytes`.
/// A quote is escaped only if preceded by an odd number of backslashes.
fn count_preceding_backslashes(bytes: &[u8], i: usize) -> usize {
    let mut count = 0;
    let mut pos = i;
    while pos > 0 {
        pos -= 1;
        if bytes[pos] == b'\\' {
            count += 1;
        } else {
            break;
        }
    }
    count
}

/// Returns true if the quote at position `i` is NOT escaped.
fn is_unescaped_quote(bytes: &[u8], i: usize) -> bool {
    count_preceding_backslashes(bytes, i).is_multiple_of(2)
}

fn split_json_top_level(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0;
    let mut in_string = false;
    let mut start = 0;
    let bytes = s.as_bytes();

    for i in 0..bytes.len() {
        match bytes[i] {
            b'"' if !in_string => in_string = true,
            b'"' if in_string && is_unescaped_quote(bytes, i) => in_string = false,
            b'[' | b'{' if !in_string => depth += 1,
            b']' | b'}' if !in_string => depth -= 1,
            b',' if !in_string && depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < s.len() {
        parts.push(&s[start..]);
    }
    parts
}

fn find_json_colon(s: &str) -> Option<usize> {
    let mut in_string = false;
    let bytes = s.as_bytes();
    for i in 0..bytes.len() {
        match bytes[i] {
            b'"' if !in_string => in_string = true,
            b'"' if in_string && is_unescaped_quote(bytes, i) => in_string = false,
            b':' if !in_string => return Some(i),
            _ => {}
        }
    }
    None
}

// ── String methods ───────────────────────────────────────────────────

/// Extract `(pattern, flags)` if `v` is a regex literal object `{__regexp__, …}`.
pub fn regexp_parts(v: &Value, heap: &Heap) -> Option<(String, String)> {
    if let Value::Object(h) = v {
        let map = heap.object(*h)?;
        if matches!(map.get("__regexp__"), Some(Value::Bool(true))) {
            let pat = match map.get("pattern") {
                Some(Value::String(s)) => s.to_string(),
                _ => String::new(),
            };
            let flags = match map.get("flags") {
                Some(Value::String(s)) => s.to_string(),
                _ => String::new(),
            };
            return Some((pat, flags));
        }
    }
    None
}

/// Compile a JS-ish regex with the supported flags (i, m, s). The `g` flag is
/// handled by callers (all matches vs first). Lookaround/backreferences aren't
/// supported by the linear-time engine and produce a clear error.
fn compile_regex(pattern: &str, flags: &str) -> Result<regex::Regex> {
    regex::RegexBuilder::new(pattern)
        .case_insensitive(flags.contains('i'))
        .multi_line(flags.contains('m'))
        .dot_matches_new_line(flags.contains('s'))
        .build()
        .map_err(|e| {
            ZapcodeError::RuntimeError(format!("invalid regex /{}/{}: {}", pattern, flags, e))
        })
}

/// Methods on a regex literal: `re.test(str)`, `re.exec(str)`.
pub fn call_regexp_method(
    pattern: &str,
    flags: &str,
    method: &str,
    args: &[Value],
    // Heap handle of the regex object, so /g (and /y) `exec`/`test` can read and
    // advance the mutable `lastIndex` cursor in place (G3). `None` only when the
    // receiver is not a heap object (shouldn't happen for regex literals).
    regex_handle: Option<Handle>,
    heap: &mut Heap,
) -> Result<Option<Value>> {
    let subject = arg_str(args, 0, heap);
    let re = compile_regex(pattern, flags)?;
    // /g and /y are "stateful": exec/test resume from `lastIndex` and advance it.
    let stateful = flags.contains('g') || flags.contains('y');

    // Read the current `lastIndex` (in chars) from the heap slot, if present.
    let read_last_index = |heap: &Heap| -> usize {
        regex_handle
            .and_then(|h| heap.object(h))
            .and_then(|m| m.get("lastIndex"))
            .map(|v| v.to_number().max(0.0) as usize)
            .unwrap_or(0)
    };
    // Write `lastIndex` (in chars) back into the heap slot.
    let write_last_index = |heap: &mut Heap, idx: usize| {
        if let Some(h) = regex_handle {
            if let Some(map) = heap.object_mut(h) {
                map.insert(Arc::from("lastIndex"), Value::Int(idx as i64));
            }
        }
    };
    // Map a char index into a byte offset within `subject`.
    let char_to_byte = |s: &str, char_idx: usize| -> usize {
        s.char_indices().nth(char_idx).map_or(s.len(), |(b, _)| b)
    };

    Ok(match method {
        "test" => {
            if stateful {
                let start_char = read_last_index(heap);
                let start_byte = char_to_byte(&subject, start_char);
                if start_byte > subject.len() {
                    write_last_index(heap, 0);
                    Some(Value::Bool(false))
                } else {
                    match re.find_at(&subject, start_byte) {
                        Some(m) => {
                            let end_char = subject[..m.end()].chars().count();
                            write_last_index(heap, end_char);
                            Some(Value::Bool(true))
                        }
                        None => {
                            write_last_index(heap, 0);
                            Some(Value::Bool(false))
                        }
                    }
                }
            } else {
                Some(Value::Bool(re.is_match(&subject)))
            }
        }
        "exec" => {
            // Determine the byte offset to start matching from.
            let start_byte = if stateful {
                let start_char = read_last_index(heap);
                char_to_byte(&subject, start_char)
            } else {
                0
            };

            let caps_opt = if start_byte > subject.len() {
                None
            } else {
                re.captures_at(&subject, start_byte)
            };

            match caps_opt {
                Some(caps) => {
                    let full = caps.get(0);
                    if stateful {
                        // Advance lastIndex past the whole match so the next call
                        // makes progress (and a zero-width match still terminates).
                        let new_char = match full {
                            Some(m) => {
                                let end = m.end();
                                let end_char = subject[..end].chars().count();
                                if m.start() == end {
                                    end_char + 1
                                } else {
                                    end_char
                                }
                            }
                            None => read_last_index(heap) + 1,
                        };
                        write_last_index(heap, new_char);
                    }
                    let items: Vec<Value> = caps
                        .iter()
                        .map(|c| {
                            c.map(|m| Value::String(Arc::from(m.as_str())))
                                .unwrap_or(Value::Undefined)
                        })
                        .collect();
                    Some(Value::Array(heap.alloc_array(items)))
                }
                None => {
                    // No (further) match: reset the cursor and report exhaustion.
                    if stateful {
                        write_last_index(heap, 0);
                    }
                    Some(Value::Null)
                }
            }
        }
        _ => None,
    })
}

/// Translate JS replacement tokens (`$&`, `$1`, `$$`) to the regex crate's
/// `${0}`/`${1}`/`$` so group substitution works as agents expect.
fn translate_replacement(repl: &str) -> String {
    let mut out = String::new();
    let mut chars = repl.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('&') => {
                chars.next();
                out.push_str("${0}");
            }
            Some('$') => {
                chars.next();
                out.push('$');
            }
            // $<name> -> named capture group.
            Some('<') => {
                chars.next();
                let mut name = String::new();
                for ch in chars.by_ref() {
                    if ch == '>' {
                        break;
                    }
                    name.push(ch);
                }
                out.push_str(&format!("${{{}}}", name));
            }
            Some(d) if d.is_ascii_digit() => {
                let mut num = String::new();
                while let Some(d2) = chars.peek() {
                    if d2.is_ascii_digit() {
                        num.push(*d2);
                        chars.next();
                    } else {
                        break;
                    }
                }
                out.push_str(&format!("${{{}}}", num));
            }
            _ => out.push('$'),
        }
    }
    out
}

/// Build a JS-style match result as an array-like heap object for a single
/// `regex::Captures`. Vec-backed arrays can't carry the extra `.index`,
/// `.input`, and `.groups` properties a JS match result has, so we model the
/// result as an object with integer-string keys `"0".."n"` for the capture
/// groups plus `length`, `index` (match start in *chars*), `input` (subject),
/// and `groups` (object of named captures, or `undefined` if the pattern has
/// none). `m[0]`, `m[1]`, `m.index`, `m.input`, `m.groups.name`, and `m.length`
/// all resolve through normal object key access. `Array.isArray` is therefore
/// `false` for this value — an accepted trade-off (see STRESS-PASS-BUGS.md).
fn alloc_match_result(
    re: &regex::Regex,
    caps: &regex::Captures,
    subject: &Arc<str>,
    heap: &mut Heap,
) -> Value {
    let mut map: IndexMap<Arc<str>, Value> = IndexMap::new();
    // Numbered capture groups, including group 0 (the whole match).
    let len = caps.len();
    for i in 0..len {
        let v = caps
            .get(i)
            .map(|mm| Value::String(Arc::from(mm.as_str())))
            .unwrap_or(Value::Undefined);
        map.insert(Arc::from(i.to_string().as_str()), v);
    }
    map.insert(Arc::from("length"), Value::Int(len as i64));
    // `index`: start of the whole match, in chars (JS uses code-unit/char index,
    // not bytes). Convert the regex crate's byte offset.
    let index_chars = caps
        .get(0)
        .map(|m| subject[..m.start()].chars().count())
        .unwrap_or(0);
    map.insert(Arc::from("index"), Value::Int(index_chars as i64));
    map.insert(Arc::from("input"), Value::String(subject.clone()));
    // Named capture groups -> `groups` object, or `undefined` if the pattern
    // declares no named groups (matching JS semantics).
    let named: Vec<&str> = re.capture_names().flatten().collect();
    let groups_val = if named.is_empty() {
        Value::Undefined
    } else {
        let mut g: IndexMap<Arc<str>, Value> = IndexMap::new();
        for name in named {
            let v = caps
                .name(name)
                .map(|mm| Value::String(Arc::from(mm.as_str())))
                .unwrap_or(Value::Undefined);
            g.insert(Arc::from(name), v);
        }
        Value::Object(heap.alloc_object(g))
    };
    map.insert(Arc::from("groups"), groups_val);
    Value::Object(heap.alloc_object(map))
}

fn call_string_method(
    s: &Arc<str>,
    method: &str,
    args: &[Value],
    limits: &ResourceLimits,
    heap: &mut Heap,
) -> Result<Option<Value>> {
    let result = match method {
        "length" => Value::Int(s.len() as i64),
        "charAt" => {
            let idx = arg_int(args, 0) as usize;
            match s.chars().nth(idx) {
                Some(c) => Value::String(Arc::from(c.to_string().as_str())),
                None => Value::String(Arc::from("")),
            }
        }
        "charCodeAt" => {
            let idx = arg_int(args, 0) as usize;
            match s.chars().nth(idx) {
                Some(c) => Value::Int(c as i64),
                None => Value::Float(f64::NAN),
            }
        }
        "codePointAt" => {
            let idx = arg_int(args, 0).max(0) as usize;
            match s.chars().nth(idx) {
                Some(c) => Value::Int(c as i64),
                None => Value::Undefined,
            }
        }
        "substr" => {
            // substr(start, length); negative start counts from the end.
            let chars: Vec<char> = s.chars().collect();
            let len = chars.len() as i64;
            let raw_start = arg_int(args, 0);
            let start = if raw_start < 0 {
                (len + raw_start).max(0)
            } else {
                raw_start.min(len)
            } as usize;
            let count = match args.get(1) {
                Some(v) if !matches!(v, Value::Undefined) => v.to_number().max(0.0) as usize,
                _ => chars.len() - start,
            };
            let end = (start + count).min(chars.len());
            Value::String(Arc::from(chars[start..end].iter().collect::<String>().as_str()))
        }
        "indexOf" => {
            let search = arg_str(args, 0, heap);
            // Optional fromIndex (in chars). Search the remainder, then map the
            // byte position back to a char index.
            let from = match args.get(1) {
                Some(v) => (v.to_number().max(0.0)) as usize,
                None => 0,
            };
            let byte_start = s.char_indices().nth(from).map_or(s.len(), |(i, _)| i);
            match s[byte_start..].find(&*search) {
                Some(rel) => Value::Int(s[..byte_start + rel].chars().count() as i64),
                None => Value::Int(-1),
            }
        }
        "lastIndexOf" => {
            let search = arg_str(args, 0, heap);
            match s.rfind(&*search) {
                Some(pos) => Value::Int(pos as i64),
                None => Value::Int(-1),
            }
        }
        "includes" => {
            let search = arg_str(args, 0, heap);
            Value::Bool(s.contains(&*search))
        }
        "startsWith" => {
            let search = arg_str(args, 0, heap);
            let pos = args.get(1).map(|v| v.to_number().max(0.0) as usize).unwrap_or(0);
            let byte_start = s.char_indices().nth(pos).map_or(s.len(), |(i, _)| i);
            Value::Bool(s[byte_start..].starts_with(&*search))
        }
        "endsWith" => {
            let search = arg_str(args, 0, heap);
            // The optional end position treats the string as if it were that many
            // characters long.
            let byte_end = match args.get(1) {
                Some(v) if !matches!(v, Value::Undefined) => {
                    let c = v.to_number().max(0.0) as usize;
                    s.char_indices().nth(c).map_or(s.len(), |(i, _)| i)
                }
                _ => s.len(),
            };
            Value::Bool(s[..byte_end].ends_with(&*search))
        }
        "slice" => {
            let len = s.len() as i64;
            let start = normalize_index(arg_int(args, 0), len);
            let end = if args.len() > 1 {
                normalize_index(arg_int(args, 1), len)
            } else {
                len as usize
            };
            if start >= end {
                Value::String(Arc::from(""))
            } else {
                Value::String(Arc::from(&s[start..end.min(s.len())]))
            }
        }
        "substring" => {
            let len = s.len();
            let start = (arg_int(args, 0).max(0) as usize).min(len);
            let end = if args.len() > 1 {
                (arg_int(args, 1).max(0) as usize).min(len)
            } else {
                len
            };
            let (start, end) = if start > end {
                (end, start)
            } else {
                (start, end)
            };
            Value::String(Arc::from(&s[start..end]))
        }
        "toUpperCase" => Value::String(Arc::from(s.to_uppercase().as_str())),
        "toLowerCase" => Value::String(Arc::from(s.to_lowercase().as_str())),
        "localeCompare" => {
            // Codepoint ordering (no locale data); returns -1, 0, or 1.
            let other = arg_str(args, 0, heap);
            let cmp = s.as_ref().cmp(other.as_str());
            Value::Int(match cmp {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            })
        }
        "trim" => Value::String(Arc::from(s.trim())),
        "trimStart" | "trimLeft" => Value::String(Arc::from(s.trim_start())),
        "trimEnd" | "trimRight" => Value::String(Arc::from(s.trim_end())),
        "repeat" => {
            let count = arg_int(args, 0).max(0) as usize;
            let result_len = s.len().saturating_mul(count);
            if result_len > limits.memory_limit_bytes {
                return Err(ZapcodeError::MemoryLimitExceeded(format!(
                    "string repeat result of {} bytes exceeds memory limit of {} bytes",
                    result_len, limits.memory_limit_bytes
                )));
            }
            Value::String(Arc::from(s.repeat(count).as_str()))
        }
        "padStart" => {
            let target_len = arg_int(args, 0).max(0) as usize;
            let pad = if args.len() > 1 {
                arg_str(args, 1, heap)
            } else {
                " ".to_string()
            };
            let current_len = s.len();
            if current_len >= target_len {
                Value::String(s.clone())
            } else {
                let pad_len = target_len - current_len;
                let padding: String = pad.chars().cycle().take(pad_len).collect();
                Value::String(Arc::from(format!("{}{}", padding, s).as_str()))
            }
        }
        "padEnd" => {
            let target_len = arg_int(args, 0).max(0) as usize;
            let pad = if args.len() > 1 {
                arg_str(args, 1, heap)
            } else {
                " ".to_string()
            };
            let current_len = s.len();
            if current_len >= target_len {
                Value::String(s.clone())
            } else {
                let pad_len = target_len - current_len;
                let padding: String = pad.chars().cycle().take(pad_len).collect();
                Value::String(Arc::from(format!("{}{}", s, padding).as_str()))
            }
        }
        "split" => {
            // Optional limit (ToUint32); 0 -> empty result.
            let limit = match args.get(1) {
                None | Some(Value::Undefined) => usize::MAX,
                Some(v) => v.to_number().max(0.0) as usize,
            };
            if limit == 0 {
                return Ok(Some(Value::Array(heap.alloc_array(Vec::new()))));
            }
            if let Some((pat, flags)) = args.first().and_then(|v| regexp_parts(v, heap)) {
                let re = compile_regex(&pat, &flags)?;
                // Splice in capture groups between the surrounding pieces, like JS.
                let mut out: Vec<Value> = Vec::new();
                let mut last = 0usize;
                for caps in re.captures_iter(s) {
                    let m = caps.get(0).unwrap();
                    out.push(Value::String(Arc::from(&s[last..m.start()])));
                    for i in 1..caps.len() {
                        out.push(
                            caps.get(i)
                                .map(|g| Value::String(Arc::from(g.as_str())))
                                .unwrap_or(Value::Undefined),
                        );
                    }
                    last = m.end();
                }
                out.push(Value::String(Arc::from(&s[last..])));
                out.truncate(limit);
                return Ok(Some(Value::Array(heap.alloc_array(out))));
            }
            let separator = arg_str(args, 0, heap);
            let mut parts: Vec<Value> = if separator.is_empty() {
                s.chars()
                    .map(|c| Value::String(Arc::from(c.to_string().as_str())))
                    .collect()
            } else {
                s.split(&*separator)
                    .map(|p| Value::String(Arc::from(p)))
                    .collect()
            };
            parts.truncate(limit);
            Value::Array(heap.alloc_array(parts))
        }
        "replace" => {
            if let Some((pat, flags)) = args.first().and_then(|v| regexp_parts(v, heap)) {
                let re = compile_regex(&pat, &flags)?;
                let repl = translate_replacement(&arg_str(args, 1, heap));
                let out = if flags.contains('g') {
                    re.replace_all(s, repl.as_str())
                } else {
                    re.replace(s, repl.as_str())
                };
                return Ok(Some(Value::String(Arc::from(out.as_ref()))));
            }
            let search = arg_str(args, 0, heap);
            let replacement = arg_str(args, 1, heap);
            Value::String(Arc::from(s.replacen(&*search, &replacement, 1).as_str()))
        }
        "replaceAll" => {
            if let Some((pat, flags)) = args.first().and_then(|v| regexp_parts(v, heap)) {
                let re = compile_regex(&pat, &flags)?;
                let repl = translate_replacement(&arg_str(args, 1, heap));
                let out = re.replace_all(s, repl.as_str());
                return Ok(Some(Value::String(Arc::from(out.as_ref()))));
            }
            let search = arg_str(args, 0, heap);
            let replacement = arg_str(args, 1, heap);
            Value::String(Arc::from(s.replace(&*search, &replacement).as_str()))
        }
        "match" => {
            if let Some((pat, flags)) = args.first().and_then(|v| regexp_parts(v, heap)) {
                let re = compile_regex(&pat, &flags)?;
                if flags.contains('g') {
                    let all: Vec<Value> = re
                        .find_iter(s)
                        .map(|m| Value::String(Arc::from(m.as_str())))
                        .collect();
                    return Ok(Some(if all.is_empty() {
                        Value::Null
                    } else {
                        Value::Array(heap.alloc_array(all))
                    }));
                }
                return Ok(Some(match re.captures(s) {
                    // Non-global match: return an array-like object carrying
                    // `.index`, `.input`, and named `.groups` (G4).
                    Some(caps) => alloc_match_result(&re, &caps, s, heap),
                    None => Value::Null,
                }));
            }
            // Non-regex arg: literal substring (kept for back-compat).
            let pattern = args.first().map(|v| v.to_js_string(heap)).unwrap_or_default();
            match s.find(&pattern) {
                Some(_) => {
                    let items = vec![Value::String(Arc::from(pattern.as_str()))];
                    Value::Array(heap.alloc_array(items))
                }
                None => Value::Null,
            }
        }
        "matchAll" => {
            if let Some((pat, flags)) = args.first().and_then(|v| regexp_parts(v, heap)) {
                let re = compile_regex(&pat, &flags)?;
                // Each yielded result is an array-like object carrying `.index`,
                // `.input`, and named `.groups` (G4). `captures_iter` borrows only
                // `re`/`s`, so we can allocate into the heap as we iterate.
                let mut all: Vec<Value> = Vec::new();
                for caps in re.captures_iter(s) {
                    all.push(alloc_match_result(&re, &caps, s, heap));
                }
                return Ok(Some(Value::Array(heap.alloc_array(all))));
            }
            Value::Array(heap.alloc_array(Vec::new()))
        }
        "search" => {
            if let Some((pat, flags)) = args.first().and_then(|v| regexp_parts(v, heap)) {
                let re = compile_regex(&pat, &flags)?;
                return Ok(Some(match re.find(s) {
                    Some(m) => Value::Int(m.start() as i64),
                    None => Value::Int(-1),
                }));
            }
            let pattern = arg_str(args, 0, heap);
            match s.find(&*pattern) {
                Some(i) => Value::Int(i as i64),
                None => Value::Int(-1),
            }
        }
        "concat" => {
            let mut result = s.to_string();
            for arg in args {
                result.push_str(&arg.to_js_string(heap));
            }
            Value::String(Arc::from(result.as_str()))
        }
        "at" => {
            let idx = arg_int(args, 0);
            let len = s.len() as i64;
            let normalized = if idx < 0 {
                (len + idx).max(0) as usize
            } else {
                idx as usize
            };
            match s.chars().nth(normalized) {
                Some(c) => Value::String(Arc::from(c.to_string().as_str())),
                None => Value::Undefined,
            }
        }
        _ => return Ok(None),
    };
    Ok(Some(result))
}

// ── Array methods ────────────────────────────────────────────────────

/// Resolve an optional `fromIndex` argument (as used by indexOf/includes) into
/// a non-negative start offset. Negative values count from the end.
fn array_from_index(arg: Option<&Value>, len: usize) -> usize {
    match arg {
        None | Some(Value::Undefined) => 0,
        Some(v) => {
            let n = v.to_number();
            if n < 0.0 {
                (len as i64 + n as i64).max(0) as usize
            } else {
                (n as usize).min(len)
            }
        }
    }
}

/// SameValueZero comparison: like `===` but `NaN` equals `NaN`.
fn same_value_zero(a: &Value, b: &Value) -> bool {
    if a.strict_eq(b) {
        return true;
    }
    matches!((a, b), (Value::Float(x), Value::Float(y)) if x.is_nan() && y.is_nan())
}

/// Recursive helper for `Array.prototype.flat(depth)`.
fn flatten_into(arr: &[Value], depth: i64, out: &mut Vec<Value>, heap: &Heap) {
    for item in arr {
        match item {
            Value::Array(inner) if depth > 0 => {
                flatten_into(&heap.array_vec(*inner), depth - 1, out, heap)
            }
            other => out.push(other.clone()),
        }
    }
}

fn call_array_method(
    handle: crate::heap::Handle,
    method: &str,
    args: &[Value],
    _limits: &ResourceLimits,
    heap: &mut Heap,
) -> Result<Option<Value>> {
    // Snapshot the elements for read-only methods. Mutating methods
    // (push/pop/shift/unshift/splice/fill/reverse/copyWithin) edit the heap slot
    // in place via `handle` so the change is visible through every alias.
    let arr = heap.array_vec(handle);
    let arr = arr.as_slice();
    let result = match method {
        "length" => Value::Int(arr.len() as i64),
        "indexOf" => {
            let search = args.first().unwrap_or(&Value::Undefined);
            let from = array_from_index(args.get(1), arr.len());
            let pos = arr
                .iter()
                .enumerate()
                .skip(from)
                .find(|(_, v)| v.strict_eq(search))
                .map(|(i, _)| i);
            Value::Int(pos.map(|p| p as i64).unwrap_or(-1))
        }
        "lastIndexOf" => {
            let search = args.first().unwrap_or(&Value::Undefined);
            let pos = arr.iter().rposition(|v| v.strict_eq(search));
            Value::Int(pos.map(|p| p as i64).unwrap_or(-1))
        }
        "includes" => {
            // includes uses SameValueZero (NaN matches NaN) and honors fromIndex.
            let search = args.first().unwrap_or(&Value::Undefined);
            let from = array_from_index(args.get(1), arr.len());
            Value::Bool(arr.iter().skip(from).any(|v| same_value_zero(v, search)))
        }
        "join" => {
            let sep = if args.is_empty() {
                ",".to_string()
            } else {
                arg_str(args, 0, heap)
            };
            // JS Array.prototype.join renders null/undefined (and holes) as "".
            let joined: Vec<String> = arr
                .iter()
                .map(|v| match v {
                    Value::Null | Value::Undefined => String::new(),
                    _ => v.to_js_string(heap),
                })
                .collect();
            Value::String(Arc::from(joined.join(&sep).as_str()))
        }
        "slice" => {
            let len = arr.len() as i64;
            let start = normalize_index(arg_int(args, 0), len);
            let end = if args.len() > 1 {
                normalize_index(arg_int(args, 1), len)
            } else {
                len as usize
            };
            if start >= end {
                Value::Array(heap.alloc_array(Vec::new()))
            } else {
                Value::Array(heap.alloc_array(arr[start..end.min(arr.len())].to_vec()))
            }
        }
        "concat" => {
            let mut result = arr.to_vec();
            for arg in args {
                match arg {
                    Value::Array(other) => result.extend(heap.array_vec(*other)),
                    other => result.push(other.clone()),
                }
            }
            Value::Array(heap.alloc_array(result))
        }
        "reverse" => {
            // Mutates in place and returns the (same) array.
            if let Some(v) = heap.array_mut(handle) {
                v.reverse();
            }
            Value::Array(handle)
        }
        "flat" => {
            let depth = match args.first() {
                None | Some(Value::Undefined) => 1,
                Some(v) => {
                    let n = v.to_number();
                    if n.is_infinite() && n > 0.0 {
                        i64::MAX
                    } else {
                        n as i64
                    }
                }
            };
            let mut result = Vec::new();
            flatten_into(arr, depth, &mut result, heap);
            Value::Array(heap.alloc_array(result))
        }
        "at" => {
            let idx = arg_int(args, 0);
            let len = arr.len() as i64;
            let normalized = if idx < 0 {
                (len + idx).max(0) as usize
            } else {
                idx as usize
            };
            arr.get(normalized).cloned().unwrap_or(Value::Undefined)
        }
        "fill" => {
            let fill_val = args.first().unwrap_or(&Value::Undefined);
            let len = arr.len();
            let start = if args.len() > 1 {
                normalize_index(arg_int(args, 1), len as i64)
            } else {
                0
            };
            let end = if args.len() > 2 {
                normalize_index(arg_int(args, 2), len as i64)
            } else {
                len
            };
            if let Some(v) = heap.array_mut(handle) {
                for item in v.iter_mut().take(end.min(len)).skip(start) {
                    *item = fill_val.clone();
                }
            }
            Value::Array(handle)
        }
        "push" => {
            if let Some(v) = heap.array_mut(handle) {
                v.extend(args.iter().cloned());
                Value::Int(v.len() as i64)
            } else {
                Value::Int(0)
            }
        }
        "pop" => heap
            .array_mut(handle)
            .and_then(|v| v.pop())
            .unwrap_or(Value::Undefined),
        "shift" => {
            if let Some(v) = heap.array_mut(handle) {
                if v.is_empty() {
                    Value::Undefined
                } else {
                    v.remove(0)
                }
            } else {
                Value::Undefined
            }
        }
        "unshift" => {
            if let Some(v) = heap.array_mut(handle) {
                for (i, a) in args.iter().enumerate() {
                    v.insert(i, a.clone());
                }
                Value::Int(v.len() as i64)
            } else {
                Value::Int(0)
            }
        }
        "splice" => {
            let len = arr.len() as i64;
            let raw_start = if args.is_empty() { 0 } else { arg_int(args, 0) };
            let start = if raw_start < 0 {
                (len + raw_start).max(0) as usize
            } else {
                (raw_start as usize).min(arr.len())
            };
            let delete_count = if args.len() > 1 {
                (arg_int(args, 1).max(0) as usize).min(arr.len() - start)
            } else {
                arr.len() - start
            };
            let inserts: Vec<Value> = if args.len() > 2 {
                args[2..].to_vec()
            } else {
                Vec::new()
            };
            let deleted: Vec<Value> = if let Some(v) = heap.array_mut(handle) {
                v.splice(start..start + delete_count, inserts).collect()
            } else {
                Vec::new()
            };
            Value::Array(heap.alloc_array(deleted))
        }
        "copyWithin" => {
            let len = arr.len() as i64;
            let target = normalize_index(arg_int(args, 0), len);
            let start = if args.len() > 1 {
                normalize_index(arg_int(args, 1), len)
            } else {
                0
            };
            let end = if args.len() > 2 {
                normalize_index(arg_int(args, 2), len)
            } else {
                len as usize
            };
            if let Some(result) = heap.array_mut(handle) {
                let slice: Vec<Value> = result
                    .get(start..end.min(result.len()))
                    .map(|s| s.to_vec())
                    .unwrap_or_default();
                for (offset, val) in slice.into_iter().enumerate() {
                    let dst = target + offset;
                    if dst < result.len() {
                        result[dst] = val;
                    } else {
                        break;
                    }
                }
            }
            Value::Array(handle)
        }
        // Array iterators. JS returns iterator objects; we return plain arrays,
        // which spread (`[...arr.entries()]`) and for-of iterate identically.
        "entries" => {
            let mut out = Vec::with_capacity(arr.len());
            for (i, v) in arr.iter().enumerate() {
                let pair = heap.alloc_array(vec![Value::Int(i as i64), v.clone()]);
                out.push(Value::Array(pair));
            }
            Value::Array(heap.alloc_array(out))
        }
        "keys" => {
            Value::Array(heap.alloc_array((0..arr.len()).map(|i| Value::Int(i as i64)).collect()))
        }
        "values" => Value::Array(heap.alloc_array(arr.to_vec())),
        "every" | "some" | "map" | "filter" | "reduce" | "reduceRight" | "forEach" | "find"
        | "findIndex" | "findLast" | "findLastIndex" | "sort" | "flatMap" => {
            // These require function callbacks — handled in VM dispatch
            return Ok(None);
        }
        _ => return Ok(None),
    };
    Ok(Some(result))
}

// ── Object static methods ────────────────────────────────────────────

fn call_object_method(method: &str, args: &[Value], heap: &mut Heap) -> Result<Option<Value>> {
    let first = args.first().cloned().unwrap_or(Value::Undefined);
    match method {
        "keys" => {
            let keys: Vec<Value> = match &first {
                Value::Object(h) => heap
                    .object(*h)
                    .map(|m| m.keys().map(|k| Value::String(k.clone())).collect())
                    .unwrap_or_default(),
                // Object.keys([...]) yields index strings.
                Value::Array(h) => (0..heap.array(*h).len())
                    .map(|i| Value::String(Arc::from(i.to_string().as_str())))
                    .collect(),
                _ => Vec::new(),
            };
            Ok(Some(Value::Array(heap.alloc_array(keys))))
        }
        "values" => {
            let values: Vec<Value> = match &first {
                Value::Object(h) => heap
                    .object(*h)
                    .map(|m| m.values().cloned().collect())
                    .unwrap_or_default(),
                Value::Array(h) => heap.array_vec(*h),
                _ => Vec::new(),
            };
            Ok(Some(Value::Array(heap.alloc_array(values))))
        }
        "entries" => {
            let pairs: Vec<(Value, Value)> = match &first {
                Value::Object(h) => heap
                    .object(*h)
                    .map(|m| {
                        m.iter()
                            .map(|(k, v)| (Value::String(k.clone()), v.clone()))
                            .collect()
                    })
                    .unwrap_or_default(),
                Value::Array(h) => heap
                    .array(*h)
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        (
                            Value::String(Arc::from(i.to_string().as_str())),
                            v.clone(),
                        )
                    })
                    .collect(),
                _ => Vec::new(),
            };
            let entries: Vec<Value> = pairs
                .into_iter()
                .map(|(k, v)| Value::Array(heap.alloc_array(vec![k, v])))
                .collect();
            Ok(Some(Value::Array(heap.alloc_array(entries))))
        }
        "hasOwn" => {
            let key = args.get(1).map(|v| v.to_js_string(heap)).unwrap_or_default();
            let has = match &first {
                Value::Object(h) => heap
                    .object(*h)
                    .map(|m| m.contains_key(key.as_str()))
                    .unwrap_or(false),
                _ => false,
            };
            Ok(Some(Value::Bool(has)))
        }
        "assign" => {
            let mut target = match &first {
                Value::Object(h) => heap.object_map(*h),
                _ => IndexMap::new(),
            };
            for src in args.iter().skip(1) {
                if let Value::Object(h) = src {
                    for (k, v) in heap.object_map(*h) {
                        target.insert(k, v);
                    }
                }
            }
            // Object.assign mutates and returns the target; write back if it's an object.
            if let Value::Object(h) = &first {
                heap.set_object(*h, target);
                Ok(Some(first))
            } else {
                Ok(Some(Value::Object(heap.alloc_object(target))))
            }
        }
        "fromEntries" => {
            // Object.fromEntries([[k, v], ...]) — inverse of Object.entries.
            let mut obj = IndexMap::new();
            if let Value::Array(ph) = &first {
                for pair in heap.array_vec(*ph) {
                    if let Value::Array(kvh) = pair {
                        let kv = heap.array_vec(kvh);
                        let key = kv.first().cloned().unwrap_or(Value::Undefined);
                        let val = kv.get(1).cloned().unwrap_or(Value::Undefined);
                        let key: Arc<str> = match key {
                            Value::String(s) => s,
                            other => Arc::from(other.to_js_string(heap).as_str()),
                        };
                        obj.insert(key, val);
                    }
                }
            }
            Ok(Some(Value::Object(heap.alloc_object(obj))))
        }
        "freeze" | "seal" => {
            // No-op in sandbox — return object as-is
            Ok(Some(first))
        }
        _ => Ok(None),
    }
}

/// Materialize the iterable/array-like accepted by `Array.from` into a Vec.
/// Handles arrays, strings (by char), built-in Set/Map, and `{ length: n }`
/// array-likes. The optional mapFn is applied by the caller (it may be a
/// guest closure that requires the VM).
pub fn array_from_source(val: &Value, heap: &mut Heap) -> Vec<Value> {
    match val {
        Value::Array(h) => heap.array_vec(*h),
        Value::String(s) => s
            .chars()
            .map(|c| Value::String(Arc::from(c.to_string().as_str())))
            .collect(),
        Value::Object(h) => {
            let map = heap.object_map(*h);
            if matches!(map.get("__set__"), Some(Value::Bool(true))) {
                return match map.get("__items__") {
                    Some(Value::Array(ih)) => heap.array_vec(*ih),
                    _ => Vec::new(),
                };
            }
            if matches!(map.get("__map__"), Some(Value::Bool(true))) {
                return match map.get("__entries__") {
                    Some(Value::Array(eh)) => {
                        let entries = heap.array_vec(*eh);
                        let mut out = Vec::new();
                        for e in entries {
                            if let Value::Object(eo) = e {
                                let em = heap.object_map(eo);
                                let pair = heap.alloc_array(vec![
                                    em.get("key").cloned().unwrap_or(Value::Undefined),
                                    em.get("value").cloned().unwrap_or(Value::Undefined),
                                ]);
                                out.push(Value::Array(pair));
                            }
                        }
                        out
                    }
                    _ => Vec::new(),
                };
            }
            // `{ length: n }` array-like: index into present keys, else undefined.
            match map.get("length") {
                Some(len_val) => {
                    let n = len_val.to_number();
                    if n.is_finite() && n >= 0.0 {
                        (0..n as usize)
                            .map(|i| {
                                map.get(i.to_string().as_str())
                                    .cloned()
                                    .unwrap_or(Value::Undefined)
                            })
                            .collect()
                    } else {
                        Vec::new()
                    }
                }
                None => Vec::new(),
            }
        }
        _ => Vec::new(),
    }
}

fn call_array_static_method(method: &str, args: &[Value], heap: &mut Heap) -> Result<Option<Value>> {
    match method {
        "isArray" => {
            let val = args.first().unwrap_or(&Value::Undefined);
            Ok(Some(Value::Bool(matches!(val, Value::Array(_)))))
        }
        "from" => {
            // Note: the (source, mapFn) form with a closure mapFn is intercepted
            // in the VM Call dispatch (it needs to invoke guest closures).
            let val = args.first().cloned().unwrap_or(Value::Undefined);
            let items = array_from_source(&val, heap);
            Ok(Some(Value::Array(heap.alloc_array(items))))
        }
        "of" => Ok(Some(Value::Array(heap.alloc_array(args.to_vec())))),
        _ => Ok(None),
    }
}

// ── Promise ──────────────────────────────────────────────────────────

/// If a `Promise.{all,allSettled,race,any}(arr)` call's array contains any
/// *deferred single-call promise* (N5 — a bare tool call collected dynamically,
/// e.g. via `.map`), lower the whole call to a `pending_all` batch promise so the
/// `Await` path forces every deferred call through the existing batch machinery
/// (`await_batch`). Each `pending_call` element is replaced by the `Value::Pending`
/// marker `await_batch` understands; non-deferred elements (resolved promises,
/// plain values) pass through unchanged. Returns `None` when no element is a
/// deferred call (so the normal synchronous combinator runs).
fn try_lower_pending_call_batch(method: &str, args: &[Value], heap: &mut Heap) -> Option<Value> {
    let kind = match method {
        "all" => "all",
        "allSettled" => "allSettled",
        "race" => "race",
        "any" => "any",
        _ => return None,
    };
    let arr = match args.first() {
        Some(Value::Array(h)) => heap.array_vec(*h),
        _ => return None,
    };
    let has_pending_call = arr.iter().any(|item| {
        matches!(item, Value::Object(h) if matches!(
            heap.object(*h).and_then(|m| m.get("status")),
            Some(Value::String(s)) if s.as_ref() == "pending_call"
        ))
    });
    if !has_pending_call {
        return None;
    }
    // Replace each deferred single-call promise with its `Value::Pending(id)`.
    let items: Vec<Value> = arr
        .into_iter()
        .map(|item| {
            if let Value::Object(h) = &item {
                let map = heap.object_map(*h);
                let is_pending_call = matches!(
                    map.get("status"),
                    Some(Value::String(s)) if s.as_ref() == "pending_call"
                );
                if is_pending_call {
                    if let Some(Value::Int(id)) = map.get("__call_id__") {
                        return Value::Pending(*id as u64);
                    }
                }
            }
            item
        })
        .collect();
    let items_arr = Value::Array(heap.alloc_array(items));
    let mut obj = IndexMap::new();
    obj.insert(Arc::from("__promise__"), Value::Bool(true));
    obj.insert(Arc::from("status"), Value::String(Arc::from("pending_all")));
    obj.insert(Arc::from("__batch_kind__"), Value::String(Arc::from(kind)));
    obj.insert(Arc::from("items"), items_arr);
    Some(Value::Object(heap.alloc_object(obj)))
}

fn call_promise_method(method: &str, args: &[Value], heap: &mut Heap) -> Result<Option<Value>> {
    // Deferred single-call promises (N5) collected into a dynamic
    // `Promise.{all,allSettled,race,any}(arr)` lower to a batch promise so the
    // host runs all of their calls; this keeps `Promise.all(items.map(f))` working
    // when `f` is a bare tool call. Literal-array combinators of *direct* external
    // calls are already lowered at compile time (`MakeBatchPromise`).
    if let Some(batch) = try_lower_pending_call_batch(method, args, heap) {
        return Ok(Some(batch));
    }
    match method {
        "resolve" => {
            let val = args.first().cloned().unwrap_or(Value::Undefined);
            // If the value is already a promise, return it as-is
            if is_promise(&val, heap) {
                return Ok(Some(val));
            }
            let mut obj = IndexMap::new();
            obj.insert(Arc::from("__promise__"), Value::Bool(true));
            obj.insert(Arc::from("status"), Value::String(Arc::from("resolved")));
            obj.insert(Arc::from("value"), val);
            Ok(Some(Value::Object(heap.alloc_object(obj))))
        }
        "reject" => {
            let reason = args.first().cloned().unwrap_or(Value::Undefined);
            let mut obj = IndexMap::new();
            obj.insert(Arc::from("__promise__"), Value::Bool(true));
            obj.insert(Arc::from("status"), Value::String(Arc::from("rejected")));
            obj.insert(Arc::from("reason"), reason);
            Ok(Some(Value::Object(heap.alloc_object(obj))))
        }
        "all" => {
            // Basic Promise.all: takes an array of resolved promises and returns
            // a resolved promise with an array of their values.
            let arr = match args.first() {
                Some(Value::Array(h)) => heap.array_vec(*h),
                _ => Vec::new(),
            };
            let mut results = Vec::with_capacity(arr.len());
            for item in &arr {
                if is_promise(item, heap) {
                    if let Value::Object(h) = item {
                        let map = heap.object_map(*h);
                        if let Some(Value::String(status)) = map.get("status") {
                            if status.as_ref() == "rejected" {
                                // Promise.all rejects with the first rejection reason
                                return Ok(Some(item.clone()));
                            }
                        }
                        results.push(map.get("value").cloned().unwrap_or(Value::Undefined));
                    }
                } else {
                    results.push(item.clone());
                }
            }
            let value_arr = Value::Array(heap.alloc_array(results));
            let mut obj = IndexMap::new();
            obj.insert(Arc::from("__promise__"), Value::Bool(true));
            obj.insert(Arc::from("status"), Value::String(Arc::from("resolved")));
            obj.insert(Arc::from("value"), value_arr);
            Ok(Some(Value::Object(heap.alloc_object(obj))))
        }
        "allSettled" => {
            let arr = match args.first() {
                Some(Value::Array(h)) => heap.array_vec(*h),
                _ => Vec::new(),
            };
            let mut results: Vec<Value> = Vec::with_capacity(arr.len());
            for item in &arr {
                let mut entry = IndexMap::new();
                let map = match item {
                    Value::Object(h) => heap.object_map(*h),
                    _ => IndexMap::new(),
                };
                if matches!(map.get("status"), Some(Value::String(s)) if s.as_ref() == "rejected") {
                    entry.insert(Arc::from("status"), Value::String(Arc::from("rejected")));
                    entry.insert(
                        Arc::from("reason"),
                        map.get("reason").cloned().unwrap_or(Value::Undefined),
                    );
                    results.push(Value::Object(heap.alloc_object(entry)));
                    continue;
                }
                let value = if is_promise(item, heap) {
                    map.get("value").cloned().unwrap_or(Value::Undefined)
                } else {
                    item.clone()
                };
                entry.insert(Arc::from("status"), Value::String(Arc::from("fulfilled")));
                entry.insert(Arc::from("value"), value);
                results.push(Value::Object(heap.alloc_object(entry)));
            }
            let value_arr = Value::Array(heap.alloc_array(results));
            Ok(Some(make_resolved_promise(value_arr, heap)))
        }
        "race" => {
            // Synchronous model: every input is already settled, so the first
            // element wins. Returns that promise (resolved or rejected).
            let first = match args.first() {
                Some(Value::Array(h)) => heap.array(*h).first().cloned(),
                _ => None,
            };
            match first {
                Some(first) => {
                    if is_promise(&first, heap) {
                        Ok(Some(first))
                    } else {
                        Ok(Some(make_resolved_promise(first, heap)))
                    }
                }
                // An empty array races forever; surface a pending promise.
                None => {
                    let mut obj = IndexMap::new();
                    obj.insert(Arc::from("__promise__"), Value::Bool(true));
                    obj.insert(Arc::from("status"), Value::String(Arc::from("pending")));
                    Ok(Some(Value::Object(heap.alloc_object(obj))))
                }
            }
        }
        "any" => {
            // First fulfilled value wins; if all reject, reject with an
            // AggregateError-shaped object.
            let arr = match args.first() {
                Some(Value::Array(h)) => heap.array_vec(*h),
                _ => Vec::new(),
            };
            let mut errors = Vec::with_capacity(arr.len());
            for item in &arr {
                if is_promise(item, heap) {
                    if let Value::Object(h) = item {
                        let map = heap.object_map(*h);
                        if matches!(map.get("status"), Some(Value::String(s)) if s.as_ref() == "rejected")
                        {
                            errors.push(map.get("reason").cloned().unwrap_or(Value::Undefined));
                        } else {
                            return Ok(Some(make_resolved_promise(
                                map.get("value").cloned().unwrap_or(Value::Undefined),
                                heap,
                            )));
                        }
                    }
                } else {
                    return Ok(Some(make_resolved_promise(item.clone(), heap)));
                }
            }
            let errors_arr = Value::Array(heap.alloc_array(errors));
            let mut agg = IndexMap::new();
            agg.insert(
                Arc::from("name"),
                Value::String(Arc::from("AggregateError")),
            );
            agg.insert(
                Arc::from("message"),
                Value::String(Arc::from("All promises were rejected")),
            );
            agg.insert(Arc::from("errors"), errors_arr);
            let agg_obj = Value::Object(heap.alloc_object(agg));
            let mut obj = IndexMap::new();
            obj.insert(Arc::from("__promise__"), Value::Bool(true));
            obj.insert(Arc::from("status"), Value::String(Arc::from("rejected")));
            obj.insert(Arc::from("reason"), agg_obj);
            Ok(Some(Value::Object(heap.alloc_object(obj))))
        }
        _ => Ok(None),
    }
}

/// Check if a value is a promise object (has __promise__: true).
pub fn is_promise(val: &Value, heap: &Heap) -> bool {
    if let Value::Object(h) = val {
        matches!(
            heap.object(*h).and_then(|m| m.get("__promise__")),
            Some(Value::Bool(true))
        )
    } else {
        false
    }
}

/// Create a resolved promise wrapping the given value.
pub fn make_resolved_promise(val: Value, heap: &mut Heap) -> Value {
    // If the value is already a promise, return it as-is (thenable unwrapping)
    if is_promise(&val, heap) {
        return val;
    }
    let mut obj = IndexMap::new();
    obj.insert(Arc::from("__promise__"), Value::Bool(true));
    obj.insert(Arc::from("status"), Value::String(Arc::from("resolved")));
    obj.insert(Arc::from("value"), val);
    Value::Object(heap.alloc_object(obj))
}

/// Create a rejected promise carrying the given reason.
pub fn make_rejected_promise(reason: Value, heap: &mut Heap) -> Value {
    let mut obj = IndexMap::new();
    obj.insert(Arc::from("__promise__"), Value::Bool(true));
    obj.insert(Arc::from("status"), Value::String(Arc::from("rejected")));
    obj.insert(Arc::from("reason"), reason);
    Value::Object(heap.alloc_object(obj))
}

// ── Helpers ──────────────────────────────────────────────────────────

fn arg_num(args: &[Value], idx: usize) -> f64 {
    args.get(idx).map(|v| v.to_number()).unwrap_or(f64::NAN)
}

fn arg_int(args: &[Value], idx: usize) -> i64 {
    args.get(idx)
        .map(|v| match v {
            Value::Int(n) => *n,
            other => other.to_number() as i64,
        })
        .unwrap_or(0)
}

fn arg_str(args: &[Value], idx: usize, heap: &Heap) -> String {
    args.get(idx)
        .map(|v| v.to_js_string(heap))
        .unwrap_or_default()
}

fn normalize_index(idx: i64, len: i64) -> usize {
    if idx < 0 {
        (len + idx).max(0) as usize
    } else {
        idx as usize
    }
}
