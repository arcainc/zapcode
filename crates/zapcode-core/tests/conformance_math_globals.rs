//! Conformance breadth: the `Math` namespace (extended) + built-in global surface.
//!
//! Complements `conformance_numbers.rs` (which focuses on `Number.prototype`
//! formatting / parsing) by exercising the `Math` function & constant surface in
//! depth, and pinning the *shape* of the built-in global object surface that an
//! LLM-authored program is likely to feature-detect against.
//!
//! Every numeric value below was cross-checked against real `node -e`.
//!
//! `Math.clz32` / `Math.fround` / `Math.imul`, the inverse hyperbolics
//! (`asinh`/`acosh`/`atanh`), `Object.is` / `Object.create` /
//! `Object.getPrototypeOf` / `Object.getOwnPropertyNames`, and
//! `String.prototype.normalize` are now fully implemented and asserted at their
//! real JS values. `Object.defineProperty` remains a documented residual
//! (`typeof === "function"` so feature-detect guards pass, but invoking throws a
//! catchable error) — out of scope for this round.
//!
//! Also pins the sandbox contract that `globalThis` / `global` access raises a
//! sandbox violation (not silently `undefined`).

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, ZapcodeError, ZapcodeRun};

fn run_str(code: &str) -> String {
    let result = ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        Vec::new(),
        ResourceLimits::default(),
    )
    .unwrap()
    .run(Vec::new())
    .unwrap();
    match result.state {
        VmState::Complete(v) => v.to_js_string(&result.heap),
        other => panic!("expected completion for `{code}`, got {other:?}"),
    }
}

/// Run code expected to error at runtime; return the error.
fn run_err(code: &str) -> ZapcodeError {
    ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        Vec::new(),
        ResourceLimits::default(),
    )
    .unwrap()
    .run(Vec::new())
    .expect_err("expected a runtime error")
}

// ============================================================================
// Math: trigonometry
// ============================================================================

#[test]
fn math_trig_basics() {
    assert_eq!(run_str("Math.sin(0)"), "0");
    assert_eq!(run_str("Math.cos(0)"), "1");
    assert_eq!(run_str("Math.tan(0)"), "0");
    assert_eq!(run_str("Math.asin(0)"), "0");
    assert_eq!(run_str("Math.acos(1)"), "0");
    assert_eq!(run_str("Math.atan(0)"), "0");
}

#[test]
fn math_atan2_quadrants() {
    assert_eq!(run_str("Math.atan2(1, 1).toFixed(5)"), "0.78540");
    assert_eq!(run_str("Math.atan2(0, -1).toFixed(5)"), "3.14159");
    assert_eq!(run_str("Math.atan2(0, 1)"), "0");
    assert_eq!(run_str("Math.atan2(1, 0).toFixed(5)"), "1.57080");
}

// ============================================================================
// Math: hyperbolic (forward implemented; inverse are residuals)
// ============================================================================

#[test]
fn math_hyperbolic_forward() {
    assert_eq!(run_str("Math.sinh(0)"), "0");
    assert_eq!(run_str("Math.cosh(0)"), "1");
    assert_eq!(run_str("Math.tanh(0)"), "0");
    assert_eq!(run_str("Math.cosh(1).toFixed(5)"), "1.54308");
}

#[test]
fn math_inverse_hyperbolics() {
    assert_eq!(run_str("typeof Math.asinh"), "function");
    assert_eq!(run_str("typeof Math.acosh"), "function");
    assert_eq!(run_str("typeof Math.atanh"), "function");
    // asinh(0) === 0, acosh(1) === 0, atanh(0) === 0.
    assert_eq!(run_str("Math.asinh(0)"), "0");
    assert_eq!(run_str("Math.acosh(1)"), "0");
    assert_eq!(run_str("Math.atanh(0)"), "0");
    assert_eq!(run_str("Math.asinh(1).toFixed(5)"), "0.88137");
    assert_eq!(run_str("Math.acosh(2).toFixed(5)"), "1.31696");
    assert_eq!(run_str("Math.atanh(0.5).toFixed(5)"), "0.54931");
}

// ============================================================================
// Math: logarithms & exponentials
// ============================================================================

#[test]
fn math_logs_and_exp() {
    assert_eq!(run_str("Math.log(1)"), "0");
    assert_eq!(run_str("Math.exp(0)"), "1");
    assert_eq!(run_str("Math.exp(1).toFixed(5)"), "2.71828");
    assert_eq!(run_str("Math.log2(8)"), "3");
    assert_eq!(run_str("Math.log2(1024)"), "10");
    assert_eq!(run_str("Math.log10(1000)"), "3");
    assert_eq!(run_str("Math.log10(1)"), "0");
}

#[test]
fn math_log1p_and_expm1() {
    assert_eq!(run_str("Math.log1p(0)"), "0");
    assert_eq!(run_str("Math.log1p(Math.E - 1).toFixed(5)"), "1.00000");
    assert_eq!(run_str("Math.expm1(0)"), "0");
    assert_eq!(run_str("Math.expm1(1).toFixed(5)"), "1.71828");
}

