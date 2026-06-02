//! Regression tests for numeric-parsing globals, Number statics, number
//! primitive methods, and Object.fromEntries (added after stress testing).

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
fn parse_int_and_float() {
    assert_eq!(run_str("parseInt(\"42px\")"), "42");
    assert_eq!(run_str("parseInt(\"  -7 \")"), "-7");
    assert_eq!(run_str("parseInt(\"ff\", 16)"), "255");
    assert_eq!(run_str("parseInt(\"nope\")"), "NaN");
    assert_eq!(run_str("parseFloat(\"3.14abc\")"), "3.14");
    assert_eq!(run_str("parseFloat(\"-0.5e2x\")"), "-50");
    assert_eq!(run_str("Number.parseInt(\"10\", 2)"), "2");
}

#[test]
fn nan_and_finite_predicates() {
    assert_eq!(
        run_str("[isNaN(NaN), isNaN(3), isNaN(\"x\")].join(\",\")"),
        "true,false,true"
    );
    assert_eq!(
        run_str("[isFinite(1/0), isFinite(5)].join(\",\")"),
        "false,true"
    );
    assert_eq!(
        run_str("[Number.isNaN(NaN), Number.isNaN(\"x\")].join(\",\")"),
        "true,false"
    );
    assert_eq!(
        run_str("[Number.isFinite(5), Number.isFinite(1/0), Number.isFinite(\"5\")].join(\",\")"),
        "true,false,false"
    );
    assert_eq!(
        run_str("[Number.isInteger(4), Number.isInteger(4.5)].join(\",\")"),
        "true,false"
    );
}

#[test]
fn number_primitive_methods() {
    assert_eq!(run_str("(3.14159).toFixed(2)"), "3.14");
    assert_eq!(run_str("(1).toFixed(3)"), "1.000");
    assert_eq!(run_str("(255).toString(16)"), "ff");
    assert_eq!(run_str("(5).toString()"), "5");
    assert_eq!(run_str("(255).toString(2)"), "11111111");
    // common money-formatting idiom
    assert_eq!(run_str("\"$\" + (1234.5).toFixed(2)"), "$1234.50");
}

#[test]
fn object_from_entries() {
    assert_eq!(
        run_str("JSON.stringify(Object.fromEntries([[\"a\", 1], [\"b\", 2]]))"),
        r#"{"a":1,"b":2}"#
    );
    // round-trips with Object.entries (pure-data .map, no destructuring)
    assert_eq!(
        run_str(
            "JSON.stringify(Object.fromEntries(Object.entries({x:1,y:2}).map(e => [e[0], e[1] * 10])))"
        ),
        r#"{"x":10,"y":20}"#
    );
}
