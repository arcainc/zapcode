//! Conformance breadth: Object semantics.
//!
//! Wide, test262-flavored coverage of object construction and reflection:
//!   - object literals: shorthand, computed keys, method shorthand,
//!     getter/setter syntax, duplicate keys
//!   - property read/write (dot vs bracket), nested writes
//!   - `in` / `delete` operators
//!   - object spread / merge and override ordering
//!   - the `Object.*` static surface that zapcode actually provides:
//!     `keys`/`values`/`entries`/`fromEntries`/`assign`/`hasOwn`/`freeze`
//!   - key insertion order and numeric-vs-string key handling
//!
//! Results are stringified with `JSON.stringify` (or reduced to a scalar
//! expression) so the harness's `to_js_string` output is deterministic and
//! byte-comparable.
//!
//! DOCUMENTED DIVERGENCES asserted here against zapcode's ACTUAL behavior
//! (NOT real-Node), each flagged inline with `DIVERGENCE`:
//!   1. Integer-like keys are kept in insertion order; real JS reorders them
//!      ascending ahead of string keys. zapcode has a single insertion-ordered
//!      key list for all keys.
//!   2. `get x()`/`set x()` are stored as ordinary function-valued properties,
//!      not accessor descriptors: reading a getter yields the function itself,
//!      and assigning through a setter just overwrites the property.
//!   3. Object-spread of a string or array spreads no own enumerable string
//!      keys (`{...[1,2]}` and `{...'ab'}` are `{}`); `Object.keys('ab')` is
//!      `[]`. (zapcode does not expose index keys as own-enumerable on those.)
//! `Object.freeze` is now enforcing (writes/adds/deletes on a frozen object are
//! silently ignored) and `Object.isFrozen`, `Object.create`,
//! `Object.getPrototypeOf`, and `Object.getOwnPropertyNames` are implemented.
//! `Object.defineProperty` is still not provided.

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

// ============================================================================
// 1. Object literals
// ============================================================================

#[test]
fn literal_basic_and_empty() {
    assert_eq!(run_str("JSON.stringify({})"), "{}");
    assert_eq!(run_str("JSON.stringify({a: 1})"), "{\"a\":1}");
    assert_eq!(run_str("JSON.stringify({a: 1, b: 2, c: 3})"), "{\"a\":1,\"b\":2,\"c\":3}");
    // nested literals
    assert_eq!(
        run_str("JSON.stringify({a: {b: {c: 1}}})"),
        "{\"a\":{\"b\":{\"c\":1}}}"
    );
    // mixed value types
    assert_eq!(
        run_str("JSON.stringify({n: 1, s: 'x', b: true, z: null, arr: [1, 2]})"),
        "{\"n\":1,\"s\":\"x\",\"b\":true,\"z\":null,\"arr\":[1,2]}"
    );
}

#[test]
fn literal_string_keys_quoted_and_unquoted() {
    assert_eq!(run_str("JSON.stringify({'a-b': 1})"), "{\"a-b\":1}");
    assert_eq!(run_str("JSON.stringify({'with space': 1})"), "{\"with space\":1}");
    assert_eq!(run_str("JSON.stringify({\"\": 0})"), "{\"\":0}");
    // unquoted reserved-ish identifiers are valid keys
    assert_eq!(run_str("JSON.stringify({if: 1, class: 2})"), "{\"if\":1,\"class\":2}");
}

#[test]
fn literal_shorthand() {
    assert_eq!(run_str("const a = 1, b = 2; JSON.stringify({a, b})"), "{\"a\":1,\"b\":2}");
    assert_eq!(
        run_str("const x = 'hi'; JSON.stringify({x, y: x + '!'})"),
        "{\"x\":\"hi\",\"y\":\"hi!\"}"
    );
    // shorthand mixed with explicit, preserving order
    assert_eq!(
        run_str("const m = 9; JSON.stringify({first: 1, m, last: 3})"),
        "{\"first\":1,\"m\":9,\"last\":3}"
    );
}

