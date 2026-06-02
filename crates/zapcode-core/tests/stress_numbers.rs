//! Regression tests for numeric formatting divergences (cluster F): toFixed
//! rounding (F3), toPrecision (F6), toExponential (F7), toString(radix) with a
//! fractional part (F8). Ground truth verified against Node.

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
        VmState::Complete(v) => v.to_js_string(),
        other => panic!("expected completion, got {other:?}"),
    }
}

#[test]
fn to_fixed_rounds_half_away_from_zero() {
    assert_eq!(run_str("(2.5).toFixed(0)"), "3");
    assert_eq!(run_str("(0.5).toFixed(0)"), "1");
    assert_eq!(run_str("(4.5).toFixed(0)"), "5");
    assert_eq!(run_str("(-2.5).toFixed(0)"), "-3");
    assert_eq!(run_str("(-0.5).toFixed(0)"), "-1");
    assert_eq!(run_str("(10.5).toFixed(0)"), "11");
    assert_eq!(run_str("(0.125).toFixed(2)"), "0.13");
    // Float-repr cases must match V8 (these halves aren't exact in f64).
    assert_eq!(run_str("(1.5).toFixed(0)"), "2");
    assert_eq!(run_str("(3.5).toFixed(0)"), "4");
    assert_eq!(run_str("(0.15).toFixed(1)"), "0.1");
    assert_eq!(run_str("(1.005).toFixed(2)"), "1.00");
    assert_eq!(run_str("(2.675).toFixed(2)"), "2.67");
    // Padding.
    assert_eq!(run_str("(123.456).toFixed(4)"), "123.4560");
    assert_eq!(run_str("(1234).toFixed(2)"), "1234.00");
    assert_eq!(run_str("(100).toFixed(5)"), "100.00000");
    assert_eq!(run_str("(0.00001234).toFixed(2)"), "0.00");
}

#[test]
fn to_precision() {
    assert_eq!(run_str("(123.456).toPrecision(4)"), "123.5");
    assert_eq!(run_str("(3).toPrecision(1)"), "3");
    assert_eq!(run_str("(1.5).toPrecision(2)"), "1.5");
    assert_eq!(run_str("(100).toPrecision(5)"), "100.00");
    assert_eq!(run_str("(1234).toPrecision(2)"), "1.2e+3");
    assert_eq!(run_str("(0.0001234).toPrecision(2)"), "0.00012");
}

#[test]
fn to_exponential() {
    assert_eq!(run_str("(12345).toExponential(2)"), "1.23e+4");
    assert_eq!(run_str("(0.5).toExponential(1)"), "5.0e-1");
    assert_eq!(run_str("(100).toExponential()"), "1e+2");
}

#[test]
fn to_string_radix_with_fraction() {
    assert_eq!(run_str("(3.5).toString(2)"), "11.1");
    assert_eq!(run_str("(255.5).toString(16)"), "ff.8");
    // Integer radix conversions still work.
    assert_eq!(run_str("(255).toString(16)"), "ff");
    assert_eq!(run_str("(-255).toString(16)"), "-ff");
}
