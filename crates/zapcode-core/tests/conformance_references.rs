//! Conformance suite (round 1): **reference semantics** (NON-cyclic).
//!
//! After the heap-with-handles migration, `Value::Array`/`Value::Object` carry a
//! `Handle` into the VM-owned `Heap` rather than owning their contents inline.
//! That gives JS reference semantics: two bindings to the same array/object
//! observe the same underlying heap slot, mutation through any alias is visible
//! through every other, identity (`===`) is handle identity, and the only ways
//! to get *independent* data are an explicit deep copy (`structuredClone`,
//! `JSON.parse(JSON.stringify(...))`) — shallow copies (spread, `slice`,
//! `concat`, `Object.assign`, `Array.from`) copy the *handles*, so nested
//! objects stay shared.
//!
//! This file is a test262-style breadth pass over that contract: aliasing,
//! mutate-through-parameter, mutation through a `for-of` loop binding, the
//! Map-of-arrays "bucket push" pattern, identity via `===` and object Map keys,
//! `structuredClone` independence, deep nested structures, and references shared
//! across multiple data structures. Every check stringifies the result with
//! `JSON.stringify` or evaluates a boolean/scalar so `to_js_string` output is
//! byte-comparable to real Node, and every asserted value was verified against
//! the live interpreter and matches Node's answer.
//!
//! Cyclic structures are intentionally out of scope (the security workflow owns
//! DoS/cycle behavior). Documented divergences this suite deliberately works
//! around (verified against the live interpreter, see STRESS-PASS-BUGS.md and
//! probing notes):
//!   * `WeakMap` is not a constructor here, so weak-keyed identity is not tested.
//!   * `Object.freeze` does not enforce immutability (writes silently succeed),
//!     so frozen-object semantics are not asserted.
//!   * `structuredClone` does NOT preserve *internal* shared-reference identity:
//!     if `o.a === o.b` (same handle reachable by two paths), the clone gets two
//!     *independent* copies (`c.a !== c.b`) rather than one shared node as real
//!     JS would. The one test that touches this asserts the interpreter's actual
//!     behavior WITH a comment instead of the real-JS answer.
//!   * `.toReversed()/.toSorted()/.with()` (ES2023 immutable array methods) are
//!     not implemented; `.slice().reverse()` / `.slice().sort()` are used.
//!   * Assigning to a property of a *call-expression* result
//!     (`m.get(k).p = v`) is a compile error ("invalid assignment target"); the
//!     reference is first bound to a local (`const v = m.get(k); v.p = ...`),
//!     which is the same mutation through the same handle.

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, ZapcodeRun};

/// Run `code` to completion and stringify the final value exactly as Node would
/// print `JSON.stringify`/scalar coercion (via the heap-aware `to_js_string`).
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

/// Assert that `code` evaluates to the JS boolean `true`.
fn assert_true(code: &str) {
    assert_eq!(run_str(code), "true", "expected `true` for `{code}`");
}

/// Assert that `code` evaluates to the JS boolean `false`.
fn assert_false(code: &str) {
    assert_eq!(run_str(code), "false", "expected `false` for `{code}`");
}

// ===========================================================================
// 1. Aliasing: a second binding to the same array/object is the SAME thing.
// ===========================================================================

#[test]
fn alias_array_push_visible_through_original() {
    // const b = a aliases the same handle; a push through b is seen through a.
    assert_eq!(run_str("const a=[1]; const b=a; b.push(2); a.length"), "2");
    assert_eq!(
        run_str("const a=[1]; const b=a; b.push(2); JSON.stringify(a)"),
        "[1,2]"
    );
    // ...and through b too (same slot).
    assert_eq!(
        run_str("const a=[1]; const b=a; a.push(2); JSON.stringify(b)"),
        "[1,2]"
    );
}

#[test]
fn alias_object_property_write_visible_through_original() {
    assert_eq!(run_str("const a={n:1}; const b=a; b.n=9; a.n"), "9");
    assert_eq!(run_str("const a={n:1}; const b=a; a.n=9; b.n"), "9");
    // adding a brand-new property through one alias appears on the other.
    assert_eq!(run_str("const a={}; const b=a; b.x=7; a.x"), "7");
    assert_true("const a={}; const b=a; b.x=7; ('x' in a)");
}

#[test]
fn alias_chain_three_deep_all_share() {
    // a -> b -> c all point at the same handle.
    assert_eq!(
        run_str("const a=[]; const b=a; const c=b; c.push('x'); JSON.stringify(a)"),
        "[\"x\"]"
    );
    assert_eq!(run_str("const a={}; const b=a; const c=b; a.k=1; c.k"), "1");
}