#[test]
fn literal_computed_keys() {
    assert_eq!(run_str("const k = 'foo'; JSON.stringify({[k]: 1})"), "{\"foo\":1}");
    assert_eq!(
        run_str("JSON.stringify({['a' + 'b']: 1, ['x' + 1]: 2})"),
        "{\"ab\":1,\"x1\":2}"
    );
    // computed numeric key coerces to string
    assert_eq!(run_str("JSON.stringify({[1 + 1]: 'a'})"), "{\"2\":\"a\"}");
    // template-literal computed key
    assert_eq!(
        run_str("const k = 'dyn'; JSON.stringify({[`${k}Key`]: 1})"),
        "{\"dynKey\":1}"
    );
    // computed key from boolean coerces to string
    assert_eq!(run_str("JSON.stringify({[true]: 1})"), "{\"true\":1}");
}

#[test]
fn literal_method_shorthand() {
    assert_eq!(run_str("const o = { greet() { return 'hi'; } }; o.greet()"), "hi");
    assert_eq!(
        run_str("const o = { add(a, b) { return a + b; } }; o.add(2, 3)"),
        "5"
    );
    // method can read `this`
    assert_eq!(
        run_str("const o = { v: 10, double() { return this.v * 2; } }; o.double()"),
        "20"
    );
    // computed method name
    assert_eq!(
        run_str("const n = 'run'; const o = { [n]() { return 7; } }; o.run()"),
        "7"
    );
}

#[test]
fn literal_duplicate_keys_last_wins() {
    // In modern JS a duplicate data key keeps the last value, single slot.
    assert_eq!(run_str("JSON.stringify({a: 1, a: 2})"), "{\"a\":2}");
    assert_eq!(run_str("JSON.stringify({a: 1, b: 2, a: 3})"), "{\"a\":3,\"b\":2}");
}

// ============================================================================
// 2. Getters / setters
//    DIVERGENCE: stored as plain function-valued properties, not accessors.
// ============================================================================

#[test]
fn getter_stored_as_function_property() {
    // DIVERGENCE: a `get x()` is a function-valued data prop; reading returns
    // the function (typeof "function"), not the computed value (real JS: 42).
    assert_eq!(run_str("const o = { get x() { return 42; } }; typeof o.x"), "function");
    // It is callable as a function and returns the body's value.
    assert_eq!(run_str("const o = { get x() { return 42; } }; o.x()"), "42");
}

#[test]
fn setter_stored_as_function_property() {
    // DIVERGENCE: a `set x()` is a function-valued data prop; assigning to `o.x`
    // overwrites the property rather than invoking the setter.
    assert_eq!(run_str("const o = { set f(v) {} }; typeof o.f"), "function");
    assert_eq!(
        run_str("const o = { _x: 0, set x(v) { this._x = v; } }; o.x = 7; o._x"),
        "0" // real JS: 7
    );
    // assignment replaces the setter function with the assigned value
    assert_eq!(
        run_str("const o = { _x: 0, set x(v) { this._x = v; } }; o.x = 7; o.x"),
        "7"
    );
}

#[test]
fn getter_excluded_from_object_literal_json() {
    // Function-valued props are dropped by JSON.stringify, so a literal with a
    // lone getter stringifies as {} (consistent with how functions serialize).
    assert_eq!(run_str("JSON.stringify({ get x() { return 5; } })"), "{}");
    // a data prop alongside it survives
    assert_eq!(
        run_str("JSON.stringify({ get x() { return 5; }, y: 2 })"),
        "{\"y\":2}"
    );
}

// ============================================================================
// 3. Property read / write
// ============================================================================

#[test]
fn read_dot_and_bracket_equivalent() {
    assert_eq!(run_str("const o = {x: 1}; o.x"), "1");
    assert_eq!(run_str("const o = {x: 1}; o['x']"), "1");
    assert_eq!(run_str("const o = {x: 1}; o['x'] === o.x"), "true");
    // missing prop reads undefined
    assert_eq!(run_str("const o = {x: 1}; String(o.zzz)"), "undefined");
    assert_eq!(run_str("const o = {x: 1}; typeof o.zzz"), "undefined");
}

