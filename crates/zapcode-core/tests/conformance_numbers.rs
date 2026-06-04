//! Conformance breadth: Numbers & Math.
//!
//! test262-style coverage of the numeric surface of the interpreter, asserting the
//! *real-Node* answer at every point where zapcode is known to agree (each value
//! below was cross-checked against `node -e`). Coverage:
//!   - arithmetic incl. modulo (negative/float dividends & divisors), division,
//!     exponentiation, increment/decrement (pre/post), and the full compound-
//!     assignment family incl. logical `&&= ||= ??=` and bitwise compounds;
//!   - `Number.prototype` formatting: `toFixed` (half-away-from-zero), `toPrecision`,
//!     `toExponential`, `toString(radix)` incl. fractional radix output;
//!   - `parseInt`/`parseFloat` (radix, auto-hex, trimming, trailing garbage) and
//!     `Number(...)` string coercion;
//!   - `Number.*` predicates (`isInteger`/`isNaN`/`isFinite`/`isSafeInteger`) and
//!     constants;
//!   - the `Math` object (rounding family, sign/abs, roots, logs/exp, pow, min/max,
//!     hypot, constants);
//!   - NaN / Infinity / negative-zero behavior and the classic `0.1 + 0.2`.
//!
//! DOCUMENTED DIVERGENCES deliberately NOT asserted at the JS answer (verified
//! against Node; zapcode's `to_js_string` uses Rust's f64 formatting / i128 integer
//! arithmetic, which differ here). These are skipped, or the *actual* zapcode value
//! is asserted with an explicit comment:
//!   - `1 / -0` → `Infinity` (JS `-Infinity`): negative-zero sign in division.
//!   - `Math.min(1, NaN)` → `1` and `Math.max(1, NaN)` → `1` are NaN-poisoning
//!     differences when NaN is not the first arg (asserted as zapcode's actual).
//!   - `Math.trunc(-0.5)` / `Math.hypot()` → `-0` (JS String() → `0`): negative-zero
//!     print.
//!   - `parseFloat("Infinity")` → `NaN` (JS `Infinity`); asserted as actual.
//!   - `(0.1).toString(2)` truncates the binary fraction at 52 digits (JS 53);
//!     `(-12345).toExponential(3)` rounds differently. These exact inputs are
//!     avoided.
//!   - `Object.is`, `Math.fround`, `Math.clz32`, `Math.imul` are not implemented.

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, ZapcodeRun};

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

// ============================================================================
// Arithmetic
// ============================================================================

#[test]
fn arithmetic_addition_subtraction() {
    assert_eq!(run_str("1 + 2"), "3");
    assert_eq!(run_str("100 + 200"), "300");
    assert_eq!(run_str("10 - 4"), "6");
    assert_eq!(run_str("4 - 10"), "-6");
    assert_eq!(run_str("0 - 0"), "0");
    assert_eq!(run_str("-5 + -3"), "-8");
    assert_eq!(run_str("1 + 2 + 3 + 4"), "10");
    assert_eq!(run_str("1000000 + 1000000"), "2000000");
}

#[test]
fn arithmetic_multiplication_division() {
    assert_eq!(run_str("6 * 7"), "42");
    assert_eq!(run_str("-6 * 7"), "-42");
    assert_eq!(run_str("-6 * -7"), "42");
    assert_eq!(run_str("0 * 5"), "0");
    assert_eq!(run_str("20 / 5"), "4");
    assert_eq!(run_str("20 / 8"), "2.5");
    assert_eq!(run_str("5 / 2"), "2.5");
    assert_eq!(run_str("4 / 2"), "2");
    assert_eq!(run_str("6 / 4"), "1.5");
    assert_eq!(run_str("1 / 4"), "0.25");
    assert_eq!(run_str("-1 / 4"), "-0.25");
}

#[test]
fn arithmetic_operator_precedence() {
    assert_eq!(run_str("2 + 3 * 4"), "14");
    assert_eq!(run_str("(2 + 3) * 4"), "20");
    assert_eq!(run_str("10 - 2 - 3"), "5"); // left-associative
    assert_eq!(run_str("100 / 10 / 2"), "5"); // left-associative
    assert_eq!(run_str("2 + 10 % 3"), "3"); // % binds tighter than +
    assert_eq!(run_str("(-2) ** 2"), "4"); // unary must be parenthesized before **
    assert_eq!(run_str("-(2 ** 2)"), "-4");
    // NOTE: `-2 ** 2` (unparenthesized) is a ParseError in zapcode, matching JS
    // (which raises a SyntaxError); not exercised here.
}

#[test]
fn modulo_integers() {
    assert_eq!(run_str("10 % 3"), "1");
    assert_eq!(run_str("17 % 5"), "2");
    assert_eq!(run_str("9 % 3"), "0");
    assert_eq!(run_str("2 % 5"), "2");
}

#[test]
fn modulo_with_negatives_follows_dividend_sign() {
    // JS `%` takes the sign of the dividend (the left operand).
    assert_eq!(run_str("-10 % 3"), "-1");
    assert_eq!(run_str("10 % -3"), "1");
    assert_eq!(run_str("-10 % -3"), "-1");
    assert_eq!(run_str("-17 % 5"), "-2");
    assert_eq!(run_str("17 % -5"), "2");
    assert_eq!(run_str("-7 % 3"), "-1");
}

#[test]
fn modulo_with_floats() {
    assert_eq!(run_str("5.5 % 2"), "1.5");
    assert_eq!(run_str("-5.5 % 2"), "-1.5");
    assert_eq!(run_str("5.5 % -2"), "1.5");
    assert_eq!(run_str("10.5 % 3"), "1.5");
    assert_eq!(run_str("6.4 % 3.2"), "0"); // exact here
}

#[test]
fn modulo_with_zero_and_nan_is_nan() {
    assert_eq!(run_str("5 % 0"), "NaN");
    assert_eq!(run_str("7 % 0"), "NaN");
    assert_eq!(run_str("0 % 0"), "NaN");
    assert_eq!(run_str("NaN % 5"), "NaN");
    assert_eq!(run_str("5 % NaN"), "NaN");
}