#[test]
fn alias_nested_object_grabbed_is_same_handle() {
    // Reading a nested object into a local binds the SAME handle, not a copy.
    assert_eq!(run_str("const o={x:{y:1}}; const p=o.x; p.y=9; o.x.y"), "9");
    assert_eq!(
        run_str("const o={x:[1]}; const p=o.x; p.push(2); JSON.stringify(o.x)"),
        "[1,2]"
    );
}

#[test]
fn alias_reassigning_binding_does_not_affect_original() {
    // Reassigning the LOCAL binding (not its contents) rebinds only the local.
    assert_eq!(
        run_str("const a=[1]; let b=a; b=[9]; JSON.stringify(a)"),
        "[1]"
    );
    assert_eq!(run_str("const a={n:1}; let b=a; b={n:9}; a.n"), "1");
}

#[test]
fn alias_element_overwrite_does_not_mutate_grabbed_ref() {
    // Grabbing a[0] then overwriting the slot a[0]={...} leaves the grabbed ref
    // pointing at the OLD object.
    assert_eq!(
        run_str("const a=[{n:1}]; const x=a[0]; a[0]={n:2}; x.n"),
        "1"
    );
    assert_eq!(run_str("const a=[{n:1}]; const x=a[0]; a[0]={n:2}; a[0].n"), "2");
}

// ===========================================================================
// 2. Mutate-through-parameter: passing an array/object passes the handle.
// ===========================================================================

#[test]
fn param_array_mutation_visible_in_caller() {
    assert_eq!(
        run_str("const a=[1]; const f=(x)=>x.push(9); f(a); a.length"),
        "2"
    );
    assert_eq!(
        run_str("const a=[1,2,3]; const f=(x)=>{x[0]=99}; f(a); a[0]"),
        "99"
    );
}

#[test]
fn param_object_mutation_visible_in_caller() {
    assert_eq!(run_str("const o={n:1}; const f=(x)=>{x.n=42}; f(o); o.n"), "42");
    assert_eq!(
        run_str("const o={}; const f=(x)=>{x.added=true}; f(o); JSON.stringify(o)"),
        "{\"added\":true}"
    );
}

#[test]
fn param_reassignment_does_not_leak() {
    // Reassigning the parameter rebinds only the callee's local copy of the ref.
    assert_eq!(
        run_str("let o={n:1}; const f=(x)=>{x={n:99}}; f(o); o.n"),
        "1"
    );
    assert_eq!(
        run_str("const a=[1]; const f=(x)=>{x=[9,9,9]}; f(a); a.length"),
        "1"
    );
}

#[test]
fn param_deep_mutation_through_nested_path() {
    assert_eq!(
        run_str("const o={a:{b:{c:1}}}; const f=(x)=>{x.a.b.c=100}; f(o); o.a.b.c"),
        "100"
    );
    assert_eq!(
        run_str(
            "const o={list:[]}; const add=(x,v)=>x.list.push(v); add(o,1); add(o,2); JSON.stringify(o.list)"
        ),
        "[1,2]"
    );
}

#[test]
fn param_same_handle_passed_to_two_functions() {
    // Both functions mutate the same handle in sequence.
    assert_eq!(
        run_str(
            "const a=[]; const f=(x)=>x.push(1); const g=(x)=>x.push(2); f(a); g(a); JSON.stringify(a)"
        ),
        "[1,2]"
    );
}

#[test]
fn returned_reference_is_live() {
    // A function returning a freshly-made object hands back the live handle.
    assert_eq!(
        run_str("const f=()=>{const o={n:1};return o}; const r=f(); r.n=9; r.n"),
        "9"
    );
    // The same captured object is returned on each call (identity stable).
    assert_true("const f=(()=>{const o={};return ()=>o})(); f()===f()");
}

// ===========================================================================
// 3. Mutating the for-of loop binding (and other loop forms).
// ===========================================================================

#[test]
fn forof_object_binding_mutation_visible_in_array() {
    // const x of arr binds each element's object handle; x.n*=10 mutates in place.
    assert_eq!(
        run_str("const a=[{n:1}]; for(const x of a) x.n*=10; a[0].n"),
        "10"
    );
    assert_eq!(
        run_str("const a=[{n:1},{n:2}]; for(const x of a){x.n*=10} JSON.stringify(a)"),
        "[{\"n\":10},{\"n\":20}]"
    );
}

#[test]
fn forof_nested_array_binding_mutation_visible() {
    assert_eq!(
        run_str("const a=[[1],[2]]; for(const row of a) row.push(0); JSON.stringify(a)"),
        "[[1,0],[2,0]]"
    );
}

#[test]
fn forof_primitive_binding_reassignment_does_not_mutate_array() {
    // Primitives are copied into the binding; reassigning x doesn't touch a.
    assert_eq!(
        run_str("const a=[1,2,3]; for(let x of a){x*=10} JSON.stringify(a)"),
        "[1,2,3]"
    );
}