#[test]
fn write_creates_and_updates() {
    assert_eq!(run_str("const o = {}; o.a = 1; JSON.stringify(o)"), "{\"a\":1}");
    assert_eq!(run_str("const o = {a: 1}; o.a = 2; o.a"), "2");
    assert_eq!(run_str("const o = {}; o['k'] = 5; o.k"), "5");
    // assignment expression yields the assigned value
    assert_eq!(run_str("const o = {}; o.a = 9"), "9");
    // newly-written key appends to insertion order
    assert_eq!(
        run_str("const o = {a: 1, b: 2}; o.c = 3; JSON.stringify(Object.keys(o))"),
        "[\"a\",\"b\",\"c\"]"
    );
}

#[test]
fn nested_write_and_deep_paths() {
    assert_eq!(
        run_str("const o = {}; o.a = {}; o.a.b = 5; JSON.stringify(o)"),
        "{\"a\":{\"b\":5}}"
    );
    assert_eq!(
        run_str("const o = {a: {b: {c: 0}}}; o.a.b.c = 99; o.a.b.c"),
        "99"
    );
    // bracket path with dynamic key
    assert_eq!(
        run_str("const o = {}; const k = 'q'; o[k] = 1; o[k + 'x'] = 2; JSON.stringify(o)"),
        "{\"q\":1,\"qx\":2}"
    );
}

#[test]
fn compound_assignment_on_props() {
    assert_eq!(run_str("const o = {n: 1}; o.n += 4; o.n"), "5");
    assert_eq!(run_str("const o = {n: 10}; o.n -= 3; o.n"), "7");
    assert_eq!(run_str("const o = {n: 2}; o.n *= 5; o.n"), "10");
    assert_eq!(run_str("const o = {s: 'a'}; o.s += 'b'; o.s"), "ab");
    assert_eq!(run_str("const o = {n: 1}; o.n++; o.n"), "2");
    assert_eq!(run_str("const o = {n: 1}; ++o.n"), "2");
}

#[test]
fn optional_chaining_reads() {
    assert_eq!(run_str("const o = {a: {b: 2}}; o?.a?.b"), "2");
    assert_eq!(run_str("const o = {}; String(o?.a?.b)"), "undefined");
    assert_eq!(run_str("const o = null; String(o?.a)"), "undefined");
    assert_eq!(run_str("const o = {f: () => 5}; o?.f?.()"), "5");
    assert_eq!(run_str("const o = {}; String(o?.['x'])"), "undefined");
}

// ============================================================================
// 4. `in` operator
// ============================================================================

#[test]
fn in_operator_own_keys() {
    assert_eq!(run_str("'a' in {a: 1}"), "true");
    assert_eq!(run_str("'b' in {a: 1}"), "false");
    assert_eq!(run_str("const o = {x: undefined}; 'x' in o"), "true"); // present, value undefined
    // numeric key membership (key coerced to string)
    assert_eq!(run_str("1 in {1: 'a'}"), "true");
    assert_eq!(run_str("'1' in {1: 'a'}"), "true");
}

#[test]
fn in_operator_after_mutation() {
    assert_eq!(run_str("const o = {a: 1}; delete o.a; 'a' in o"), "false");
    assert_eq!(run_str("const o = {}; o.k = 1; 'k' in o"), "true");
    assert_eq!(run_str("const o = {a: 1}; o.a = undefined; 'a' in o"), "true");
}

#[test]
fn in_operator_does_not_walk_prototype() {
    // DIVERGENCE-adjacent: zapcode objects have no reachable Object.prototype via
    // `in`, so inherited names like toString are not "in" a plain object.
    assert_eq!(run_str("'toString' in {}"), "false"); // real JS: true
    assert_eq!(run_str("'hasOwnProperty' in {}"), "false"); // real JS: true
}

// ============================================================================
// 5. `delete` operator
// ============================================================================

#[test]
fn delete_removes_key_and_returns_true() {
    assert_eq!(run_str("const o = {a: 1, b: 2}; delete o.a; JSON.stringify(o)"), "{\"b\":2}");
    assert_eq!(run_str("const o = {a: 1}; delete o.a"), "true");
    // deleting an absent key still returns true
    assert_eq!(run_str("const o = {a: 1}; delete o.zzz"), "true");
    // bracket form
    assert_eq!(run_str("const o = {x: 1, y: 2}; delete o['x']; JSON.stringify(o)"), "{\"y\":2}");
}

