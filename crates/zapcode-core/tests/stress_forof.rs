//! Regression tests for nested for-of (J4): the outer loop must run every
//! iteration even when the body contains another for-of.

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
fn nested_for_of_runs_all_outer_iterations() {
    assert_eq!(
        run_str("const out=[]; for (const a of ['a','b']) { for (const n of [1,2]) { out.push(a+n); } } out.join(',')"),
        "a1,a2,b1,b2"
    );
}

#[test]
fn triple_nested_for_of() {
    assert_eq!(
        run_str("let c=0; for (const a of [1,2]) for (const b of [1,2,3]) for (const d of [1,2]) c++; c"),
        "12"
    );
}

#[test]
fn nested_for_of_with_break_continue() {
    assert_eq!(
        run_str("const out=[]; for (const a of [1,2,3]) { for (const b of [1,2,3]) { if (b===2) continue; if (b===3) break; out.push(a*10+b); } } out.join(',')"),
        "11,21,31"
    );
}

#[test]
fn for_of_over_map_set_string_still_works() {
    assert_eq!(run_str("let s=''; for (const c of 'abc') s+=c; s"), "abc");
    assert_eq!(run_str("const m=new Map([['a',1],['b',2]]); let o=[]; for (const [k,v] of m) o.push(k+v); o.join(',')"), "a1,b2");
    assert_eq!(run_str("let t=0; for (const x of new Set([1,2,2,3])) t+=x; t"), "6");
    // nested map-in-array
    assert_eq!(run_str("let o=[]; for (const row of [['a','b'],['c']]) { for (const x of row) o.push(x); } o.join('')"), "abc");
}