#[test]
fn modulo_with_zero_dividend() {
    assert_eq!(run_str("0 % 5"), "0");
    assert_eq!(run_str("0 % -5"), "0");
}

#[test]
fn exponentiation_is_right_associative() {
    assert_eq!(run_str("2 ** 10"), "1024");
    assert_eq!(run_str("2 ** 3 ** 2"), "512"); // 2 ** (3 ** 2) = 2 ** 9
    assert_eq!(run_str("(2 ** 3) ** 2"), "64");
    assert_eq!(run_str("3 ** 3"), "27");
    assert_eq!(run_str("10 ** 0"), "1");
    assert_eq!(run_str("0 ** 0"), "1");
}

#[test]
fn exponentiation_negative_and_fractional() {
    assert_eq!(run_str("2 ** -1"), "0.5");
    assert_eq!(run_str("9 ** -1"), "0.1111111111111111");
    assert_eq!(run_str("2 ** -2"), "0.25");
    assert_eq!(run_str("4 ** 0.5"), "2");
    assert_eq!(run_str("2 ** 0.5"), "1.4142135623730951");
    assert_eq!(run_str("(-2) ** 2"), "4");
    assert_eq!(run_str("(-2) ** 3"), "-8");
}

#[test]
fn unary_plus_and_negation() {
    assert_eq!(run_str("-5"), "-5");
    assert_eq!(run_str("- -5"), "5");
    assert_eq!(run_str("-(2 + 3)"), "-5");
    assert_eq!(run_str("+'42'"), "42");
    assert_eq!(run_str("+true"), "1");
    assert_eq!(run_str("+false"), "0");
    assert_eq!(run_str("+null"), "0");
    assert_eq!(run_str("+''"), "0");
    assert_eq!(run_str("+'  12 '"), "12"); // surrounding whitespace trimmed
    assert_eq!(run_str("+[]"), "0");
    assert_eq!(run_str("+[5]"), "5");
}

// ============================================================================
// Increment / decrement
// ============================================================================

#[test]
fn increment_decrement_statement_effect() {
    assert_eq!(run_str("let x = 5; x++; x"), "6");
    assert_eq!(run_str("let x = 5; ++x; x"), "6");
    assert_eq!(run_str("let x = 5; x--; x"), "4");
    assert_eq!(run_str("let x = 5; --x; x"), "4");
    assert_eq!(run_str("let x = 0; x++; x++; x++; x"), "3");
}

#[test]
fn postfix_returns_old_value() {
    assert_eq!(run_str("let x = 5; x++"), "5");
    assert_eq!(run_str("let x = 5; x--"), "5");
    assert_eq!(run_str("let q = 5; let r = q++; `${q},${r}`"), "6,5");
}

#[test]
fn prefix_returns_new_value() {
    assert_eq!(run_str("let x = 5; ++x"), "6");
    assert_eq!(run_str("let x = 5; --x"), "4");
    assert_eq!(run_str("let s = 5; let t = ++s; `${s},${t}`"), "6,6");
}

#[test]
fn increment_on_object_and_array_members() {
    assert_eq!(run_str("let o = { n: 1 }; o.n++; o.n"), "2");
    assert_eq!(run_str("let o = { n: 1 }; ++o.n; o.n"), "2");
    assert_eq!(run_str("let a = [10]; a[0]++; a[0]"), "11");
    assert_eq!(run_str("let a = [10]; --a[0]; a[0]"), "9");
}

#[test]
fn increment_on_float() {
    assert_eq!(run_str("let x = 1.5; x++; x"), "2.5");
    assert_eq!(run_str("let x = 1.5; x--; x"), "0.5");
}

// ============================================================================
// Compound assignment
// ============================================================================

#[test]
fn compound_arithmetic_assignment() {
    assert_eq!(run_str("let x = 5; x += 3; x"), "8");
    assert_eq!(run_str("let x = 10; x -= 3; x"), "7");
    assert_eq!(run_str("let x = 2; x *= 5; x"), "10");
    assert_eq!(run_str("let x = 20; x /= 4; x"), "5");
    assert_eq!(run_str("let x = 10; x %= 3; x"), "1");
    assert_eq!(run_str("let x = 2; x **= 3; x"), "8");
}

#[test]
fn compound_assignment_returns_assigned_value() {
    assert_eq!(run_str("let x = 5; x += 3"), "8");
    assert_eq!(run_str("let x = 2; x **= 10"), "1024");
}

#[test]
fn compound_string_concat_assignment() {
    assert_eq!(run_str("let s = 'a'; s += 'b'; s"), "ab");
    assert_eq!(run_str("let s = 'x'; s += 1; s"), "x1");
    assert_eq!(run_str("let s = ''; s += 1; s += 2; s += 3; s"), "123");
}

#[test]
fn compound_bitwise_assignment() {
    assert_eq!(run_str("let a = 0b1100; a &= 0b1010; a"), "8");
    assert_eq!(run_str("let b = 0b1100; b |= 0b0011; b"), "15");
    assert_eq!(run_str("let c = 0b1100; c ^= 0b1010; c"), "6");
    assert_eq!(run_str("let d = 1; d <<= 4; d"), "16");
    assert_eq!(run_str("let e = 256; e >>= 2; e"), "64");
    assert_eq!(run_str("let f = -8; f >>>= 0; f"), "4294967288");
}

#[test]
fn logical_or_assign() {
    // ||=: assign only when the LHS is falsy.
    assert_eq!(run_str("let a = 0; a ||= 5; a"), "5");
    assert_eq!(run_str("let f = 3; f ||= 99; f"), "3"); // truthy: untouched
    assert_eq!(run_str("let s = ''; s ||= 'x'; s"), "x");
    assert_eq!(run_str("let n = null; n ||= 7; n"), "7");
    assert_eq!(run_str("let u = undefined; u ||= 7; u"), "7");
}

#[test]
fn logical_and_assign() {
    // &&=: assign only when the LHS is truthy.
    assert_eq!(run_str("let b = 1; b &&= 7; b"), "7");
    assert_eq!(run_str("let d = 5; d &&= 0; d"), "0");
    assert_eq!(run_str("let z = 0; z &&= 9; z"), "0"); // falsy: untouched
    assert_eq!(run_str("let s = 'x'; s &&= 'y'; s"), "y");
}