#[test]
fn delete_nested_and_effect_on_keys() {
    assert_eq!(
        run_str("const o = {a: {b: 1, c: 2}}; delete o.a.b; JSON.stringify(o)"),
        "{\"a\":{\"c\":2}}"
    );
    assert_eq!(
        run_str("const o = {a: 1, b: 2, c: 3}; delete o.b; JSON.stringify(Object.keys(o))"),
        "[\"a\",\"c\"]"
    );
    // delete then re-add appends at the end
    assert_eq!(
        run_str("const o = {a: 1, b: 2}; delete o.a; o.a = 9; JSON.stringify(Object.keys(o))"),
        "[\"b\",\"a\"]"
    );
}

#[test]
fn delete_numeric_key() {
    assert_eq!(
        run_str("const o = {1: 'a', 2: 'b'}; delete o[1]; JSON.stringify(o)"),
        "{\"2\":\"b\"}"
    );
    assert_eq!(run_str("const o = {1: 'a'}; delete o['1']; '1' in o"), "false");
}

// ============================================================================
// 6. Spread / merge
// ============================================================================

#[test]
fn spread_basic_merge() {
    assert_eq!(run_str("JSON.stringify({...{a: 1}, ...{b: 2}})"), "{\"a\":1,\"b\":2}");
    assert_eq!(run_str("JSON.stringify({...{a: 1, b: 2}})"), "{\"a\":1,\"b\":2}");
    assert_eq!(
        run_str("const base = {a: 1, b: 2}; JSON.stringify({...base, c: 3})"),
        "{\"a\":1,\"b\":2,\"c\":3}"
    );
}

#[test]
fn spread_override_order_last_wins() {
    // later occurrence wins
    assert_eq!(run_str("JSON.stringify({...{a: 1, b: 1}, b: 2})"), "{\"a\":1,\"b\":2}");
    // explicit key before a spread is overridden by the spread
    assert_eq!(run_str("JSON.stringify({a: 9, ...{a: 1, b: 2}})"), "{\"a\":1,\"b\":2}");
    // interleaved: spread's `a` overrides earlier, later literal keeps its slot
    assert_eq!(
        run_str("JSON.stringify({a: 1, ...{a: 2, c: 3}, b: 4})"),
        "{\"a\":2,\"c\":3,\"b\":4}"
    );
    // first occurrence fixes key position even when later value overrides
    assert_eq!(
        run_str("JSON.stringify({a: 1, b: 2, ...{a: 99}})"),
        "{\"a\":99,\"b\":2}"
    );
}

#[test]
fn spread_with_nullish_sources_is_ignored() {
    assert_eq!(run_str("JSON.stringify({...null, a: 1})"), "{\"a\":1}");
    assert_eq!(run_str("JSON.stringify({...undefined, a: 1})"), "{\"a\":1}");
    assert_eq!(run_str("JSON.stringify({a: 1, ...null, ...undefined})"), "{\"a\":1}");
}

#[test]
fn spread_of_string_or_array_into_object() {
    // DIVERGENCE: spreading a string or array into an object yields no index
    // keys here (real JS: {0:..,1:..}). Asserted against zapcode's behavior.
    assert_eq!(run_str("JSON.stringify({...'ab'})"), "{}"); // real JS: {"0":"a","1":"b"}
    assert_eq!(run_str("JSON.stringify({...[10, 20]})"), "{}"); // real JS: {"0":10,"1":20}
}

// ============================================================================
// 7. Object.keys / values / entries
// ============================================================================

#[test]
fn object_keys_insertion_order() {
    assert_eq!(run_str("JSON.stringify(Object.keys({z: 1, a: 2, m: 3}))"), "[\"z\",\"a\",\"m\"]");
    assert_eq!(run_str("JSON.stringify(Object.keys({}))"), "[]");
    assert_eq!(run_str("JSON.stringify(Object.keys({only: 1}))"), "[\"only\"]");
}

