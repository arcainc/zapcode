//! Regression tests for user-defined ToPrimitive hooks (O4): when an object
//! defines a callable `valueOf` / `toString`, the VM honors it when coercing the
//! object to a primitive at the operator and `String()`/`Number()` coercion
//! points. Symbol.toPrimitive is NOT covered (the crate's Symbol support is a
//! stub — see STRESS-PASS-BUGS.md); only the common valueOf/toString cases are.

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
fn valueof_participates_in_addition() {
    // The canonical case from the feature spec.
    assert_eq!(run_str("({valueOf(){return 42}}) + 1"), "43");
    // Result is the strict-equal of 43.
    assert_eq!(run_str("(({valueOf(){return 42}}) + 1) === 43"), "true");
    // valueOf on both operands.
    assert_eq!(
        run_str("({valueOf(){return 10}}) + ({valueOf(){return 5}})"),
        "15"
    );
}

#[test]
fn tostring_participates_in_string_concat() {
    // toString hook honored via `+ ""` (default hint, no valueOf so toString runs).
    assert_eq!(run_str("({toString(){return \"hi\"}}) + \"\""), "hi");
    assert_eq!(
        run_str("(({toString(){return \"hi\"}}) + \"\") === \"hi\""),
        "true"
    );
    // Template literal uses the string hint.
    assert_eq!(run_str("`val=${({toString(){return \"x\"}})}`"), "val=x");
}

#[test]
fn number_global_uses_valueof() {
    assert_eq!(run_str("Number({valueOf(){return 3}})"), "3");
    assert_eq!(run_str("Number({valueOf(){return 3}}) === 3"), "true");
}

#[test]
fn string_global_uses_tostring() {
    assert_eq!(run_str("String({toString(){return \"abc\"}})"), "abc");
    // valueOf returning a primitive does NOT win for the string hint when
    // toString is present (toString is tried first).
    assert_eq!(
        run_str("String({toString(){return \"t\"}, valueOf(){return 9}})"),
        "t"
    );
}

#[test]
fn relational_comparison_uses_valueof() {
    assert_eq!(run_str("({valueOf(){return 100}}) < 200"), "true");
    assert_eq!(run_str("({valueOf(){return 100}}) > 200"), "false");
    assert_eq!(run_str("({valueOf(){return 5}}) <= 5"), "true");
    assert_eq!(run_str("({valueOf(){return 5}}) >= 6"), "false");
}

#[test]
fn arithmetic_operators_use_valueof() {
    assert_eq!(run_str("({valueOf(){return 10}}) - 3"), "7");
    assert_eq!(run_str("({valueOf(){return 6}}) * 7"), "42");
    assert_eq!(run_str("({valueOf(){return 20}}) / 4"), "5");
    assert_eq!(run_str("({valueOf(){return 17}}) % 5"), "2");
    assert_eq!(run_str("({valueOf(){return 2}}) ** 8"), "256");
    assert_eq!(run_str("-({valueOf(){return 9}})"), "-9");
}

#[test]
fn number_hint_prefers_valueof_over_tostring() {
    // For arithmetic (number hint) valueOf wins when both are present.
    assert_eq!(
        run_str("({valueOf(){return 2}, toString(){return \"99\"}}) + 0"),
        "2"
    );
    assert_eq!(
        run_str("({valueOf(){return 2}, toString(){return \"99\"}}) * 1"),
        "2"
    );
}

#[test]
fn falls_back_to_tostring_when_valueof_returns_object() {
    // valueOf returns an object (itself non-primitive) -> skipped, toString used.
    assert_eq!(
        run_str("({valueOf(){return {}}, toString(){return \"fb\"}}) + \"\""),
        "fb"
    );
    // For the number hint, valueOf returning an object is skipped and toString's
    // numeric coercion is used.
    assert_eq!(
        run_str("({valueOf(){return {}}, toString(){return \"7\"}}) - 0"),
        "7"
    );
}

#[test]
fn this_is_bound_inside_hook() {
    // The hook body can read `this` to compute its result.
    assert_eq!(
        run_str("({n: 41, valueOf(){return this.n + 1}}) + 0"),
        "42"
    );
}

#[test]
fn plain_object_without_hooks_unchanged() {
    // No hooks: existing built-in coercion (plain object -> "[object Object]").
    assert_eq!(run_str("({}) + \"\""), "[object Object]");
    assert_eq!(run_str("String({})"), "[object Object]");
    // Number of a plain object is NaN.
    assert_eq!(run_str("Number({}) !== Number({})"), "true"); // NaN !== NaN
}

#[test]
fn hook_used_in_array_join_via_string_concat() {
    // Building a string from a hook-bearing object inside a normal flow.
    assert_eq!(
        run_str("let o = {valueOf(){return 7}}; let s = `${o + 3}`; s"),
        "10"
    );
}