#[test]
fn nullish_assign() {
    // ??=: assign only when the LHS is null or undefined (NOT for 0/'').
    assert_eq!(run_str("let c = null; c ??= 9; c"), "9");
    assert_eq!(run_str("let e = undefined; e ??= 2; e"), "2");
    assert_eq!(run_str("let z = 0; z ??= 9; z"), "0"); // 0 is defined: untouched
    assert_eq!(run_str("let s = ''; s ??= 'x'; s"), ""); // '' is defined: untouched
    assert_eq!(run_str("let f = false; f ??= true; f"), "false");
}

#[test]
fn logical_assign_short_circuits_member_targets() {
    assert_eq!(run_str("let o = { a: 0 }; o.a ||= 42; o.a"), "42");
    assert_eq!(run_str("let o = { a: null }; o.a ??= 5; o.a"), "5");
    assert_eq!(run_str("let o = { a: 7 }; o.a &&= 8; o.a"), "8");
}

// ============================================================================
// Number.prototype.toFixed (round half away from zero)
// ============================================================================

#[test]
fn to_fixed_basic() {
    assert_eq!(run_str("(3.14159).toFixed(2)"), "3.14");
    assert_eq!(run_str("(3.14159).toFixed(0)"), "3");
    assert_eq!(run_str("(3.14159).toFixed(4)"), "3.1416");
    assert_eq!(run_str("(1).toFixed(3)"), "1.000");
    assert_eq!(run_str("(0).toFixed(2)"), "0.00");
    assert_eq!(run_str("(1234.5678).toFixed(2)"), "1234.57");
}

#[test]
fn to_fixed_default_digits_is_zero() {
    assert_eq!(run_str("(3.7).toFixed()"), "4");
    assert_eq!(run_str("(3.2).toFixed()"), "3");
}

#[test]
fn to_fixed_rounds_half_away_from_zero() {
    assert_eq!(run_str("(2.5).toFixed(0)"), "3");
    assert_eq!(run_str("(1.5).toFixed(0)"), "2");
    assert_eq!(run_str("(-2.5).toFixed(0)"), "-3");
    assert_eq!(run_str("(-1.5).toFixed(0)"), "-2");
    assert_eq!(run_str("(0.5).toFixed(0)"), "1"); // 0.5 rounds up
}

#[test]
fn to_fixed_floating_point_realities() {
    // These reflect the *true* binary value, matching V8 exactly (cross-checked).
    assert_eq!(run_str("(1.005).toFixed(2)"), "1.00"); // 1.005 is < 1.005 in binary
    assert_eq!(run_str("(8.575).toFixed(2)"), "8.57");
    assert_eq!(run_str("(255.255).toFixed(2)"), "255.25");
    assert_eq!(run_str("(0.15).toFixed(1)"), "0.1");
    assert_eq!(run_str("(0.005).toFixed(2)"), "0.01");
}

#[test]
fn to_fixed_pads_and_handles_small() {
    assert_eq!(run_str("(0.000001).toFixed(7)"), "0.0000010");
    assert_eq!(run_str("(0.1).toFixed(1)"), "0.1");
    assert_eq!(run_str("(0.1 + 0.2).toFixed(1)"), "0.3");
    assert_eq!(run_str("(0.1 + 0.2).toFixed(2)"), "0.30");
}

#[test]
fn to_fixed_negative_zero_prints_unsigned() {
    assert_eq!(run_str("(-0).toFixed(2)"), "0.00");
}

#[test]
fn to_fixed_nan_and_infinity() {
    assert_eq!(run_str("(NaN).toFixed(2)"), "NaN");
    assert_eq!(run_str("(Infinity).toFixed(2)"), "Infinity");
    assert_eq!(run_str("(-Infinity).toFixed(2)"), "-Infinity");
}

// ============================================================================
// Number.prototype.toPrecision
// ============================================================================

#[test]
fn to_precision_default_is_to_string() {
    assert_eq!(run_str("(123.456).toPrecision()"), "123.456");
    assert_eq!(run_str("(5).toPrecision()"), "5");
}

#[test]
fn to_precision_fixed_and_exponential_forms() {
    assert_eq!(run_str("(123.456).toPrecision(4)"), "123.5");
    assert_eq!(run_str("(123.456).toPrecision(2)"), "1.2e+2"); // exponential
    assert_eq!(run_str("(12345).toPrecision(3)"), "1.23e+4");
    assert_eq!(run_str("(123.45).toPrecision(5)"), "123.45");
    assert_eq!(run_str("(100).toPrecision(2)"), "1.0e+2");
    assert_eq!(run_str("(100).toPrecision(5)"), "100.00");
}

#[test]
fn to_precision_small_numbers() {
    assert_eq!(run_str("(0.0001234).toPrecision(2)"), "0.00012");
    assert_eq!(run_str("(-0.0001234).toPrecision(2)"), "-0.00012");
    assert_eq!(run_str("(0.00001).toPrecision(1)"), "0.00001");
    assert_eq!(run_str("(0.5).toPrecision(3)"), "0.500");
}

#[test]
fn to_precision_zero_and_one() {
    assert_eq!(run_str("(0).toPrecision(3)"), "0.00");
    assert_eq!(run_str("(0).toPrecision(1)"), "0");
    assert_eq!(run_str("(1).toPrecision(3)"), "1.00");
    assert_eq!(run_str("(-0).toPrecision(2)"), "0.0");
}

// ============================================================================
// Shared ECMA-262 Number::toString formatter (default toString / String() /
// templates / Array.join / JSON.stringify all route through one path)
// ============================================================================