#[test]
fn forof_entries_binding_value_is_live_object() {
    // Destructuring [i, el] in for-of still binds el to the live element handle.
    assert_eq!(
        run_str(
            "const a=[{n:0},{n:0}]; for(const [i,el] of a.entries()) el.n=i; JSON.stringify(a)"
        ),
        "[{\"n\":0},{\"n\":1}]"
    );
}

#[test]
fn forin_keys_mutation_through_object() {
    // for-in over keys; writing through the object is visible afterwards.
    assert_eq!(
        run_str("const o={a:1,b:2}; for(const k in o){o[k]*=10} JSON.stringify(o)"),
        "{\"a\":10,\"b\":20}"
    );
}

#[test]
fn forof_map_iteration_yields_live_value_handles() {
    // Iterating a Map yields the SAME stored value handle.
    assert_true(
        "const inner={n:1}; const m=new Map(); m.set('a',inner); let ok=false; for(const [k,v] of m){ok=(v===inner)} ok",
    );
    // ...and mutating via the iterated binding is visible in the Map.
    assert_eq!(
        run_str(
            "const m=new Map([['a',{n:1}]]); for(const [k,v] of m){v.n=99} const g=m.get('a'); g.n"
        ),
        "99"
    );
}

#[test]
fn array_callbacks_can_mutate_elements_in_place() {
    // forEach/map callbacks receive the live element handle.
    assert_eq!(run_str("const a=[{n:1}]; a.forEach(e=>e.n=9); a[0].n"), "9");
    assert_eq!(
        run_str("const a=[{n:1},{n:2}]; a.map(e=>{e.n+=100;return e}); JSON.stringify(a)"),
        "[{\"n\":101},{\"n\":102}]"
    );
}

// ===========================================================================
// 4. Map-of-arrays "bucket push" pattern.
// ===========================================================================

#[test]
fn map_bucket_push_visible_on_next_get() {
    // m.get(k) returns the SAME bucket-array handle, so push is durable.
    assert_eq!(
        run_str(
            "const m=new Map(); m.set('k',[]); m.get('k').push(1); m.get('k').push(2); m.get('k').length"
        ),
        "2"
    );
    assert_eq!(
        run_str(
            "const m=new Map(); m.set('k',[]); const b=m.get('k'); b.push('a'); b.push('b'); JSON.stringify(m.get('k'))"
        ),
        "[\"a\",\"b\"]"
    );
}

#[test]
fn map_bucket_getor_create_pattern() {
    // The classic "get-or-create bucket" grouping pattern.
    assert_eq!(
        run_str(
            "const m=new Map(); const get=(k)=>{ if(!m.has(k)) m.set(k,[]); return m.get(k) }; \
             get('a').push(1); get('a').push(2); get('b').push(3); \
             JSON.stringify([m.get('a'),m.get('b')])"
        ),
        "[[1,2],[3]]"
    );
}

#[test]
fn object_bucket_nullish_assign_pattern() {
    // (buckets[k] ??= []).push(v) — bucket grouping on a plain object.
    assert_eq!(
        run_str(
            "const buckets={}; const add=(k,v)=>{ (buckets[k] ??= []).push(v) }; \
             add('x',1); add('x',2); add('y',3); JSON.stringify(buckets)"
        ),
        "{\"x\":[1,2],\"y\":[3]}"
    );
}

#[test]
fn map_value_object_mutation_through_get() {
    // The value object returned by get() is the live handle.
    assert_eq!(
        run_str("const m=new Map(); m.set('k',{n:1}); const v=m.get('k'); v.n=42; const v2=m.get('k'); v2.n"),
        "42"
    );
}

#[test]
fn map_bucket_count_pattern() {
    // count occurrences: map value is a number (copied), reassigned via set.
    assert_eq!(
        run_str(
            "const m=new Map(); for(const w of ['a','b','a','a','b']){ m.set(w,(m.get(w)??0)+1) } \
             JSON.stringify([m.get('a'),m.get('b')])"
        ),
        "[3,2]"
    );
}

// ===========================================================================
// 5. Identity: === is handle identity; object keys in a Map use identity.
// ===========================================================================

#[test]
fn identity_is_reflexive() {
    assert_true("const a=[1]; a===a");
    assert_true("const o={}; o===o");
    assert_true("const o={a:{}}; o.a===o.a");
}

#[test]
fn distinct_literals_are_not_identical() {
    assert_false("[1]===[1]");
    assert_false("({})===({})");
    assert_false("[]===[]");
    // ...but two bindings to ONE literal are.
    assert_true("const a=[]; const b=a; a===b");
}

