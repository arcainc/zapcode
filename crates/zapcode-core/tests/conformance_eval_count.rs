//! Evaluation-count conformance: expressions with side effects run EXACTLY
//! as many times as JS runs them. Value-asserting tests are blind to this
//! class of bug (double evaluation is invisible unless the side effect
//! feeds the result), which is how `f().x += 1` calling `f` twice survived
//! three test layers. Every assertion here threads a call counter through
//! the result and is ground-truthed against real Node.

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
        other => panic!("expected completion for `{code}`, got {other:?}"),
    }
}

const COUNTER: &str = "let calls = 0; const o = { x: 10, n: 1, v: 0, a: { x: 10 } }; \
                       function f() { calls += 1; return o; }";

#[test]
fn plain_assignment_evaluates_object_once() {
    assert_eq!(
        run_str(&format!("{COUNTER} f().x = 1; calls + ':' + o.x")),
        "1:1"
    );
    // Two-level chain — the case the shallow is_store_target check missed.
    assert_eq!(
        run_str(&format!("{COUNTER} f().a.x = 2; calls + ':' + o.a.x")),
        "1:2"
    );
}

#[test]
fn compound_assignment_evaluates_target_once() {
    assert_eq!(
        run_str(&format!("{COUNTER} f().x += 5; calls + ':' + o.x")),
        "1:15"
    );
    assert_eq!(
        run_str(&format!("{COUNTER} f().a.x += 5; calls + ':' + o.a.x")),
        "1:15"
    );
    assert_eq!(
        run_str(&format!("{COUNTER} f().x -= 2; f().x *= 3; calls + ':' + o.x")),
        "2:24"
    );
}

#[test]
fn computed_compound_evaluates_object_and_index_once() {
    assert_eq!(
        run_str(
            "let kc = 0, oc = 0; const o = { k1: 5 }; \
             function key() { kc += 1; return 'k1'; } \
             function obj() { oc += 1; return o; } \
             obj()[key()] += 1; \
             oc + ',' + kc + ':' + o.k1"
        ),
        "1,1:6"
    );
}

#[test]
fn increment_decrement_evaluate_target_once() {
    assert_eq!(
        run_str(&format!("{COUNTER} const r = f().n++; calls + ':' + o.n + ':' + r")),
        "1:2:1"
    );
    assert_eq!(
        run_str(&format!("{COUNTER} const r = ++f().n; calls + ':' + o.n + ':' + r")),
        "1:2:2"
    );
    assert_eq!(
        run_str(&format!("{COUNTER} f().n--; calls + ':' + o.n")),
        "1:0"
    );
}

#[test]
fn logical_assignments_evaluate_target_once() {
    assert_eq!(
        run_str(&format!("{COUNTER} f().v ||= 9; calls + ':' + o.v")),
        "1:9"
    );
    assert_eq!(
        run_str(&format!("{COUNTER} f().n &&= 7; calls + ':' + o.n")),
        "1:7"
    );
    assert_eq!(
        run_str(&format!("{COUNTER} f().missing ??= 3; calls + ':' + o.missing")),
        "1:3"
    );
    // Keep path (no store) still evaluates the reference exactly once.
    assert_eq!(
        run_str(&format!("{COUNTER} f().n ??= 99; calls + ':' + o.n")),
        "1:1"
    );
}

#[test]
fn delete_works_through_call_results_and_evaluates_once() {
    assert_eq!(
        run_str(&format!(
            "{COUNTER} const d = delete f().x; calls + ':' + ('x' in o) + ':' + d"
        )),
        "1:false:true"
    );
    assert_eq!(
        run_str(&format!("{COUNTER} delete f()['n']; calls + ':' + ('n' in o)")),
        "1:false"
    );
}

#[test]
fn value_expression_side_effects_run_once() {
    // The assigned VALUE expression must also be single-shot.
    assert_eq!(
        run_str(
            "let vc = 0; const o = { x: 1 }; \
             function v() { vc += 1; return 10; } \
             o.x += v(); vc + ':' + o.x"
        ),
        "1:11"
    );
}
