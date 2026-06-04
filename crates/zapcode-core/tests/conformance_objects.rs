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
//! Property enumeration follows ECMA-262 OrdinaryOwnPropertyKeys order
//! (integer-index keys ascending, then string keys in insertion order), and
//! `get x()`/`set x()` are real accessor properties (invoked on read/write,
//! enumerable, getters invoked by JSON/spread/Object.*).
//!
//! DOCUMENTED DIVERGENCE asserted here against zapcode's ACTUAL behavior:
//!   - Object-spread of a string or array spreads no own enumerable string
//!     keys (`{...[1,2]}` and `{...'ab'}` are `{}`); `Object.keys('ab')` is
//!     `[]`. (zapcode does not expose index keys as own-enumerable on those.)
//! `Object.freeze` is enforcing; `Object.isFrozen`, `Object.create`,
//! `Object.getPrototypeOf`, and `Object.getOwnPropertyNames` are implemented.
//! `Object.defineProperty`/`defineProperties`/`getOwnPropertyDescriptor` are
//! implemented (data + accessor descriptors; enumerable/writable honored via
//! per-object `__non_*__` marker lists). `configurable:false` is recorded and
//! reported but its redefine/delete restriction is not enforced.

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
// 2. Getters / setters — real accessor properties (invoked on read/write,
//    enumerable in source order, getters invoked by JSON/spread/Object.*).
// ============================================================================

#[test]
fn object_literal_getter_is_invoked_on_read() {
    // Reading invokes the getter and yields the computed value (not the fn).
    assert_eq!(run_str("const o = { get x() { return 42; } }; o.x"), "42");
    assert_eq!(run_str("const o = { get x() { return 42; } }; typeof o.x"), "number");
    // `this` inside the getter is the object.
    assert_eq!(run_str("const o = { base: 10, get total() { return this.base + 5; } }; o.total"), "15");
}

#[test]
fn object_literal_setter_is_invoked_on_write() {
    assert_eq!(
        run_str("const o = { _x: 0, set x(v) { this._x = v * 2; } }; o.x = 7; o._x"),
        "14"
    );
    // A get/set pair on the same key round-trips through the accessor.
    assert_eq!(
        run_str("const o = { _n: 1, get n() { return this._n; }, set n(v) { this._n = v + 10; } }; o.n = 5; o.n"),
        "15"
    );
}

#[test]
fn accessor_properties_are_enumerable_in_source_order() {
    // The accessor key enumerates in source order, like a data property.
    assert_eq!(run_str("Object.keys({ a: 1, get b() { return 2; }, c: 3 }).join(',')"), "a,b,c");
    assert_eq!(
        run_str("let o = { a: 1, get b() { return 2; } }; let r = []; for (const k in o) r.push(k); r.join(',')"),
        "a,b"
    );
    // A setter-only property is enumerable too.
    assert_eq!(run_str("Object.keys({ a: 1, set x(v) {} }).join(',')"), "a,x");
}