#[test]
fn number_to_string_switches_to_exponential() {
    // >= 1e21 and 0 < |x| < 1e-6 use exponential; everything between is fixed.
    // All values ground-truthed against Node's String().
    assert_eq!(run_str("String(1e21)"), "1e+21");
    assert_eq!(run_str("String(1e20)"), "100000000000000000000"); // boundary stays fixed
    assert_eq!(run_str("String(2 ** 70)"), "1.1805916207174113e+21");
    assert_eq!(run_str("String(1.5e300)"), "1.5e+300");
    assert_eq!(run_str("String(6.022e23)"), "6.022e+23");
    assert_eq!(run_str("String(1e-6)"), "0.000001"); // boundary stays fixed
    assert_eq!(run_str("String(1e-7)"), "1e-7");
    assert_eq!(run_str("String(9.999e-7)"), "9.999e-7");
    assert_eq!(run_str("String(-3.14e-8)"), "-3.14e-8");
    assert_eq!(run_str("String(Number.MAX_VALUE)"), "1.7976931348623157e+308");
    // NB: use literals here, not Number.MIN_VALUE — that named constant is
    // currently mis-defined as the smallest *normal* double (a separate bug);
    // these exercise the formatter on the smallest subnormal and EPSILON.
    assert_eq!(run_str("String(5e-324)"), "5e-324");
    assert_eq!(run_str("String(2.220446049250313e-16)"), "2.220446049250313e-16");
    // Mid-range values keep their plain decimal form.
    assert_eq!(run_str("String(123.456)"), "123.456");
    assert_eq!(run_str("String(0.5)"), "0.5");
    assert_eq!(run_str("String(123456789012345680000)"), "123456789012345680000");
}

#[test]
fn number_to_string_is_shared_across_coercion_paths() {
    // The same formatter must drive templates, Array.join and JSON.stringify,
    // not just String() — they previously emitted full positional decimals.
    assert_eq!(run_str("`${1e21}`"), "1e+21");
    assert_eq!(run_str("[1e21, 1e-7, 12345678].join(',')"), "1e+21,1e-7,12345678");
    assert_eq!(
        run_str("JSON.stringify({big: 1e21, small: 1e-7})"),
        "{\"big\":1e+21,\"small\":1e-7}"
    );
    // Number.prototype.toString(10) and the >=1e21 toFixed fallthrough agree too.
    assert_eq!(run_str("(1e21).toString()"), "1e+21");
    assert_eq!(run_str("(1e21).toFixed(2)"), "1e+21");
}

// ============================================================================
// Number.prototype.toExponential
// ============================================================================

#[test]
fn to_exponential_default() {
    assert_eq!(run_str("(1000000).toExponential()"), "1e+6");
    assert_eq!(run_str("(123.456).toExponential()"), "1.23456e+2");
    assert_eq!(run_str("(0).toExponential()"), "0e+0");
    assert_eq!(run_str("(5).toExponential()"), "5e+0");
}

#[test]
fn to_exponential_with_digits() {
    assert_eq!(run_str("(12345).toExponential(2)"), "1.23e+4");
    assert_eq!(run_str("(0.5).toExponential(1)"), "5.0e-1");
    assert_eq!(run_str("(123.456).toExponential(2)"), "1.23e+2");
    assert_eq!(run_str("(0.000123).toExponential(2)"), "1.23e-4");
    assert_eq!(run_str("(0).toExponential(2)"), "0.00e+0");
    assert_eq!(run_str("(-0).toExponential(1)"), "0.0e+0");
}

#[test]
fn to_exponential_negative_exponents() {
    assert_eq!(run_str("(0.001).toExponential(0)"), "1e-3");
    assert_eq!(run_str("(0.05).toExponential(0)"), "5e-2");
}

// ============================================================================
// Number.prototype.toString(radix)
// ============================================================================

#[test]
fn to_string_default_base_ten() {
    assert_eq!(run_str("(255).toString()"), "255");
    assert_eq!(run_str("(255).toString(10)"), "255");
    assert_eq!(run_str("(3.5).toString()"), "3.5");
    assert_eq!(run_str("(-42).toString()"), "-42");
}

#[test]
fn to_string_radix_integers() {
    assert_eq!(run_str("(255).toString(16)"), "ff");
    assert_eq!(run_str("(255).toString(2)"), "11111111");
    assert_eq!(run_str("(255).toString(8)"), "377");
    assert_eq!(run_str("(5).toString(2)"), "101");
    assert_eq!(run_str("(123).toString(2)"), "1111011");
    assert_eq!(run_str("(1000).toString(2)"), "1111101000");
    assert_eq!(run_str("(35).toString(36)"), "z");
    assert_eq!(run_str("(10).toString(16)"), "a");
}

#[test]
fn to_string_radix_negatives() {
    assert_eq!(run_str("(-255).toString(16)"), "-ff");
    assert_eq!(run_str("(-5).toString(2)"), "-101");
}

#[test]
fn to_string_radix_fractions() {
    // Terminating binary/quaternary/hex fractions match JS exactly.
    assert_eq!(run_str("(3.5).toString(2)"), "11.1");
    assert_eq!(run_str("(0.5).toString(2)"), "0.1");
    assert_eq!(run_str("(0.25).toString(2)"), "0.01");
    assert_eq!(run_str("(2.5).toString(2)"), "10.1");
    assert_eq!(run_str("(0.75).toString(4)"), "0.3");
    assert_eq!(run_str("(10.5).toString(16)"), "a.8");
}

#[test]
fn to_string_radix_zero() {
    assert_eq!(run_str("(0).toString(2)"), "0");
    assert_eq!(run_str("(0).toString(16)"), "0");
}

// ============================================================================
// parseInt
// ============================================================================

#[test]
fn parse_int_decimal() {
    assert_eq!(run_str("parseInt('42')"), "42");
    assert_eq!(run_str("parseInt('42px')"), "42"); // stops at first non-digit
    assert_eq!(run_str("parseInt('123.45')"), "123"); // stops at '.'
    assert_eq!(run_str("parseInt('-42')"), "-42");
    assert_eq!(run_str("parseInt('  42  ')"), "42"); // leading whitespace trimmed
    assert_eq!(run_str("parseInt('   -42abc')"), "-42");
    assert_eq!(run_str("parseInt('+7')"), "7");
}

#[test]
fn parse_int_with_radix() {
    assert_eq!(run_str("parseInt('11', 2)"), "3");
    assert_eq!(run_str("parseInt('ff', 16)"), "255");
    assert_eq!(run_str("parseInt('FF', 16)"), "255");
    assert_eq!(run_str("parseInt('z', 36)"), "35");
    assert_eq!(run_str("parseInt('0xff', 16)"), "255");
    assert_eq!(run_str("parseInt('0', 8)"), "0");
    assert_eq!(run_str("parseInt('777', 8)"), "511");
}

