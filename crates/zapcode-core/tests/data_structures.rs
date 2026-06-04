//! Regression tests for Error constructors, Set, and Map completeness.

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
fn error_constructors_and_instanceof() {
    assert_eq!(run_str(r#"(new Error("boom")).message"#), "boom");
    assert_eq!(run_str(r#"String(new Error("oops"))"#), "Error: oops");
    assert_eq!(
        run_str("let m; try { throw new Error(\"bad\"); } catch (e) { m = e.message; } m"),
        "bad"
    );
    assert_eq!(
        run_str("let r; try { throw new TypeError(\"x\"); } catch (e) { r = [e instanceof Error, e instanceof TypeError, e.name].join(\",\"); } r"),
        "true,true,TypeError"
    );
}

#[test]
fn set_basics() {
    assert_eq!(run_str("const s = new Set([1,1,2,3]); s.size"), "3");
    assert_eq!(
        run_str("const s = new Set([1,2]); [s.has(2), s.has(9)].join(\",\")"),
        "true,false"
    );
    assert_eq!(
        run_str("const s = new Set(); s.add(\"a\"); s.add(\"a\"); s.add(\"b\"); s.delete(\"a\"); [...s].join(\",\")"),
        "b"
    );
    assert_eq!(
        run_str("const s = new Set([1,2,3]); let t = 0; for (const x of s) { t += x; } t"),
        "6"
    );
}

#[test]
fn map_iterable_ctor_size_and_iteration() {
    assert_eq!(
        run_str("const m = new Map([[\"a\",1],[\"b\",2]]); [m.size, m.get(\"a\"), m.get(\"b\")].join(\",\")"),
        "2,1,2"
    );
    assert_eq!(
        run_str("const m = new Map([[\"x\",1],[\"y\",2]]); const out=[]; for (const [k,v] of m) { out.push(k + v); } out.join(\",\")"),
        "x1,y2"
    );
    assert_eq!(
        // keys()/values() return iterators (no .join), so spread them first —
        // matches JS, where `m.keys().join` would throw.
        run_str("const m = new Map([[\"a\",1],[\"b\",2]]); [...m.keys()].join(\",\") + \"|\" + [...m.values()].join(\",\")"),
        "a,b|1,2"
    );
    assert_eq!(
        run_str("const m = new Map(); m.set(\"k\", 9); m.has(\"k\") + \",\" + m.size"),
        "true,1"
    );
}

#[test]
fn set_dedup_use_case() {
    // The idiomatic dedup: [...new Set(arr)]
    assert_eq!(run_str("[...new Set([3,1,2,3,1])].join(\",\")"), "3,1,2");
}
