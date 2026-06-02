use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;

use crate::error::{Result, ZapcodeError};
use crate::sandbox::ResourceLimits;
use crate::value::Value;

/// Register built-in global objects and functions.
pub fn register_globals(globals: &mut HashMap<String, Value>) {
    // Register known globals as empty objects — method calls are intercepted by the VM
    globals.insert("console".to_string(), Value::Object(IndexMap::new()));
    globals.insert("JSON".to_string(), Value::Object(IndexMap::new()));
    globals.insert("Object".to_string(), builtin_constructor("Object"));
    globals.insert("Array".to_string(), builtin_constructor("Array"));
    globals.insert("Promise".to_string(), Value::Object(IndexMap::new()));
    globals.insert("Map".to_string(), builtin_constructor("Map"));
    globals.insert("Set".to_string(), builtin_constructor("Set"));
    globals.insert("Date".to_string(), builtin_constructor("Date"));
    for err in [
        "Error",
        "TypeError",
        "RangeError",
        "SyntaxError",
        "ReferenceError",
    ] {
        globals.insert(err.to_string(), builtin_constructor(err));
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
    ] {
        globals.insert(name.to_string(), global_fn(name));
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
    globals.insert("Math".to_string(), Value::Object(math));
}

fn builtin_constructor(name: &str) -> Value {
    let mut obj = IndexMap::new();
    obj.insert(
        Arc::from("__builtin_constructor__"),
        Value::String(Arc::from(name)),
    );
    Value::Object(obj)
}

/// A bare type-conversion function (`String`/`Number`/`Boolean`), represented as
/// an object so it can be both *called* (via the `__global_fn__` marker, handled
/// in the VM's Call instruction) and carry static properties (e.g.
/// `Number.MAX_SAFE_INTEGER`).
fn global_fn(name: &str) -> Value {
    let mut obj = IndexMap::new();
    obj.insert(Arc::from("__global_fn__"), Value::String(Arc::from(name)));
    if name == "String" {
        for (prop, target) in [
            ("fromCharCode", "String.fromCharCode"),
            ("fromCodePoint", "String.fromCodePoint"),
        ] {
            let mut s = IndexMap::new();
            s.insert(
                Arc::from("__global_fn__"),
                Value::String(Arc::from(target)),
            );
            obj.insert(Arc::from(prop), Value::Object(s));
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
            obj.insert(Arc::from(m), global_fn(&format!("Number.{m}")));
        }
    }
    Value::Object(obj)
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
pub fn call_global_fn(kind: &str, args: &[Value]) -> Result<Value> {
    let arg = args.first().cloned().unwrap_or(Value::Undefined);
    Ok(match kind {
        "String" => Value::String(Arc::from(arg.to_js_string().as_str())),
        "Number" => finite_number(arg.to_number()),
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
                other => other.to_js_string(),
            };
            Value::Float(js_parse_int(&s, radix))
        }
        "parseFloat" | "Number.parseFloat" => {
            let s = match &arg {
                Value::String(s) => s.to_string(),
                other => other.to_js_string(),
            };
            Value::Float(js_parse_float(&s))
        }
        "isNaN" => Value::Bool(arg.to_number().is_nan()),
        "isFinite" => Value::Bool(arg.to_number().is_finite()),
        // Values are deep-copied on assignment in this VM, so a clone suffices.
        "structuredClone" => arg,
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
) -> Result<Option<Value>> {
    match object {
        Value::String(s) => call_string_method(s, method, args, limits),
        Value::Array(arr) => call_array_method(arr, method, args, limits),
        _ => Ok(None),
    }
}

/// Execute a global builtin function/method like console.log, Math.floor, JSON.parse.
pub fn call_global_method(
    global_name: &str,
    method: &str,
    args: &[Value],
    stdout: &mut String,
) -> Result<Option<Value>> {
    match global_name {
        "console" => call_console_method(method, args, stdout),
        "Math" => call_math_method(method, args),
        "JSON" => call_json_method(method, args),
        "Object" => call_object_method(method, args),
        "Array" => call_array_static_method(method, args),
        "Promise" => call_promise_method(method, args),
        _ => Ok(None),
    }
}

// ── Console ──────────────────────────────────────────────────────────

fn call_console_method(method: &str, args: &[Value], stdout: &mut String) -> Result<Option<Value>> {
    match method {
        "log" | "info" | "warn" | "error" | "debug" => {
            let output: Vec<String> = args.iter().map(|v| v.to_js_string()).collect();
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

fn call_json_method(method: &str, args: &[Value]) -> Result<Option<Value>> {
    match method {
        "stringify" => {
            let val = args.first().unwrap_or(&Value::Undefined);
            // Third arg `space` enables pretty-printing (number of spaces or a string).
            let indent = match args.get(2) {
                Some(Value::Int(n)) if *n > 0 => Some(" ".repeat((*n).min(10) as usize)),
                Some(Value::Float(n)) if *n >= 1.0 => Some(" ".repeat((*n as usize).min(10))),
                Some(Value::String(s)) if !s.is_empty() => Some(s.to_string()),
                _ => None,
            };
            let json = match indent {
                Some(unit) => value_to_json_pretty(val, &unit, 0),
                None => value_to_json(val),
            };
            Ok(Some(Value::String(Arc::from(json.as_str()))))
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
            let val = json_to_value(&s)?;
            Ok(Some(val))
        }
        _ => Ok(None),
    }
}

fn value_to_json(val: &Value) -> String {
    match val {
        Value::Undefined => "undefined".to_string(),
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(n) => {
            if n.is_nan() || n.is_infinite() {
                "null".to_string()
            } else {
                n.to_string()
            }
        }
        Value::String(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
        Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(value_to_json).collect();
            format!("[{}]", items.join(","))
        }
        Value::Object(map) => {
            let pairs: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("\"{}\":{}", k, value_to_json(v)))
                .collect();
            format!("{{{}}}", pairs.join(","))
        }
        Value::Function(_)
        | Value::BuiltinMethod { .. }
        | Value::Generator(_)
        | Value::Pending(_) => "undefined".to_string(),
    }
}

/// Pretty-printing JSON with an indent unit (the `space` arg of JSON.stringify).
fn value_to_json_pretty(val: &Value, unit: &str, depth: usize) -> String {
    let pad = unit.repeat(depth + 1);
    let close_pad = unit.repeat(depth);
    match val {
        Value::Array(arr) if !arr.is_empty() => {
            let items: Vec<String> = arr
                .iter()
                .map(|v| format!("{}{}", pad, value_to_json_pretty(v, unit, depth + 1)))
                .collect();
            format!("[\n{}\n{}]", items.join(",\n"), close_pad)
        }
        Value::Object(map) if !map.is_empty() => {
            let pairs: Vec<String> = map
                .iter()
                .map(|(k, v)| {
                    format!(
                        "{}\"{}\": {}",
                        pad,
                        k,
                        value_to_json_pretty(v, unit, depth + 1)
                    )
                })
                .collect();
            format!("{{\n{}\n{}}}", pairs.join(",\n"), close_pad)
        }
        // Scalars (and empty containers) render the same as compact JSON.
        other => value_to_json(other),
    }
}

/// Maximum nesting depth for JSON parsing to prevent stack overflow.
const JSON_MAX_DEPTH: usize = 64;

fn json_to_value(s: &str) -> Result<Value> {
    json_to_value_depth(s, 0)
}

fn json_to_value_depth(s: &str, depth: usize) -> Result<Value> {
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
        return parse_json_array(s, depth);
    }
    if s.starts_with('{') {
        return parse_json_object(s, depth);
    }
    Err(ZapcodeError::RuntimeError(format!("Invalid JSON: {}", s)))
}

fn parse_json_array(s: &str, depth: usize) -> Result<Value> {
    let inner = &s[1..s.len() - 1].trim();
    if inner.is_empty() {
        return Ok(Value::Array(Vec::new()));
    }
    let mut items = Vec::new();
    for part in split_json_top_level(inner) {
        items.push(json_to_value_depth(part.trim(), depth + 1)?);
    }
    Ok(Value::Array(items))
}

fn parse_json_object(s: &str, depth: usize) -> Result<Value> {
    let inner = &s[1..s.len() - 1].trim();
    if inner.is_empty() {
        return Ok(Value::Object(IndexMap::new()));
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
            map.insert(Arc::from(key), json_to_value_depth(val, depth + 1)?);
        }
    }
    Ok(Value::Object(map))
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
pub fn regexp_parts(v: &Value) -> Option<(String, String)> {
    if let Value::Object(map) = v {
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
) -> Result<Option<Value>> {
    let subject = arg_str(args, 0);
    let re = compile_regex(pattern, flags)?;
    Ok(match method {
        "test" => Some(Value::Bool(re.is_match(&subject))),
        "exec" => Some(match re.captures(&subject) {
            Some(caps) => Value::Array(
                caps.iter()
                    .map(|c| {
                        c.map(|m| Value::String(Arc::from(m.as_str())))
                            .unwrap_or(Value::Undefined)
                    })
                    .collect(),
            ),
            None => Value::Null,
        }),
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

fn call_string_method(
    s: &Arc<str>,
    method: &str,
    args: &[Value],
    limits: &ResourceLimits,
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
            let search = arg_str(args, 0);
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
            let search = arg_str(args, 0);
            match s.rfind(&*search) {
                Some(pos) => Value::Int(pos as i64),
                None => Value::Int(-1),
            }
        }
        "includes" => {
            let search = arg_str(args, 0);
            Value::Bool(s.contains(&*search))
        }
        "startsWith" => {
            let search = arg_str(args, 0);
            let pos = args.get(1).map(|v| v.to_number().max(0.0) as usize).unwrap_or(0);
            let byte_start = s.char_indices().nth(pos).map_or(s.len(), |(i, _)| i);
            Value::Bool(s[byte_start..].starts_with(&*search))
        }
        "endsWith" => {
            let search = arg_str(args, 0);
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
            let other = arg_str(args, 0);
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
                arg_str(args, 1)
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
                arg_str(args, 1)
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
                return Ok(Some(Value::Array(Vec::new())));
            }
            if let Some((pat, flags)) = args.first().and_then(regexp_parts) {
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
                return Ok(Some(Value::Array(out)));
            }
            let separator = arg_str(args, 0);
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
            Value::Array(parts)
        }
        "replace" => {
            if let Some((pat, flags)) = args.first().and_then(regexp_parts) {
                let re = compile_regex(&pat, &flags)?;
                let repl = translate_replacement(&arg_str(args, 1));
                let out = if flags.contains('g') {
                    re.replace_all(s, repl.as_str())
                } else {
                    re.replace(s, repl.as_str())
                };
                return Ok(Some(Value::String(Arc::from(out.as_ref()))));
            }
            let search = arg_str(args, 0);
            let replacement = arg_str(args, 1);
            Value::String(Arc::from(s.replacen(&*search, &replacement, 1).as_str()))
        }
        "replaceAll" => {
            if let Some((pat, flags)) = args.first().and_then(regexp_parts) {
                let re = compile_regex(&pat, &flags)?;
                let repl = translate_replacement(&arg_str(args, 1));
                let out = re.replace_all(s, repl.as_str());
                return Ok(Some(Value::String(Arc::from(out.as_ref()))));
            }
            let search = arg_str(args, 0);
            let replacement = arg_str(args, 1);
            Value::String(Arc::from(s.replace(&*search, &replacement).as_str()))
        }
        "match" => {
            if let Some((pat, flags)) = args.first().and_then(regexp_parts) {
                let re = compile_regex(&pat, &flags)?;
                if flags.contains('g') {
                    let all: Vec<Value> = re
                        .find_iter(s)
                        .map(|m| Value::String(Arc::from(m.as_str())))
                        .collect();
                    return Ok(Some(if all.is_empty() {
                        Value::Null
                    } else {
                        Value::Array(all)
                    }));
                }
                return Ok(Some(match re.captures(s) {
                    Some(caps) => Value::Array(
                        caps.iter()
                            .map(|c| {
                                c.map(|m| Value::String(Arc::from(m.as_str())))
                                    .unwrap_or(Value::Undefined)
                            })
                            .collect(),
                    ),
                    None => Value::Null,
                }));
            }
            // Non-regex arg: literal substring (kept for back-compat).
            let pattern = args.first().map(|v| v.to_js_string()).unwrap_or_default();
            match s.find(&pattern) {
                Some(_) => Value::Array(vec![Value::String(Arc::from(pattern.as_str()))]),
                None => Value::Null,
            }
        }
        "matchAll" => {
            if let Some((pat, flags)) = args.first().and_then(regexp_parts) {
                let re = compile_regex(&pat, &flags)?;
                let all: Vec<Value> = re
                    .captures_iter(s)
                    .map(|caps| {
                        Value::Array(
                            caps.iter()
                                .map(|c| {
                                    c.map(|m| Value::String(Arc::from(m.as_str())))
                                        .unwrap_or(Value::Undefined)
                                })
                                .collect(),
                        )
                    })
                    .collect();
                return Ok(Some(Value::Array(all)));
            }
            Value::Array(Vec::new())
        }
        "search" => {
            if let Some((pat, flags)) = args.first().and_then(regexp_parts) {
                let re = compile_regex(&pat, &flags)?;
                return Ok(Some(match re.find(s) {
                    Some(m) => Value::Int(m.start() as i64),
                    None => Value::Int(-1),
                }));
            }
            let pattern = arg_str(args, 0);
            match s.find(&*pattern) {
                Some(i) => Value::Int(i as i64),
                None => Value::Int(-1),
            }
        }
        "concat" => {
            let mut result = s.to_string();
            for arg in args {
                result.push_str(&arg.to_js_string());
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

fn call_array_method(
    arr: &[Value],
    method: &str,
    args: &[Value],
    _limits: &ResourceLimits,
) -> Result<Option<Value>> {
    let result = match method {
        "length" => Value::Int(arr.len() as i64),
        "indexOf" => {
            let search = args.first().unwrap_or(&Value::Undefined);
            let pos = arr.iter().position(|v| v.strict_eq(search));
            Value::Int(pos.map(|p| p as i64).unwrap_or(-1))
        }
        "lastIndexOf" => {
            let search = args.first().unwrap_or(&Value::Undefined);
            let pos = arr.iter().rposition(|v| v.strict_eq(search));
            Value::Int(pos.map(|p| p as i64).unwrap_or(-1))
        }
        "includes" => {
            let search = args.first().unwrap_or(&Value::Undefined);
            Value::Bool(arr.iter().any(|v| v.strict_eq(search)))
        }
        "join" => {
            let sep = if args.is_empty() {
                ",".to_string()
            } else {
                arg_str(args, 0)
            };
            // JS Array.prototype.join renders null/undefined (and holes) as "".
            let joined: Vec<String> = arr
                .iter()
                .map(|v| match v {
                    Value::Null | Value::Undefined => String::new(),
                    _ => v.to_js_string(),
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
                Value::Array(Vec::new())
            } else {
                Value::Array(arr[start..end.min(arr.len())].to_vec())
            }
        }
        "concat" => {
            let mut result = arr.to_vec();
            for arg in args {
                match arg {
                    Value::Array(other) => result.extend_from_slice(other),
                    other => result.push(other.clone()),
                }
            }
            Value::Array(result)
        }
        "reverse" => {
            let mut result = arr.to_vec();
            result.reverse();
            Value::Array(result)
        }
        "flat" => {
            let mut result = Vec::new();
            for item in arr {
                match item {
                    Value::Array(inner) => result.extend_from_slice(inner),
                    other => result.push(other.clone()),
                }
            }
            Value::Array(result)
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
            let mut result = arr.to_vec();
            for item in result.iter_mut().take(end.min(len)).skip(start) {
                *item = fill_val.clone();
            }
            Value::Array(result)
        }
        "push" => {
            let new_len = (arr.len() + args.len()) as i64;
            Value::Int(new_len)
        }
        "pop" => arr.last().cloned().unwrap_or(Value::Undefined),
        "shift" => arr.first().cloned().unwrap_or(Value::Undefined),
        "unshift" => {
            let new_len = (arr.len() + args.len()) as i64;
            Value::Int(new_len)
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
            let deleted: Vec<Value> = arr[start..start + delete_count].to_vec();
            Value::Array(deleted)
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
            let mut result = arr.to_vec();
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
            Value::Array(result)
        }
        // Array iterators. JS returns iterator objects; we return plain arrays,
        // which spread (`[...arr.entries()]`) and for-of iterate identically.
        "entries" => Value::Array(
            arr.iter()
                .enumerate()
                .map(|(i, v)| Value::Array(vec![Value::Int(i as i64), v.clone()]))
                .collect(),
        ),
        "keys" => Value::Array((0..arr.len()).map(|i| Value::Int(i as i64)).collect()),
        "values" => Value::Array(arr.to_vec()),
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

fn call_object_method(method: &str, args: &[Value]) -> Result<Option<Value>> {
    match method {
        "keys" => {
            let obj = args.first().unwrap_or(&Value::Undefined);
            match obj {
                Value::Object(map) => {
                    let keys: Vec<Value> = map.keys().map(|k| Value::String(k.clone())).collect();
                    Ok(Some(Value::Array(keys)))
                }
                // Object.keys([...]) yields index strings.
                Value::Array(arr) => Ok(Some(Value::Array(
                    (0..arr.len())
                        .map(|i| Value::String(Arc::from(i.to_string().as_str())))
                        .collect(),
                ))),
                _ => Ok(Some(Value::Array(Vec::new()))),
            }
        }
        "values" => {
            let obj = args.first().unwrap_or(&Value::Undefined);
            match obj {
                Value::Object(map) => {
                    let values: Vec<Value> = map.values().cloned().collect();
                    Ok(Some(Value::Array(values)))
                }
                Value::Array(arr) => Ok(Some(Value::Array(arr.clone()))),
                _ => Ok(Some(Value::Array(Vec::new()))),
            }
        }
        "entries" => {
            let obj = args.first().unwrap_or(&Value::Undefined);
            match obj {
                Value::Object(map) => {
                    let entries: Vec<Value> = map
                        .iter()
                        .map(|(k, v)| Value::Array(vec![Value::String(k.clone()), v.clone()]))
                        .collect();
                    Ok(Some(Value::Array(entries)))
                }
                Value::Array(arr) => Ok(Some(Value::Array(
                    arr.iter()
                        .enumerate()
                        .map(|(i, v)| {
                            Value::Array(vec![
                                Value::String(Arc::from(i.to_string().as_str())),
                                v.clone(),
                            ])
                        })
                        .collect(),
                ))),
                _ => Ok(Some(Value::Array(Vec::new()))),
            }
        }
        "hasOwn" => {
            let has = match args.first() {
                Some(Value::Object(map)) => {
                    let key = args.get(1).map(|v| v.to_js_string()).unwrap_or_default();
                    map.contains_key(key.as_str())
                }
                _ => false,
            };
            Ok(Some(Value::Bool(has)))
        }
        "assign" => {
            let mut target = match args.first() {
                Some(Value::Object(map)) => map.clone(),
                _ => IndexMap::new(),
            };
            for src in args.iter().skip(1) {
                if let Value::Object(map) = src {
                    for (k, v) in map {
                        target.insert(k.clone(), v.clone());
                    }
                }
            }
            Ok(Some(Value::Object(target)))
        }
        "fromEntries" => {
            // Object.fromEntries([[k, v], ...]) — inverse of Object.entries.
            let mut obj = IndexMap::new();
            if let Some(Value::Array(pairs)) = args.first() {
                for pair in pairs {
                    if let Value::Array(kv) = pair {
                        let key = kv.first().cloned().unwrap_or(Value::Undefined);
                        let val = kv.get(1).cloned().unwrap_or(Value::Undefined);
                        let key: Arc<str> = match key {
                            Value::String(s) => s,
                            other => Arc::from(other.to_js_string().as_str()),
                        };
                        obj.insert(key, val);
                    }
                }
            }
            Ok(Some(Value::Object(obj)))
        }
        "freeze" | "seal" => {
            // No-op in sandbox — return object as-is
            Ok(args.first().cloned())
        }
        _ => Ok(None),
    }
}

/// Materialize the iterable/array-like accepted by `Array.from` into a Vec.
/// Handles arrays, strings (by char), built-in Set/Map, and `{ length: n }`
/// array-likes. The optional mapFn is applied by the caller (it may be a
/// guest closure that requires the VM).
pub fn array_from_source(val: &Value) -> Vec<Value> {
    match val {
        Value::Array(arr) => arr.clone(),
        Value::String(s) => s
            .chars()
            .map(|c| Value::String(Arc::from(c.to_string().as_str())))
            .collect(),
        Value::Object(m) if matches!(m.get("__set__"), Some(Value::Bool(true))) => {
            match m.get("__items__") {
                Some(Value::Array(items)) => items.clone(),
                _ => Vec::new(),
            }
        }
        Value::Object(m) if matches!(m.get("__map__"), Some(Value::Bool(true))) => {
            match m.get("__entries__") {
                Some(Value::Array(entries)) => entries
                    .iter()
                    .filter_map(|e| match e {
                        Value::Object(e) => Some(Value::Array(vec![
                            e.get("key").cloned().unwrap_or(Value::Undefined),
                            e.get("value").cloned().unwrap_or(Value::Undefined),
                        ])),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            }
        }
        // `{ length: n }` array-like: index into present keys, else undefined.
        Value::Object(m) => match m.get("length") {
            Some(len_val) => {
                let n = len_val.to_number();
                if n.is_finite() && n >= 0.0 {
                    let len = n as usize;
                    (0..len)
                        .map(|i| {
                            m.get(i.to_string().as_str())
                                .cloned()
                                .unwrap_or(Value::Undefined)
                        })
                        .collect()
                } else {
                    Vec::new()
                }
            }
            None => Vec::new(),
        },
        _ => Vec::new(),
    }
}

fn call_array_static_method(method: &str, args: &[Value]) -> Result<Option<Value>> {
    match method {
        "isArray" => {
            let val = args.first().unwrap_or(&Value::Undefined);
            Ok(Some(Value::Bool(matches!(val, Value::Array(_)))))
        }
        "from" => {
            // Note: the (source, mapFn) form with a closure mapFn is intercepted
            // in the VM Call dispatch (it needs to invoke guest closures).
            let val = args.first().unwrap_or(&Value::Undefined);
            Ok(Some(Value::Array(array_from_source(val))))
        }
        "of" => Ok(Some(Value::Array(args.to_vec()))),
        _ => Ok(None),
    }
}

// ── Promise ──────────────────────────────────────────────────────────

fn call_promise_method(method: &str, args: &[Value]) -> Result<Option<Value>> {
    match method {
        "resolve" => {
            let val = args.first().cloned().unwrap_or(Value::Undefined);
            // If the value is already a promise, return it as-is
            if is_promise(&val) {
                return Ok(Some(val));
            }
            let mut obj = IndexMap::new();
            obj.insert(Arc::from("__promise__"), Value::Bool(true));
            obj.insert(Arc::from("status"), Value::String(Arc::from("resolved")));
            obj.insert(Arc::from("value"), val);
            Ok(Some(Value::Object(obj)))
        }
        "reject" => {
            let reason = args.first().cloned().unwrap_or(Value::Undefined);
            let mut obj = IndexMap::new();
            obj.insert(Arc::from("__promise__"), Value::Bool(true));
            obj.insert(Arc::from("status"), Value::String(Arc::from("rejected")));
            obj.insert(Arc::from("reason"), reason);
            Ok(Some(Value::Object(obj)))
        }
        "all" => {
            // Basic Promise.all: takes an array of resolved promises and returns
            // a resolved promise with an array of their values.
            let arr = match args.first() {
                Some(Value::Array(arr)) => arr.clone(),
                _ => Vec::new(),
            };
            let mut results = Vec::with_capacity(arr.len());
            for item in &arr {
                if is_promise(item) {
                    if let Value::Object(map) = item {
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
            let mut obj = IndexMap::new();
            obj.insert(Arc::from("__promise__"), Value::Bool(true));
            obj.insert(Arc::from("status"), Value::String(Arc::from("resolved")));
            obj.insert(Arc::from("value"), Value::Array(results));
            Ok(Some(Value::Object(obj)))
        }
        "allSettled" => {
            let arr = match args.first() {
                Some(Value::Array(arr)) => arr.clone(),
                _ => Vec::new(),
            };
            let results: Vec<Value> = arr
                .iter()
                .map(|item| {
                    let mut entry = IndexMap::new();
                    if let Value::Object(map) = item {
                        if matches!(map.get("status"), Some(Value::String(s)) if s.as_ref() == "rejected")
                        {
                            entry.insert(Arc::from("status"), Value::String(Arc::from("rejected")));
                            entry.insert(
                                Arc::from("reason"),
                                map.get("reason").cloned().unwrap_or(Value::Undefined),
                            );
                            return Value::Object(entry);
                        }
                    }
                    let value = match item {
                        Value::Object(map) if is_promise(item) => {
                            map.get("value").cloned().unwrap_or(Value::Undefined)
                        }
                        other => other.clone(),
                    };
                    entry.insert(Arc::from("status"), Value::String(Arc::from("fulfilled")));
                    entry.insert(Arc::from("value"), value);
                    Value::Object(entry)
                })
                .collect();
            Ok(Some(make_resolved_promise(Value::Array(results))))
        }
        "race" => {
            // Synchronous model: every input is already settled, so the first
            // element wins. Returns that promise (resolved or rejected).
            match args.first() {
                Some(Value::Array(arr)) if !arr.is_empty() => {
                    let first = &arr[0];
                    if is_promise(first) {
                        Ok(Some(first.clone()))
                    } else {
                        Ok(Some(make_resolved_promise(first.clone())))
                    }
                }
                // An empty array races forever; surface a pending promise.
                _ => {
                    let mut obj = IndexMap::new();
                    obj.insert(Arc::from("__promise__"), Value::Bool(true));
                    obj.insert(Arc::from("status"), Value::String(Arc::from("pending")));
                    Ok(Some(Value::Object(obj)))
                }
            }
        }
        "any" => {
            // First fulfilled value wins; if all reject, reject with an
            // AggregateError-shaped object.
            let arr = match args.first() {
                Some(Value::Array(arr)) => arr.clone(),
                _ => Vec::new(),
            };
            let mut errors = Vec::with_capacity(arr.len());
            for item in &arr {
                match item {
                    Value::Object(map) if is_promise(item) => {
                        if matches!(map.get("status"), Some(Value::String(s)) if s.as_ref() == "rejected")
                        {
                            errors.push(map.get("reason").cloned().unwrap_or(Value::Undefined));
                        } else {
                            return Ok(Some(make_resolved_promise(
                                map.get("value").cloned().unwrap_or(Value::Undefined),
                            )));
                        }
                    }
                    other => return Ok(Some(make_resolved_promise(other.clone()))),
                }
            }
            let mut agg = IndexMap::new();
            agg.insert(
                Arc::from("name"),
                Value::String(Arc::from("AggregateError")),
            );
            agg.insert(
                Arc::from("message"),
                Value::String(Arc::from("All promises were rejected")),
            );
            agg.insert(Arc::from("errors"), Value::Array(errors));
            let mut obj = IndexMap::new();
            obj.insert(Arc::from("__promise__"), Value::Bool(true));
            obj.insert(Arc::from("status"), Value::String(Arc::from("rejected")));
            obj.insert(Arc::from("reason"), Value::Object(agg));
            Ok(Some(Value::Object(obj)))
        }
        _ => Ok(None),
    }
}

/// Check if a value is a promise object (has __promise__: true).
pub fn is_promise(val: &Value) -> bool {
    if let Value::Object(map) = val {
        matches!(map.get("__promise__"), Some(Value::Bool(true)))
    } else {
        false
    }
}

/// Create a resolved promise wrapping the given value.
pub fn make_resolved_promise(val: Value) -> Value {
    // If the value is already a promise, return it as-is (thenable unwrapping)
    if is_promise(&val) {
        return val;
    }
    let mut obj = IndexMap::new();
    obj.insert(Arc::from("__promise__"), Value::Bool(true));
    obj.insert(Arc::from("status"), Value::String(Arc::from("resolved")));
    obj.insert(Arc::from("value"), val);
    Value::Object(obj)
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

fn arg_str(args: &[Value], idx: usize) -> String {
    args.get(idx).map(|v| v.to_js_string()).unwrap_or_default()
}

fn normalize_index(idx: i64, len: i64) -> usize {
    if idx < 0 {
        (len + idx).max(0) as usize
    } else {
        idx as usize
    }
}
