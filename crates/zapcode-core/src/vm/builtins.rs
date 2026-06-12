use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;

use crate::error::{Result, ZapcodeError};
use crate::jsstring::JsString;
use crate::heap::{Handle, Heap};
use crate::sandbox::ResourceLimits;
use crate::value::{Value, MAX_RENDER_DEPTH};

/// Stable string property key used to stand in for the well-known
/// `Symbol.iterator`. This crate has no symbol-keyed storage, so `Symbol.iterator`
/// resolves to this sentinel string; a computed key `{ [Symbol.iterator]() {} }`
/// stores its method under this key, and the iteration protocol reads it back.
/// The `__`-prefix follows the crate's internal-key convention so the synthetic
/// key is hidden from `Object.keys`/`for-in`/`JSON.stringify`/`getOwnPropertyNames`
/// (real `Symbol.iterator` is a non-enumerable symbol, not an own string key).
pub const SYMBOL_ITERATOR_KEY: &str = "__@@iterator";

/// EXACT allowlist of reserved internal marker keys. These are VM bookkeeping
/// properties (brands, class metadata, accessor tables, collection backing
/// stores, promise/batch plumbing) that live in the same heap object map as
/// guest data but must never be exposed to the guest as own properties.
///
/// IMPORTANT: this is an exact-match set, NOT a `starts_with("__")` prefix
/// filter. A blanket prefix check silently eats real user keys like `__id__`,
/// `__typename`, or `__v`, which Node treats as ordinary enumerable own
/// properties. Reflection that already exposes `__`-keys (hasOwnProperty,
/// `in`, get-property) stays self-consistent with `Object.keys`/`values`/
/// `entries`, object spread, `getOwnPropertyNames`, and `JSON.stringify`
/// because all of them filter ONLY the keys in this list.
const INTERNAL_MARKER_KEYS: &[&str] = &[
    // Symbol brand and the synthetic `Symbol.iterator` key.
    "__symbol__",
    SYMBOL_ITERATOR_KEY,
    // Error brands.
    "__error__",
    "__error_base__",
    // Class / instance metadata.
    "__class__",
    "__class_name__",
    "__class_chain__",
    "__constructor__",
    "__prototype__",
    "__super__",
    "__builtin_constructor__",
    // Promise-executor capability functions (`new Promise((resolve, reject))`).
    "__promise_capability__",
    "__capability_reject__",
    // Accessor descriptor tables and field initializers.
    "__getters__",
    "__setters__",
    "__static_getters__",
    "__static_setters__",
    "__field_inits__",
    // Object.freeze brand.
    "__frozen__",
    // Object.defineProperty property-attribute tables: key-name lists of
    // non-enumerable / non-writable / non-configurable own properties.
    "__non_enum__",
    "__non_writable__",
    "__non_config__",
    // Date backing value.
    "__date_ms__",
    // Map / Set / RegExp brands and their backing stores.
    "__map__",
    "__set__",
    "__regexp__",
    "__items__",
    "__entries__",
    // Built-in array/Map/Set iterator object (`.keys()/.values()/.entries()`).
    "__array_iterator__",
    "__cursor__",
    // Promise / deferred-batch plumbing.
    "__promise__",
    "__call_id__",
    "__batch_kind__",
    // Global-function thunk marker.
    "__global_fn__",
];

