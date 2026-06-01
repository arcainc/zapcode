//! Regression tests for destructuring in parameters and for-of bindings
//! (variable-declaration destructuring already worked).

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
fn object_param_destructuring() {
    assert_eq!(run_str("const f = ({a}) => a; f({a: 42})"), "42");
    assert_eq!(
        run_str("function f({a, b}) { return a + b; } f({a: 10, b: 20})"),
        "30"
    );
    assert_eq!(run_str("const f = ({a: x}) => x; f({a: 5})"), "5");
}

#[test]
fn array_and_nested_param_destructuring() {
    assert_eq!(run_str("const f = ([x, y]) => x + y; f([3, 4])"), "7");
    assert_eq!(run_str("const f = ({a: {b}}) => b; f({a: {b: 99}})"), "99");
    assert_eq!(
        run_str(
            "function f({a, ...rest}) { return JSON.stringify([a, rest]); } f({a:1, b:2, c:3})"
        ),
        r#"[1,{"b":2,"c":3}]"#
    );
}

#[test]
fn destructuring_param_in_map_callback() {
    // The common `.map(([k, v]) => ...)` over Object.entries.
    assert_eq!(
        run_str(
            "JSON.stringify(Object.fromEntries(Object.entries({x:1, y:2}).map(([k, v]) => [k, v * 10])))"
        ),
        r#"{"x":10,"y":20}"#
    );
}

#[test]
fn for_of_object_destructuring() {
    assert_eq!(
        run_str(
            "const out=[]; for (const {id} of [{id:7},{id:8}]) { out.push(id); } out.join(\",\")"
        ),
        "7,8"
    );
    assert_eq!(
        run_str("const out=[]; for (const {id: i, name} of [{id:1, name:\"a\"}]) { out.push(i + name); } out.join(\",\")"),
        "1a"
    );
}

#[test]
fn for_of_array_destructuring() {
    assert_eq!(
        run_str("const out=[]; for (const [k, v] of [[\"a\",1],[\"b\",2]]) { out.push(k + v); } out.join(\",\")"),
        "a1,b2"
    );
    assert_eq!(
        run_str("const out=[]; for (const [k, v] of Object.entries({x:1, y:2})) { out.push(k + \"=\" + v); } out.join(\",\")"),
        "x=1,y=2"
    );
}
