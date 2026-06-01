//! Regression tests for receiver write-back: a method's receiver and write-back
//! place are captured at property-load time, so argument evaluation can't
//! clobber them, and mutations persist through nested paths (obj.a.push, this).

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
fn push_to_a_nested_object_array_persists() {
    assert_eq!(
        run_str("const o = { a: [], n: 3 }; o.a.push(1); o.a.push(2); JSON.stringify(o)"),
        r#"{"a":[1,2],"n":3}"#
    );
    assert_eq!(
        run_str("const o = { a: { b: [] } }; o.a.b.push(7); JSON.stringify(o)"),
        r#"{"a":{"b":[7]}}"#
    );
}

#[test]
fn for_of_pushing_a_field_into_an_outer_array() {
    assert_eq!(
        run_str(
            "const items=[{id:1},{id:2},{id:3}]; const out=[]; for (const r of items) { out.push(r.id); } out.join(\",\")"
        ),
        "1,2,3"
    );
}

#[test]
fn push_with_a_method_call_or_builtin_argument() {
    assert_eq!(
        run_str("const a=[]; a.push([1,2].join(\"-\")); a.join(\"|\")"),
        "1-2"
    );
    assert_eq!(
        run_str("const a=[]; a.push(Math.max(3,7)); a.join(\",\")"),
        "7"
    );
    // The arg's own method load must not steal the outer receiver.
    assert_eq!(
        run_str("const a=[]; const s=\"x:y\"; a.push(s.slice(0, s.indexOf(\":\"))); a.join(\",\")"),
        "x"
    );
}

#[test]
fn method_chaining_with_method_call_argument() {
    assert_eq!(
        run_str("const u=\"a:b:c\"; u.slice(0, u.indexOf(\":\"))"),
        "a"
    );
}

#[test]
fn index_path_write_back() {
    assert_eq!(
        run_str("const rows=[{t:[]},{t:[]}]; rows[1].t.push(\"x\"); JSON.stringify(rows)"),
        r#"[{"t":[]},{"t":["x"]}]"#
    );
}

#[test]
fn this_array_field_mutation_in_a_method() {
    assert_eq!(
        run_str(
            "class C { constructor(){ this.items=[]; } add(x){ this.items.push(x); return this.items.length; } } const c=new C(); c.add(\"a\"); c.add(\"b\"); JSON.stringify(c.items)"
        ),
        r#"["a","b"]"#
    );
}