#[test]
fn same_object_stored_twice_in_array_is_identical() {
    assert_true("const inner={n:1}; const a=[inner,inner]; a[0]===a[1]");
    // mutating one view is the other.
    assert_eq!(
        run_str("const inner={n:1}; const a=[inner,inner]; a[0].n=5; a[1].n"),
        "5"
    );
}

#[test]
fn map_get_returns_same_handle_as_set() {
    assert_true("const m=new Map(); const o={}; m.set('k',o); m.get('k')===o");
    assert_true("const a=[1]; const m=new Map(); m.set('k',a); m.get('k')===a");
}

#[test]
fn map_object_key_uses_reference_identity() {
    // The same object handle round-trips as a key.
    assert_eq!(run_str("const k={}; const m=new Map(); m.set(k,9); m.get(k)"), "9");
    // A distinct same-shaped object is a different key (miss).
    assert_true("const k={}; const m=new Map(); m.set(k,9); m.get({})===undefined");
    assert_false("const k={}; const m=new Map(); m.set(k,9); m.has({})");
    assert_true("const k={}; const m=new Map(); m.set(k,9); m.has(k)");
}

#[test]
fn map_two_distinct_object_keys_coexist() {
    assert_eq!(
        run_str(
            "const a={}; const b={}; const m=new Map(); m.set(a,1); m.set(b,2); JSON.stringify([m.get(a),m.get(b),m.size])"
        ),
        "[1,2,2]"
    );
}

#[test]
fn map_array_key_identity() {
    assert_eq!(
        run_str("const k=[1,2]; const m=new Map(); m.set(k,'v'); m.get(k)"),
        "v"
    );
    assert_false("const k=[1,2]; const m=new Map(); m.set(k,'v'); m.has([1,2])");
}

#[test]
fn set_membership_uses_reference_identity() {
    assert_true("const o={}; const s=new Set(); s.add(o); s.has(o)");
    assert_false("const s=new Set(); s.add({}); s.has({})");
    // Adding the same handle twice keeps size 1.
    assert_eq!(run_str("const o={}; const s=new Set(); s.add(o); s.add(o); s.size"), "1");
    // Two distinct objects are two members.
    assert_eq!(run_str("const s=new Set(); s.add({}); s.add({}); s.size"), "2");
}

#[test]
fn set_iteration_yields_live_handles() {
    assert_true("const o={n:1}; const s=new Set([o]); let ok=false; for(const x of s){ok=(x===o)} ok");
    assert_eq!(
        run_str("const o={n:1}; const s=new Set([o]); for(const x of s){x.n=9} o.n"),
        "9"
    );
}

// ===========================================================================
// 6. structuredClone: deep, fully independent copy.
// ===========================================================================

#[test]
fn structured_clone_top_level_independent() {
    // Mutating the original array does not affect the clone.
    assert_eq!(
        run_str(
            "const o={a:[1]}; const c=structuredClone(o); o.a.push(2); JSON.stringify([o.a,c.a])"
        ),
        "[[1,2],[1]]"
    );
    // The clone is a distinct top-level identity.
    assert_false("const o={a:[1]}; const c=structuredClone(o); o===c");
}

#[test]
fn structured_clone_nested_nodes_are_distinct_handles() {
    // No node anywhere in the clone shares a handle with the original.
    assert_false(
        "const o={a:[{b:[1]}]}; const c=structuredClone(o); (o.a===c.a)||(o.a[0]===c.a[0])||(o.a[0].b===c.a[0].b)",
    );
}

#[test]
fn structured_clone_deep_independence_both_directions() {
    // Mutating deep in the original.
    assert_eq!(
        run_str(
            "const o={a:{b:{c:[1,2]}}}; const c=structuredClone(o); o.a.b.c.push(3); JSON.stringify([o.a.b.c,c.a.b.c])"
        ),
        "[[1,2,3],[1,2]]"
    );
    // Mutating deep in the clone leaves the original untouched.
    assert_eq!(
        run_str(
            "const o={a:{b:[1]}}; const c=structuredClone(o); c.a.b.push(99); JSON.stringify(o.a.b)"
        ),
        "[1]"
    );
}

#[test]
fn structured_clone_array_of_arrays_independent() {
    assert_eq!(
        run_str(
            "const a=[[1],[2]]; const c=structuredClone(a); a[0].push(9); JSON.stringify([a[0],c[0]])"
        ),
        "[[1,9],[1]]"
    );
}

#[test]
fn structured_clone_of_class_instance_independent() {
    // Cloning a class instance yields a fresh, independent object with the data.
    assert_eq!(run_str("class P{constructor(n){this.n=n}} const p=new P(5); const c=structuredClone(p); c.n"), "5");
    assert_eq!(
        run_str(
            "class P{constructor(n){this.n=n}} const p=new P(1); const c=structuredClone(p); c.n=9; p.n"
        ),
        "1"
    );
}

