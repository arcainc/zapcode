//! Small correctness fixes from the second stress pass: typeof null, template
//! object coercion, Math.round half-rounding, parseInt hex, and Array.copyWithin.

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
fn typeof_null_is_object() {
    assert_eq!(run_str("typeof null"), "object");
    assert_eq!(run_str("typeof undefined"), "undefined");
    assert_eq!(run_str("typeof {}"), "object");
    assert_eq!(run_str("typeof (() => 1)"), "function");
}

#[test]
fn template_coerces_object() {
    assert_eq!(run_str("const o = {}; `${o}`"), "[object Object]");
    assert_eq!(run_str("const a = [1,2]; `${a}`"), "1,2");
    assert_eq!(run_str("`${null}`"), "null");
    assert_eq!(run_str("const n = 5; `val=${n}`"), "val=5");
}

#[test]
fn math_round_rounds_half_toward_positive() {
    assert_eq!(run_str("Math.round(-2.5)"), "-2");
    assert_eq!(run_str("Math.round(2.5)"), "3");
    assert_eq!(run_str("Math.round(-2.6)"), "-3");
    assert_eq!(run_str("Math.round(0.5)"), "1");
}

#[test]
fn parse_int_hex_prefix() {
    assert_eq!(run_str("parseInt('0xff')"), "255");
    assert_eq!(run_str("parseInt('0x10')"), "16");
    assert_eq!(run_str("parseInt('0xff', 16)"), "255");
    assert_eq!(run_str("parseInt('42')"), "42");
    assert_eq!(run_str("parseInt('10', 2)"), "2");
}

#[test]
fn object_destructuring_defaults() {
    // Variable-declaration object destructuring with defaults.
    assert_eq!(
        run_str("const {a = 10, b = 20} = {a: 1}; a + ',' + b"),
        "1,20"
    );
    assert_eq!(run_str("const {x = 'd'} = {x: undefined}; x"), "d");
    assert_eq!(run_str("const {y = 5} = {y: 0}; y"), "0");
    // Destructured object parameter with field defaults.
    assert_eq!(run_str("function f({a = 5}) { return a; } f({})"), "5");
    assert_eq!(run_str("function f({a = 5}) { return a; } f({a: 9})"), "9");
}

#[test]
fn array_copy_within() {
    assert_eq!(
        run_str("[1,2,3,4,5].copyWithin(0, 3).join(',')"),
        "4,5,3,4,5"
    );
    assert_eq!(
        run_str("[1,2,3,4,5].copyWithin(0, 3, 4).join(',')"),
        "4,2,3,4,5"
    );
    // Mutates the receiver in place.
    assert_eq!(
        run_str("const a = [1,2,3,4,5]; a.copyWithin(1, 3); a.join(',')"),
        "1,4,5,4,5"
    );
}
