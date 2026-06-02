//! Reference-semantics regression tests for the heap-handle migration.
//!
//! After the Value::Array/Object -> Handle migration, arrays and objects carry
//! a Handle into the VM heap instead of owning their contents inline. This gives
//! JS reference semantics: aliasing, mutate-through-param, and shared identity all
//! observe the same underlying heap slot. These tests pin that behavior so a future
//! regression back to value-copy semantics fails loudly.

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
fn alias_push_is_visible_through_original() {
    // const b=a aliases the SAME array handle; pushing through b is visible via a.
    assert_eq!(run_str("const a=[1]; const b=a; b.push(2); a.length"), "2");
}

#[test]
fn mutate_through_param_is_visible() {
    // Passing an array to a function passes the handle, not a copy: f(a) mutates a.
    assert_eq!(
        run_str("const a=[1]; const f=(x)=>x.push(9); f(a); a.length"),
        "2"
    );
}

#[test]
fn mutate_loop_var_object_is_visible() {
    // The for-of loop binding aliases the element's object handle, so x.n*=10
    // mutates the object that lives in the array.
    assert_eq!(
        run_str("const a=[{n:1}]; for(const x of a) x.n*=10; a[0].n"),
        "10"
    );
}

#[test]
fn map_of_arrays_bucket_push_is_visible() {
    // m.get(k) returns the SAME bucket-array handle stored in the Map, so pushing
    // into it is observed on a subsequent m.get(k).
    assert_eq!(
        run_str(
            "const m=new Map(); m.set('k',[]); m.get('k').push(1); m.get('k').push(2); m.get('k').length"
        ),
        "2"
    );
}

#[test]
fn array_identity_is_reflexive() {
    // strict_eq is handle identity for arrays/objects: a===a is true.
    assert_eq!(run_str("const a=[1]; a===a"), "true");
    assert_eq!(run_str("const o={}; o===o"), "true");
}

#[test]
fn distinct_literals_are_not_identical() {
    // Two distinct literals get distinct handles, so they are NOT ===.
    assert_eq!(run_str("[1]===[1]"), "false");
    assert_eq!(run_str("({})===({})"), "false");
}

#[test]
fn map_with_object_key_uses_identity() {
    // Object keys in a Map use reference identity: the same handle round-trips.
    assert_eq!(
        run_str("const k={}; const m=new Map(); m.set(k,9); m.get(k)"),
        "9"
    );
    // A different object with the same shape is a distinct key (miss).
    assert_eq!(
        run_str("const k={}; const m=new Map(); m.set(k,9); m.get({})===undefined"),
        "true"
    );
}

#[test]
fn structured_clone_is_independent() {
    // structuredClone deep-copies into fresh handles; mutating the original after
    // cloning must NOT affect the clone.
    assert_eq!(
        run_str(
            "const o={a:[1]}; const c=structuredClone(o); o.a.push(2); JSON.stringify([o.a, c.a])"
        ),
        "[[1,2],[1]]"
    );
    // ...and the clone is a distinct object/array identity.
    assert_eq!(
        run_str("const o={a:[1]}; const c=structuredClone(o); (o===c)||(o.a===c.a)"),
        "false"
    );
}