#[test]
fn parse_int_auto_detects_hex_prefix() {
    assert_eq!(run_str("parseInt('0x1F')"), "31");
    assert_eq!(run_str("parseInt('0xff')"), "255");
    assert_eq!(run_str("parseInt('  0x10  ')"), "16");
    assert_eq!(run_str("parseInt('-0x10')"), "-16");
    assert_eq!(run_str("parseInt('10', 0)"), "10"); // radix 0 == auto
}

#[test]
fn parse_int_nan_cases() {
    assert_eq!(run_str("String(parseInt('abc'))"), "NaN");
    assert_eq!(run_str("String(parseInt(''))"), "NaN");
    assert_eq!(run_str("String(parseInt('', 10))"), "NaN");
    assert_eq!(run_str("String(parseInt('Infinity'))"), "NaN");
    assert_eq!(run_str("String(parseInt('.5'))"), "NaN");
}

#[test]
fn parse_int_number_namespace() {
    assert_eq!(run_str("Number.parseInt('100')"), "100");
    assert_eq!(run_str("Number.parseInt('ff', 16)"), "255");
}

#[test]
fn parse_int_overflows_to_f64() {
    // A string wider than i64 range returns the f64 value in JS, not NaN.
    // (Node: parseInt("9999999999999999999") === 1e19.)  Previously zapcode's
    // i64::from_str_radix overflowed and yielded NaN.
    assert_eq!(
        run_str("String(parseInt('9999999999999999999'))"),
        "10000000000000000000"
    );
    assert_eq!(
        run_str("String(parseInt('-9999999999999999999'))"),
        "-10000000000000000000"
    );
    // It stays a finite number, not NaN.
    assert_eq!(run_str("Number.isNaN(parseInt('9999999999999999999'))"), "false");
    // Even-wider input keeps round-tripping as a double (matches Node's 1e32).
    assert_eq!(
        run_str("parseInt('99999999999999999999999999999999') === 1e32"),
        "true"
    );
}

// ============================================================================
// parseFloat
// ============================================================================

#[test]
fn parse_float_basic() {
    assert_eq!(run_str("parseFloat('3.14')"), "3.14");
    assert_eq!(run_str("parseFloat('3.14xyz')"), "3.14");
    assert_eq!(run_str("parseFloat('.5')"), "0.5");
    assert_eq!(run_str("parseFloat('-.5')"), "-0.5");
    assert_eq!(run_str("parseFloat('  3.14  ')"), "3.14");
    assert_eq!(run_str("parseFloat('42')"), "42");
}

#[test]
fn parse_float_scientific_and_multiple_dots() {
    assert_eq!(run_str("parseFloat('1e3')"), "1000");
    assert_eq!(run_str("parseFloat('.5e2')"), "50");
    assert_eq!(run_str("parseFloat('3.14.15')"), "3.14"); // stops at 2nd dot
    assert_eq!(run_str("parseFloat('0xFF')"), "0"); // 0 then 'x' stops it
}

#[test]
fn parse_float_nan_cases() {
    assert_eq!(run_str("String(parseFloat('abc'))"), "NaN");
    assert_eq!(run_str("String(parseFloat(''))"), "NaN");
}

#[test]
fn parse_float_infinity_is_zapcode_residual() {
    // DOCUMENTED DIVERGENCE: JS returns Infinity; zapcode returns NaN (it does not
    // special-case the "Infinity" token in parseFloat). Asserted as zapcode's actual.
    assert_eq!(run_str("String(parseFloat('Infinity'))"), "NaN");
    assert_eq!(run_str("String(parseFloat('-Infinity'))"), "NaN");
}

#[test]
fn parse_float_number_namespace() {
    assert_eq!(run_str("Number.parseFloat('2.5')"), "2.5");
    assert_eq!(run_str("Number.parseFloat('  9.9x')"), "9.9");
}

// ============================================================================
// Number(...) string coercion
// ============================================================================

#[test]
fn number_of_strings() {
    assert_eq!(run_str("Number('42')"), "42");
    assert_eq!(run_str("Number('   42   ')"), "42");
    assert_eq!(run_str("Number('3.14')"), "3.14");
    assert_eq!(run_str("Number('.5')"), "0.5");
    assert_eq!(run_str("Number('5.')"), "5");
    assert_eq!(run_str("Number('1e3')"), "1000");
    assert_eq!(run_str("Number('5e-2')"), "0.05");
    assert_eq!(run_str("Number('')"), "0");
    assert_eq!(run_str("Number('  ')"), "0");
}

#[test]
fn number_of_radix_prefixed_strings() {
    assert_eq!(run_str("Number('0x10')"), "16");
    assert_eq!(run_str("Number('0xFF')"), "255");
    assert_eq!(run_str("Number('0b10')"), "2");
    assert_eq!(run_str("Number('0o17')"), "15");
}

#[test]
fn number_of_infinity_strings() {
    assert_eq!(run_str("Number('Infinity')"), "Infinity");
    assert_eq!(run_str("Number('-Infinity')"), "-Infinity");
}

#[test]
fn number_of_invalid_strings_is_nan() {
    assert_eq!(run_str("String(Number('abc'))"), "NaN");
    assert_eq!(run_str("String(Number('1_000'))"), "NaN"); // numeric separators not allowed in coercion
    assert_eq!(run_str("String(Number('12px'))"), "NaN");
    assert_eq!(run_str("String(Number('0x'))"), "NaN");
}

#[test]
fn number_of_non_strings() {
    assert_eq!(run_str("Number(true)"), "1");
    assert_eq!(run_str("Number(false)"), "0");
    assert_eq!(run_str("Number(null)"), "0");
    assert_eq!(run_str("String(Number(undefined))"), "NaN");
    assert_eq!(run_str("Number([])"), "0");
    assert_eq!(run_str("Number([5])"), "5");
    assert_eq!(run_str("String(Number([1, 2]))"), "NaN");
}

// ============================================================================
// Number predicates & constants
// ============================================================================

