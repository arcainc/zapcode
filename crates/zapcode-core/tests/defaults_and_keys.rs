//! Regression tests for default parameters and computed object keys.

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
        other => panic!("expected completion, got {other:?}"),
    }
}

#[test]
fn default_param_applied_when_omitted() {
    assert_eq!(run_str("function f(x = 42) { return x; } f()"), "42");
    assert_eq!(run_str("function f(x = 42) { return x; } f(7)"), "7");
    // Explicit undefined triggers the default.
    assert_eq!(
        run_str("function f(x = 42) { return x; } f(undefined)"),
        "42"
    );
    // Explicit null does NOT.
    assert_eq!(run_str("function f(x = 42) { return x; } f(null)"), "null");
}

#[test]
fn default_param_object_and_expr() {
    assert_eq!(
        run_str("function f(opts = {}) { return JSON.stringify(opts); } f()"),
        "{}"
    );
    assert_eq!(
        run_str("function f(a, b = a * 2) { return a + b; } f(3)"),
        "9"
    );
}

#[test]
fn default_param_arrow() {
    assert_eq!(run_str("const f = (x = 5) => x + 1; f()"), "6");
    assert_eq!(run_str("const f = (x = 5) => x + 1; f(10)"), "11");
}

#[test]
fn computed_object_key() {
    assert_eq!(
        run_str("const k = 'name'; const o = { [k]: 'Ada' }; o.name"),
        "Ada"
    );
    assert_eq!(
        run_str("const k = 'a'; const o = { [k + 'b']: 1, x: 2 }; JSON.stringify(o)"),
        r#"{"ab":1,"x":2}"#
    );
    // Numeric computed key is coerced to string.
    assert_eq!(
        run_str("const i = 3; const o = { [i]: 'three' }; o[3]"),
        "three"
    );
}

#[test]
fn computed_key_with_spread() {
    assert_eq!(
        run_str("const k = 'b'; const base = { a: 1 }; const o = { ...base, [k]: 2 }; JSON.stringify(o)"),
        r#"{"a":1,"b":2}"#
    );
}