#[test]
fn getter_invoked_by_json_spread_and_object_helpers() {
    // JSON.stringify invokes the getter and serializes its result.
    assert_eq!(run_str("JSON.stringify({ a: 1, get b() { return 2; } })"), "{\"a\":1,\"b\":2}");
    assert_eq!(run_str("JSON.stringify({ get x() { return 5; } })"), "{\"x\":5}");
    // A setter-only property reads as undefined -> omitted from JSON.
    assert_eq!(run_str("JSON.stringify({ a: 1, set x(v) {} })"), "{\"a\":1}");
    // Spread / Object.values / Object.entries / Object.assign invoke the getter.
    assert_eq!(run_str("JSON.stringify({ ...{ a: 1, get b() { return 7; } } })"), "{\"a\":1,\"b\":7}");
    assert_eq!(run_str("JSON.stringify(Object.values({ a: 1, get b() { return 9; } }))"), "[1,9]");
    assert_eq!(
        run_str("JSON.stringify(Object.entries({ a: 1, get b() { return 9; } }))"),
        "[[\"a\",1],[\"b\",9]]"
    );
    assert_eq!(
        run_str("JSON.stringify(Object.assign({}, { a: 1, get b() { return 8; } }))"),
        "{\"a\":1,\"b\":8}"
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
fn numeric_keys_ordered_ascending() {
    // Integer-index keys enumerate in ascending NUMERIC order (not insertion or
    // lexicographic), matching JS.
    assert_eq!(run_str("JSON.stringify(Object.keys({3: 1, 1: 2, 2: 3}))"), "[\"1\",\"2\",\"3\"]");
    assert_eq!(
        run_str("const o = {}; o[3] = 'a'; o[1] = 'b'; o[2] = 'c'; JSON.stringify(Object.keys(o))"),
        "[\"1\",\"2\",\"3\"]"
    );
    assert_eq!(
        run_str("JSON.stringify(Object.keys({100: 'a', 2: 'b', 30: 'c'}))"),
        "[\"2\",\"30\",\"100\"]"
    );
    // values follow the same (reordered) key order: keys [2, 30, 100] -> b, c, a.
    assert_eq!(
        run_str("JSON.stringify(Object.values({100: 'a', 2: 'b', 30: 'c'}))"),
        "[\"b\",\"c\",\"a\"]"
    );
}

#[test]
fn mixed_numeric_and_string_keys_order() {
    // Integer keys are hoisted ahead of string keys (ascending), then string
    // keys follow in insertion order — matching JS.
    assert_eq!(
        run_str("JSON.stringify(Object.keys({foo: 1, 2: 2, bar: 3, 1: 4}))"),
        "[\"1\",\"2\",\"foo\",\"bar\"]"
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
    // negative key is a string key, so the integer key 0 sorts ahead of it.
    assert_eq!(run_str("JSON.stringify(Object.keys({[-1]: 'a', 0: 'b'}))"), "[\"0\",\"-1\"]");
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

// ============================================================================
// User-defined keys that merely start with `__` are ORDINARY own properties.
// The interpreter must only hide its EXACT reserved internal markers, never a
// blanket `__`-prefix; otherwise it silently eats user keys like `__id__`,
// `__typename`, `__v`. Each assertion below matches real Node.
// ============================================================================

#[test]
fn double_underscore_user_keys_are_enumerable() {
    // Object.keys / values / entries keep user `__`-keys (Node-verified).
    assert_eq!(run_str("const o={__id__:5,a:1}; Object.keys(o).join(',')"), "__id__,a");
    assert_eq!(
        run_str("Object.values({__typename:'X',a:1}).join(',')"),
        "X,1"
    );
    assert_eq!(run_str("Object.entries({__t__:9}).length"), "1");
    // getOwnPropertyNames likewise keeps them.
    assert_eq!(
        run_str("Object.getOwnPropertyNames({__id__:1,b:2}).join(',')"),
        "__id__,b"
    );
    // for-in (lowers to Object.keys) enumerates them too.
    assert_eq!(
        run_str("let ks=[]; for (const k in {__id__:1,b:2}) { ks.push(k); } ks.join(',')"),
        "__id__,b"
    );
}

#[test]
fn double_underscore_user_keys_survive_spread() {
    // `{...o}` copies user `__`-keys (Node: `({...{__x__:1}}).__x__` === 1).
    assert_eq!(run_str("String(({...{__x__:1}}).__x__)"), "1");
    assert_eq!(
        run_str("const s={__id__:1,a:2}; String(({...s, c:3}).__id__)"),
        "1"
    );
    assert_eq!(
        run_str("JSON.stringify({...{__id__:7,a:1}})"),
        "{\"__id__\":7,\"a\":1}"
    );
}

#[test]
fn double_underscore_user_keys_reflection_self_consistent() {
    // Object.hasOwn / `in` / get-property all already expose user `__`-keys;
    // keys/values/entries/stringify now agree.
    assert_eq!(run_str("String(Object.hasOwn({__id__:1}, '__id__'))"), "true");
    assert_eq!(run_str("String('__id__' in {__id__:1})"), "true");
    assert_eq!(run_str("String(({__id__:7}).__id__)"), "7");
}

#[test]
fn class_instance_internal_markers_stay_hidden_on_spread() {
    // Spreading a class instance must NOT leak internal brand keys
    // (`__class__`, `__class_chain__`, …) — only real own data props copy.
    assert_eq!(
        run_str(
            "class C { constructor(){ this.a=1; this.b=2; } } JSON.stringify({...new C()})"
        ),
        "{\"a\":1,\"b\":2}"
    );
    assert_eq!(
        run_str("class C { a = 1; b = 2; } Object.keys({...new C()}).join(',')"),
        "a,b"
    );
}

#[test]
fn property_enumeration_order_matches_ecma() {
    // Integer-index keys ascending, then string keys in insertion order — the
    // single OrdinaryOwnPropertyKeys order shared by keys/values/entries,
    // for-in, and JSON.stringify.
    assert_eq!(run_str("Object.keys({2:'a',1:'b',10:'c',z:'d',a:'e'}).join(',')"), "1,2,10,z,a");
    assert_eq!(run_str("JSON.stringify({2:'a',1:'b',z:'c'})"), "{\"1\":\"b\",\"2\":\"a\",\"z\":\"c\"}");
    // Canonical-index edge cases: leading zero, 2^32-1, and negatives are STRING
    // keys (not integer indices), so they sort after and in insertion order.
    assert_eq!(run_str("Object.keys({'01':1, 1:2, '0':3}).join(',')"), "0,1,01");
    assert_eq!(
        run_str("Object.keys({4294967295:'a', 5:'b', 4294967294:'c'}).join(',')"),
        "5,4294967294,4294967295"
    );
    assert_eq!(run_str("Object.keys({'-1':1, 2:2, 1:3}).join(',')"), "1,2,-1");
    // Spread / assign results reorder when read, too.
    assert_eq!(run_str("Object.keys({...{2:'a',1:'b'}}).join(',')"), "1,2");
    assert_eq!(run_str("JSON.stringify(Object.assign({}, {3:'c'}, {1:'a'}))"), "{\"1\":\"a\",\"3\":\"c\"}");
}

#[test]
fn define_property_data_accessor_and_descriptors() {
    // Data descriptor: value installed; enumerable defaults to false (hidden
    // from keys/JSON), writable defaults to false (assignment ignored).
    assert_eq!(run_str("const o = {}; Object.defineProperty(o, 'x', { value: 5 }); o.x"), "5");
    assert_eq!(run_str("const o = {a:1}; Object.defineProperty(o, 'x', { value: 5 }); Object.keys(o).join(',')"), "a");
    assert_eq!(run_str("const o = {}; Object.defineProperty(o, 'x', { value: 5 }); JSON.stringify(o)"), "{}");
    assert_eq!(run_str("const o = {}; Object.defineProperty(o, 'x', { value: 5, writable: false }); o.x = 9; o.x"), "5");
    assert_eq!(run_str("const o = {}; Object.defineProperty(o, 'x', { value: 5, enumerable: true }); Object.keys(o).join(',')"), "x");
    // Accessor descriptor: get/set installed and invoked.
    assert_eq!(
        run_str("const o = {}; let v = 0; Object.defineProperty(o, 'x', { get(){ return 9; }, set(n){ v = n; } }); const r = o.x; o.x = 3; `${r},${v}`"),
        "9,3"
    );
    // Non-enumerable property is not spread or for-in'd; getOwnPropertyNames includes it.
    assert_eq!(run_str("const o = {a:1}; Object.defineProperty(o, 'h', { value: 2 }); JSON.stringify({...o})"), "{\"a\":1}");
    assert_eq!(run_str("const o = {a:1}; Object.defineProperty(o, 'h', { value: 2 }); Object.getOwnPropertyNames(o).sort().join(',')"), "a,h");
    // defineProperties applies several at once; returns the object.
    assert_eq!(
        run_str("const o = {}; Object.defineProperties(o, { a: { value: 1, enumerable: true }, b: { value: 2 } }); `${Object.keys(o).join(',')}|${o.b}`"),
        "a|2"
    );
}

#[test]
fn get_own_property_descriptor() {
    // A plain data property reports all-true attributes.
    assert_eq!(
        run_str("JSON.stringify(Object.getOwnPropertyDescriptor({x:1}, 'x'))"),
        "{\"value\":1,\"writable\":true,\"enumerable\":true,\"configurable\":true}"
    );
    // A defined non-enumerable, non-writable data prop reports its flags.
    assert_eq!(
        run_str("const o={}; Object.defineProperty(o,'x',{value:5}); JSON.stringify(Object.getOwnPropertyDescriptor(o,'x'))"),
        "{\"value\":5,\"writable\":false,\"enumerable\":false,\"configurable\":false}"
    );
    // An accessor descriptor reports get/set (not value/writable).
    assert_eq!(
        run_str("const o={}; Object.defineProperty(o,'x',{get(){return 5},enumerable:true}); const d=Object.getOwnPropertyDescriptor(o,'x'); `${typeof d.get},${d.enumerable},${'value' in d}`"),
        "function,true,false"
    );
    // A missing property -> undefined.
    assert_eq!(run_str("Object.getOwnPropertyDescriptor({a:1}, 'zzz') === undefined"), "true");
    // RESIDUAL: configurable:false is recorded/reported but its redefine-throws
    // restriction is NOT enforced (a second defineProperty succeeds here; JS
    // throws "Cannot redefine property").
}