#[test]
fn object_values_and_entries() {
    assert_eq!(run_str("JSON.stringify(Object.values({a: 1, b: 2}))"), "[1,2]");
    assert_eq!(run_str("JSON.stringify(Object.values({}))"), "[]");
    assert_eq!(
        run_str("JSON.stringify(Object.entries({a: 1, b: 2}))"),
        "[[\"a\",1],[\"b\",2]]"
    );
    // entries iterate in insertion order
    assert_eq!(
        run_str("JSON.stringify(Object.entries({c: 1, a: 2, b: 3}))"),
        "[[\"c\",1],[\"a\",2],[\"b\",3]]"
    );
}

#[test]
fn object_entries_destructuring_loop() {
    assert_eq!(
        run_str("let s = ''; for (const [k, v] of Object.entries({a: 1, b: 2})) s += k + v; s"),
        "a1b2"
    );
    assert_eq!(
        run_str("let t = 0; for (const [, v] of Object.entries({a: 10, b: 20})) t += v; t"),
        "30"
    );
}

#[test]
fn object_keys_values_entries_on_array() {
    assert_eq!(run_str("JSON.stringify(Object.keys([10, 20, 30]))"), "[\"0\",\"1\",\"2\"]");
    assert_eq!(run_str("JSON.stringify(Object.values([10, 20]))"), "[10,20]");
    assert_eq!(
        run_str("JSON.stringify(Object.entries(['x', 'y']))"),
        "[[\"0\",\"x\"],[\"1\",\"y\"]]"
    );
}

#[test]
fn object_keys_on_string() {
    // DIVERGENCE: Object.keys of a string yields [] here (real JS: ["0","1"]).
    assert_eq!(run_str("JSON.stringify(Object.keys('ab'))"), "[]");
}

// ============================================================================
// 8. Object.assign
// ============================================================================

#[test]
fn object_assign_merge_and_override() {
    assert_eq!(run_str("JSON.stringify(Object.assign({}, {a: 1}))"), "{\"a\":1}");
    assert_eq!(run_str("JSON.stringify(Object.assign({}, {a: 1}, {b: 2}))"), "{\"a\":1,\"b\":2}");
    // later source wins
    assert_eq!(
        run_str("JSON.stringify(Object.assign({a: 1}, {a: 9, b: 2}))"),
        "{\"a\":9,\"b\":2}"
    );
    assert_eq!(
        run_str("JSON.stringify(Object.assign({}, {a: 1}, {a: 2, b: 3}, {c: 4}))"),
        "{\"a\":2,\"b\":3,\"c\":4}"
    );
}

#[test]
fn object_assign_mutates_and_returns_target() {
    // returns the (same) target reference
    assert_eq!(
        run_str("const t = {a: 1}; const r = Object.assign(t, {b: 2}); t === r"),
        "true"
    );
    // target is mutated in place
    assert_eq!(
        run_str("const t = {a: 1}; Object.assign(t, {b: 2}); JSON.stringify(t)"),
        "{\"a\":1,\"b\":2}"
    );
    // an alias sees the mutation
    assert_eq!(
        run_str("const t = {a: 1}; const alias = t; Object.assign(t, {z: 9}); JSON.stringify(alias)"),
        "{\"a\":1,\"z\":9}"
    );
}

#[test]
fn object_assign_skips_nullish_sources() {
    assert_eq!(
        run_str("JSON.stringify(Object.assign({a: 1}, null, undefined, {b: 2}))"),
        "{\"a\":1,\"b\":2}"
    );
    assert_eq!(run_str("JSON.stringify(Object.assign({}, null))"), "{}");
}

// ============================================================================
// 9. Object.fromEntries
// ============================================================================

#[test]
fn object_from_entries() {
    assert_eq!(
        run_str("JSON.stringify(Object.fromEntries([['a', 1], ['b', 2]]))"),
        "{\"a\":1,\"b\":2}"
    );
    assert_eq!(run_str("JSON.stringify(Object.fromEntries([]))"), "{}");
    // round-trips with entries, preserving order
    assert_eq!(
        run_str("const o = {a: 1, b: 2, c: 3}; JSON.stringify(Object.fromEntries(Object.entries(o)))"),
        "{\"a\":1,\"b\":2,\"c\":3}"
    );
    // later pair with same key wins
    assert_eq!(
        run_str("JSON.stringify(Object.fromEntries([['a', 1], ['a', 2]]))"),
        "{\"a\":2}"
    );
}