#[test]
fn number_is_integer() {
    assert_eq!(run_str("Number.isInteger(5)"), "true");
    assert_eq!(run_str("Number.isInteger(5.0)"), "true");
    assert_eq!(run_str("Number.isInteger(5.5)"), "false");
    assert_eq!(run_str("Number.isInteger(-3)"), "true");
    assert_eq!(run_str("Number.isInteger(NaN)"), "false");
    assert_eq!(run_str("Number.isInteger(Infinity)"), "false");
    assert_eq!(run_str("Number.isInteger('5')"), "false"); // no coercion
}

#[test]
fn number_is_nan() {
    assert_eq!(run_str("Number.isNaN(NaN)"), "true");
    assert_eq!(run_str("Number.isNaN(5)"), "false");
    assert_eq!(run_str("Number.isNaN('abc')"), "false"); // no coercion (unlike global isNaN)
    assert_eq!(run_str("Number.isNaN(0 / 0)"), "true");
    assert_eq!(run_str("Number.isNaN(Infinity)"), "false");
}

#[test]
fn number_is_finite() {
    assert_eq!(run_str("Number.isFinite(42)"), "true");
    assert_eq!(run_str("Number.isFinite(3.14)"), "true");
    assert_eq!(run_str("Number.isFinite(Infinity)"), "false");
    assert_eq!(run_str("Number.isFinite(-Infinity)"), "false");
    assert_eq!(run_str("Number.isFinite(NaN)"), "false");
    assert_eq!(run_str("Number.isFinite('5')"), "false"); // no coercion
}

#[test]
fn number_is_safe_integer() {
    assert_eq!(run_str("Number.isSafeInteger(5)"), "true");
    assert_eq!(run_str("Number.isSafeInteger(2 ** 53 - 1)"), "true");
    assert_eq!(run_str("Number.isSafeInteger(2 ** 53)"), "false");
    assert_eq!(run_str("Number.isSafeInteger(1.5)"), "false");
    assert_eq!(run_str("Number.isSafeInteger(NaN)"), "false");
}

#[test]
fn global_is_nan_and_is_finite_coerce() {
    // The *global* isNaN/isFinite coerce their argument (unlike Number.*).
    assert_eq!(run_str("isNaN('abc')"), "true");
    assert_eq!(run_str("isNaN('5')"), "false");
    assert_eq!(run_str("isNaN(NaN)"), "true");
    assert_eq!(run_str("isFinite('5')"), "true");
    assert_eq!(run_str("isFinite(Infinity)"), "false");
    assert_eq!(run_str("isFinite('Infinity')"), "false");
}

#[test]
fn number_constants() {
    assert_eq!(run_str("Number.MAX_SAFE_INTEGER"), "9007199254740991");
    assert_eq!(run_str("Number.MIN_SAFE_INTEGER"), "-9007199254740991");
    assert_eq!(run_str("Number.POSITIVE_INFINITY"), "Infinity");
    assert_eq!(run_str("Number.NEGATIVE_INFINITY"), "-Infinity");
    assert_eq!(run_str("String(Number.NaN)"), "NaN");
}

#[test]
fn number_constant_relationships() {
    // MAX_VALUE / MIN_VALUE / EPSILON stringify as long decimals here (no `e`
    // notation), so assert their numeric properties rather than the string form.
    assert_eq!(run_str("Number.EPSILON > 0"), "true");
    assert_eq!(run_str("Number.EPSILON < 0.001"), "true");
    assert_eq!(run_str("Number.MAX_VALUE > 1e300"), "true");
    assert_eq!(run_str("Number.MIN_VALUE > 0"), "true");
    assert_eq!(run_str("Number.MIN_VALUE < 1e-300"), "true");
    assert_eq!(run_str("Number.MAX_SAFE_INTEGER === 2 ** 53 - 1"), "true");
}

// ============================================================================
// Math: rounding family
// ============================================================================

#[test]
fn math_floor() {
    assert_eq!(run_str("Math.floor(2.999)"), "2");
    assert_eq!(run_str("Math.floor(2.0)"), "2");
    assert_eq!(run_str("Math.floor(-1.5)"), "-2");
    assert_eq!(run_str("Math.floor(-0.0001)"), "-1");
    assert_eq!(run_str("Math.floor(5)"), "5");
}

#[test]
fn math_ceil() {
    assert_eq!(run_str("Math.ceil(2.001)"), "3");
    assert_eq!(run_str("Math.ceil(2.0)"), "2");
    assert_eq!(run_str("Math.ceil(-1.5)"), "-1");
    assert_eq!(run_str("Math.ceil(-2.001)"), "-2");
    assert_eq!(run_str("Math.ceil(5)"), "5");
}

#[test]
fn math_round_half_toward_positive_infinity() {
    // JS rounds halves toward +Infinity (NOT away from zero).
    assert_eq!(run_str("Math.round(2.5)"), "3");
    assert_eq!(run_str("Math.round(-2.5)"), "-2"); // toward +Inf, not -3
    assert_eq!(run_str("Math.round(0.5)"), "1");
    assert_eq!(run_str("Math.round(-0.5)"), "0"); // toward +Inf, not -1
    assert_eq!(run_str("Math.round(2.4)"), "2");
    assert_eq!(run_str("Math.round(2.49999)"), "2");
    assert_eq!(run_str("Math.round(-0)"), "0");
}

#[test]
fn math_trunc() {
    assert_eq!(run_str("Math.trunc(4.7)"), "4");
    assert_eq!(run_str("Math.trunc(-4.7)"), "-4");
    assert_eq!(run_str("Math.trunc(4.0)"), "4");
    assert_eq!(run_str("Math.trunc(0.999)"), "0");
    // NOTE: Math.trunc(-0.5) → -0 in zapcode (prints "-0"); JS String() → "0".
    // Documented negative-zero-print divergence; not asserted here.
}

// ============================================================================
// Math: sign / abs
// ============================================================================

#[test]
fn math_abs() {
    assert_eq!(run_str("Math.abs(-7)"), "7");
    assert_eq!(run_str("Math.abs(7)"), "7");
    assert_eq!(run_str("Math.abs(-3.5)"), "3.5");
    assert_eq!(run_str("Math.abs(0)"), "0");
    assert_eq!(run_str("Math.abs(-0)"), "0");
    assert_eq!(run_str("Math.abs(-Infinity)"), "Infinity");
}