#[test]
fn structured_clone_of_map_is_independent() {
    // A different Map instance.
    assert_false("const m=new Map([['k',1]]); const c=structuredClone(m); c===m");
    // Setting a key on the clone does not affect the original.
    assert_eq!(
        run_str("const m=new Map([['k',1]]); const c=structuredClone(m); c.set('k',2); m.get('k')"),
        "1"
    );
    // Nested value objects are deep-copied.
    assert_eq!(
        run_str(
            "const m=new Map([['k',{n:1}]]); const c=structuredClone(m); const cv=c.get('k'); cv.n=9; const mv=m.get('k'); mv.n"
        ),
        "1"
    );
}

#[test]
fn structured_clone_of_set_is_independent() {
    assert_eq!(
        run_str(
            "const s=new Set([1,2]); const c=structuredClone(s); c.add(3); JSON.stringify([[...s],[...c]])"
        ),
        "[[1,2],[1,2,3]]"
    );
    // Object members are deep-copied.
    assert_eq!(
        run_str(
            "const o={n:1}; const s=new Set([o]); const c=structuredClone(s); const arr=[...c]; const e=arr[0]; e.n=9; o.n"
        ),
        "1"
    );
}

#[test]
fn structured_clone_primitives_pass_through() {
    assert_eq!(run_str("structuredClone(5)"), "5");
    assert_eq!(run_str("structuredClone('hi')"), "hi");
    assert_true("structuredClone(null)===null");
    assert_true("structuredClone(true)===true");
}

#[test]
fn structured_clone_of_date_keeps_value_and_brand() {
    assert_true("const d=new Date(0); const c=structuredClone(d); c instanceof Date");
    assert_eq!(run_str("const d=new Date(1000); const c=structuredClone(d); c.getTime()"), "1000");
    assert_false("const d=new Date(0); const c=structuredClone(d); c===d");
}

#[test]
fn structured_clone_internal_shared_ref_documented_divergence() {
    // DOCUMENTED DIVERGENCE (asserting the interpreter's actual behavior, NOT
    // real JS): when one handle is reachable by two paths in the source object
    // (o.a === o.b), real `structuredClone` preserves that aliasing in the clone
    // (clone.a === clone.b). This interpreter instead deep-copies each path into
    // an INDEPENDENT node, so the clone's two paths are distinct objects.
    // (Real JS would be `true` / `42` here.)
    assert_false("const shared={n:1}; const o={a:shared,b:shared}; const c=structuredClone(o); c.a===c.b");
    assert_eq!(
        run_str(
            "const shared={n:1}; const o={a:shared,b:shared}; const c=structuredClone(o); \
             const ca=c.a; ca.n=42; const cb=c.b; cb.n"
        ),
        "1" // real JS: 42 (clone preserves the alias). Interpreter copies twice.
    );
    // Sanity: the ORIGINAL still shares the handle (this part matches real JS).
    assert_true("const shared={n:1}; const o={a:shared,b:shared}; o.a===o.b");
}

// ===========================================================================
// 7. Shallow copies share nested references (the counterpart to deep clone).
// ===========================================================================

#[test]
fn object_spread_is_shallow() {
    // {...o} copies the top level but shares nested handles.
    assert_true("const inner={n:1}; const o={x:inner}; const o2={...o}; o2.x===o.x");
    assert_eq!(
        run_str("const o={a:[1]}; const c={...o}; c.a.push(2); JSON.stringify(o.a)"),
        "[1,2]"
    );
    // ...but the top-level object is a distinct identity.
    assert_false("const o={n:1}; const o2={...o}; o===o2");
}

#[test]
fn array_spread_is_shallow() {
    assert_true("const inner={n:1}; const a=[inner]; const b=[...a]; b[0]===a[0]");
    assert_eq!(
        run_str("const a=[[1]]; const b=[...a]; b[0].push(2); JSON.stringify(a[0])"),
        "[1,2]"
    );
    assert_false("const a=[1]; const b=[...a]; a===b");
}

#[test]
fn array_slice_is_shallow() {
    assert_true("const inner={n:1}; const a=[inner]; const b=a.slice(); b[0]===a[0]");
    assert_false("const a=[1]; const b=a.slice(); a===b");
}

#[test]
fn array_concat_is_shallow() {
    assert_true("const inner={n:1}; const a=[inner]; const b=[].concat(a); b[0]===a[0]");
}

#[test]
fn array_from_is_shallow() {
    assert_true("const inner={n:1}; const a=[inner]; const b=Array.from(a); b[0]===a[0]");
    assert_true("const inner={n:1}; const s=new Set([inner]); const a=Array.from(s); a[0]===inner");
}