/// True iff `key` must be hidden from guest reflection: either a reserved
/// internal marker (see [`INTERNAL_MARKER_KEYS`]) or a class private field /
/// method, which the parser mangles to a `#`-prefixed key. Private members are
/// excluded from Object.keys/values/entries, for-in, JSON.stringify, and
/// spread, like real JS. Any other key — including user keys that merely start
/// with `__` such as `__id__` — is a real, guest-visible own property.
pub fn is_internal_marker_key(key: &str) -> bool {
    key.starts_with('#') || INTERNAL_MARKER_KEYS.contains(&key)
}

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
    globals.insert("Promise".to_string(), builtin_constructor("Promise", heap));
    globals.insert("Map".to_string(), builtin_constructor("Map", heap));
    globals.insert("Set".to_string(), builtin_constructor("Set", heap));
    globals.insert("WeakMap".to_string(), builtin_constructor("WeakMap", heap));
    globals.insert("WeakSet".to_string(), builtin_constructor("WeakSet", heap));
    globals.insert("RegExp".to_string(), builtin_constructor("RegExp", heap));
    // `Function` is registered as a non-constructible builtin VALUE so that
    // `typeof Function === "function"` and `f instanceof Function` match Node.
    // Actually CALLING `Function(...)` or `new Function(...)` is still rejected
    // (with a catchable sandbox violation) by the VM's Call/Construct handlers.
    globals.insert("Function".to_string(), builtin_constructor("Function", heap));
    globals.insert("Date".to_string(), {
        let mut d = IndexMap::new();
        d.insert(
            Arc::from("__builtin_constructor__"),
            Value::String(JsString::from("Date")),
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
        "BigInt",
        "Boolean",
        "parseInt",
        "parseFloat",
        "isNaN",
        "isFinite",
        "structuredClone",
        // Minimal Symbol factory (O8): callable, typeof === "function", and
        // `Symbol()` yields a unique marker value.
        "Symbol",
        "encodeURIComponent",
        "decodeURIComponent",
        "encodeURI",
        "decodeURI",
        "btoa",
        "atob",
        "setTimeout",
        "clearTimeout",
        "clearInterval",
        "queueMicrotask",
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
    s.insert(Arc::from("__global_fn__"), Value::String(JsString::from(target)));
    Value::Object(heap.alloc_object(s))
}

fn builtin_constructor(name: &str, heap: &mut Heap) -> Value {
    let mut obj = IndexMap::new();
    obj.insert(
        Arc::from("__builtin_constructor__"),
        Value::String(JsString::from(name)),
    );
    Value::Object(heap.alloc_object(obj))
}

/// A bare type-conversion function (`String`/`Number`/`Boolean`), represented as
/// an object so it can be both *called* (via the `__global_fn__` marker, handled
/// in the VM's Call instruction) and carry static properties (e.g.
/// `Number.MAX_SAFE_INTEGER`).
fn global_fn(name: &str, heap: &mut Heap) -> Value {
    let mut obj = IndexMap::new();
    obj.insert(Arc::from("__global_fn__"), Value::String(JsString::from(name)));
    if name == "Symbol" {
        // Well-known `Symbol.iterator`. Real JS exposes a unique symbol;
        // this crate has no symbol-keyed property storage, so the well-known
        // iterator is a SYMBOL-BRANDED object (`typeof` reports "symbol")
        // whose stringification is a stable sentinel key. A computed key
        // `{ [Symbol.iterator]() {} }` stores the method under that string
        // key, and `for...of`/spread/destructure look it up for the protocol.
        let mut iter_sym = IndexMap::new();
        iter_sym.insert(Arc::from("__symbol__"), Value::Bool(true));
        iter_sym.insert(
            Arc::from("__sym_key__"),
            Value::String(JsString::from(SYMBOL_ITERATOR_KEY)),
        );
        iter_sym.insert(
            Arc::from("description"),
            Value::String(JsString::from("Symbol.iterator")),
        );
        obj.insert(
            Arc::from("iterator"),
            Value::Object(heap.alloc_object(iter_sym)),
        );
        // Global symbol registry: Symbol.for(key) / Symbol.keyFor(sym).
        obj.insert(Arc::from("for"), global_fn_marker("Symbol.for", heap));
        obj.insert(Arc::from("keyFor"), global_fn_marker("Symbol.keyFor", heap));
    }
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
        // JS Number.MIN_VALUE is the smallest positive *subnormal* double
        // (5e-324 == f64::from_bits(1)), NOT f64::MIN_POSITIVE which is the
        // smallest *normal* double (~2.2e-308).
        obj.insert(Arc::from("MIN_VALUE"), Value::Float(f64::from_bits(1)));
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
    // JS `parseInt` has no integer range limit: it returns an f64, so a value
    // wider than i64 must not become NaN. On i64 overflow, recover the f64 value
    // (matching the rounding a double parse produces in Node, e.g.
    // `parseInt("9999999999999999999")` -> 1e19, not NaN).
    let n = match i64::from_str_radix(&digits, radix) {
        Ok(n) => n as f64,
        // Base 10 has an exact, correctly-rounded f64 string parser; use it so the
        // result matches Node's mathematical-value-to-double conversion bit for bit.
        Err(_) if radix == 10 => digits.parse::<f64>().unwrap_or(f64::NAN),
        // Other radixes have no string->f64 parser; accumulate digit by digit.
        // (Overflowing a non-decimal radix in `parseInt` is exceedingly rare.)
        Err(_) => {
            let radix_f = radix as f64;
            digits.chars().fold(0.0_f64, |acc, c| {
                // `digits` only contains characters that passed `is_digit(radix)`.
                acc * radix_f + c.to_digit(radix).unwrap_or(0) as f64
            })
        }
    };
    if neg {
        -n
    } else {
        n
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
        "toFixed" | "toString" | "toPrecision" | "toExponential" | "valueOf" | "toLocaleString"
    )
}

/// `Number.prototype.toLocaleString()` with the default (en-US-style) locale:
/// thousands-grouped integer part and up to 3 fractional digits (Intl's default
/// maximumFractionDigits), trailing zeros stripped. The sandbox has no ICU /
/// locale data, so locale/options arguments are ignored and this fixed,
/// deterministic default is used.
fn js_to_locale_string(n: f64) -> String {
    if n.is_nan() {
        return "NaN".to_string();
    }
    if n.is_infinite() {
        return if n > 0.0 { "∞" } else { "-∞" }.to_string();
    }
    // Round half-away-from-zero to 3 fractional digits, like Intl's default.
    let fixed = js_to_fixed(n, 3);
    let (sign, body) = match fixed.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", fixed.as_str()),
    };
    let (int_part, frac_part) = body.split_once('.').unwrap_or((body, ""));
    let frac = frac_part.trim_end_matches('0');
    // Group the integer part in threes from the right: "1234567" -> "1,234,567".
    let bytes = int_part.as_bytes();
    let len = bytes.len();
    let mut out = String::from(sign);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    if !frac.is_empty() {
        out.push('.');
        out.push_str(frac);
    }
    out
}

/// Methods on number primitives: `(3.14159).toFixed(2)`, `(255).toString(16)`, …
pub fn call_number_method(n: f64, method: &str, args: &[Value]) -> Result<Option<Value>> {
    let result = match method {
        "toFixed" => {
            let digits = match args.first() {
                Some(v) => v.to_number().clamp(0.0, 100.0) as usize,
                None => 0,
            };
            Value::String(JsString::from(js_to_fixed(n, digits).as_str()))
        }
        "toPrecision" => match args.first() {
            None | Some(Value::Undefined) => Value::String(JsString::from(format_number(n).as_str())),
            Some(v) => {
                let p = (v.to_number() as usize).clamp(1, 100);
                Value::String(JsString::from(js_to_precision(n, p).as_str()))
            }
        },
        "toExponential" => {
            let digits = match args.first() {
                None | Some(Value::Undefined) => None,
                Some(v) => Some((v.to_number() as usize).min(100)),
            };
            Value::String(JsString::from(js_to_exponential(n, digits).as_str()))
        }
        "toString" => {
            let radix = match args.first() {
                Some(v) => v.to_number() as u32,
                None => 10,
            };
            if radix == 10 || !(2..=36).contains(&radix) {
                Value::String(JsString::from(format_number(n).as_str()))
            } else {
                Value::String(JsString::from(radix_to_string(n, radix).as_str()))
            }
        }
        "valueOf" => finite_number(n),
        "toLocaleString" => Value::String(JsString::from(js_to_locale_string(n).as_str())),
        _ => return Ok(None),
    };
    Ok(Some(result))
}

pub fn is_bigint_method(name: &str) -> bool {
    matches!(name, "toString" | "valueOf" | "toLocaleString")
}

/// Methods on BigInt primitives: `(255n).toString(16)`, `(10n).valueOf()`, …
pub fn call_bigint_method(
    n: &num_bigint::BigInt,
    method: &str,
    args: &[Value],
) -> Result<Option<Value>> {
    let result = match method {
        "toString" => {
            let radix = match args.first() {
                None | Some(Value::Undefined) => 10u32,
                Some(v) => v.to_number() as u32,
            };
            if !(2..=36).contains(&radix) {
                return Err(ZapcodeError::RangeError(
                    "toString() radix must be between 2 and 36".to_string(),
                ));
            }
            Value::String(JsString::from(n.to_str_radix(radix).as_str()))
        }
        "valueOf" => Value::BigInt(n.clone()),
        "toLocaleString" => {
            // No ICU/locale data — group the decimal digits in threes, like the
            // Number default (sign preserved).
            let s = n.to_string();
            let (sign, digits) = match s.strip_prefix('-') {
                Some(rest) => ("-", rest),
                None => ("", s.as_str()),
            };
            let bytes = digits.as_bytes();
            let len = bytes.len();
            let mut out = String::from(sign);
            for (i, b) in bytes.iter().enumerate() {
                if i > 0 && (len - i) % 3 == 0 {
                    out.push(',');
                }
                out.push(*b as char);
            }
            Value::String(JsString::from(out.as_str()))
        }
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
    // toExponential drops the sign of negative zero ((-0).toExponential(1) is
    // "0.0e+0", not "-0.0e+0"); canonicalize -0 to +0 before formatting.
    let n = if n == 0.0 { 0.0 } else { n };
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
pub fn format_number(n: f64) -> String {
    if n.is_nan() {
        "NaN".to_string()
    } else if n.is_infinite() {
        if n > 0.0 {
            "Infinity".to_string()
        } else {
            "-Infinity".to_string()
        }
    } else if n == 0.0 {
        // Covers both +0 and -0: JS ToString(-0) === "0".
        "0".to_string()
    } else if n.fract() == 0.0 && n.abs() < 1e15 {
        // Fast path: small exact integers cast cleanly to i64.
        (n as i64).to_string()
    } else {
        js_number_to_string(n)
    }
}

/// ECMA-262 `Number::toString` for a finite, nonzero `n`.
///
/// Rust's default `f64::to_string`/`Display` round-trips but never uses
/// exponential notation, so it diverges from JS for very large or very small
/// magnitudes (`String(1e21)` must be `"1e+21"`, `String(1e-7)` must be
/// `"1e-7"`). This produces the shortest decimal that round-trips (via Rust's
/// `LowerExp`, whose digit choice matches V8's) and then applies the JS
/// notation rules: fixed-point in `(1e-6, 1e21)`, exponential outside it.
fn js_number_to_string(n: f64) -> String {
    let negative = n < 0.0;
    // `{:e}` yields the shortest round-tripping mantissa, e.g. "1.2345e20",
    // "1e-7". Split into the digit string and the base-10 exponent of the
    // leading digit.
    let exp_form = format!("{:e}", n.abs());
    let (mantissa, exp_part) = exp_form.split_once('e').expect("LowerExp always has 'e'");
    let big_e: i32 = exp_part.parse().expect("LowerExp exponent is a valid integer");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    // ECMA point position `n`: value == digits * 10^(point - k).
    let point = big_e + 1;

    let body = if k <= point && point <= 21 {
        // Integer, possibly with trailing zeros: "12300".
        format!("{}{}", digits, "0".repeat((point - k) as usize))
    } else if 0 < point && point <= 21 {
        // Decimal point inside the digit run: "12.34".
        format!("{}.{}", &digits[..point as usize], &digits[point as usize..])
    } else if -6 < point && point <= 0 {
        // Small magnitude, leading zeros: "0.00123".
        format!("0.{}{}", "0".repeat((-point) as usize), digits)
    } else {
        // Exponential notation: "1.23e+45" / "1e-7".
        let exp = point - 1;
        let sign = if exp >= 0 { "+" } else { "-" };
        let mant = if k == 1 {
            digits
        } else {
            format!("{}.{}", &digits[..1], &digits[1..])
        };
        format!("{}e{}{}", mant, sign, exp.abs())
    };

    if negative {
        format!("-{}", body)
    } else {
        body
    }
}

/// Dispatch a callable bare global / Number static (see `global_fn`).
pub fn call_global_fn(kind: &str, args: &[Value], heap: &mut Heap) -> Result<Value> {
    let arg = args.first().cloned().unwrap_or(Value::Undefined);
    Ok(match kind {
        "String" => Value::String(JsString::from(arg.to_js_string(heap).as_str())),
        // URI codecs (spec-faithful safe sets; agents build query strings
        // constantly). Malformed percent-sequences raise a catchable
        // URIError, like Node.
        "encodeURIComponent" => Value::String(JsString::from(
            uri_encode(&arg.to_js_string(heap), "-_.!~*'()").as_str(),
        )),
        "encodeURI" => Value::String(JsString::from(
            uri_encode(&arg.to_js_string(heap), "-_.!~*'()#$&+,/:;=?@").as_str(),
        )),
        "decodeURIComponent" => {
            Value::String(JsString::from(uri_decode(&arg.to_js_string(heap), "")?.as_str()))
        }
        "decodeURI" => Value::String(JsString::from(
            uri_decode(&arg.to_js_string(heap), "#$&+,/:;=?@")?.as_str(),
        )),
        // Base64 codecs over latin-1 strings (the WHATWG btoa/atob Node also
        // ships). A code point above U+00FF raises, like Node.
        "btoa" => {
            let s = arg.to_js_string(heap);
            let mut bytes = Vec::with_capacity(s.len());
            for ch in s.chars() {
                let c = ch as u32;
                if c > 0xff {
                    return Err(ZapcodeError::RuntimeError(
                        "InvalidCharacterError: btoa accepts latin-1 only".to_string(),
                    ));
                }
                bytes.push(c as u8);
            }
            Value::String(JsString::from(base64_encode(&bytes).as_str()))
        }
        "atob" => {
            let s = arg.to_js_string(heap);
            let bytes = base64_decode(&s).ok_or_else(|| {
                ZapcodeError::RuntimeError(
                    "InvalidCharacterError: atob input is not valid base64".to_string(),
                )
            })?;
            Value::String(JsString::from(
                bytes.iter().map(|b| *b as char).collect::<String>().as_str(),
            ))
        }
        "Number" => finite_number(arg.to_number_heap(heap)),
        // BigInt(x): integers/booleans/numeric strings -> BigInt; a non-integer
        // Number or unparseable string is a RangeError/SyntaxError, like JS.
        "BigInt" => match &arg {
            Value::BigInt(n) => Value::BigInt(n.clone()),
            Value::Int(n) => Value::BigInt(num_bigint::BigInt::from(*n)),
            Value::Bool(b) => Value::BigInt(num_bigint::BigInt::from(*b as i64)),
            Value::Float(f) => {
                if f.is_finite() && f.fract() == 0.0 {
                    match <num_bigint::BigInt as num_traits::FromPrimitive>::from_f64(*f) {
                        Some(v) => Value::BigInt(v),
                        None => return Err(ZapcodeError::RangeError(
                            "The number is not a safe integer".to_string(),
                        )),
                    }
                } else {
                    return Err(ZapcodeError::RangeError(
                        "The number is not a safe integer".to_string(),
                    ));
                }
            }
            other => {
                let s = other.to_js_string(heap);
                let t = s.trim();
                let parsed = if t.is_empty() {
                    Some(num_bigint::BigInt::from(0))
                } else {
                    num_bigint::BigInt::parse_bytes(t.as_bytes(), 10)
                };
                match parsed {
                    Some(v) => Value::BigInt(v),
                    None => return Err(ZapcodeError::TypeError(format!(
                        "Cannot convert {} to a BigInt",
                        s
                    ))),
                }
            }
        },
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
        "structuredClone" => heap.deep_clone(&arg)?,
        "String.fromCharCode" => {
            // Each argument is a UTF-16 code unit (truncated to 16 bits);
            // adjacent surrogates combine into astral chars, and a lone
            // surrogate is preserved (-> a Wtf JsString).
            let units: Vec<u16> = args.iter().map(|v| v.to_number() as u32 as u16).collect();
            Value::String(JsString::from_units(&units))
        }
        "String.fromCodePoint" => {
            // Each argument is a Unicode code point; encode it to UTF-16 units.
            let mut units: Vec<u16> = Vec::new();
            for v in args {
                let cp = v.to_number() as u32;
                match char::from_u32(cp) {
                    Some(c) => {
                        let mut buf = [0u16; 2];
                        units.extend_from_slice(c.encode_utf16(&mut buf));
                    }
                    None => units.push(cp as u16),
                }
            }
            Value::String(JsString::from_units(&units))
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
                    Value::String(JsString::from(arg.to_js_string(heap).as_str())),
                );
            }
            Value::Object(heap.alloc_object(s))
        }
        // `Symbol.for(key)` returns the registered symbol for `key`. We don't keep
        // a persistent registry; instead each registered symbol carries its key in
        // `__symbol_for__`, and `strict_eq` treats two registered symbols with the
        // same key as identical — so `Symbol.for('x') === Symbol.for('x')`.
        "Symbol.for" => {
            let key = arg.to_js_string(heap);
            let mut s = IndexMap::new();
            s.insert(Arc::from("__symbol__"), Value::Bool(true));
            s.insert(
                Arc::from("__symbol_for__"),
                Value::String(JsString::from(key.as_str())),
            );
            s.insert(
                Arc::from("description"),
                Value::String(JsString::from(key.as_str())),
            );
            Value::Object(heap.alloc_object(s))
        }
        // `Symbol.keyFor(sym)` returns a registered symbol's key, else undefined.
        "Symbol.keyFor" => match &arg {
            Value::Object(h) => heap
                .object(*h)
                .and_then(|m| m.get("__symbol_for__").cloned())
                .unwrap_or(Value::Undefined),
            _ => Value::Undefined,
        },
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
        Value::String(s) => call_string_method(s, method, args, limits, heap),
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
            // Per spec, ANY NaN argument poisons the result to NaN.
            let mut max = f64::NEG_INFINITY;
            for arg in args {
                let n = arg.to_number();
                if n.is_nan() {
                    return Ok(Some(Value::Float(f64::NAN)));
                }
                // Treat +0 as greater than -0 (Math.max(-0, +0) === +0).
                if n > max || (n == 0.0 && max == 0.0 && n.is_sign_positive()) {
                    max = n;
                }
            }
            Value::Float(max)
        }
        "min" => {
            // Per spec, ANY NaN argument poisons the result to NaN.
            let mut min = f64::INFINITY;
            for arg in args {
                let n = arg.to_number();
                if n.is_nan() {
                    return Ok(Some(Value::Float(f64::NAN)));
                }
                // Treat -0 as less than +0 (Math.min(-0, +0) === -0).
                if n < min || (n == 0.0 && min == 0.0 && n.is_sign_negative()) {
                    min = n;
                }
            }
            Value::Float(min)
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
        // Inverse hyperbolics (asinh/acosh/atanh).
        "asinh" => Value::Float(arg_num(args, 0).asinh()),
        "acosh" => Value::Float(arg_num(args, 0).acosh()),
        "atanh" => Value::Float(arg_num(args, 0).atanh()),
        // Math.clz32: count leading zeros of the ToUint32 of the argument.
        "clz32" => {
            let n = arg_num(args, 0);
            let u = to_uint32(n);
            Value::Float(u.leading_zeros() as f64)
        }
        // Math.fround: round to the nearest 32-bit float.
        "fround" => {
            let n = arg_num(args, 0);
            Value::Float(n as f32 as f64)
        }
        // Math.imul: 32-bit integer multiplication (wrapping), result as i32.
        "imul" => {
            let a = to_int32(arg_num(args, 0));
            let b = to_int32(arg_num(args, 1));
            Value::Float(a.wrapping_mul(b) as f64)
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
            let mut seen: Vec<Handle> = Vec::new();
            match serialize_json(val, whitelist.as_deref(), indent.as_deref(), 0, &mut seen, heap)? {
                // JSON.stringify(undefined) / of a function returns the value undefined.
                Some(s) => Ok(Some(Value::String(JsString::from(s.as_str())))),
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
pub fn json_escape_string(s: &str) -> String {
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

/// Serialize a value to JSON. Returns `Ok(None)` for values JSON omits
/// (undefined, functions) so callers can drop object properties / emit `null` in
/// arrays. `whitelist` is the array-replacer key filter; `indent` enables pretty
/// output.
///
/// `seen` carries the chain of currently-open Array/Object handles so a reference
/// cycle (`const a = []; a.push(a)`) is detected and reported as the JS-faithful
/// `TypeError: Converting circular structure to JSON` — instead of recursing
/// until the native stack overflows and aborts the host process. A hard
/// [`MAX_RENDER_DEPTH`] cap is also enforced as defense-in-depth so a very deep
/// (acyclic) structure surfaces a catchable error rather than crashing.
fn serialize_json(
    val: &Value,
    whitelist: Option<&[String]>,
    indent: Option<&str>,
    depth: usize,
    seen: &mut Vec<Handle>,
    heap: &Heap,
) -> Result<Option<String>> {
    if depth > MAX_RENDER_DEPTH {
        return Err(ZapcodeError::RuntimeError(format!(
            "JSON nesting depth exceeded (max {})",
            MAX_RENDER_DEPTH
        )));
    }
    match val {
        Value::Undefined
        | Value::Function(_)
        | Value::BuiltinMethod { .. }
        | Value::Generator(_)
        | Value::Pending(_) => Ok(None),
        Value::Null => Ok(Some("null".to_string())),
        Value::Bool(b) => Ok(Some(b.to_string())),
        Value::Int(n) => Ok(Some(n.to_string())),
        Value::Float(n) => Ok(Some(if n.is_finite() {
            format_number(*n)
        } else {
            "null".to_string()
        })),
        // JSON.stringify(bigint) throws a TypeError in JS.
        Value::BigInt(_) => Err(ZapcodeError::TypeError(
            "Do not know how to serialize a BigInt".to_string(),
        )),
        Value::String(s) => Ok(Some(json_escape_string(s))),
        Value::Array(h) => {
            if seen.contains(h) {
                return Err(ZapcodeError::TypeError(
                    "Converting circular structure to JSON".to_string(),
                ));
            }
            seen.push(*h);
            let mut items: Vec<String> = Vec::new();
            for v in heap.array(*h).iter() {
                let rendered = serialize_json(v, whitelist, indent, depth + 1, seen, heap)?
                    .unwrap_or_else(|| "null".to_string());
                items.push(rendered);
            }
            seen.pop();
            Ok(Some(join_json_array(&items, indent, depth)))
        }
        Value::Object(h) => {
            let Some(map) = heap.object(*h) else {
                return Ok(None);
            };
            // Date -> ISO string (matches Date.prototype.toJSON).
            if map.contains_key("__date_ms__") {
                let ms = map.get("__date_ms__").map(|v| v.to_number() as i64).unwrap_or(0);
                return Ok(Some(json_escape_string(&crate::vm::unix_millis_to_iso(ms))));
            }
            // Map/Set/RegExp/Error have no enumerable own data properties in JS.
            if map.contains_key("__map__")
                || map.contains_key("__set__")
                || map.contains_key("__regexp__")
                || map.contains_key("__error__")
            {
                return Ok(Some("{}".to_string()));
            }
            if seen.contains(h) {
                return Err(ZapcodeError::TypeError(
                    "Converting circular structure to JSON".to_string(),
                ));
            }
            seen.push(*h);
            let mut pairs: Vec<(String, String)> = Vec::new();
            for (k, v) in map.iter() {
                if is_internal_marker_key(k) {
                    continue;
                }
                if let Some(w) = whitelist {
                    if !w.iter().any(|x| x == k.as_ref()) {
                        continue;
                    }
                }
                if let Some(s) = serialize_json(v, whitelist, indent, depth + 1, seen, heap)? {
                    pairs.push((k.to_string(), s));
                }
            }
            seen.pop();
            Ok(Some(join_json_object(&pairs, indent, depth)))
        }
    }
}

pub fn join_json_array(items: &[String], indent: Option<&str>, depth: usize) -> String {
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

pub fn join_json_object(pairs: &[(String, String)], indent: Option<&str>, depth: usize) -> String {
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

/// Strict recursive-descent JSON parser matching the ECMA-404 / ES `JSON.parse`
/// grammar. Unlike a lenient `s.parse::<f64>()` fallthrough, this REJECTS the
/// inputs Node rejects — `Infinity`/`NaN`, leading-`+`, leading zeros (`01`), a
/// bare `1.`/`.5`, single-quoted strings, unquoted object keys, and trailing or
/// empty array/object segments — and decodes string escapes with a single
/// left-to-right scan so `"a\\nb"` stays a literal backslash-n (not a newline)
/// and `\uXXXX` is decoded to its code point. Every malformed input returns a
/// catchable `RuntimeError`.
fn json_err(msg: impl Into<String>) -> ZapcodeError {
    ZapcodeError::RuntimeError(format!("Unexpected token in JSON: {}", msg.into()))
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

pub fn json_to_value(s: &str, heap: &mut Heap) -> Result<Value> {
    let mut p = JsonParser {
        bytes: s.as_bytes(),
        pos: 0,
    };
    p.skip_ws();
    let value = p.parse_value(0, heap)?;
    p.skip_ws();
    if p.pos != p.bytes.len() {
        return Err(json_err("trailing characters after JSON value"));
    }
    Ok(value)
}

impl<'a> JsonParser<'a> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    /// JSON insignificant whitespace: space, tab, LF, CR only.
    fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn parse_value(&mut self, depth: usize, heap: &mut Heap) -> Result<Value> {
        if depth > JSON_MAX_DEPTH {
            return Err(ZapcodeError::RuntimeError(
                "JSON nesting depth exceeded (max 64)".to_string(),
            ));
        }
        match self.peek() {
            Some(b'{') => self.parse_object(depth, heap),
            Some(b'[') => self.parse_array(depth, heap),
            Some(b'"') => {
                let s = self.parse_string()?;
                Ok(Value::String(JsString::from(s.as_str())))
            }
            Some(b't') => {
                self.expect_literal("true")?;
                Ok(Value::Bool(true))
            }
            Some(b'f') => {
                self.expect_literal("false")?;
                Ok(Value::Bool(false))
            }
            Some(b'n') => {
                self.expect_literal("null")?;
                Ok(Value::Null)
            }
            Some(b'-') | Some(b'0'..=b'9') => self.parse_number(),
            Some(c) => Err(json_err(format!("unexpected character `{}`", c as char))),
            None => Err(json_err("unexpected end of input")),
        }
    }

    fn expect_literal(&mut self, lit: &str) -> Result<()> {
        let lb = lit.as_bytes();
        if self.bytes[self.pos..].starts_with(lb) {
            self.pos += lb.len();
            Ok(())
        } else {
            Err(json_err(format!("expected `{}`", lit)))
        }
    }

    /// Strict JSON number grammar: optional `-`, an int part that is either `0`
    /// or a nonzero digit followed by digits (no leading zeros, no leading `+`),
    /// an optional `.` followed by at least one digit, and an optional exponent
    /// `[eE][+-]?` followed by at least one digit. `Infinity`/`NaN`/`1.`/`.5`
    /// are all rejected by construction.
    fn parse_number(&mut self) -> Result<Value> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => {
                self.pos += 1;
            }
            Some(b'1'..=b'9') => {
                self.pos += 1;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            _ => return Err(json_err("invalid number")),
        }
        let mut is_float = false;
        if self.peek() == Some(b'.') {
            is_float = true;
            self.pos += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(json_err("invalid number: digit expected after `.`"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            is_float = true;
            self.pos += 1;
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(json_err("invalid number: digit expected in exponent"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        // The slice is pure ASCII digits/`-`/`.`/`eE+-`, so it is valid UTF-8.
        let text = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap();
        if !is_float {
            if let Ok(n) = text.parse::<i64>() {
                return Ok(Value::Int(n));
            }
        }
        match text.parse::<f64>() {
            Ok(n) => Ok(Value::Float(n)),
            Err(_) => Err(json_err("invalid number")),
        }
    }

    /// Parse a double-quoted JSON string, decoding escapes left-to-right.
    /// Assumes the cursor is on the opening `"`.
    fn parse_string(&mut self) -> Result<String> {
        debug_assert_eq!(self.peek(), Some(b'"'));
        self.pos += 1; // opening quote
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err(json_err("unterminated string")),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    match self.peek() {
                        Some(b'"') => {
                            out.push('"');
                            self.pos += 1;
                        }
                        Some(b'\\') => {
                            out.push('\\');
                            self.pos += 1;
                        }
                        Some(b'/') => {
                            out.push('/');
                            self.pos += 1;
                        }
                        Some(b'n') => {
                            out.push('\n');
                            self.pos += 1;
                        }
                        Some(b'r') => {
                            out.push('\r');
                            self.pos += 1;
                        }
                        Some(b't') => {
                            out.push('\t');
                            self.pos += 1;
                        }
                        Some(b'b') => {
                            out.push('\u{0008}');
                            self.pos += 1;
                        }
                        Some(b'f') => {
                            out.push('\u{000C}');
                            self.pos += 1;
                        }
                        Some(b'u') => {
                            self.pos += 1;
                            let cp = self.parse_hex4()?;
                            // Combine a high surrogate with a following low
                            // surrogate into a single code point (matching JS).
                            if (0xD800..=0xDBFF).contains(&cp)
                                && self.bytes[self.pos..].starts_with(b"\\u")
                            {
                                let save = self.pos;
                                self.pos += 2;
                                let lo = self.parse_hex4()?;
                                if (0xDC00..=0xDFFF).contains(&lo) {
                                    let c = 0x10000
                                        + ((cp - 0xD800) << 10)
                                        + (lo - 0xDC00);
                                    out.push(char::from_u32(c).unwrap_or('\u{FFFD}'));
                                } else {
                                    // Not a low surrogate: emit replacement for the
                                    // lone high surrogate and rewind to reparse `lo`.
                                    out.push('\u{FFFD}');
                                    self.pos = save;
                                }
                            } else {
                                out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                            }
                        }
                        _ => return Err(json_err("invalid escape sequence")),
                    }
                }
                Some(b) if b < 0x20 => {
                    return Err(json_err("unescaped control character in string"));
                }
                Some(_) => {
                    // Copy one full UTF-8 scalar verbatim.
                    let rest = &self.bytes[self.pos..];
                    let s = std::str::from_utf8(rest)
                        .map_err(|_| json_err("invalid UTF-8 in string"))?;
                    let ch = s.chars().next().unwrap();
                    out.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
    }

    fn parse_hex4(&mut self) -> Result<u32> {
        if self.pos + 4 > self.bytes.len() {
            return Err(json_err("incomplete \\u escape"));
        }
        let mut cp = 0u32;
        for _ in 0..4 {
            let d = (self.bytes[self.pos] as char)
                .to_digit(16)
                .ok_or_else(|| json_err("invalid \\u hex digit"))?;
            cp = cp * 16 + d;
            self.pos += 1;
        }
        Ok(cp)
    }

    fn parse_array(&mut self, depth: usize, heap: &mut Heap) -> Result<Value> {
        self.pos += 1; // `[`
        self.skip_ws();
        let mut items = Vec::new();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Value::Array(heap.alloc_array(items)));
        }
        loop {
            self.skip_ws();
            items.push(self.parse_value(depth + 1, heap)?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    // A trailing comma (`,]`) is rejected: the next parse_value
                    // would fail, but check explicitly for a clearer error.
                    self.skip_ws();
                    if self.peek() == Some(b']') {
                        return Err(json_err("trailing comma in array"));
                    }
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Value::Array(heap.alloc_array(items)));
                }
                _ => return Err(json_err("expected `,` or `]` in array")),
            }
        }
    }

    fn parse_object(&mut self, depth: usize, heap: &mut Heap) -> Result<Value> {
        self.pos += 1; // `{`
        self.skip_ws();
        let mut map: IndexMap<Arc<str>, Value> = IndexMap::new();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Value::Object(heap.alloc_object(map)));
        }
        loop {
            self.skip_ws();
            // Keys MUST be double-quoted strings.
            if self.peek() != Some(b'"') {
                return Err(json_err("expected double-quoted property name"));
            }
            let key = self.parse_string()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(json_err("expected `:` after property name"));
            }
            self.pos += 1;
            self.skip_ws();
            let value = self.parse_value(depth + 1, heap)?;
            map.insert(Arc::from(key.as_str()), value);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_ws();
                    if self.peek() == Some(b'}') {
                        return Err(json_err("trailing comma in object"));
                    }
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Value::Object(heap.alloc_object(map)));
                }
                _ => return Err(json_err("expected `,` or `}` in object")),
            }
        }
    }
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

/// Rewrite the ASCII shorthand classes (`\d \D \w \W \b \B`) to their explicit
/// ASCII forms. The `regex` crate is Unicode-by-default, so a bare `\d`/`\w`
/// matches Unicode digits/word chars (e.g. Arabic-Indic digits, accented
/// letters) — JS shorthands are ASCII-only. Without this, ID/ZIP/slug
/// validation built from `\d`/`\w` silently accepts non-ASCII input.
///
/// Translation (matching real Node):
///   - outside a char class: `\d`→`[0-9]`, `\D`→`[^0-9]`,
///     `\w`→`[0-9A-Za-z_]`, `\W`→`[^0-9A-Za-z_]`,
///     `\b`/`\B`→`(?-u:\b)`/`(?-u:\B)` (word-boundary on the ASCII word set);
///   - inside a char class `[...]`: `\d`→`0-9`, `\w`→`0-9A-Za-z_` (bare members,
///     no nested brackets), and `\D`/`\W`→nested negated classes `[^0-9]` /
///     `[^0-9A-Za-z_]` (the crate unions class members, matching JS). Inside a
///     class, `\b` is a backspace, so it is left untouched.
/// All other escapes (`\s \S \n \. \\ \uXXXX` …) and the rest of the grammar are
/// passed through verbatim.
fn rewrite_ascii_shorthands(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars().peekable();
    let mut in_class = false;
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                // Look at the escaped character without consuming non-shorthands.
                match chars.peek().copied() {
                    Some('d') => {
                        chars.next();
                        out.push_str(if in_class { "0-9" } else { "[0-9]" });
                    }
                    Some('w') => {
                        chars.next();
                        out.push_str(if in_class { "0-9A-Za-z_" } else { "[0-9A-Za-z_]" });
                    }
                    // `\D`/`\W` map to a negated ASCII class. Inside a class the
                    // crate accepts a *nested* negated class (`[a[^0-9]]`) and
                    // unions its members, matching JS `[a\D]` semantics.
                    Some('D') => {
                        chars.next();
                        out.push_str("[^0-9]");
                    }
                    Some('W') => {
                        chars.next();
                        out.push_str("[^0-9A-Za-z_]");
                    }
                    Some('b') if !in_class => {
                        chars.next();
                        out.push_str("(?-u:\\b)");
                    }
                    Some('B') if !in_class => {
                        chars.next();
                        out.push_str("(?-u:\\B)");
                    }
                    // Any other escape (incl. `\b` inside a class, where it is a
                    // backspace, and `\s \S \n \. \\` …) is preserved exactly,
                    // escaped char and all, so the next iteration doesn't
                    // re-interpret it.
                    Some(next) => {
                        chars.next();
                        out.push('\\');
                        out.push(next);
                    }
                    None => out.push('\\'),
                }
            }
            '[' if !in_class => {
                in_class = true;
                out.push('[');
            }
            ']' if in_class => {
                in_class = false;
                out.push(']');
            }
            other => out.push(other),
        }
    }
    out
}

/// Compile a JS-ish regex with the supported flags (i, m, s). The `g` flag is
/// handled by callers (all matches vs first). Lookaround/backreferences aren't
/// supported by the linear-time engine and produce a clear error. ASCII
/// shorthand classes are rewritten first so they match JS (ASCII) rather than
/// the crate's Unicode defaults — see `rewrite_ascii_shorthands`.
pub fn compile_regex(pattern: &str, flags: &str) -> Result<regex::Regex> {
    let rewritten = rewrite_ascii_shorthands(pattern);
    regex::RegexBuilder::new(&rewritten)
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
    let sticky = flags.contains('y');
    let stateful = flags.contains('g') || sticky;

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
    // Map a UTF-16 code-unit index (JS `lastIndex` is in code units) into a byte
    // offset within `subject`. For BMP text a unit index equals a char index; an
    // index landing inside an astral char's surrogate pair maps to that char.
    let char_to_byte = |s: &str, unit_idx: usize| -> usize {
        let mut units = 0usize;
        for (b, c) in s.char_indices() {
            if units >= unit_idx {
                return b;
            }
            units += c.len_utf16();
        }
        s.len()
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
                    // For the sticky flag `y`, the match must START exactly at
                    // `lastIndex` (it anchors, it does not scan forward). The
                    // crate's `find_at` returns the leftmost match at-or-after
                    // the offset, so we reject any match that starts later.
                    match re.find_at(&subject, start_byte) {
                        Some(m) if !sticky || m.start() == start_byte => {
                            let end_char = subject[..m.end()].encode_utf16().count();
                            write_last_index(heap, end_char);
                            Some(Value::Bool(true))
                        }
                        _ => {
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

            // For the sticky flag `y`, the match must START exactly at
            // `lastIndex`; `captures_at` would otherwise scan forward. Drop a
            // match that begins past the offset so the result matches Node.
            let caps_opt = match caps_opt {
                Some(caps)
                    if sticky
                        && caps.get(0).map(|m| m.start()) != Some(start_byte) =>
                {
                    None
                }
                other => other,
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
                                let end_char = subject[..end].encode_utf16().count();
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
                    // Return the rich match-result object (same shape as
                    // `match()`'s non-global result): integer-string keys for the
                    // capture groups plus `length`, `index`, `input`, and `groups`
                    // (named captures). `m[0]`, `m[1]`, `m.index`, `m.input`, and
                    // `m.groups.name` all resolve through object key access.
                    let subject_arc: Arc<str> = Arc::from(subject.as_str());
                    Some(alloc_match_result(&re, &caps, &subject_arc, heap))
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

/// Expand JS string-replacement tokens for a *string-search* replace (no capture
/// groups): `$$` -> `$`, `$&` -> the matched substring, `` $` `` -> the portion
/// before the match, `$'` -> the portion after the match. A `$n` (digits) and any
/// other `$x` are left literal, exactly like `String.prototype.replace` with a
/// plain-string search value (which has no captures to reference).
fn expand_string_replacement(repl: &str, whole: &str, match_start: usize, match_len: usize) -> String {
    if !repl.contains('$') {
        return repl.to_string();
    }
    let matched = &whole[match_start..match_start + match_len];
    let before = &whole[..match_start];
    let after = &whole[match_start + match_len..];
    let mut out = String::with_capacity(repl.len());
    let mut chars = repl.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('$') => {
                chars.next();
                out.push('$');
            }
            Some('&') => {
                chars.next();
                out.push_str(matched);
            }
            Some('`') => {
                chars.next();
                out.push_str(before);
            }
            Some('\'') => {
                chars.next();
                out.push_str(after);
            }
            // `$n` / `$<name>` and any other `$x`: literal (no captures here).
            _ => out.push('$'),
        }
    }
    out
}

/// Translate JS replacement tokens (`$&`, `$1`, `$$`) to the regex crate's
/// `${0}`/`${1}`/`$` so group substitution works as agents expect. `group_count`
/// is the number of capture groups *including* group 0 (i.e. `re.captures_len()`);
/// a `$n` that references a group `>= group_count` is left literal, matching JS
/// (`"abc".replace(/b/, "$1")` keeps `$1` because `/b/` has no group 1). To skip
/// the range check (translate every `$n`), pass `usize::MAX`.
pub fn translate_replacement(repl: &str, group_count: usize) -> String {
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
                // A group reference past the pattern's groups stays literal (JS).
                // Emit `$$` (the regex crate's literal-`$` escape) so the crate's
                // own replacer renders a literal `$n` rather than re-interpreting it.
                let in_range = num
                    .parse::<usize>()
                    .map(|n| n < group_count)
                    .unwrap_or(false);
                if in_range {
                    out.push_str(&format!("${{{}}}", num));
                } else {
                    out.push_str("$$");
                    out.push_str(&num);
                }
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
    subject: &str,
    heap: &mut Heap,
) -> Value {
    let mut map: IndexMap<Arc<str>, Value> = IndexMap::new();
    // Numbered capture groups, including group 0 (the whole match).
    let len = caps.len();
    for i in 0..len {
        let v = caps
            .get(i)
            .map(|mm| Value::String(JsString::from(mm.as_str())))
            .unwrap_or(Value::Undefined);
        map.insert(Arc::from(i.to_string().as_str()), v);
    }
    map.insert(Arc::from("length"), Value::Int(len as i64));
    // `index`: start of the whole match, in chars (JS uses code-unit/char index,
    // not bytes). Convert the regex crate's byte offset.
    let index_chars = caps
        .get(0)
        .map(|m| subject[..m.start()].encode_utf16().count())
        .unwrap_or(0);
    map.insert(Arc::from("index"), Value::Int(index_chars as i64));
    map.insert(Arc::from("input"), Value::String(subject.clone().into()));
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
                .map(|mm| Value::String(JsString::from(mm.as_str())))
                .unwrap_or(Value::Undefined);
            g.insert(Arc::from(name), v);
        }
        Value::Object(heap.alloc_object(g))
    };
    map.insert(Arc::from("groups"), groups_val);
    Value::Object(heap.alloc_object(map))
}

/// First index `>= from` at which `needle` occurs in `haystack` (UTF-16 code
/// units). An empty needle matches at `from` (clamped), like JS `indexOf`.
fn find_units(haystack: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    let from = from.min(haystack.len());
    if needle.is_empty() {
        return Some(from);
    }
    if needle.len() > haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// Last index `<= max_start` at which `needle` occurs in `haystack` (UTF-16).
fn rfind_units(haystack: &[u16], needle: &[u16], max_start: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(max_start.min(haystack.len()));
    }
    if needle.len() > haystack.len() {
        return None;
    }
    let limit = max_start.min(haystack.len() - needle.len());
    (0..=limit).rev().find(|&i| &haystack[i..i + needle.len()] == needle)
}

fn call_string_method(
    s: &JsString,
    method: &str,
    args: &[Value],
    limits: &ResourceLimits,
    heap: &mut Heap,
) -> Result<Option<Value>> {
    let result = match method {
        // All position/index string operations are indexed by UTF-16 code unit
        // (JS semantics): a non-BMP char (e.g. an emoji) is two units, and
        // `charAt`/`slice` can produce a lone surrogate (-> a `Wtf` JsString).
        "length" => Value::Int(s.len_utf16() as i64),
        "charAt" => {
            let idx = arg_int(args, 0);
            let units = s.units();
            if idx < 0 || idx as usize >= units.len() {
                Value::String(JsString::from(""))
            } else {
                let i = idx as usize;
                Value::String(JsString::from_units(&units[i..i + 1]))
            }
        }
        "charCodeAt" => {
            let idx = arg_int(args, 0);
            let units = s.units();
            if idx < 0 || idx as usize >= units.len() {
                Value::Float(f64::NAN)
            } else {
                Value::Int(units[idx as usize] as i64)
            }
        }
        "codePointAt" => {
            let idx = arg_int(args, 0);
            let units = s.units();
            if idx < 0 || idx as usize >= units.len() {
                Value::Undefined
            } else {
                let i = idx as usize;
                let u = units[i];
                // Combine a high+low surrogate pair into the astral code point.
                let cp = if (0xD800..=0xDBFF).contains(&u)
                    && i + 1 < units.len()
                    && (0xDC00..=0xDFFF).contains(&units[i + 1])
                {
                    0x10000 + (((u as u32 - 0xD800) << 10) | (units[i + 1] as u32 - 0xDC00))
                } else {
                    u as u32
                };
                Value::Int(cp as i64)
            }
        }
        "substr" => {
            // substr(start, length); negative start counts from the end.
            let units = s.units();
            let len = units.len() as i64;
            let raw_start = arg_int(args, 0);
            let start = if raw_start < 0 {
                (len + raw_start).max(0)
            } else {
                raw_start.min(len)
            } as usize;
            let count = match args.get(1) {
                Some(v) if !matches!(v, Value::Undefined) => v.to_number().max(0.0) as usize,
                _ => units.len() - start,
            };
            let end = (start + count).min(units.len());
            Value::String(JsString::from_units(&units[start..end]))
        }
        "indexOf" => {
            let needle: Vec<u16> = arg_str(args, 0, heap).encode_utf16().collect();
            let units = s.units();
            let from = match args.get(1) {
                Some(v) => v.to_number().max(0.0) as usize,
                None => 0,
            };
            Value::Int(find_units(&units, &needle, from).map_or(-1, |p| p as i64))
        }
        "lastIndexOf" => {
            let needle: Vec<u16> = arg_str(args, 0, heap).encode_utf16().collect();
            let units = s.units();
            let max_start = match args.get(1) {
                Some(v) if !matches!(v, Value::Undefined) => {
                    let n = v.to_number();
                    if n.is_nan() {
                        units.len()
                    } else {
                        (n.max(0.0) as usize).min(units.len())
                    }
                }
                _ => units.len(),
            };
            Value::Int(rfind_units(&units, &needle, max_start).map_or(-1, |p| p as i64))
        }
        "includes" => {
            let needle: Vec<u16> = arg_str(args, 0, heap).encode_utf16().collect();
            let units = s.units();
            let from = match args.get(1) {
                Some(v) if !matches!(v, Value::Undefined) => v.to_number().max(0.0) as usize,
                _ => 0,
            };
            Value::Bool(find_units(&units, &needle, from).is_some())
        }
        "startsWith" => {
            let needle: Vec<u16> = arg_str(args, 0, heap).encode_utf16().collect();
            let units = s.units();
            let pos = args.get(1).map(|v| v.to_number().max(0.0) as usize).unwrap_or(0);
            Value::Bool(pos <= units.len() && units[pos..].starts_with(&needle))
        }
        "endsWith" => {
            let needle: Vec<u16> = arg_str(args, 0, heap).encode_utf16().collect();
            let units = s.units();
            // The optional end position treats the string as that many units long.
            let end = match args.get(1) {
                Some(v) if !matches!(v, Value::Undefined) => {
                    (v.to_number().max(0.0) as usize).min(units.len())
                }
                _ => units.len(),
            };
            Value::Bool(units[..end].ends_with(&needle))
        }
        "slice" => {
            let units = s.units();
            let len = units.len() as i64;
            let start = normalize_index(arg_int(args, 0), len).min(units.len());
            let end = if args.len() > 1 {
                normalize_index(arg_int(args, 1), len).min(units.len())
            } else {
                units.len()
            };
            if start >= end {
                Value::String(JsString::from(""))
            } else {
                Value::String(JsString::from_units(&units[start..end]))
            }
        }
        "substring" => {
            let units = s.units();
            let len = units.len();
            let start = (arg_int(args, 0).max(0) as usize).min(len);
            let end = if args.len() > 1 {
                (arg_int(args, 1).max(0) as usize).min(len)
            } else {
                len
            };
            let (start, end) = if start > end { (end, start) } else { (start, end) };
            Value::String(JsString::from_units(&units[start..end]))
        }
        "toUpperCase" => Value::String(JsString::from(s.to_uppercase().as_str())),
        "toLowerCase" => Value::String(JsString::from(s.to_lowercase().as_str())),
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
        "trim" => Value::String(JsString::from(s.trim())),
        "trimStart" | "trimLeft" => Value::String(JsString::from(s.trim_start())),
        "trimEnd" | "trimRight" => Value::String(JsString::from(s.trim_end())),
        "repeat" => {
            let count = arg_int(args, 0).max(0) as usize;
            let result_len = s.len().saturating_mul(count);
            if result_len > limits.memory_limit_bytes {
                return Err(ZapcodeError::MemoryLimitExceeded(format!(
                    "string repeat result of {} bytes exceeds memory limit of {} bytes",
                    result_len, limits.memory_limit_bytes
                )));
            }
            Value::String(JsString::from(s.repeat(count).as_str()))
        }
        "padStart" => {
            let target_len = arg_int(args, 0).max(0) as usize;
            let pad = if args.len() > 1 {
                arg_str(args, 1, heap)
            } else {
                " ".to_string()
            };
            let current_len = s.len_utf16();
            if current_len >= target_len {
                Value::String(s.clone().into())
            } else {
                // Guard the projected size before materializing, like `repeat`,
                // so a huge `target_len` can't allocate an untracked giant string
                // (memory-limit bypass / host OOM).
                check_string_alloc(target_len, limits)?;
                let pad_len = target_len - current_len;
                let padding: String = pad.chars().cycle().take(pad_len).collect();
                Value::String(JsString::from(format!("{}{}", padding, s).as_str()))
            }
        }
        "padEnd" => {
            let target_len = arg_int(args, 0).max(0) as usize;
            let pad = if args.len() > 1 {
                arg_str(args, 1, heap)
            } else {
                " ".to_string()
            };
            let current_len = s.len_utf16();
            if current_len >= target_len {
                Value::String(s.clone().into())
            } else {
                check_string_alloc(target_len, limits)?;
                let pad_len = target_len - current_len;
                let padding: String = pad.chars().cycle().take(pad_len).collect();
                Value::String(JsString::from(format!("{}{}", s, padding).as_str()))
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
            // `"abc".split()` (separator omitted/undefined) returns the whole
            // string as a single element — NOT a per-character split.
            if matches!(args.first(), None | Some(Value::Undefined)) {
                let whole = vec![Value::String(s.clone().into())];
                return Ok(Some(Value::Array(heap.alloc_array(whole))));
            }
            if let Some((pat, flags)) = args.first().and_then(|v| regexp_parts(v, heap)) {
                let re = compile_regex(&pat, &flags)?;
                // Splice in capture groups between the surrounding pieces, like JS.
                let mut out: Vec<Value> = Vec::new();
                let mut last = 0usize;
                for caps in re.captures_iter(s) {
                    let m = caps.get(0).unwrap();
                    out.push(Value::String(JsString::from(&s[last..m.start()])));
                    for i in 1..caps.len() {
                        out.push(
                            caps.get(i)
                                .map(|g| Value::String(JsString::from(g.as_str())))
                                .unwrap_or(Value::Undefined),
                        );
                    }
                    last = m.end();
                }
                out.push(Value::String(JsString::from(&s[last..])));
                out.truncate(limit);
                return Ok(Some(Value::Array(heap.alloc_array(out))));
            }
            let separator = arg_str(args, 0, heap);
            let mut parts: Vec<Value> = if separator.is_empty() {
                // Empty separator splits into individual UTF-16 code units
                // (an astral char becomes its two lone surrogates), like JS.
                s.units()
                    .iter()
                    .map(|u| Value::String(JsString::from_units(&[*u])))
                    .collect()
            } else {
                s.split(&*separator)
                    .map(|p| Value::String(JsString::from(p)))
                    .collect()
            };
            parts.truncate(limit);
            Value::Array(heap.alloc_array(parts))
        }
        "replace" => {
            if let Some((pat, flags)) = args.first().and_then(|v| regexp_parts(v, heap)) {
                let re = compile_regex(&pat, &flags)?;
                let repl = translate_replacement(&arg_str(args, 1, heap), re.captures_len());
                let out = if flags.contains('g') {
                    re.replace_all(s, repl.as_str())
                } else {
                    re.replace(s, repl.as_str())
                };
                return Ok(Some(Value::String(JsString::from(out.as_ref()))));
            }
            let search = arg_str(args, 0, heap);
            let replacement = arg_str(args, 1, heap);
            // Replace the first occurrence, expanding `$`-tokens against the match.
            match s.find(&*search) {
                Some(byte) => {
                    let expanded =
                        expand_string_replacement(&replacement, s, byte, search.len());
                    let mut out = String::with_capacity(s.len());
                    out.push_str(&s[..byte]);
                    out.push_str(&expanded);
                    out.push_str(&s[byte + search.len()..]);
                    Value::String(JsString::from(out.as_str()))
                }
                None => Value::String(s.clone().into()),
            }
        }
        "replaceAll" => {
            if let Some((pat, flags)) = args.first().and_then(|v| regexp_parts(v, heap)) {
                let re = compile_regex(&pat, &flags)?;
                let repl = translate_replacement(&arg_str(args, 1, heap), re.captures_len());
                let out = re.replace_all(s, repl.as_str());
                return Ok(Some(Value::String(JsString::from(out.as_ref()))));
            }
            let search = arg_str(args, 0, heap);
            let replacement = arg_str(args, 1, heap);
            if search.is_empty() {
                // Match JS empty-search edge: insert between every char (and ends).
                // Fall back to the simple replace which already handles this shape.
                Value::String(JsString::from(s.replace(&*search, &replacement).as_str()))
            } else {
                // Replace every non-overlapping occurrence, expanding `$`-tokens
                // against each match.
                let mut out = String::with_capacity(s.len());
                let mut last = 0usize;
                for (byte, _) in s.match_indices(&*search) {
                    out.push_str(&s[last..byte]);
                    out.push_str(&expand_string_replacement(
                        &replacement,
                        s,
                        byte,
                        search.len(),
                    ));
                    last = byte + search.len();
                }
                out.push_str(&s[last..]);
                Value::String(JsString::from(out.as_str()))
            }
        }
        "match" => {
            if let Some((pat, flags)) = args.first().and_then(|v| regexp_parts(v, heap)) {
                let re = compile_regex(&pat, &flags)?;
                if flags.contains('g') {
                    let all: Vec<Value> = re
                        .find_iter(s)
                        .map(|m| Value::String(JsString::from(m.as_str())))
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
                    let items = vec![Value::String(JsString::from(pattern.as_str()))];
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
            // Sum the projected size first and reject up front so a string
            // built from many large pieces can't bypass the memory limit.
            let mut projected = s.len();
            let rendered: Vec<String> = args.iter().map(|a| a.to_js_string(heap)).collect();
            for piece in &rendered {
                projected = projected.saturating_add(piece.len());
            }
            check_string_alloc(projected, limits)?;
            let mut result = s.to_string();
            for piece in rendered {
                result.push_str(&piece);
            }
            Value::String(JsString::from(result.as_str()))
        }
        "at" => {
            let idx = arg_int(args, 0);
            let units = s.units();
            let len = units.len() as i64;
            // A negative index counts from the end; an index out of range (either
            // direction, including too-negative) yields `undefined` — no clamping.
            let resolved = if idx < 0 { len + idx } else { idx };
            if resolved < 0 || resolved >= len {
                Value::Undefined
            } else {
                let i = resolved as usize;
                Value::String(JsString::from_units(&units[i..i + 1]))
            }
        }
        "normalize" => {
            // Unicode normalization (default NFC), per String.prototype.normalize.
            use unicode_normalization::UnicodeNormalization;
            let form = args
                .first()
                .map(|v| v.to_js_string(heap))
                .filter(|f| !f.is_empty())
                .unwrap_or_else(|| "NFC".to_string());
            let normalized: String = match form.as_str() {
                "NFC" => s.nfc().collect(),
                "NFD" => s.nfd().collect(),
                "NFKC" => s.nfkc().collect(),
                "NFKD" => s.nfkd().collect(),
                other => {
                    // Match JS: an invalid form throws a RangeError.
                    return Err(ZapcodeError::RangeError(format!(
                        "The normalization form should be one of NFC, NFD, NFKC, NFKD. Received: {}",
                        other
                    )));
                }
            };
            Value::String(JsString::from(normalized.as_str()))
        }
        _ => return Ok(None),
    };
    Ok(Some(result))
}

/// Reject a string allocation whose projected byte length exceeds the memory
/// limit before it is materialized. Mirrors the in-line guard `String.repeat`
/// uses; shared by the other unbounded string-builders (padStart/padEnd, concat,
/// Array.join) so a guest can't bypass `memory_limit_bytes` through them.
fn check_string_alloc(projected_len: usize, limits: &ResourceLimits) -> Result<()> {
    if projected_len > limits.memory_limit_bytes {
        return Err(ZapcodeError::MemoryLimitExceeded(format!(
            "string result of {} bytes exceeds memory limit of {} bytes",
            projected_len, limits.memory_limit_bytes
        )));
    }
    Ok(())
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
pub(crate) fn same_value_zero(a: &Value, b: &Value) -> bool {
    if a.strict_eq(b) {
        return true;
    }
    matches!((a, b), (Value::Float(x), Value::Float(y)) if x.is_nan() && y.is_nan())
}

/// Recursive helper for `Array.prototype.flat(depth)`. `native_depth` bounds the
/// native recursion so a cyclic (`a.push(a); a.flat(Infinity)`) or pathologically
/// deep array can't overflow the host stack (SIGSEGV). Exceeding the cap surfaces a
/// catchable `RuntimeError` instead, matching the [`MAX_RENDER_DEPTH`] contract the
/// JSON and structuredClone walkers already use.
fn flatten_into(
    arr: &[Value],
    depth: i64,
    out: &mut Vec<Value>,
    heap: &Heap,
    native_depth: usize,
) -> Result<()> {
    if native_depth > MAX_RENDER_DEPTH {
        return Err(ZapcodeError::RuntimeError(format!(
            "flatten depth exceeded (max {})",
            MAX_RENDER_DEPTH
        )));
    }
    for item in arr {
        match item {
            Value::Array(inner) if depth > 0 => {
                flatten_into(&heap.array_vec(*inner), depth - 1, out, heap, native_depth + 1)?;
            }
            other => out.push(other.clone()),
        }
    }
    Ok(())
}

fn call_array_method(
    handle: crate::heap::Handle,
    method: &str,
    args: &[Value],
    limits: &ResourceLimits,
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
            // Reject before allocating the contiguous result so a join of many
            // large strings can't bypass the memory limit (host OOM DoS).
            let body: usize = joined.iter().map(|p| p.len()).sum();
            let sep_total = sep.len().saturating_mul(joined.len().saturating_sub(1));
            check_string_alloc(body.saturating_add(sep_total), limits)?;
            Value::String(JsString::from(joined.join(&sep).as_str()))
        }
        "toLocaleString" => {
            // Join with "," (the implementation default separator), formatting
            // each element via its own toLocaleString (numbers get grouping).
            let joined: Vec<String> = arr
                .iter()
                .map(|v| match v {
                    Value::Null | Value::Undefined => String::new(),
                    Value::Int(_) | Value::Float(_) => js_to_locale_string(v.to_number()),
                    _ => v.to_js_string(heap),
                })
                .collect();
            let body: usize = joined.iter().map(|p| p.len()).sum();
            check_string_alloc(body.saturating_add(joined.len()), limits)?;
            Value::String(JsString::from(joined.join(",").as_str()))
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
            flatten_into(arr, depth, &mut result, heap, 0)?;
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
        // ── ES2023 immutable (change-array-by-copy) methods ──
        "toReversed" => {
            let mut v = arr.to_vec();
            v.reverse();
            Value::Array(heap.alloc_array(v))
        }
        "with" => {
            // Returns a copy with index `i` replaced; out-of-range -> RangeError.
            let len = arr.len() as i64;
            let raw = arg_int(args, 0);
            let i = if raw < 0 { raw + len } else { raw };
            if i < 0 || i >= len {
                return Err(ZapcodeError::RangeError(format!("Invalid index : {}", raw)));
            }
            let mut v = arr.to_vec();
            v[i as usize] = args.get(1).cloned().unwrap_or(Value::Undefined);
            Value::Array(heap.alloc_array(v))
        }
        "toSpliced" => {
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
            let inserts: Vec<Value> = if args.len() > 2 { args[2..].to_vec() } else { Vec::new() };
            let mut v = arr.to_vec();
            v.splice(start..start + delete_count, inserts);
            Value::Array(heap.alloc_array(v))
        }
        // Array iterators: real iterator objects (`__array_iterator__`), so
        // `.next()` works AND spread / for-of consume them via the cursor.
        "entries" => {
            let mut out = Vec::with_capacity(arr.len());
            for (i, v) in arr.iter().enumerate() {
                let pair = heap.alloc_array(vec![Value::Int(i as i64), v.clone()]);
                out.push(Value::Array(pair));
            }
            make_array_iterator(out, heap)
        }
        "keys" => {
            make_array_iterator((0..arr.len()).map(|i| Value::Int(i as i64)).collect(), heap)
        }
        "values" => make_array_iterator(arr.to_vec(), heap),
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

/// If `s` is a canonical array-index key (per ECMA-262: `ToString(ToUint32(s))
/// === s` and the value is not 2^32-1), return that integer. Such keys sort
/// *ascending and before* string keys in property enumeration order.
pub fn array_index_key(s: &str) -> Option<u32> {
    // parse::<u32> already rejects "+1", " 1", "-1", "1.0", "" and overflow; the
    // round-trip check additionally rejects non-canonical forms like "01".
    let n: u32 = s.parse().ok()?;
    if n == u32::MAX {
        return None;
    }
    (n.to_string() == s).then_some(n)
}

/// Own enumerable property keys of `map` in ECMA-262 OrdinaryOwnPropertyKeys
/// order: integer-index keys ascending, then the remaining string keys in
/// insertion order. Reserved internal markers are filtered out. This is the
/// single ordering used by Object.keys/values/entries, for-in (which desugars
/// to Object.keys), and JSON.stringify so they all agree.
pub fn ordered_visible_keys(map: &IndexMap<Arc<str>, Value>) -> Vec<Arc<str>> {
    let non_enum = marker_list(map, "__non_enum__");
    let mut indices: Vec<(u32, Arc<str>)> = Vec::new();
    let mut strings: Vec<Arc<str>> = Vec::new();
    for k in map.keys() {
        if is_internal_marker_key(k) || non_enum.iter().any(|n| n == k.as_ref()) {
            continue;
        }
        match array_index_key(k) {
            Some(n) => indices.push((n, k.clone())),
            None => strings.push(k.clone()),
        }
    }
    indices.sort_by_key(|(n, _)| *n);
    indices
        .into_iter()
        .map(|(_, k)| k)
        .chain(strings)
        .collect()
}

/// Read a property-attribute marker list (`__non_enum__` / `__non_writable__` /
/// `__non_config__`) off an object map. Stored as a single NUL-joined string
/// value (NOT a heap array) so it is readable from the map alone — `ordered_
/// visible_keys` and the spread path have the map but not the heap. Real
/// property keys never contain a NUL, so it is an unambiguous separator.
pub fn marker_list(map: &IndexMap<Arc<str>, Value>, marker: &str) -> Vec<String> {
    match map.get(marker) {
        Some(Value::String(s)) if !s.is_empty() => {
            s.split('\u{0}').map(|x| x.to_string()).collect()
        }
        _ => Vec::new(),
    }
}

/// True iff `key` is listed in the object's `marker` attribute list.
pub fn marker_contains(map: &IndexMap<Arc<str>, Value>, marker: &str, key: &str) -> bool {
    match map.get(marker) {
        Some(Value::String(s)) => s.split('\u{0}').any(|x| x == key),
        _ => false,
    }
}

/// Add `key` to an object's `marker` attribute list (NUL-joined string),
/// creating it if absent and avoiding duplicates.
fn marker_add(map: &mut IndexMap<Arc<str>, Value>, marker: &str, key: &str) {
    let mut list: Vec<String> = match map.get(marker) {
        Some(Value::String(s)) if !s.is_empty() => {
            s.split('\u{0}').map(|x| x.to_string()).collect()
        }
        _ => Vec::new(),
    };
    if !list.iter().any(|x| x == key) {
        list.push(key.to_string());
    }
    map.insert(Arc::from(marker), Value::String(JsString::from(list.join("\u{0}").as_str())));
}

/// Remove `key` from an object's `marker` attribute list.
fn marker_remove(map: &mut IndexMap<Arc<str>, Value>, marker: &str, key: &str) {
    if let Some(Value::String(s)) = map.get(marker) {
        let kept: Vec<&str> = s.split('\u{0}').filter(|x| *x != key).collect();
        map.insert(Arc::from(marker), Value::String(JsString::from(kept.join("\u{0}").as_str())));
    }
}

/// Apply one ECMA property descriptor (`{value|get|set, writable, enumerable,
/// configurable}`) to `obj_h[key]`. Attributes default to `false`/absent per
/// the spec. Accessor descriptors install into the object's
/// `__getters__`/`__setters__` tables (the same the runtime consults on
/// read/write); the enumerable/writable/configurable flags are recorded in the
/// object's `__non_*__` marker lists, which the enumeration and write paths
/// honor. `configurable: false` is recorded for getOwnPropertyDescriptor but
/// its redefine/delete restriction is not enforced.
fn apply_descriptor(obj_h: Handle, key: &str, desc: &Value, heap: &mut Heap) -> Result<()> {
    let desc_map = match desc {
        Value::Object(dh) => heap.object_map(*dh),
        _ => {
            return Err(ZapcodeError::TypeError(
                "Property description must be an object".to_string(),
            ))
        }
    };
    let truthy = |k: &str| desc_map.get(k).map(|v| v.is_truthy()).unwrap_or(false);
    let getf = |k: &str| desc_map.get(k).cloned();
    let is_accessor = desc_map.contains_key("get") || desc_map.contains_key("set");
    let enumerable = truthy("enumerable");
    let writable = truthy("writable");
    let configurable = truthy("configurable");

    if is_accessor {
        for (table, field) in [("__getters__", "get"), ("__setters__", "set")] {
            if let Some(f @ Value::Function(_)) = getf(field) {
                let table_h = match heap.object(obj_h).and_then(|m| m.get(table)) {
                    Some(Value::Object(th)) => *th,
                    _ => {
                        let th = heap.alloc_object(IndexMap::new());
                        if let Some(m) = heap.object_mut(obj_h) {
                            m.insert(Arc::from(table), Value::Object(th));
                        }
                        th
                    }
                };
                if let Some(tm) = heap.object_mut(table_h) {
                    tm.insert(Arc::from(key), f);
                }
            }
        }
        // An enumerable accessor needs a data placeholder so the key enumerates
        // (matching the object-literal accessor model); non-enumerable accessors
        // (defineProperty's default) get no data key, so they stay hidden.
        if enumerable {
            let placeholder = getf("get").or_else(|| getf("set")).unwrap_or(Value::Undefined);
            if let Some(m) = heap.object_mut(obj_h) {
                m.insert(Arc::from(key), placeholder);
            }
        }
    } else {
        let value = getf("value").unwrap_or(Value::Undefined);
        if let Some(m) = heap.object_mut(obj_h) {
            m.insert(Arc::from(key), value);
        }
    }

    if let Some(m) = heap.object_mut(obj_h) {
        if enumerable {
            marker_remove(m, "__non_enum__", key);
        } else {
            marker_add(m, "__non_enum__", key);
        }
        if !is_accessor {
            if writable {
                marker_remove(m, "__non_writable__", key);
            } else {
                marker_add(m, "__non_writable__", key);
            }
        }
        if configurable {
            marker_remove(m, "__non_config__", key);
        } else {
            marker_add(m, "__non_config__", key);
        }
    }
    Ok(())
}

fn call_object_method(method: &str, args: &[Value], heap: &mut Heap) -> Result<Option<Value>> {
    let first = args.first().cloned().unwrap_or(Value::Undefined);
    match method {
        "keys" => {
            let keys: Vec<Value> = match &first {
                Value::Object(h) => heap
                    .object(*h)
                    .map(|m| {
                        ordered_visible_keys(m)
                            .into_iter()
                            .map(|k| Value::String(k.into()))
                            .collect()
                    })
                    .unwrap_or_default(),
                // Object.keys([...]) yields index strings.
                Value::Array(h) => (0..heap.array(*h).len())
                    .map(|i| Value::String(JsString::from(i.to_string().as_str())))
                    .collect(),
                _ => Vec::new(),
            };
            Ok(Some(Value::Array(heap.alloc_array(keys))))
        }
        "values" => {
            let values: Vec<Value> = match &first {
                Value::Object(h) => heap
                    .object(*h)
                    .map(|m| {
                        ordered_visible_keys(m)
                            .into_iter()
                            .filter_map(|k| m.get(&k).cloned())
                            .collect()
                    })
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
                        ordered_visible_keys(m)
                            .into_iter()
                            .filter_map(|k| {
                                m.get(&k).cloned().map(|v| (Value::String(k.into()), v))
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                Value::Array(h) => heap
                    .array(*h)
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        (
                            Value::String(JsString::from(i.to_string().as_str())),
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
                            Value::String(s) => Arc::from(s.as_str()),
                            other => Arc::from(other.to_js_string(heap).as_str()),
                        };
                        obj.insert(key, val);
                    }
                }
            }
            Ok(Some(Value::Object(heap.alloc_object(obj))))
        }
        "freeze" => {
            // Mark the object (or array) frozen so subsequent property/index
            // writes are silently ignored (sloppy mode) and Object.isFrozen
            // reports true. Returns the same reference.
            match &first {
                Value::Object(h) => {
                    if let Some(map) = heap.object_mut(*h) {
                        map.insert(Arc::from("__frozen__"), Value::Bool(true));
                    }
                }
                _ => {}
            }
            Ok(Some(first))
        }
        "seal" => {
            // No-op in sandbox — return object as-is (sealing's distinction from
            // freeze, value-mutability, is not modeled).
            Ok(Some(first))
        }
        "isFrozen" => {
            let frozen = match &first {
                Value::Object(h) => matches!(
                    heap.object(*h).and_then(|m| m.get("__frozen__")),
                    Some(Value::Bool(true))
                ),
                // Primitives are considered frozen in JS.
                Value::Undefined | Value::Null | Value::Bool(_) | Value::Int(_)
                | Value::Float(_) | Value::String(_) => true,
                _ => false,
            };
            Ok(Some(Value::Bool(frozen)))
        }
        "is" => {
            // SameValue: like ===, but NaN is equal to NaN and +0 !== -0.
            let a = args.first().cloned().unwrap_or(Value::Undefined);
            let b = args.get(1).cloned().unwrap_or(Value::Undefined);
            Ok(Some(Value::Bool(same_value(&a, &b))))
        }
        "create" => {
            // Minimal: create a fresh object whose own properties come from the
            // optional second argument's property descriptors. The prototype
            // argument's chain is not modeled, but `Object.create(null)` /
            // `Object.create(proto)` both yield a usable plain object.
            let mut obj = IndexMap::new();
            if let Some(Value::Object(props_h)) = args.get(1) {
                let props = heap.object_map(*props_h);
                for (k, desc) in props {
                    // Each descriptor is `{ value: ... }` (data) — pull `value`.
                    let val = match &desc {
                        Value::Object(dh) => heap
                            .object(*dh)
                            .and_then(|m| m.get("value").cloned())
                            .unwrap_or(Value::Undefined),
                        other => other.clone(),
                    };
                    obj.insert(k, val);
                }
            }
            Ok(Some(Value::Object(heap.alloc_object(obj))))
        }
        "getPrototypeOf" => {
            // Prototype chains aren't modeled; for a plain object we report a
            // stand-in empty object (so `Object.getPrototypeOf({})` is non-null
            // and usable), and null for null/undefined.
            match &first {
                Value::Object(_) | Value::Array(_) => {
                    Ok(Some(Value::Object(heap.alloc_object(IndexMap::new()))))
                }
                _ => Ok(Some(Value::Null)),
            }
        }
        "getOwnPropertyNames" => {
            // Like Object.keys but conceptually includes non-enumerable names;
            // reserved internal brand keys are hidden from guest view (but real
            // user keys that merely start with `__` are kept).
            let keys: Vec<Value> = match &first {
                Value::Object(h) => heap
                    .object(*h)
                    .map(|m| {
                        m.keys()
                            .filter(|k| !is_internal_marker_key(k))
                            .map(|k| Value::String(k.clone().into()))
                            .collect()
                    })
                    .unwrap_or_default(),
                Value::Array(h) => {
                    let mut names: Vec<Value> = (0..heap.array(*h).len())
                        .map(|i| Value::String(JsString::from(i.to_string().as_str())))
                        .collect();
                    names.push(Value::String(JsString::from("length")));
                    names
                }
                _ => Vec::new(),
            };
            Ok(Some(Value::Array(heap.alloc_array(keys))))
        }
        "defineProperty" => {
            if let Value::Object(h) = &first {
                let key = arg_str(args, 1, heap);
                let desc = args.get(2).cloned().unwrap_or(Value::Undefined);
                apply_descriptor(*h, &key, &desc, heap)?;
            }
            Ok(Some(first))
        }
        "defineProperties" => {
            if let (Value::Object(h), Some(Value::Object(props_h))) = (&first, args.get(1)) {
                for (k, desc) in heap.object_map(*props_h) {
                    if is_internal_marker_key(&k) {
                        continue;
                    }
                    apply_descriptor(*h, &k, &desc, heap)?;
                }
            }
            Ok(Some(first))
        }
        "getOwnPropertyDescriptor" => {
            let key = arg_str(args, 1, heap);
            let Value::Object(h) = &first else {
                return Ok(Some(Value::Undefined));
            };
            let m = heap.object_map(*h);
            let getter = match m.get("__getters__") {
                Some(Value::Object(gh)) => {
                    heap.object(*gh).and_then(|gm| gm.get(key.as_str()).cloned())
                }
                _ => None,
            };
            let setter = match m.get("__setters__") {
                Some(Value::Object(sh)) => {
                    heap.object(*sh).and_then(|sm| sm.get(key.as_str()).cloned())
                }
                _ => None,
            };
            let is_accessor = getter.is_some() || setter.is_some();
            // Absent own property -> undefined (private/internal keys are hidden).
            if !is_accessor && (!m.contains_key(key.as_str()) || is_internal_marker_key(&key)) {
                return Ok(Some(Value::Undefined));
            }
            let enumerable = !marker_contains(&m, "__non_enum__", &key);
            let configurable = !marker_contains(&m, "__non_config__", &key);
            let mut desc: IndexMap<Arc<str>, Value> = IndexMap::new();
            if is_accessor {
                desc.insert(Arc::from("get"), getter.unwrap_or(Value::Undefined));
                desc.insert(Arc::from("set"), setter.unwrap_or(Value::Undefined));
            } else {
                desc.insert(
                    Arc::from("value"),
                    m.get(key.as_str()).cloned().unwrap_or(Value::Undefined),
                );
                desc.insert(
                    Arc::from("writable"),
                    Value::Bool(!marker_contains(&m, "__non_writable__", &key)),
                );
            }
            desc.insert(Arc::from("enumerable"), Value::Bool(enumerable));
            desc.insert(Arc::from("configurable"), Value::Bool(configurable));
            Ok(Some(Value::Object(heap.alloc_object(desc))))
        }
        _ => Ok(None),
    }
}

/// JS `SameValue` (the algorithm behind `Object.is`): strict equality except
/// `NaN` is the same as `NaN`, and `+0` is distinct from `-0`.
fn same_value(a: &Value, b: &Value) -> bool {
    let an = numeric(a);
    let bn = numeric(b);
    if let (Some(x), Some(y)) = (an, bn) {
        if x.is_nan() && y.is_nan() {
            return true;
        }
        if x == 0.0 && y == 0.0 {
            // Distinguish +0 from -0 via sign bit.
            return x.is_sign_negative() == y.is_sign_negative();
        }
        return x == y;
    }
    a.strict_eq(b)
}

/// Extract an `f64` for SameValue numeric handling, or None for non-numbers.
fn numeric(v: &Value) -> Option<f64> {
    match v {
        Value::Int(n) => Some(*n as f64),
        Value::Float(n) => Some(*n),
        _ => None,
    }
}

/// Materialize the iterable/array-like accepted by `Array.from` into a Vec.
/// Handles arrays, strings (by char), built-in Set/Map, and `{ length: n }`
/// array-likes. The optional mapFn is applied by the caller (it may be a
/// guest closure that requires the VM).
/// The number of elements `array_from_source` would produce for `val`, computed
/// *without* materializing the (possibly enormous) Vec. Callers use this to
/// charge the allocation against the resource limit before building it, so a
/// `{length: 200_000_000}` array-like can't allocate untracked and OOM the host.
pub fn array_from_source_len(val: &Value, heap: &Heap) -> usize {
    match val {
        Value::Array(h) => heap.array(*h).len(),
        Value::String(s) => s.chars().count(),
        Value::Object(h) => {
            let Some(map) = heap.object(*h) else {
                return 0;
            };
            if matches!(map.get("__array_iterator__"), Some(Value::Bool(true))) {
                let cursor = match map.get("__cursor__") {
                    Some(Value::Int(i)) => (*i).max(0) as usize,
                    _ => 0,
                };
                return match map.get("__items__") {
                    Some(Value::Array(ih)) => heap.array(*ih).len().saturating_sub(cursor),
                    _ => 0,
                };
            }
            if matches!(map.get("__set__"), Some(Value::Bool(true))) {
                return match map.get("__items__") {
                    Some(Value::Array(ih)) => heap.array(*ih).len(),
                    _ => 0,
                };
            }
            if matches!(map.get("__map__"), Some(Value::Bool(true))) {
                return match map.get("__entries__") {
                    Some(Value::Array(eh)) => heap.array(*eh).len(),
                    _ => 0,
                };
            }
            match map.get("length") {
                Some(len_val) => {
                    let n = len_val.to_number();
                    if n.is_finite() && n >= 0.0 {
                        n as usize
                    } else {
                        0
                    }
                }
                None => 0,
            }
        }
        _ => 0,
    }
}

pub fn array_from_source(val: &Value, heap: &mut Heap) -> Vec<Value> {
    match val {
        Value::Array(h) => heap.array_vec(*h),
        Value::String(s) => s
            .chars()
            .map(|c| Value::String(JsString::from(c.to_string().as_str())))
            .collect(),
        Value::Object(h) => {
            let map = heap.object_map(*h);
            if matches!(map.get("__array_iterator__"), Some(Value::Bool(true))) {
                let cursor = match map.get("__cursor__") {
                    Some(Value::Int(i)) => (*i).max(0) as usize,
                    _ => 0,
                };
                return match map.get("__items__") {
                    Some(Value::Array(ih)) => {
                        heap.array_vec(*ih).into_iter().skip(cursor).collect()
                    }
                    _ => Vec::new(),
                };
            }
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
    // Lower to a batch promise when any element is not yet settled: a deferred
    // single-call promise (host call) or a microtask-pending `.then` chain.
    // The `Await` pending_all arm drains microtasks / suspends on the host so
    // every element is settled before the batch is assembled.
    let has_unsettled = arr.iter().any(|item| {
        matches!(item, Value::Object(h) if matches!(
            heap.object(*h).and_then(|m| m.get("status")),
            Some(Value::String(s)) if s.as_ref() == "pending_call"
                || (s.as_ref() == "pending"
                    && heap.object(*h).is_some_and(|m| m.contains_key("__reactions__")))
        ))
    });
    if !has_unsettled {
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
    obj.insert(Arc::from("status"), Value::String(JsString::from("pending_all")));
    obj.insert(Arc::from("__batch_kind__"), Value::String(JsString::from(kind)));
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
            obj.insert(Arc::from("status"), Value::String(JsString::from("resolved")));
            obj.insert(Arc::from("value"), val);
            Ok(Some(Value::Object(heap.alloc_object(obj))))
        }
        "reject" => {
            let reason = args.first().cloned().unwrap_or(Value::Undefined);
            let mut obj = IndexMap::new();
            obj.insert(Arc::from("__promise__"), Value::Bool(true));
            obj.insert(Arc::from("status"), Value::String(JsString::from("rejected")));
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
            obj.insert(Arc::from("status"), Value::String(JsString::from("resolved")));
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
                    entry.insert(Arc::from("status"), Value::String(JsString::from("rejected")));
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
                entry.insert(Arc::from("status"), Value::String(JsString::from("fulfilled")));
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
                    obj.insert(Arc::from("status"), Value::String(JsString::from("pending")));
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
            let agg_obj = make_aggregate_error(errors, heap);
            let mut obj = IndexMap::new();
            obj.insert(Arc::from("__promise__"), Value::Bool(true));
            obj.insert(Arc::from("status"), Value::String(JsString::from("rejected")));
            obj.insert(Arc::from("reason"), agg_obj);
            Ok(Some(Value::Object(heap.alloc_object(obj))))
        }
        _ => Ok(None),
    }
}

/// Percent-encode `s` for a URI: ASCII alphanumerics and the characters in
/// `safe` pass through; everything else becomes uppercase %XX UTF-8 bytes.
fn uri_encode(s: &str, safe: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() || safe.contains(ch) {
            out.push(ch);
        } else {
            let mut buf = [0u8; 4];
            for b in ch.encode_utf8(&mut buf).bytes() {
                out.push('%');
                out.push(char::from_digit(u32::from(b >> 4), 16).unwrap().to_ascii_uppercase());
                out.push(char::from_digit(u32::from(b & 0xf), 16).unwrap().to_ascii_uppercase());
            }
        }
    }
    out
}

/// Decode %XX sequences in `s`. A decoded single-byte character listed in
/// `keep_encoded` stays as its original %XX text (`decodeURI` preserves the
/// URI's reserved separators). Malformed sequences raise a URIError, like JS.
fn uri_decode(s: &str, keep_encoded: &str) -> Result<String> {
    let malformed = || ZapcodeError::RuntimeError("URIError: URI malformed".to_string());
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3).ok_or_else(malformed)?;
            let hi = (hex[0] as char).to_digit(16).ok_or_else(malformed)?;
            let lo = (hex[1] as char).to_digit(16).ok_or_else(malformed)?;
            let byte = (hi * 16 + lo) as u8;
            if byte < 0x80 && keep_encoded.contains(byte as char) {
                out.extend_from_slice(&bytes[i..i + 3]);
            } else {
                out.push(byte);
            }
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| malformed())
}

const B64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(B64_ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(B64_ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64_ALPHABET[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64_ALPHABET[n as usize & 63] as char } else { '=' });
    }
    out
}

/// WHATWG forgiving-base64-decode (what `atob` specifies, Node included):
/// strip ASCII whitespace; a length that is a multiple of 4 may carry one or
/// two trailing `=`; after that, a remaining length of 1 (mod 4) is invalid
/// and `=` may not appear at all. A short final chunk (2 or 3 digits — i.e.
/// unpadded input) decodes as if implicitly padded.
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut data: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if data.len() % 4 == 0 {
        let pad = data.iter().rev().take_while(|b| **b == b'=').take(2).count();
        data.truncate(data.len() - pad);
    }
    if data.len() % 4 == 1 {
        return None;
    }
    let val = |b: u8| -> Option<u32> {
        B64_ALPHABET.iter().position(|a| *a == b).map(|i| i as u32)
    };
    let mut out = Vec::with_capacity(data.len() / 4 * 3 + 2);
    for chunk in data.chunks(4) {
        let mut n: u32 = 0;
        for b in chunk {
            n = (n << 6) | val(*b)?; // '=' is not in the alphabet → None
        }
        // Left-justify a short final chunk to a full 24-bit group; the
        // surplus low bits are the implicit-padding bits and are dropped.
        n <<= 6 * (4 - chunk.len() as u32);
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

/// Build the AggregateError-shaped object `Promise.any` rejects with when
/// every element rejects. The `__error__` brand makes `e instanceof Error`
/// true (Node: AggregateError extends Error).
pub fn make_aggregate_error(errors: Vec<Value>, heap: &mut Heap) -> Value {
    let errors_arr = Value::Array(heap.alloc_array(errors));
    let mut agg = IndexMap::new();
    agg.insert(Arc::from("__error__"), Value::Bool(true));
    agg.insert(
        Arc::from("name"),
        Value::String(JsString::from("AggregateError")),
    );
    agg.insert(
        Arc::from("message"),
        Value::String(JsString::from("All promises were rejected")),
    );
    agg.insert(Arc::from("errors"), errors_arr);
    Value::Object(heap.alloc_object(agg))
}

/// Build a built-in array-iterator object: `for…of`, spread, and `.next()`
/// all consume it through the `__items__`/`__cursor__` protocol.
pub fn make_array_iterator(items: Vec<Value>, heap: &mut Heap) -> Value {
    let items_h = heap.alloc_array(items);
    let mut obj = IndexMap::new();
    obj.insert(Arc::from("__array_iterator__"), Value::Bool(true));
    obj.insert(Arc::from("__items__"), Value::Array(items_h));
    obj.insert(Arc::from("__cursor__"), Value::Int(0));
    Value::Object(heap.alloc_object(obj))
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

/// Create a pending internal promise with an empty reaction list.
/// `.then`/`.catch`/`.finally` on it register reactions (see
/// `Vm::register_reaction`); `Vm::settle_promise` flips it to
/// resolved/rejected and enqueues the reactions as microtasks.
pub fn make_pending_promise(heap: &mut Heap) -> Value {
    let reactions = heap.alloc_array(Vec::new());
    let mut obj = IndexMap::new();
    obj.insert(Arc::from("__promise__"), Value::Bool(true));
    obj.insert(Arc::from("status"), Value::String(JsString::from("pending")));
    obj.insert(Arc::from("__reactions__"), Value::Array(reactions));
    Value::Object(heap.alloc_object(obj))
}

/// Create a resolved promise wrapping the given value.
pub fn make_resolved_promise(val: Value, heap: &mut Heap) -> Value {
    // If the value is already a promise, return it as-is (thenable unwrapping)
    if is_promise(&val, heap) {
        return val;
    }
    let mut obj = IndexMap::new();
    obj.insert(Arc::from("__promise__"), Value::Bool(true));
    obj.insert(Arc::from("status"), Value::String(JsString::from("resolved")));
    obj.insert(Arc::from("value"), val);
    Value::Object(heap.alloc_object(obj))
}

/// Create a rejected promise carrying the given reason.
pub fn make_rejected_promise(reason: Value, heap: &mut Heap) -> Value {
    let mut obj = IndexMap::new();
    obj.insert(Arc::from("__promise__"), Value::Bool(true));
    obj.insert(Arc::from("status"), Value::String(JsString::from("rejected")));
    obj.insert(Arc::from("reason"), reason);
    Value::Object(heap.alloc_object(obj))
}

// ── Helpers ──────────────────────────────────────────────────────────

fn arg_num(args: &[Value], idx: usize) -> f64 {
    args.get(idx).map(|v| v.to_number()).unwrap_or(f64::NAN)
}

/// JS `ToInt32` — wrap to a signed 32-bit integer (NaN/Infinity -> 0).
fn to_int32(n: f64) -> i32 {
    if !n.is_finite() || n == 0.0 {
        return 0;
    }
    let m = n.trunc().rem_euclid(4_294_967_296.0); // [0, 2^32)
    if m >= 2_147_483_648.0 {
        (m - 4_294_967_296.0) as i32
    } else {
        m as i32
    }
}

/// JS `ToUint32` — wrap to an unsigned 32-bit integer (NaN/Infinity -> 0).
fn to_uint32(n: f64) -> u32 {
    if !n.is_finite() || n == 0.0 {
        return 0;
    }
    n.trunc().rem_euclid(4_294_967_296.0) as u32
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