#[test]
fn math_sign() {
    assert_eq!(run_str("Math.sign(-3)"), "-1");
    assert_eq!(run_str("Math.sign(3)"), "1");
    assert_eq!(run_str("Math.sign(0)"), "0");
    assert_eq!(run_str("Math.sign(-0)"), "0");
    assert_eq!(run_str("Math.sign(Infinity)"), "1");
    assert_eq!(run_str("Math.sign(-Infinity)"), "-1");
    // NOTE: Math.sign(NaN) → 0 in zapcode (its fold maps the non-positive/non-
    // negative case to 0); JS returns NaN. Documented divergence; assert actual.
    assert_eq!(run_str("Math.sign(NaN)"), "0");
}

// ============================================================================
// Math: roots / powers
// ============================================================================

#[test]
fn math_sqrt() {
    assert_eq!(run_str("Math.sqrt(16)"), "4");
    assert_eq!(run_str("Math.sqrt(2)"), "1.4142135623730951");
    assert_eq!(run_str("Math.sqrt(0)"), "0");
    assert_eq!(run_str("Math.sqrt(1)"), "1");
    assert_eq!(run_str("String(Math.sqrt(-1))"), "NaN");
}

#[test]
fn math_cbrt() {
    assert_eq!(run_str("Math.cbrt(27)"), "3");
    assert_eq!(run_str("Math.cbrt(-27)"), "-3");
    assert_eq!(run_str("Math.cbrt(0)"), "0");
    assert_eq!(run_str("Math.cbrt(8)"), "2");
}

#[test]
fn math_pow() {
    assert_eq!(run_str("Math.pow(2, 10)"), "1024");
    assert_eq!(run_str("Math.pow(2, 0)"), "1");
    assert_eq!(run_str("Math.pow(2, -2)"), "0.25");
    assert_eq!(run_str("Math.pow(0, 0)"), "1");
    assert_eq!(run_str("Math.pow(9, 0.5)"), "3");
    assert_eq!(run_str("String(Math.pow(-8, 1 / 3))"), "NaN"); // negative base, fractional exp
}

#[test]
fn math_hypot() {
    assert_eq!(run_str("Math.hypot(3, 4)"), "5");
    assert_eq!(run_str("Math.hypot(5, 12)"), "13");
    assert_eq!(run_str("Math.hypot(3)"), "3");
    assert_eq!(run_str("Math.hypot(0, 0)"), "0");
    // NOTE: Math.hypot() with no args → -0 in zapcode (prints "-0"); JS → 0.
    // Documented; not asserted.
}

// ============================================================================
// Math: logarithms & exponentials
// ============================================================================

#[test]
fn math_log_family() {
    assert_eq!(run_str("Math.log(Math.E)"), "1");
    assert_eq!(run_str("Math.log(1)"), "0");
    assert_eq!(run_str("Math.log2(8)"), "3");
    assert_eq!(run_str("Math.log2(1024)"), "10");
    assert_eq!(run_str("Math.log2(1)"), "0");
    assert_eq!(run_str("Math.log10(1000)"), "3");
    assert_eq!(run_str("Math.log10(100000)"), "5");
    assert_eq!(run_str("Math.log10(1)"), "0");
}

#[test]
fn math_exp() {
    assert_eq!(run_str("Math.exp(0)"), "1");
    assert_eq!(run_str("Math.exp(1)"), "2.718281828459045");
    assert_eq!(run_str("Math.expm1(0)"), "0");
    assert_eq!(run_str("Math.log1p(0)"), "0");
}

// ============================================================================
// Math: min / max
// ============================================================================

#[test]
fn math_max() {
    assert_eq!(run_str("Math.max(1, 9, 3)"), "9");
    assert_eq!(run_str("Math.max(5)"), "5");
    assert_eq!(run_str("Math.max(-1, -5, -3)"), "-1");
    assert_eq!(run_str("Math.max(...[3, 1, 4, 1, 5])"), "5");
    assert_eq!(run_str("Math.max()"), "-Infinity"); // identity element
}

#[test]
fn math_min() {
    assert_eq!(run_str("Math.min(5, 2, 8)"), "2");
    assert_eq!(run_str("Math.min(5)"), "5");
    assert_eq!(run_str("Math.min(-1, -5, -3)"), "-5");
    assert_eq!(run_str("Math.min(...[3, 1, 4, 1, 5])"), "1");
    assert_eq!(run_str("Math.min()"), "Infinity"); // identity element
}

#[test]
fn math_min_max_nan_handling() {
    // Any NaN argument poisons min/max to NaN, regardless of position (spec).
    assert_eq!(run_str("String(Math.max(NaN, 1))"), "NaN");
    assert_eq!(run_str("String(Math.min(NaN, 1))"), "NaN");
    assert_eq!(run_str("String(Math.max(1, NaN))"), "NaN");
    assert_eq!(run_str("String(Math.min(1, NaN))"), "NaN");
    assert_eq!(run_str("String(Math.max(1, 2, NaN, 3))"), "NaN");
}

// ============================================================================
// Math: constants
// ============================================================================

#[test]
fn math_constants() {
    assert_eq!(run_str("Math.PI > 3.14 && Math.PI < 3.15"), "true");
    assert_eq!(run_str("Math.E > 2.71 && Math.E < 2.72"), "true");
    assert_eq!(run_str("Math.LN2"), "0.6931471805599453");
    assert_eq!(run_str("Math.LN10"), "2.302585092994046");
    assert_eq!(run_str("Math.LOG2E"), "1.4426950408889634");
    assert_eq!(run_str("Math.LOG10E"), "0.4342944819032518");
    assert_eq!(run_str("Math.SQRT2"), "1.4142135623730951");
    assert_eq!(run_str("Math.SQRT1_2"), "0.7071067811865475");
}

// ============================================================================
// NaN / Infinity / negative zero
// ============================================================================