// ============================================================================
// Math: roots, powers, magnitude
// ============================================================================

#[test]
fn math_roots_and_powers() {
    assert_eq!(run_str("Math.sqrt(16)"), "4");
    assert_eq!(run_str("Math.sqrt(2).toFixed(5)"), "1.41421");
    assert_eq!(run_str("Math.cbrt(27)"), "3");
    assert_eq!(run_str("Math.cbrt(-8)"), "-2");
    assert_eq!(run_str("Math.pow(2, 10)"), "1024");
    assert_eq!(run_str("Math.pow(4, 0.5)"), "2");
}

#[test]
fn math_hypot() {
    assert_eq!(run_str("Math.hypot(3, 4)"), "5");
    assert_eq!(run_str("Math.hypot(5, 12)"), "13");
    assert_eq!(run_str("Math.hypot(1, 2, 2)"), "3");
}

#[test]
fn math_abs_and_sign() {
    assert_eq!(run_str("Math.abs(-7)"), "7");
    assert_eq!(run_str("Math.abs(7)"), "7");
    assert_eq!(run_str("Math.sign(-5)"), "-1");
    assert_eq!(run_str("Math.sign(5)"), "1");
    assert_eq!(run_str("Math.sign(0)"), "0");
    // Math.sign(-0) → -0; String(-0) is "0" in both JS and here.
    assert_eq!(run_str("Math.sign(-0)"), "0");
}

// ============================================================================
// Math: rounding family
// ============================================================================

#[test]
fn math_rounding_family() {
    assert_eq!(run_str("Math.floor(4.7)"), "4");
    assert_eq!(run_str("Math.floor(-4.2)"), "-5");
    assert_eq!(run_str("Math.ceil(4.2)"), "5");
    assert_eq!(run_str("Math.ceil(-4.7)"), "-4");
    assert_eq!(run_str("Math.round(4.5)"), "5");
    // Math.round rounds half toward +Infinity, so -4.5 → -4 (not -5).
    assert_eq!(run_str("Math.round(-4.5)"), "-4");
    assert_eq!(run_str("Math.round(2.4)"), "2");
    assert_eq!(run_str("Math.trunc(4.9)"), "4");
    assert_eq!(run_str("Math.trunc(-4.9)"), "-4");
}

#[test]
fn math_min_max() {
    assert_eq!(run_str("Math.min(3, 1, 2)"), "1");
    assert_eq!(run_str("Math.max(3, 1, 2)"), "3");
    assert_eq!(run_str("Math.min(-5, -1)"), "-5");
    assert_eq!(run_str("Math.max(...[1, 9, 3])"), "9");
    assert_eq!(run_str("Math.min(...[4, 2, 8])"), "2");
}

// ============================================================================
// Math: constants
// ============================================================================

#[test]
fn math_constants() {
    assert_eq!(run_str("Math.PI.toFixed(5)"), "3.14159");
    assert_eq!(run_str("Math.E.toFixed(5)"), "2.71828");
    assert_eq!(run_str("Math.SQRT2.toFixed(5)"), "1.41421");
    assert_eq!(run_str("Math.SQRT1_2.toFixed(5)"), "0.70711");
    assert_eq!(run_str("Math.LN2.toFixed(5)"), "0.69315");
    assert_eq!(run_str("Math.LN10.toFixed(5)"), "2.30259");
    assert_eq!(run_str("Math.LOG2E.toFixed(5)"), "1.44270");
    assert_eq!(run_str("Math.LOG10E.toFixed(5)"), "0.43429");
}

#[test]
fn math_constant_relationships() {
    // Internal numeric consistency checks (don't depend on String() formatting).
    assert_eq!(run_str("(Math.SQRT2 * Math.SQRT2).toFixed(5)"), "2.00000");
    assert_eq!(run_str("(Math.LOG2E * Math.LN2).toFixed(5)"), "1.00000");
    assert_eq!(run_str("Math.abs(Math.SQRT1_2 - 1 / Math.SQRT2) < 1e-12"), "true");
}

// ============================================================================
// Math: documented residual functions (typeof-function but throws on call)
// ============================================================================

#[test]
fn math_clz32_fround_imul() {
    assert_eq!(run_str("typeof Math.clz32"), "function");
    assert_eq!(run_str("typeof Math.fround"), "function");
    assert_eq!(run_str("typeof Math.imul"), "function");
    // clz32: leading zeros of the ToUint32 of the argument.
    assert_eq!(run_str("Math.clz32(1)"), "31");
    assert_eq!(run_str("Math.clz32(0)"), "32");
    assert_eq!(run_str("Math.clz32(1000)"), "22");
    // imul: 32-bit integer multiplication (signed, wrapping).
    assert_eq!(run_str("Math.imul(3, 4)"), "12");
    assert_eq!(run_str("Math.imul(-5, 12)"), "-60");
    assert_eq!(run_str("Math.imul(0xffffffff, 5)"), "-5");
    // fround: round to the nearest 32-bit float.
    assert_eq!(run_str("Math.fround(1.5)"), "1.5");
    assert_eq!(run_str("Math.fround(5.05).toFixed(5)"), "5.05000");
}

