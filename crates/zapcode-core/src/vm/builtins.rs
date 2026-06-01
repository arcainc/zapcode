use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;

use crate::error::{Result, ZapcodeError};
use crate::value::Value;

/// Register built-in global objects and functions.
pub fn register_globals(globals: &mut HashMap<String, Value>) {
    // Register known globals as empty objects — method calls are intercepted by the VM
    globals.insert("console".to_string(), Value::Object(IndexMap::new()));
    globals.insert("JSON".to_string(), Value::Object(IndexMap::new()));
    globals.insert("Object".to_string(), Value::Object(IndexMap::new()));
    globals.insert("Array".to_string(), Value::Object(IndexMap::new()));
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
        obj.insert(Arc::from("fromCharCode"), {
            let mut s = IndexMap::new();
            s.insert(
                Arc::from("__global_fn__"),
                Value::String(Arc::from("String.fromCharCode")),
            );
            Value::Object(s)
        });
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
        for m in ["isInteger", "isNaN", "isFinite", "parseInt", "parseFloat"] {
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
    matches!(name, "toFixed" | "toString" | "toPrecision" | "valueOf")
}

/// Methods on number primitives: `(3.14159).toFixed(2)`, `(255).toString(16)`, …
pub fn call_number_method(n: f64, method: &str, args: &[Value]) -> Result<Option<Value>> {
    let result = match method {
        "toFixed" => {
            let digits = match args.first() {
                Some(v) => v.to_number().clamp(0.0, 100.0) as usize,
                None => 0,
            };
            Value::String(Arc::from(format!("{:.*}", digits, n).as_str()))
        }
        "toPrecision" => match args.first() {
            None => Value::String(Arc::from(format_number(n).as_str())),
            Some(v) => {
                let p = (v.to_number() as usize).clamp(1, 100);
                Value::String(Arc::from(
                    format!("{:.*e}", p.saturating_sub(1), n).as_str(),
                ))
            }
        },
        "toString" => {
            let radix = match args.first() {
                Some(v) => v.to_number() as u32,
                None => 10,
            };
            if radix == 10 || !(2..=36).contains(&radix) {
                Value::String(Arc::from(format_number(n).as_str()))
            } else {
                // Integer radix conversion (JS only does integer part for non-10 here).
                let mut i = n.trunc() as i64;
                if i == 0 {
                    Value::String(Arc::from("0"))
                } else {
                    let neg = i < 0;
                    i = i.abs();
                    let mut digits = Vec::new();
                    while i > 0 {
                        let d = (i % radix as i64) as u32;
                        digits.push(std::char::from_digit(d, radix).unwrap());
                        i /= radix as i64;
                    }
                    if neg {
                        digits.push('-');
                    }
                    let s: String = digits.into_iter().rev().collect();
                    Value::String(Arc::from(s.as_str()))
                }
            }
        }
        "valueOf" => finite_number(n),
        _ => return Ok(None),
    };
    Ok(Some(result))
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
            let radix = match args.get(1) {
                Some(Value::Int(r)) if (2..=36).contains(r) => *r as u32,
                Some(Value::Float(r)) if (2.0..=36.0).contains(r) => *r as u32,
                _ => 10,
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
        "Number.isNaN" => Value::Bool(matches!(arg, Value::Float(n) if n.is_nan())),
        "Number.isFinite" => Value::Bool(
            matches!(arg, Value::Int(_)) || matches!(arg, Value::Float(n) if n.is_finite()),
        ),
        "Number.isInteger" => Value::Bool(match arg {
            Value::Int(_) => true,
            Value::Float(n) => n.is_finite() && n.fract() == 0.0,
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
    _stdout: &mut String,
) -> Result<Option<Value>> {
    match object {
        Value::String(s) => call_string_method(s, method, args),
        Value::Array(arr) => call_array_method(arr, method, args),
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
            let n = arg_num(args, 0);
            Value::Float(n.round())
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

fn call_string_method(s: &Arc<str>, method: &str, args: &[Value]) -> Result<Option<Value>> {
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
            Value::Bool(s.starts_with(&*search))
        }
        "endsWith" => {
            let search = arg_str(args, 0);
            Value::Bool(s.ends_with(&*search))
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
        "trim" => Value::String(Arc::from(s.trim())),
        "trimStart" | "trimLeft" => Value::String(Arc::from(s.trim_start())),
        "trimEnd" | "trimRight" => Value::String(Arc::from(s.trim_end())),
        "repeat" => {
            let count = arg_int(args, 0).max(0) as usize;
            let result_len = s.len().saturating_mul(count);
            if result_len > 10_000_000 {
                return Err(ZapcodeError::AllocationLimitExceeded);
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
            if let Some((pat, flags)) = args.first().and_then(regexp_parts) {
                let re = compile_regex(&pat, &flags)?;
                let parts: Vec<Value> = re.split(s).map(|p| Value::String(Arc::from(p))).collect();
                return Ok(Some(Value::Array(parts)));
            }
            let separator = arg_str(args, 0);
            let parts: Vec<Value> = if separator.is_empty() {
                s.chars()
                    .map(|c| Value::String(Arc::from(c.to_string().as_str())))
                    .collect()
            } else {
                s.split(&*separator)
                    .map(|p| Value::String(Arc::from(p)))
                    .collect()
            };
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

fn call_array_method(arr: &[Value], method: &str, args: &[Value]) -> Result<Option<Value>> {
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
            let joined: Vec<String> = arr.iter().map(|v| v.to_js_string()).collect();
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
        "every" | "some" | "map" | "filter" | "reduce" | "forEach" | "find" | "findIndex"
        | "sort" | "flatMap" => {
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
                _ => Ok(Some(Value::Array(Vec::new()))),
            }
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

fn call_array_static_method(method: &str, args: &[Value]) -> Result<Option<Value>> {
    match method {
        "isArray" => {
            let val = args.first().unwrap_or(&Value::Undefined);
            Ok(Some(Value::Bool(matches!(val, Value::Array(_)))))
        }
        "from" => {
            let val = args.first().unwrap_or(&Value::Undefined);
            match val {
                Value::Array(arr) => Ok(Some(Value::Array(arr.clone()))),
                Value::String(s) => {
                    let chars: Vec<Value> = s
                        .chars()
                        .map(|c| Value::String(Arc::from(c.to_string().as_str())))
                        .collect();
                    Ok(Some(Value::Array(chars)))
                }
                // Array.from(set) / Array.from(map) over the built-in collections.
                Value::Object(m) if matches!(m.get("__set__"), Some(Value::Bool(true))) => {
                    Ok(Some(
                        m.get("__items__")
                            .cloned()
                            .unwrap_or(Value::Array(Vec::new())),
                    ))
                }
                Value::Object(m) if matches!(m.get("__map__"), Some(Value::Bool(true))) => {
                    let pairs = match m.get("__entries__") {
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
                    };
                    Ok(Some(Value::Array(pairs)))
                }
                _ => Ok(Some(Value::Array(Vec::new()))),
            }
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
