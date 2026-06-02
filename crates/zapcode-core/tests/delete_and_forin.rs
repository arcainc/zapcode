//! Regression tests for the `delete` operator and `for...in` loops.

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
fn delete_property() {
    assert_eq!(
        run_str("const o = {a: 1, b: 2}; delete o.a; JSON.stringify(o)"),
        r#"{"b":2}"#
    );
    // delete yields true.
    assert_eq!(run_str("const o = {a: 1}; delete o.a"), "true");
    // deleting a missing key is still true and harmless.
    assert_eq!(
        run_str("const o = {a: 1}; delete o.z; JSON.stringify(o)"),
        r#"{"a":1}"#
    );
}

#[test]
fn delete_computed_and_nested() {
    assert_eq!(
        run_str("const o = {a: 1, b: 2}; const k = 'b'; delete o[k]; JSON.stringify(o)"),
        r#"{"a":1}"#
    );
    assert_eq!(
        run_str("const o = {inner: {x: 1, y: 2}}; delete o.inner.x; JSON.stringify(o)"),
        r#"{"inner":{"y":2}}"#
    );
}

#[test]
fn delete_array_index_leaves_hole() {
    // delete arr[1] keeps length 3 but the slot becomes undefined.
    assert_eq!(run_str("const a = [1, 2, 3]; delete a[1]; a.length"), "3");
    assert_eq!(
        run_str("const a = [1, 2, 3]; delete a[1]; a[1]"),
        "undefined"
    );
}

#[test]
fn for_in_object_keys() {
    assert_eq!(
        run_str("const o = {a: 1, b: 2}; let k = ''; for (const x in o) k += x; k"),
        "ab"
    );
    assert_eq!(
        run_str("const o = {a: 1, b: 2, c: 3}; let s = 0; for (const x in o) s += o[x]; s"),
        "6"
    );
}

#[test]
fn for_in_array_indices() {
    assert_eq!(
        run_str("const a = ['x', 'y', 'z']; let r = ''; for (const i in a) r += i; r"),
        "012"
    );
}
