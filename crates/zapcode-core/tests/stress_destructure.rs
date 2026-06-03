//! Regression tests for array-rest destructuring (H3).

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, ZapcodeRun};

fn run_str(code: &str) -> String {
    let result = ZapcodeRun::new(code.to_string(), Vec::new(), Vec::new(), ResourceLimits::default())
        .unwrap()
        .run(Vec::new())
        .unwrap();
    match result.state {
        VmState::Complete(v) => v.to_js_string(&result.heap),
        other => panic!("expected completion, got {other:?}"),
    }
}

#[test]
fn array_rest_in_var_decl() {
    assert_eq!(run_str("const [a, ...rest] = [1,2,3]; a + '|' + rest.join(',')"), "1|2,3");
    assert_eq!(run_str("const [...r] = [1,2,3]; r.join(',')"), "1,2,3");
    assert_eq!(run_str("const [a, b, ...rest] = [1,2]; rest.length"), "0");
    assert_eq!(run_str("const [first, ...others] = ['x','y','z']; others.join('-')"), "y-z");
}

#[test]
fn array_rest_in_params() {
    assert_eq!(
        run_str("function f([a, ...rest]) { return a + ':' + rest.join(','); } f([1,2,3])"),
        "1:2,3"
    );
}