#[test]
fn object_from_entries_from_spread_map() {
    // a Map must be spread into an array of pairs first
    assert_eq!(
        run_str("JSON.stringify(Object.fromEntries([...new Map([['x', 1]])]))"),
        "{\"x\":1}"
    );
    assert_eq!(
        run_str("JSON.stringify(Object.fromEntries([...new Map([['a', 1], ['b', 2]])]))"),
        "{\"a\":1,\"b\":2}"
    );
}

// ============================================================================
// 10. Object.hasOwn / hasOwnProperty
// ============================================================================

#[test]
fn object_has_own() {
    assert_eq!(run_str("Object.hasOwn({a: 1}, 'a')"), "true");
    assert_eq!(run_str("Object.hasOwn({a: 1}, 'b')"), "false");
    assert_eq!(run_str("Object.hasOwn({a: undefined}, 'a')"), "true"); // present though undefined
    // numeric key, looked up by number or its string form
    assert_eq!(run_str("Object.hasOwn({1: 'a'}, 1)"), "true");
    assert_eq!(run_str("Object.hasOwn({1: 'a'}, '1')"), "true");
}

#[test]
fn has_own_property_method() {
    assert_eq!(run_str("({a: 1}).hasOwnProperty('a')"), "true");
    assert_eq!(run_str("({a: 1}).hasOwnProperty('b')"), "false");
    assert_eq!(run_str("const o = {x: 1}; o.hasOwnProperty('x')"), "true");
    // not inherited names
    assert_eq!(run_str("({}).hasOwnProperty('toString')"), "false");
}

#[test]
fn has_own_after_delete() {
    assert_eq!(run_str("const o = {a: 1}; delete o.a; Object.hasOwn(o, 'a')"), "false");
}

// ============================================================================
// 11. Object.freeze (enforcing)
// ============================================================================

#[test]
fn freeze_returns_same_object() {
    assert_eq!(run_str("const o = {a: 1}; Object.freeze(o) === o"), "true");
}

#[test]
fn freeze_prevents_write() {
    // A frozen object silently ignores property writes (sloppy mode).
    assert_eq!(run_str("const o = Object.freeze({a: 1}); o.a = 9; o.a"), "1");
}

#[test]
fn freeze_prevents_add_or_delete() {
    // Adding a new property and deleting an existing one are both ignored.
    assert_eq!(
        run_str("const o = Object.freeze({a: 1}); o.b = 2; JSON.stringify(o)"),
        "{\"a\":1}"
    );
    assert_eq!(
        run_str("const o = Object.freeze({a: 1, b: 2}); delete o.a; JSON.stringify(o)"),
        "{\"a\":1,\"b\":2}"
    );
    // Freezing the outer object is shallow: nested objects stay mutable.
    assert_eq!(
        run_str("const o = Object.freeze({a: {b: 1}}); o.a.b = 9; o.a.b"),
        "9"
    );
}

#[test]
fn is_frozen_reports_frozen_state() {
    assert_eq!(run_str("const o = {a: 1}; String(Object.isFrozen(o))"), "false");
    assert_eq!(
        run_str("const o = Object.freeze({a: 1}); String(Object.isFrozen(o))"),
        "true"
    );
}

// ============================================================================
// 12. Key insertion order & numeric vs string keys
// ============================================================================

#[test]
fn string_key_insertion_order_is_preserved() {
    assert_eq!(
        run_str("const o = {}; o.c = 1; o.a = 2; o.b = 3; JSON.stringify(Object.keys(o))"),
        "[\"c\",\"a\",\"b\"]"
    );
    assert_eq!(
        run_str("JSON.stringify(Object.keys({zebra: 1, apple: 2, mango: 3}))"),
        "[\"zebra\",\"apple\",\"mango\"]"
    );
}

