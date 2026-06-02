//! Regression tests for JSON.stringify divergences (cluster I + M6).

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, ZapcodeRun};

fn run_str(code: &str) -> String {
    let result = ZapcodeRun::new(code.to_string(), Vec::new(), Vec::new(), ResourceLimits::default())
        .unwrap().run(Vec::new()).unwrap();
    match result.state {
        VmState::Complete(v) => v.to_js_string(&result.heap),
        other => panic!("expected completion, got {other:?}"),
    }
}

#[test]
fn drops_undefined() {
    assert_eq!(run_str("JSON.stringify({a:1,b:undefined,c:3})"), r#"{"a":1,"c":3}"#);
    assert_eq!(run_str("JSON.stringify([1,undefined,3])"), "[1,null,3]");
}

#[test]
fn escapes_control_chars() {
    assert_eq!(run_str(r#"JSON.stringify("a\nb")"#), r#""a\nb""#);
    assert_eq!(run_str(r#"JSON.stringify("a\tb")"#), r#""a\tb""#);
}

#[test]
fn array_replacer_whitelist() {
    assert_eq!(run_str(r#"JSON.stringify({a:1,b:2,c:3},["a","c"])"#), r#"{"a":1,"c":3}"#);
}

#[test]
fn internal_wrappers() {
    assert_eq!(run_str("JSON.stringify(new Map([['a',1]]))"), "{}");
    assert_eq!(run_str("JSON.stringify(new Set([1,2]))"), "{}");
    assert_eq!(run_str("JSON.stringify(new Date(1700000000123))"), r#""2023-11-14T22:13:20.123Z""#);
}