#[test]
fn object_assign_is_shallow() {
    assert_true("const inner={n:1}; const o={x:inner}; const t=Object.assign({},o); t.x===o.x");
    assert_eq!(
        run_str("const o={a:{n:1}}; const t=Object.assign({},o); const ta=t.a; ta.n=9; o.a.n"),
        "9"
    );
}

#[test]
fn object_spread_merge_preserves_shared_ref() {
    // Same inner object reachable through two spread sources stays one handle.
    assert_true("const inner={n:1}; const a={x:inner}; const b={y:inner}; const m={...a,...b}; m.x===m.y");
}

#[test]
fn json_roundtrip_breaks_identity_and_is_independent() {
    // JSON.parse(JSON.stringify(...)) is a deep, independent copy.
    assert_false("const o={a:[1]}; const c=JSON.parse(JSON.stringify(o)); o.a===c.a");
    assert_eq!(
        run_str(
            "const o={a:{b:1}}; const c=JSON.parse(JSON.stringify(o)); const ca=c.a; ca.b=99; o.a.b"
        ),
        "1"
    );
}

#[test]
fn array_fill_with_object_shares_one_handle() {
    // new Array(n).fill(obj) puts the SAME object in every slot.
    assert_true("const o={n:0}; const a=new Array(3).fill(o); a[0]===a[1]");
    assert_eq!(run_str("const o={n:0}; const a=new Array(3).fill(o); a[0].n=5; a[2].n"), "5");
}

// ===========================================================================
// 8. References returned by query/transform methods are live.
// ===========================================================================

#[test]
fn find_filter_return_live_element_handles() {
    assert_true("const inner={n:1}; const a=[inner]; a.find(e=>e.n===1)===inner");
    assert_true("const inner={n:1}; const a=[inner]; a.filter(()=>true)[0]===inner");
}

#[test]
fn splice_returns_live_removed_handles() {
    assert_true("const inner={n:1}; const a=[inner]; const removed=a.splice(0,1); removed[0]===inner");
    // mutating the removed object is the same object that was in the array.
    assert_eq!(
        run_str("const inner={n:1}; const a=[inner]; const r=a.splice(0,1); const e=r[0]; e.n=9; inner.n"),
        "9"
    );
}

#[test]
fn flat_and_flatmap_preserve_element_handles() {
    assert_true("const inner={n:1}; const a=[[inner]]; a.flat()[0]===inner");
    assert_true("const inner={n:1}; const a=[inner]; a.flatMap(x=>[x])[0]===inner");
}

#[test]
fn sort_and_reverse_keep_element_identity() {
    // In-place sort reorders but keeps the same element handles.
    assert_true("const x={v:2}; const y={v:1}; const a=[x,y]; a.sort((p,q)=>p.v-q.v); a[0]===y");
    assert_true("const x={v:1}; const a=[x,{v:2}]; a.reverse(); a[1]===x");
    // slice().reverse() (immutable .toReversed not available) keeps the handle.
    assert_true("const x={v:1}; const a=[x]; const b=a.slice().reverse(); b[0]===x");
}

#[test]
fn object_values_and_entries_return_live_handles() {
    assert_true("const inner={n:1}; const o={a:inner}; Object.values(o)[0]===inner");
    assert_true("const inner={n:1}; const o={a:inner}; Object.entries(o)[0][1]===inner");
}

#[test]
fn reduce_accumulator_is_threaded_by_reference() {
    assert_eq!(
        run_str("const a=[1,2,3]; const r=a.reduce((acc,x)=>{acc.push(x*2);return acc},[]); JSON.stringify(r)"),
        "[2,4,6]"
    );
    // grouping reduce: object accumulator mutated in place. (Buckets are created
    // in ascending key order so insertion order == numeric order and the result
    // matches Node regardless of integer-key reordering nuances.)
    assert_eq!(
        run_str(
            "const words=['a','b','cc','ddd']; \
             const r=words.reduce((acc,w)=>{ (acc[w.length] ??= []).push(w); return acc },{}); \
             JSON.stringify(r)"
        ),
        "{\"1\":[\"a\",\"b\"],\"2\":[\"cc\"],\"3\":[\"ddd\"]}"
    );
}

// ===========================================================================
// 9. Deep nested structures (matrices, trees, adjacency lists).
// ===========================================================================

#[test]
fn matrix_row_alias_write_visible() {
    assert_eq!(run_str("const grid=[[0,0],[0,0]]; const row=grid[0]; row[1]=9; grid[0][1]"), "9");
    // build identity matrix via shared row refs in a loop.
    assert_eq!(
        run_str(
            "const g=[[0,0,0],[0,0,0],[0,0,0]]; for(let i=0;i<3;i++){ const r=g[i]; r[i]=1 } \
             JSON.stringify(g)"
        ),
        "[[1,0,0],[0,1,0],[0,0,1]]"
    );
}

