//! Regression tests for function-declaration hoisting (D1/D2).

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, ZapcodeRun};

fn run_str(code: &str) -> String {
    let result = ZapcodeRun::new(code.to_string(), Vec::new(), Vec::new(), ResourceLimits::default())
        .unwrap().run(Vec::new()).unwrap();
    match result.state {
        VmState::Complete(v) => v.to_js_string(),
        other => panic!("expected completion, got {other:?}"),
    }
}

#[test]
fn nested_function_declaration() {
    assert_eq!(run_str("function outer(){ function inner(){ return 5; } return inner(); } outer()"), "5");
    assert_eq!(run_str("(() => { function f(){ return 3; } return f(); })()"), "3");
}

#[test]
fn forward_reference_top_level() {
    assert_eq!(run_str("const r = f(); function f(){ return 1; } r"), "1");
}

#[test]
fn forward_reference_inside_function() {
    assert_eq!(run_str("function run(){ return helper(2); function helper(x){ return x*10; } } run()"), "20");
}

#[test]
fn mutual_recursion() {
    assert_eq!(
        run_str("function isEven(n){ return n===0 ? true : isOdd(n-1); } function isOdd(n){ return n===0 ? false : isEven(n-1); } isEven(10)"),
        "true"
    );
}

#[test]
fn function_decl_in_block_still_works() {
    assert_eq!(run_str("let out; if (true) { function g(){ return 7; } out = g(); } out"), "7");
}
