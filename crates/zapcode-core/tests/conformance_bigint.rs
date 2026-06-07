//! Conformance: BigInt (arbitrary-precision integers).
//!
//! Literals (`10n`, `0xFFn`), arithmetic at arbitrary precision, the BigInt vs
//! Number type boundary (mixing is a TypeError; comparisons/loose-equality
//! coerce), `BigInt()` conversion, `typeof`, `.toString(radix)`, and the
//! `JSON.stringify` TypeError — all asserted at the real-Node answer.

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

#[test]
fn bigint_literals_and_typeof() {
    assert_eq!(run_str("String(10n)"), "10");
    assert_eq!(run_str("typeof 10n"), "bigint");
    assert_eq!(run_str("String(0xFFn)"), "255");
    assert_eq!(run_str("String(0b1010n)"), "10");
    assert_eq!(run_str("String(1_000n)"), "1000");
    assert_eq!(run_str("String(-5n)"), "-5");
}

#[test]
fn bigint_arithmetic_is_arbitrary_precision() {
    assert_eq!(run_str("String(10n + 20n)"), "30");
    assert_eq!(run_str("String(9007199254740993n + 1n)"), "9007199254740994");
    assert_eq!(
        run_str("String(123456789012345678901234567890n * 2n)"),
        "246913578024691357802469135780"
    );
    assert_eq!(run_str("String(5n - 8n)"), "-3");
    assert_eq!(run_str("String(7n / 2n)"), "3"); // truncates toward zero
    assert_eq!(run_str("String(-7n / 2n)"), "-3");
    assert_eq!(run_str("String(7n % 3n)"), "1");
    assert_eq!(run_str("String(2n ** 10n)"), "1024");
    assert_eq!(run_str("(function(){ let x = 5n; x++; return String(x); })()"), "6");
}

#[test]
fn bigint_division_and_exponent_errors() {
    assert_eq!(
        run_str("(function(){ try { return String(5n / 0n); } catch (e) { return e.name; } })()"),
        "RangeError"
    );
    assert_eq!(
        run_str("(function(){ try { return String(5n % 0n); } catch (e) { return e.name; } })()"),
        "RangeError"
    );
    assert_eq!(
        run_str("(function(){ try { return String(2n ** -1n); } catch (e) { return e.name; } })()"),
        "RangeError"
    );
}

#[test]
fn mixing_bigint_and_number_in_arithmetic_throws() {
    for op in ["10n + 5", "10n - 5", "10n * 5", "10n / 5", "10n % 5", "2n ** 3"] {
        assert_eq!(
            run_str(&format!(
                "(function(){{ try {{ return String({op}); }} catch (e) {{ return e.name; }} }})()"
            )),
            "TypeError",
            "expected `{op}` to throw a TypeError"
        );
    }
    // But BigInt + string is string concatenation.
    assert_eq!(run_str("10n + \"abc\""), "10abc");
}

#[test]
fn bigint_comparisons_and_equality() {
    // Comparisons coerce across the BigInt/Number boundary (no TypeError).
    assert_eq!(run_str("10n < 20"), "true");
    assert_eq!(run_str("100n > 5"), "true");
    assert_eq!(run_str("10n >= 10"), "true");
    assert_eq!(run_str("(10n ** 30n) > (10n ** 20n)"), "true");
    // Loose equality compares mathematical values; strict checks the type too.
    assert_eq!(run_str("10n == 10"), "true");
    assert_eq!(run_str("10n == '10'"), "true");
    assert_eq!(run_str("10n == 11"), "false");
    assert_eq!(run_str("10n === 10n"), "true");
    assert_eq!(run_str("10n === 10"), "false");
}

#[test]
fn bigint_constructor_and_methods() {
    assert_eq!(run_str("String(BigInt(42))"), "42");
    assert_eq!(run_str("String(BigInt('100'))"), "100");
    assert_eq!(run_str("String(BigInt(true))"), "1");
    assert_eq!(
        run_str("(function(){ try { return String(BigInt(1.5)); } catch (e) { return e.name; } })()"),
        "RangeError"
    );
    assert_eq!(run_str("(255n).toString(16)"), "ff");
    assert_eq!(run_str("(255n).toString()"), "255");
    assert_eq!(run_str("typeof (5n).valueOf()"), "bigint");
    assert_eq!(run_str("new TypeError().constructor === TypeError"), "true"); // sanity
    assert_eq!(run_str("(5n).constructor === BigInt"), "true");
}

#[test]
fn json_stringify_of_bigint_throws() {
    assert_eq!(
        run_str("(function(){ try { JSON.stringify(10n); return 'no'; } catch (e) { return e.name; } })()"),
        "TypeError"
    );
}