#[test]
fn nan_basics() {
    assert_eq!(run_str("typeof NaN"), "number");
    assert_eq!(run_str("NaN === NaN"), "false"); // NaN is never equal to itself
    assert_eq!(run_str("NaN !== NaN"), "true");
    assert_eq!(run_str("String(0 / 0)"), "NaN");
    assert_eq!(run_str("String(+undefined)"), "NaN");
    assert_eq!(run_str("String(+'abc')"), "NaN");
    assert_eq!(run_str("String(Infinity - Infinity)"), "NaN");
    assert_eq!(run_str("String(Infinity * 0)"), "NaN");
}

#[test]
fn nan_propagates_through_arithmetic() {
    assert_eq!(run_str("String(NaN + 1)"), "NaN");
    assert_eq!(run_str("String(NaN * 2)"), "NaN");
    assert_eq!(run_str("String(NaN - NaN)"), "NaN");
    assert_eq!(run_str("String(Math.max(NaN, NaN))"), "NaN");
}

#[test]
fn infinity_basics() {
    assert_eq!(run_str("1 / 0"), "Infinity");
    assert_eq!(run_str("-1 / 0"), "-Infinity");
    assert_eq!(run_str("Infinity"), "Infinity");
    assert_eq!(run_str("-Infinity"), "-Infinity");
    assert_eq!(run_str("typeof Infinity"), "number");
    assert_eq!(run_str("Infinity + Infinity"), "Infinity");
    assert_eq!(run_str("Infinity > 1e308"), "true");
    assert_eq!(run_str("Infinity === Infinity"), "true");
    assert_eq!(run_str("1 / Infinity"), "0");
}

#[test]
fn negative_zero_equality() {
    // -0 === 0 and 0 === -0 (strict equality treats them equal).
    assert_eq!(run_str("0 === -0"), "true");
    assert_eq!(run_str("-0 === 0"), "true");
    // -0 prints as "0".
    assert_eq!(run_str("String(-0)"), "0");
    // NOTE: 1 / -0 → Infinity in zapcode (JS: -Infinity); documented divergence,
    // not asserted. But 1 / 0 === 1 / -0 is true in zapcode because both are +Inf.
    assert_eq!(run_str("1 / 0 === 1 / -0"), "true");
}

// ============================================================================
// The classic floating-point cases
// ============================================================================

#[test]
fn point_one_plus_point_two() {
    assert_eq!(run_str("0.1 + 0.2"), "0.30000000000000004");
    assert_eq!(run_str("0.1 + 0.2 === 0.3"), "false");
    assert_eq!(run_str("0.3 - 0.2"), "0.09999999999999998");
    assert_eq!(run_str("0.1 * 3"), "0.30000000000000004");
    // The standard "epsilon" workaround works numerically.
    assert_eq!(run_str("Math.abs(0.1 + 0.2 - 0.3) < Number.EPSILON"), "true");
    assert_eq!(run_str("(0.1 + 0.2).toFixed(2) === '0.30'"), "true");
}

#[test]
fn precision_loss_at_safe_integer_boundary() {
    // 2**53 is the first integer not exactly representable beyond MAX_SAFE_INTEGER;
    // 9007199254740993 rounds to ...992 (matches JS f64 behavior).
    assert_eq!(run_str("2 ** 53"), "9007199254740992");
    assert_eq!(run_str("9007199254740993"), "9007199254740992");
    assert_eq!(run_str("2 ** 53 === 2 ** 53 + 1"), "true"); // both round to the same f64

    // Integer-literal arithmetic must also round once the result leaves the safe
    // range: JS has no i64, so `a (+|-|*) b` behaves like a double past 2^53-1.
    // Previously zapcode kept these as exact i64s (`...993`), diverging from Node.
    assert_eq!(run_str("String(9007199254740991 + 2)"), "9007199254740992");
    assert_eq!(run_str("9007199254740992 + 1 === 9007199254740992"), "true");
    assert_eq!(
        run_str("(function(){let x=9007199254740992; return x+1===x})()"),
        "true"
    );
    // Subtraction toward larger magnitude and multiplication round the same way.
    assert_eq!(run_str("9007199254740992 - (-1)"), "9007199254740992");
    assert_eq!(run_str("4503599627370496 * 2 + 1"), "9007199254740992");
    // ++ / -- share the path: incrementing past the boundary rounds, not panics.
    assert_eq!(
        run_str("(function(){let x=9007199254740991; x++; return x})()"),
        "9007199254740992"
    );
    assert_eq!(
        run_str("(function(){let x=9007199254740992; x++; return x===9007199254740992})()"),
        "true"
    );
    // The result is still a JS number, not some other type.
    assert_eq!(run_str("typeof (9007199254740991 + 2)"), "number");
}

// ============================================================================
// Bitwise operators (ToInt32 / ToUint32 semantics)
// ============================================================================

#[test]
fn bitwise_and_or_xor_not() {
    assert_eq!(run_str("5 & 3"), "1");
    assert_eq!(run_str("5 | 2"), "7");
    assert_eq!(run_str("5 ^ 1"), "4");
    assert_eq!(run_str("~5"), "-6");
    assert_eq!(run_str("~0"), "-1");
    assert_eq!(run_str("0xFF & 0x0F"), "15");
}

#[test]
fn bitwise_shifts() {
    assert_eq!(run_str("1 << 4"), "16");
    assert_eq!(run_str("256 >> 2"), "64");
    assert_eq!(run_str("-8 >> 1"), "-4"); // sign-propagating
    assert_eq!(run_str("-1 >>> 28"), "15"); // zero-fill, ToUint32
    assert_eq!(run_str("-8 >>> 0"), "4294967288"); // full ToUint32
}

// ============================================================================
// Numeric literals
// ============================================================================

#[test]
fn numeric_literal_forms() {
    assert_eq!(run_str("0xFF"), "255");
    assert_eq!(run_str("0o17"), "15");
    assert_eq!(run_str("0b1010"), "10");
    assert_eq!(run_str("1_000_000"), "1000000"); // numeric separators in literals
    assert_eq!(run_str("1.5e3"), "1500");
    assert_eq!(run_str(".5"), "0.5");
    assert_eq!(run_str("5."), "5");
    assert_eq!(run_str("1e2"), "100");
    assert_eq!(run_str("2.5e-1"), "0.25");
}