#[test]
fn numeric_keys_kept_in_insertion_order() {
    // DIVERGENCE: real JS reorders integer-like keys ascending; zapcode keeps a
    // single insertion-ordered list. Asserted against zapcode's behavior.
    assert_eq!(run_str("JSON.stringify(Object.keys({3: 1, 1: 2, 2: 3}))"), "[\"3\",\"1\",\"2\"]"); // real JS: ["1","2","3"]
    assert_eq!(
        run_str("const o = {}; o[3] = 'a'; o[1] = 'b'; o[2] = 'c'; JSON.stringify(Object.keys(o))"),
        "[\"3\",\"1\",\"2\"]" // real JS: ["1","2","3"]
    );
    assert_eq!(
        run_str("JSON.stringify(Object.keys({100: 'a', 2: 'b', 30: 'c'}))"),
        "[\"100\",\"2\",\"30\"]" // real JS: ["2","30","100"]
    );
    // values follow the same insertion order
    assert_eq!(
        run_str("JSON.stringify(Object.values({100: 'a', 2: 'b', 30: 'c'}))"),
        "[\"a\",\"b\",\"c\"]" // real JS: ["b","c","a"]
    );
}

#[test]
fn mixed_numeric_and_string_keys_kept_in_insertion_order() {
    // DIVERGENCE: real JS hoists integer keys ahead of string keys; zapcode
    // preserves the literal insertion order.
    assert_eq!(
        run_str("JSON.stringify(Object.keys({foo: 1, 2: 2, bar: 3, 1: 4}))"),
        "[\"foo\",\"2\",\"bar\",\"1\"]" // real JS: ["1","2","foo","bar"]
    );
}

#[test]
fn numeric_key_coercion_and_access() {
    // numeric literal key becomes its string form
    assert_eq!(run_str("JSON.stringify(Object.keys({1: 'a'}))"), "[\"1\"]");
    // number and its string form address the same slot
    assert_eq!(run_str("const o = {}; o[1] = 'a'; o['1'] === 'a'"), "true");
    assert_eq!(run_str("const o = {1: 'x'}; o['1'] === o[1]"), "true");
    // float key keeps its source string form
    assert_eq!(run_str("const o = {}; o[1.5] = 'a'; JSON.stringify(Object.keys(o))"), "[\"1.5\"]");
    // negative key is a string key
    assert_eq!(run_str("JSON.stringify(Object.keys({[-1]: 'a', 0: 'b'}))"), "[\"-1\",\"0\"]");
}

// ============================================================================
// 13. Cross-cutting integration
// ============================================================================

#[test]
fn keys_values_entries_consistent_lengths() {
    assert_eq!(
        run_str("const o = {a: 1, b: 2, c: 3}; `${Object.keys(o).length}:${Object.values(o).length}:${Object.entries(o).length}`"),
        "3:3:3"
    );
}

#[test]
fn build_object_dynamically_then_reflect() {
    assert_eq!(
        run_str(
            "const o = {}; for (let i = 0; i < 3; i++) o['k' + i] = i * 10; \
             JSON.stringify(Object.entries(o))"
        ),
        "[[\"k0\",0],[\"k1\",10],[\"k2\",20]]"
    );
}

#[test]
fn merge_then_query_membership() {
    assert_eq!(
        run_str(
            "const merged = {...{a: 1, b: 2}, ...{b: 3, c: 4}}; \
             `${'a' in merged}:${merged.b}:${'c' in merged}:${'z' in merged}`"
        ),
        "true:3:true:false"
    );
}

#[test]
fn from_entries_assign_roundtrip() {
    assert_eq!(
        run_str(
            "const a = {x: 1, y: 2}; \
             const b = Object.fromEntries(Object.entries(a).map(([k, v]) => [k, v * 2])); \
             JSON.stringify(Object.assign({}, a, b))"
        ),
        "{\"x\":2,\"y\":4}"
    );
}

#[test]
fn delete_all_keys_leaves_empty() {
    assert_eq!(
        run_str("const o = {a: 1, b: 2}; for (const k of Object.keys(o)) delete o[k]; JSON.stringify(o)"),
        "{}"
    );
}