#[test]
fn deep_path_alias_push_visible() {
    assert_eq!(
        run_str("const o={a:{b:{c:{d:[1]}}}}; const p=o.a.b.c.d; p.push(2); JSON.stringify(o.a.b.c.d)"),
        "[1,2]"
    );
}

#[test]
fn nested_object_in_array_in_object_mutation() {
    assert_eq!(
        run_str(
            "const state={users:[{name:'a',tags:[]}]}; const u=state.users[0]; u.tags.push('admin'); \
             JSON.stringify(state.users[0].tags)"
        ),
        "[\"admin\"]"
    );
}

#[test]
fn tree_node_child_push_visible_from_root() {
    assert_eq!(
        run_str(
            "const root={v:1,children:[]}; const child={v:2,children:[]}; root.children.push(child); \
             const grandchild={v:3,children:[]}; root.children[0].children.push(grandchild); \
             JSON.stringify(root)"
        ),
        "{\"v\":1,\"children\":[{\"v\":2,\"children\":[{\"v\":3,\"children\":[]}]}]}"
    );
}

#[test]
fn adjacency_list_shared_bucket_push() {
    assert_eq!(
        run_str("const g={a:[]}; const adj=g.a; adj.push('b'); adj.push('c'); JSON.stringify(g.a)"),
        "[\"b\",\"c\"]"
    );
}

#[test]
fn nested_map_in_object_mutation() {
    assert_eq!(run_str("const o={m:new Map()}; o.m.set('k',1); o.m.get('k')"), "1");
    // array holding a map; mutating through the array view is visible on the map.
    assert_eq!(run_str("const m=new Map(); const a=[m]; a[0].set('x',1); m.get('x')"), "1");
}

// ===========================================================================
// 10. Shared references across multiple data structures.
// ===========================================================================

#[test]
fn one_object_shared_between_array_and_object() {
    assert_eq!(
        run_str("const shared={c:0}; const a=[shared]; const o={ref:shared}; a[0].c=3; o.ref.c"),
        "3"
    );
    // and the reverse direction.
    assert_eq!(
        run_str("const shared={c:0}; const a=[shared]; const o={ref:shared}; o.ref.c=7; a[0].c"),
        "7"
    );
}

#[test]
fn one_array_shared_between_two_maps() {
    assert_eq!(
        run_str(
            "const shared=[]; const m1=new Map(); const m2=new Map(); m1.set('x',shared); m2.set('y',shared); \
             m1.get('x').push(7); m2.get('y').length"
        ),
        "1"
    );
}

#[test]
fn one_array_shared_between_map_and_set() {
    assert_eq!(
        run_str(
            "const a=[]; const m=new Map(); const s=new Set(); m.set('k',a); s.add(a); a.push('x'); \
             JSON.stringify([m.get('k'),[...s][0]])"
        ),
        "[[\"x\"],[\"x\"]]"
    );
}

#[test]
fn map_value_shared_with_outer_array_stays_in_sync() {
    assert_eq!(
        run_str("const arr=[1]; const m=new Map(); m.set('k',arr); arr.push(2); m.get('k').length"),
        "2"
    );
}

#[test]
fn destructuring_aliases_the_same_nested_handle() {
    // Two destructured names over the same source give the same handle.
    assert_true("const o={p:{n:1}}; const {p}=o; const {p:p2}=o; p===p2");
    // Array destructuring aliases the element.
    assert_eq!(run_str("const inner={n:1}; const a=[inner]; const [first]=a; first.n=7; a[0].n"), "7");
    // Object destructuring aliases the nested object.
    assert_eq!(run_str("const o={inner:{n:1}}; const {inner}=o; inner.n=7; o.inner.n"), "7");
    // Rest pattern copies handles (shallow).
    assert_true("const inner={n:1}; const a=[0,inner]; const [,...rest]=a; rest[0]===inner");
}

// ===========================================================================
// 11. Closures capture references (shared mutable state).
// ===========================================================================

#[test]
fn closure_captures_shared_array() {
    assert_eq!(
        run_str(
            "const make=()=>{const a=[];return {add:(x)=>a.push(x),get:()=>a}}; \
             const m=make(); m.add(1); m.add(2); JSON.stringify(m.get())"
        ),
        "[1,2]"
    );
}

#[test]
fn two_closures_share_one_captured_object() {
    // Both closures over the same captured object see each other's writes.
    assert_eq!(
        run_str(
            "const make=()=>{const o={n:0};return [()=>o.n++, ()=>o.n]}; \
             const [inc,read]=make(); inc(); inc(); read()"
        ),
        "2"
    );
    assert_eq!(
        run_str("const inc=(()=>{const o={n:0};return ()=>++o.n})(); JSON.stringify([inc(),inc(),inc()])"),
        "[1,2,3]"
    );
}

