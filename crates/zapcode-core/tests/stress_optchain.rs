//! Regression tests for optional-chaining short-circuit (E1/E2).

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
fn trailing_non_optional_member_short_circuits() {
    // E2: a?.b.c must yield undefined (not throw) when a is nullish.
    assert_eq!(run_str("const a = null; let r = a?.b.c; r === undefined"), "true");
    assert_eq!(run_str("const a = null; let r = a?.b.c.d; r === undefined"), "true");
    // present path still works
    assert_eq!(run_str("const a = {b:{c:7}}; a?.b.c"), "7");
}

#[test]
fn optional_call_on_nullish() {
    // E1: optional call/method on a nullish receiver yields undefined, not throw.
    assert_eq!(run_str("const x = null; let r = x?.(); r === undefined"), "true");
    assert_eq!(run_str("const x = undefined; let r = x?.f(); r === undefined"), "true");
    assert_eq!(run_str("const x = null; let r = x?.at(0); r === undefined"), "true");
    assert_eq!(run_str("const o = {}; let r = o?.miss?.(); r === undefined"), "true");
}

#[test]
fn optional_call_present_path() {
    assert_eq!(run_str("const o = { f(){ return 9; } }; o?.f()"), "9");
    assert_eq!(run_str("const arr = [1,2,3]; arr?.at(-1)"), "3");
}

#[test]
fn nullish_coalescing_with_optional_chain() {
    assert_eq!(run_str("const rec = null; rec?.geo?.region ?? 'unknown'"), "unknown");
    assert_eq!(run_str("const rec = {a:null}; rec?.a?.b ?? 'fallback'"), "fallback");
    assert_eq!(run_str("const rec = {geo:{region:'US'}}; rec?.geo?.region ?? 'unknown'"), "US");
    // mixed: optional then non-optional then ??
    assert_eq!(run_str("const rec = null; (rec?.geo.region) ?? 'x'"), "x");
}

#[test]
fn plain_member_and_call_unaffected() {
    assert_eq!(run_str("const o = {a:{b:5}}; o.a.b"), "5");
    assert_eq!(run_str("[1,2,3].map(n=>n+1).join(',')"), "2,3,4");
}