// ============================================================================
// Object: static surface — implemented vs documented residual
// ============================================================================

#[test]
fn object_implemented_statics() {
    assert_eq!(run_str("Object.keys({ a: 1, b: 2 }).join(',')"), "a,b");
    assert_eq!(run_str("Object.values({ a: 1, b: 2 }).join(',')"), "1,2");
    assert_eq!(run_str("Object.entries({ a: 1 }).length"), "1");
    assert_eq!(run_str("JSON.stringify(Object.fromEntries([['a', 1]]))"), "{\"a\":1}");
    assert_eq!(run_str("Object.assign({}, { a: 1 }, { b: 2 }).b"), "2");
    assert_eq!(run_str("Object.hasOwn({ a: 1 }, 'a')"), "true");
}

#[test]
fn object_reflection_statics() {
    assert_eq!(run_str("typeof Object.is"), "function");
    assert_eq!(run_str("typeof Object.create"), "function");
    assert_eq!(run_str("typeof Object.getPrototypeOf"), "function");
    assert_eq!(run_str("typeof Object.getOwnPropertyNames"), "function");
    // Object.is — SameValue: NaN equals NaN; regular values compare like ===.
    assert_eq!(run_str("String(Object.is(1, 1))"), "true");
    assert_eq!(run_str("String(Object.is(NaN, NaN))"), "true");
    assert_eq!(run_str("String(Object.is(1, 2))"), "false");
    assert_eq!(run_str("String(Object.is('a', 'a'))"), "true");
    // SameValue distinguishes +0 from -0: unary `-0` now yields a real
    // Float(-0.0) (not the integer 0), so Object.is(-0, 0) is false and
    // Object.is(-0, -0) is true, matching JS.
    assert_eq!(run_str("String(Object.is(-0, 0))"), "false");
    assert_eq!(run_str("String(Object.is(-0, -0))"), "true");
    assert_eq!(run_str("String(Object.is(0, 0))"), "true");
    // Object.create yields a usable plain object; the property bag's data
    // descriptors become own properties.
    assert_eq!(run_str("typeof Object.create(null)"), "object");
    assert_eq!(run_str("Object.create({}, { x: { value: 42 } }).x"), "42");
    // Object.getPrototypeOf returns an object for object/array operands.
    assert_eq!(run_str("typeof Object.getPrototypeOf({})"), "object");
    // Object.getOwnPropertyNames lists own (data) property names.
    assert_eq!(run_str("Object.getOwnPropertyNames({ a: 1, b: 2 }).join(',')"), "a,b");
}

// ============================================================================
// String.prototype.normalize — documented residual
// ============================================================================

#[test]
fn string_normalize() {
    assert_eq!(run_str("typeof 'x'.normalize"), "function");
    // Default form is NFC; an already-composed string is unchanged.
    assert_eq!(run_str("'café'.normalize()"), "café");
    // A decomposed "e + combining acute" normalizes (NFC) to the single é.
    assert_eq!(run_str("'cafe\\u0301'.normalize('NFC') === 'café'"), "true");
    assert_eq!(run_str("String('cafe\\u0301'.normalize('NFC').length)"), "4");
    // NFD decomposes the composed é into two code units.
    assert_eq!(run_str("String('café'.normalize('NFD').length)"), "5");
    // An invalid form throws a RangeError (as in real JS).
    assert_eq!(
        run_str("try { 'x'.normalize('BOGUS'); 'no' } catch (e) { e.name }"),
        "RangeError"
    );
}

// ============================================================================
// Sandbox-forbidden globals
// ============================================================================

#[test]
fn globalthis_access_is_a_sandbox_violation() {
    let err = run_err("globalThis");
    assert!(
        matches!(err, ZapcodeError::SandboxViolation(_)),
        "expected SandboxViolation, got {err:?}"
    );
    let err2 = run_err("typeof globalThis");
    assert!(
        matches!(err2, ZapcodeError::SandboxViolation(_)),
        "expected SandboxViolation, got {err2:?}"
    );
}

// ============================================================================
// Realistic Math-driven programs
// ============================================================================

#[test]
fn math_statistics_pipeline() {
    // mean / max / min / range over a dataset.
    assert_eq!(
        run_str(
            "const xs = [4, 8, 15, 16, 23, 42]; \
             const sum = xs.reduce((a, b) => a + b, 0); \
             const mean = sum / xs.length; \
             [mean, Math.max(...xs), Math.min(...xs), Math.max(...xs) - Math.min(...xs)].join(',')"
        ),
        "18,42,4,38"
    );
}

#[test]
fn math_geometry_distance() {
    assert_eq!(
        run_str(
            "const dist = (x1, y1, x2, y2) => Math.hypot(x2 - x1, y2 - y1); \
             dist(0, 0, 3, 4)"
        ),
        "5"
    );
}