#[test]
fn default_param_value_is_a_fresh_handle_each_call() {
    // A default `[]` is re-created per call, so calls don't share the array.
    assert_eq!(
        run_str("const f=(a=[])=>{a.push(1);return a.length}; JSON.stringify([f(),f()])"),
        "[1,1]"
    );
}

#[test]
fn captured_object_outlives_function_call() {
    // The returned closure keeps the captured object's handle alive & mutable.
    assert_eq!(
        run_str(
            "function counter(){ const state={count:0}; return {bump(){state.count++}, value(){return state.count}} } \
             const c=counter(); c.bump(); c.bump(); c.bump(); c.value()"
        ),
        "3"
    );
}

// ===========================================================================
// 12. Class instances are reference types too.
// ===========================================================================

#[test]
fn class_instance_alias_shares_state() {
    assert_eq!(run_str("class P{constructor(n){this.n=n}} const p=new P(1); const q=p; q.n=9; p.n"), "9");
    assert_eq!(run_str("class P{constructor(n){this.n=n}} const a=[new P(1)]; const x=a[0]; x.n=5; a[0].n"), "5");
}

#[test]
fn class_method_mutates_this_visible_through_alias() {
    assert_eq!(
        run_str("class C{constructor(){this.v=0} inc(){this.v++}} const c=new C(); const d=c; d.inc(); d.inc(); c.v"),
        "2"
    );
}

#[test]
fn class_instance_internal_array_field_is_live() {
    assert_eq!(
        run_str(
            "class P{constructor(){this.items=[]} add(x){this.items.push(x)}} \
             const p=new P(); p.add(1); p.add(2); JSON.stringify(p.items)"
        ),
        "[1,2]"
    );
}

#[test]
fn class_instances_sort_keeps_identity() {
    assert_true(
        "class P{constructor(v){this.v=v}} const a=[new P(2),new P(1)]; const orig=a[0]; a.sort((x,y)=>x.v-y.v); a[1]===orig",
    );
}

#[test]
fn class_instance_passed_to_function_is_mutated_in_place() {
    assert_eq!(
        run_str(
            "class Box{constructor(){this.v=0}} const reset=(b)=>{b.v=100}; \
             const box=new Box(); reset(box); box.v"
        ),
        "100"
    );
}

// ===========================================================================
// 13. Sequenced, multi-step scenarios (integration of the above).
// ===========================================================================

#[test]
fn shopping_cart_shared_line_item() {
    // A line item object is referenced from the cart array and an index map;
    // updating quantity through the map view is reflected in the cart.
    assert_eq!(
        run_str(
            "const item={sku:'A',qty:1}; \
             const cart=[item]; const bySku=new Map([['A',item]]); \
             const found=bySku.get('A'); found.qty+=4; \
             JSON.stringify(cart[0])"
        ),
        "{\"sku\":\"A\",\"qty\":5}"
    );
}

#[test]
fn snapshot_via_clone_then_keep_mutating_original() {
    // Take an independent snapshot, then keep mutating the live state.
    assert_eq!(
        run_str(
            "const state={log:['init']}; const snapshot=structuredClone(state); \
             state.log.push('a'); state.log.push('b'); \
             JSON.stringify([snapshot.log, state.log])"
        ),
        "[[\"init\"],[\"init\",\"a\",\"b\"]]"
    );
}

#[test]
fn move_object_between_collections_keeps_identity() {
    // Pop an object out of one array and push into another; it's still the same.
    assert_true(
        "const a=[{id:1}]; const b=[]; const moved=a.pop(); b.push(moved); b[0]===moved && a.length===0",
    );
    assert_eq!(
        run_str(
            "const a=[{id:1}]; const b=[]; const moved=a.pop(); b.push(moved); moved.id=99; b[0].id"
        ),
        "99"
    );
}

#[test]
fn dedup_by_identity_via_set() {
    // Pushing the same handle multiple times; Set collapses to unique handles.
    assert_eq!(
        run_str(
            "const x={}; const y={}; const items=[x,x,y,x,y]; const seen=new Set(); const out=[]; \
             for(const it of items){ if(!seen.has(it)){ seen.add(it); out.push(it) } } out.length"
        ),
        "2"
    );
}

#[test]
fn fan_out_then_fan_in_shared_accumulator() {
    // Several functions accumulate into one shared array passed around.
    assert_eq!(
        run_str(
            "const acc=[]; const stepA=(a)=>a.push('A'); const stepB=(a)=>{a.push('B1');a.push('B2')}; \
             stepA(acc); stepB(acc); stepA(acc); JSON.stringify(acc)"
        ),
        "[\"A\",\"B1\",\"B2\",\"A\"]"
    );
}
