//! Conformance breadth: the full `Map` and `Set` surface.
//!
//! Construction (no-arg / from entries / from array / from a string / from
//! another Map-or-Set / from a Set of pairs); the mutator+query core
//! (`get`/`set`/`add`/`has`/`delete`/`clear`/`size`); SameValueZero key/element
//! semantics (NaN collapses to one slot, `0`/`-0` unify, no `1` vs `"1"`
//! coercion); object/array *identity* keys; insertion-order iteration with
//! `keys`/`values`/`entries`/`forEach` and direct `for…of`/spread; the
//! collection-returning chaining contract (`set`/`add` return the *same*
//! collection handle so a chain mutates the original); and structural
//! independence of a copy-constructed collection.
//!
//! All expected values were checked against Node. A handful of zapcode
//! divergences from real JS are deliberately NOT asserted as the real-JS answer
//! (they are documented residuals / known gaps):
//!   * a bare `Map`/`Set` value string-coerces to `"[object Object]"` (not
//!     `"[object Map]"` / `"[object Set]"`), so this suite never stringifies a
//!     collection directly — it always spreads / JSON-stringifies / reads `.size`;
//!   * `Set.prototype.entries()` throws here (only `keys()`/`values()` are
//!     implemented for Set), whereas Map exposes all three — Set's `entries`
//!     is exercised only via its actual (throwing) shape is left untested;
//!   * a *function* used as a key/element is not retained (identity keying is
//!     wired for plain objects/arrays only), and `Object.fromEntries(map)`
//!     yields `{}` here — neither is asserted against the real-JS answer.
//! Everything asserted below matches Node exactly.

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
// Map — construction
// ============================================================================

#[test]
fn map_construct_empty() {
    assert_eq!(run_str("new Map().size"), "0");
    assert_eq!(run_str("typeof new Map()"), "object");
    assert_eq!(run_str("new Map() instanceof Map"), "true");
    // No-arg and nullish-arg construction both yield an empty map.
    assert_eq!(run_str("new Map(null).size"), "0");
    assert_eq!(run_str("new Map(undefined).size"), "0");
}

#[test]
fn map_construct_from_entries_array() {
    assert_eq!(run_str("new Map([['a', 1], ['b', 2]]).size"), "2");
    assert_eq!(run_str("const m = new Map([['a', 1], ['b', 2]]); `${m.get('a')},${m.get('b')}`"), "1,2");
    // A single entry.
    assert_eq!(run_str("new Map([['only', 42]]).get('only')"), "42");
    // Numeric-keyed entries.
    assert_eq!(run_str("const m = new Map([[1, 'a'], [2, 'b']]); m.get(2)"), "b");
    // Mixed value types survive construction.
    assert_eq!(
        run_str("const m = new Map([['n', 1], ['s', 'x'], ['b', true], ['z', null]]); `${m.get('n')},${m.get('s')},${m.get('b')},${String(m.get('z'))}`"),
        "1,x,true,null"
    );
}

#[test]
fn map_construct_duplicate_keys_last_wins() {
    // Duplicate keys in the initializer collapse to one slot, last value wins.
    assert_eq!(run_str("new Map([['a', 1], ['b', 2], ['a', 3]]).size"), "2");
    assert_eq!(run_str("new Map([['a', 1], ['a', 3]]).get('a')"), "3");
    // The first occurrence still fixes the iteration position.
    assert_eq!(run_str("const m = new Map([['a', 1], ['b', 2], ['a', 9]]); [...m.keys()].join(',')"), "a,b");
}

#[test]
fn map_construct_from_another_map_copy() {
    assert_eq!(run_str("const a = new Map([['x', 1]]); new Map(a).get('x')"), "1");
    // Full entry set is copied.
    assert_eq!(
        run_str("const a = new Map([['a', 1], ['b', 2]]); const b = new Map(a); [...b.entries()].map(e => e[0] + e[1]).join(',')"),
        "a1,b2"
    );
    // The copy is structurally independent of the source.
    assert_eq!(
        run_str("const a = new Map([['x', 1]]); const b = new Map(a); b.set('x', 9); `${a.get('x')},${b.get('x')}`"),
        "1,9"
    );
    assert_eq!(
        run_str("const a = new Map([['x', 1]]); const b = new Map(a); b.set('y', 2); `${a.size},${b.size}`"),
        "1,2"
    );
}

#[test]
fn map_construct_from_set_of_pairs() {
    // A Set whose elements are [k, v] pairs builds an equivalent Map.
    assert_eq!(
        run_str("const s = new Set([['a', 1], ['b', 2]]); const m = new Map(s); [...m.entries()].map(e => e[0] + e[1]).join(',')"),
        "a1,b2"
    );
    assert_eq!(run_str("const s = new Set([['k', 9]]); new Map(s).get('k')"), "9");
}

// ============================================================================
// Map — get / set / has / delete / clear / size
// ============================================================================

#[test]
fn map_set_get_basic() {
    assert_eq!(run_str("const m = new Map(); m.set('a', 1); m.get('a')"), "1");
    assert_eq!(run_str("const m = new Map(); m.set('a', 1); m.set('b', 2); `${m.get('a')},${m.get('b')},${m.size}`"), "1,2,2");
    // Missing key reads `undefined`.
    assert_eq!(run_str("String(new Map().get('missing'))"), "undefined");
    // Updating an existing key overwrites without growing size.
    assert_eq!(run_str("const m = new Map([['k', 1]]); m.set('k', 9); `${m.get('k')},${m.size}`"), "9,1");
    // Empty-string and falsy-string keys are ordinary keys.
    assert_eq!(run_str("const m = new Map(); m.set('', 'empty'); m.get('')"), "empty");
}

#[test]
fn map_has() {
    assert_eq!(run_str("const m = new Map([['x', 1]]); `${m.has('x')},${m.has('y')}`"), "true,false");
    // A key whose value is undefined is still "has"-present.
    assert_eq!(run_str("const m = new Map(); m.set('u', undefined); String(m.has('u'))"), "true");
    assert_eq!(run_str("String(new Map().has('nope'))"), "false");
}

#[test]
fn map_delete() {
    assert_eq!(run_str("const m = new Map([['x', 1]]); const d = m.delete('x'); `${d},${m.size}`"), "true,0");
    // Deleting an absent key returns false and leaves size unchanged.
    assert_eq!(run_str("const m = new Map([['x', 1]]); `${m.delete('y')},${m.size}`"), "false,1");
    assert_eq!(run_str("String(new Map().delete('x'))"), "false");
    // has() reflects the deletion.
    assert_eq!(run_str("const m = new Map([['a', 1]]); m.delete('a'); String(m.has('a'))"), "false");
}

#[test]
fn map_clear() {
    assert_eq!(run_str("const m = new Map([['a', 1], ['b', 2]]); m.clear(); m.size"), "0");
    // clear() returns undefined (per spec).
    assert_eq!(run_str("String(new Map([['a', 1]]).clear())"), "undefined");
    // After clear, has() is false and the map is reusable.
    assert_eq!(run_str("const m = new Map([['a', 1]]); m.clear(); String(m.has('a'))"), "false");
    assert_eq!(run_str("const m = new Map([['a', 1]]); m.clear(); m.set('z', 9); `${m.size},${m.get('z')}`"), "1,9");
}

#[test]
fn map_size_tracks_mutations() {
    assert_eq!(run_str("const m = new Map(); m.set('a', 1); m.set('b', 2); m.set('c', 3); m.size"), "3");
    assert_eq!(run_str("const m = new Map(); m.set('a', 1); m.set('a', 2); m.size"), "1"); // dup key
    assert_eq!(run_str("const m = new Map([['a', 1], ['b', 2]]); m.delete('a'); m.size"), "1");
}

// ============================================================================
// Map — chaining returns the collection
// ============================================================================

#[test]
fn map_set_returns_map_for_chaining() {
    assert_eq!(run_str("const m = new Map(); m.set('a', 1).set('b', 2).set('c', 3); m.size"), "3");
    // The chain mutates the original map, and values are readable in order.
    assert_eq!(run_str("const m = new Map(); m.set('a', 1).set('b', 2).set('c', 3); [...m.values()].join(',')"), "1,2,3");
    // set() returns the *same* collection (identity).
    assert_eq!(run_str("const m = new Map(); String(m.set(1, 2) === m)"), "true");
    // The returned handle is the original: mutating it grows the original.
    assert_eq!(run_str("const m = new Map(); const r = m.set('a', 1); r.set('b', 2); m.size"), "2");
}

// ============================================================================
// Map — identity & SameValueZero keys
// ============================================================================

#[test]
fn map_object_identity_keys() {
    // Same object reference is the same key.
    assert_eq!(run_str("const k = {}; const m = new Map(); m.set(k, 9); m.get(k)"), "9");
    assert_eq!(run_str("const k = { id: 1 }; const m = new Map(); m.set(k, 'v'); String(m.has(k))"), "true");
    // Distinct object literals are distinct keys (structural equality is NOT used).
    assert_eq!(run_str("const m = new Map(); m.set({}, 1); String(m.get({}))"), "undefined");
    assert_eq!(run_str("const a = { id: 1 }; const b = { id: 1 }; const m = new Map(); m.set(a, 'x'); String(m.get(b))"), "undefined");
    // Arrays are identity keys too.
    assert_eq!(run_str("const k = [1, 2]; const m = new Map(); m.set(k, 'x'); m.get(k)"), "x");
}

#[test]
fn map_primitive_key_types_distinct() {
    // Number and the string of that number are different keys (no coercion).
    assert_eq!(run_str("const m = new Map(); m.set(1, 'n'); m.set('1', 's'); `${m.get(1)},${m.get('1')}`"), "n,s");
    assert_eq!(run_str("const m = new Map([[1, 'a']]); String(m.has('1'))"), "false");
    // Booleans, null, and undefined are usable distinct keys.
    assert_eq!(run_str("const m = new Map(); m.set(true, 't'); m.set(false, 'f'); m.get(true) + m.get(false)"), "tf");
    assert_eq!(run_str("const m = new Map(); m.set(null, 7); m.get(null)"), "7");
    assert_eq!(run_str("const m = new Map(); m.set(undefined, 5); m.get(undefined)"), "5");
}

#[test]
fn map_nan_is_one_key_same_value_zero() {
    // NaN is a usable key and collapses to a single slot (SameValueZero).
    assert_eq!(run_str("const m = new Map(); m.set(NaN, 1); m.get(NaN)"), "1");
    assert_eq!(run_str("const m = new Map(); m.set(NaN, 1); m.set(NaN, 2); `${m.size},${m.get(NaN)}`"), "1,2");
    assert_eq!(run_str("const m = new Map(); m.set(NaN, 9); String(m.has(NaN))"), "true");
}

#[test]
fn map_zero_and_negative_zero_unify() {
    // 0 and -0 are the same key (SameValueZero), and either retrieves the value.
    assert_eq!(run_str("const m = new Map(); m.set(-0, 'z'); m.get(0)"), "z");
    assert_eq!(run_str("const m = new Map(); m.set(0, 'a'); m.set(-0, 'b'); `${m.size},${m.get(0)}`"), "1,b");
}

// ============================================================================
// Map — iteration order, keys/values/entries, forEach
// ============================================================================

#[test]
fn map_iteration_insertion_order() {
    assert_eq!(run_str("let s = ''; for (const [k, v] of new Map([['a', 1], ['b', 2]])) s += k + v; s"), "a1b2");
    assert_eq!(run_str("JSON.stringify([...new Map([['a', 1], ['b', 2]]).keys()])"), "[\"a\",\"b\"]");
    assert_eq!(run_str("JSON.stringify([...new Map([['a', 1], ['b', 2]]).values()])"), "[1,2]");
    assert_eq!(run_str("JSON.stringify([...new Map([['a', 1], ['b', 2]]).entries()])"), "[[\"a\",1],[\"b\",2]]");
    // Direct spread of the map yields entry pairs.
    assert_eq!(run_str("JSON.stringify([...new Map([['a', 1], ['b', 2]])])"), "[[\"a\",1],[\"b\",2]]");
}

#[test]
fn map_iteration_distinct_key_types_order() {
    // Distinct keys of different primitive types preserve insertion order.
    assert_eq!(
        run_str("const m = new Map(); m.set(1, 'a'); m.set('1', 'b'); m.set(true, 'c'); m.set(null, 'd'); [...m.keys()].map(k => `${k}`).join(',')"),
        "1,1,true,null"
    );
}

#[test]
fn map_update_keeps_position() {
    // Re-setting an existing key keeps its original iteration position.
    assert_eq!(run_str("const m = new Map([['a', 1], ['b', 2]]); m.set('a', 9); [...m.keys()].join(',')"), "a,b");
    // Delete + re-insert moves the key to the end.
    assert_eq!(run_str("const m = new Map([['a', 1], ['b', 2], ['c', 3]]); m.delete('a'); m.set('a', 9); [...m.keys()].join(',')"), "b,c,a");
}

#[test]
fn map_keys_values_entries_are_iterators() {
    // The three views are independently iterable with for…of.
    assert_eq!(run_str("let r = ''; for (const k of new Map([['a', 1], ['b', 2]]).keys()) r += k; r"), "ab");
    assert_eq!(run_str("let r = 0; for (const v of new Map([['a', 1], ['b', 2]]).values()) r += v; r"), "3");
    assert_eq!(run_str("let r = ''; for (const [k, v] of new Map([['a', 1], ['b', 2]]).entries()) r += k + v; r"), "a1b2");
}

#[test]
fn map_for_each() {
    // forEach receives (value, key, map) and visits in insertion order.
    assert_eq!(run_str("let s = 0; new Map([['a', 1], ['b', 2]]).forEach(v => s += v); s"), "3");
    assert_eq!(run_str("let s = ''; new Map([['a', 1], ['b', 2]]).forEach((v, k) => s += k + v); s"), "a1b2");
    // The third argument is the map itself.
    assert_eq!(run_str("let same = false; const m = new Map([['a', 1]]); m.forEach((v, k, mm) => { same = (mm === m); }); String(same)"), "true");
}

#[test]
fn map_array_from_and_object_values() {
    // Array.from over a map yields entry pairs.
    assert_eq!(run_str("JSON.stringify(Array.from(new Map([['a', 1]])))"), "[[\"a\",1]]");
    // Object/array values are preserved as references through get().
    assert_eq!(run_str("const m = new Map(); m.set('k', [1, 2, 3]); m.get('k').join('-')"), "1-2-3");
    assert_eq!(run_str("JSON.stringify([...new Map([['a', { x: 1 }]]).values()])"), "[{\"x\":1}]");
}

#[test]
fn map_empty_iteration() {
    assert_eq!(run_str("let r = 'x'; for (const k of new Map().keys()) r += k; r"), "x");
    assert_eq!(run_str("JSON.stringify([...new Map()])"), "[]");
    assert_eq!(run_str("let n = 0; new Map().forEach(() => n++); n"), "0");
}

// ============================================================================
// Set — construction
// ============================================================================

#[test]
fn set_construct_empty() {
    assert_eq!(run_str("new Set().size"), "0");
    assert_eq!(run_str("typeof new Set()"), "object");
    assert_eq!(run_str("new Set() instanceof Set"), "true");
    assert_eq!(run_str("new Set(null).size"), "0");
    assert_eq!(run_str("new Set(undefined).size"), "0");
}

#[test]
fn set_construct_from_array_dedup() {
    assert_eq!(run_str("new Set([1, 2, 3]).size"), "3");
    assert_eq!(run_str("new Set([1, 2, 2, 3]).size"), "3"); // dedup
    // First occurrence fixes the iteration position; later dups are dropped.
    assert_eq!(run_str("[...new Set([1, 2, 1, 3, 2])].join(',')"), "1,2,3");
    assert_eq!(run_str("[...new Set([5, 3, 5, 1, 3])].join(',')"), "5,3,1");
    // Mixed primitive element types coexist.
    assert_eq!(run_str("new Set([1, '1', true, null, undefined]).size"), "5");
}

#[test]
fn set_construct_from_string_iterates_chars() {
    assert_eq!(run_str("new Set('aab').size"), "2");
    assert_eq!(run_str("[...new Set('aab')].join('')"), "ab");
    assert_eq!(run_str("[...new Set('hello')].join('')"), "helo");
    assert_eq!(run_str("new Set('').size"), "0");
}

#[test]
fn set_construct_from_another_collection() {
    // Copy of a Set.
    assert_eq!(run_str("[...new Set(new Set([1, 2, 3]))].join(',')"), "1,2,3");
    // The copy is independent of its source.
    assert_eq!(run_str("const a = new Set([1, 2]); const b = new Set(a); b.add(3); `${a.size},${b.size}`"), "2,3");
    // A Set can also be built from a Map's iterator output (array of pairs).
    assert_eq!(run_str("const m = new Map([['a', 1]]); const s = new Set([...m.keys()]); String(s.has('a'))"), "true");
}

// ============================================================================
// Set — add / has / delete / clear / size
// ============================================================================

#[test]
fn set_add_and_has() {
    assert_eq!(run_str("const s = new Set(); s.add(1); s.add(2); s.size"), "2");
    assert_eq!(run_str("const s = new Set([1, 2]); `${s.has(2)},${s.has(9)}`"), "true,false");
    // Re-adding an existing element is a no-op for size.
    assert_eq!(run_str("const s = new Set(); s.add(1); s.add(1); s.size"), "1");
    assert_eq!(run_str("String(new Set().has('nope'))"), "false");
}

#[test]
fn set_delete() {
    assert_eq!(run_str("const s = new Set([1, 2, 3]); const d = s.delete(2); `${d},${s.size}`"), "true,2");
    // Deleting an absent element returns false.
    assert_eq!(run_str("const s = new Set([1]); `${s.delete(9)},${s.size}`"), "false,1");
    assert_eq!(run_str("String(new Set().delete('x'))"), "false");
    assert_eq!(run_str("const s = new Set([1, 2]); s.delete(1); String(s.has(1))"), "false");
}

#[test]
fn set_clear() {
    assert_eq!(run_str("const s = new Set([1, 2, 3]); s.clear(); s.size"), "0");
    assert_eq!(run_str("const s = new Set([1, 2, 3]); s.clear(); s.add(9); s.size"), "1");
}

#[test]
fn set_size_tracks_mutations() {
    assert_eq!(run_str("const s = new Set(); s.add(1); s.add(2); s.add(3); s.size"), "3");
    assert_eq!(run_str("const s = new Set([1, 2, 3]); s.delete(2); s.size"), "2");
}

// ============================================================================
// Set — chaining returns the collection
// ============================================================================

#[test]
fn set_add_returns_set_for_chaining() {
    assert_eq!(run_str("const s = new Set(); s.add(1).add(2).add(1); s.size"), "2"); // chains + dedups
    assert_eq!(run_str("const s = new Set(); s.add(1).add(2).add(3); [...s].join(',')"), "1,2,3");
    // add() returns the *same* collection (identity), even for an existing element.
    assert_eq!(run_str("const s = new Set(); String(s.add(1) === s)"), "true");
    assert_eq!(run_str("const s = new Set([1]); String(s.add(1) === s)"), "true");
    // The returned handle is the original.
    assert_eq!(run_str("const s = new Set(); const r = s.add(1); r.add(2); s.size"), "2");
}

// ============================================================================
// Set — identity & SameValueZero elements
// ============================================================================

#[test]
fn set_object_identity_elements() {
    // Same object reference is one element.
    assert_eq!(run_str("const o = {}; const s = new Set(); s.add(o); s.add(o); s.size"), "1");
    assert_eq!(run_str("const o = {}; const s = new Set(); s.add(o); String(s.has(o))"), "true");
    // Distinct object literals are distinct elements.
    assert_eq!(run_str("const s = new Set(); s.add({}); String(s.has({}))"), "false");
    assert_eq!(run_str("new Set([{}, {}]).size"), "2");
    // Arrays are identity elements; their structure is preserved.
    assert_eq!(run_str("JSON.stringify([...new Set([[1], [2]])])"), "[[1],[2]]");
}

#[test]
fn set_primitive_elements_distinct() {
    // No coercion: 1 and "1" are distinct elements.
    assert_eq!(run_str("const s = new Set([1]); String(s.has('1'))"), "false");
    assert_eq!(run_str("new Set([1, '1']).size"), "2");
}

#[test]
fn set_nan_is_one_element_same_value_zero() {
    assert_eq!(run_str("new Set([NaN, NaN]).size"), "1");
    assert_eq!(run_str("new Set([NaN, NaN, NaN]).size"), "1");
    assert_eq!(run_str("String(new Set([NaN]).has(NaN))"), "true");
    assert_eq!(run_str("const s = new Set(); s.add(NaN); s.add(NaN); s.size"), "1");
}

#[test]
fn set_zero_and_negative_zero_unify() {
    assert_eq!(run_str("new Set([0, -0]).size"), "1");
    assert_eq!(run_str("new Set([0, -0, 0]).size"), "1");
    assert_eq!(run_str("const s = new Set([-0]); String(s.has(0))"), "true");
}

// ============================================================================
// Set — iteration order, keys/values, forEach, spread
// ============================================================================

#[test]
fn set_iteration_insertion_order() {
    assert_eq!(run_str("let r = []; for (const x of new Set([1, 2, 3])) r.push(x); JSON.stringify(r)"), "[1,2,3]");
    assert_eq!(run_str("JSON.stringify([...new Set([3, 1, 2, 1])])"), "[3,1,2]"); // order preserved, deduped
    assert_eq!(run_str("JSON.stringify([...new Set('aabbc')])"), "[\"a\",\"b\",\"c\"]");
    // Delete + re-add moves an element to the end.
    assert_eq!(run_str("const s = new Set([1, 2, 3]); s.delete(1); s.add(1); [...s].join(',')"), "2,3,1");
}

#[test]
fn set_keys_and_values_are_same_iterator() {
    // For a Set, keys() and values() yield the same elements (and equal contents).
    assert_eq!(run_str("[...new Set([1, 2, 3]).values()].join(',')"), "1,2,3");
    assert_eq!(run_str("let r = 0; for (const v of new Set([1, 2, 3]).values()) r += v; r"), "6");
    assert_eq!(run_str("let r = 0; for (const v of new Set([1, 2, 3]).keys()) r += v; r"), "6");
    assert_eq!(run_str("const s = new Set([1, 2]); JSON.stringify([[...s.keys()], [...s.values()]])"), "[[1,2],[1,2]]");
}

#[test]
fn set_for_each() {
    assert_eq!(run_str("let sum = 0; new Set([1, 2, 3]).forEach(x => sum += x); sum"), "6");
    // forEach visits in insertion order.
    assert_eq!(run_str("let r = ''; new Set([3, 1, 2]).forEach(v => r += v); r"), "312");
    // Set's forEach passes (value, value, set): the first two args are equal.
    assert_eq!(run_str("let r = ''; new Set([1, 2]).forEach((v, v2) => r += v + '=' + v2 + ';'); r"), "1=1;2=2;");
}

#[test]
fn set_spread_and_array_from() {
    assert_eq!(run_str("[...new Set([1, 1, 2, 3, 3, 3])].join(',')"), "1,2,3"); // dedup via round-trip
    assert_eq!(run_str("JSON.stringify(Array.from(new Set([1, 2, 3])))"), "[1,2,3]");
    // Array.from with a map function.
    assert_eq!(run_str("JSON.stringify(Array.from(new Set([1, 2, 3]), x => x * 2))"), "[2,4,6]");
    assert_eq!(run_str("JSON.stringify([...new Set()])"), "[]");
}

#[test]
fn set_empty_iteration() {
    assert_eq!(run_str("let r = 'x'; for (const v of new Set()) r += v; r"), "x");
    assert_eq!(run_str("let n = 0; new Set().forEach(() => n++); n"), "0");
}

// ============================================================================
// Map + Set — interplay & spread
// ============================================================================

#[test]
fn map_spread_into_array_then_transform() {
    // The canonical "[...map] then transform" idiom.
    assert_eq!(
        run_str("const m = new Map([['a', 1], ['b', 2]]); [...m].map(e => e[0] + '=' + e[1]).join(',')"),
        "a=1,b=2"
    );
    // Spread of .entries() pairs.
    assert_eq!(run_str("JSON.stringify([...new Map([['a', 1]]).entries()])"), "[[\"a\",1]]");
}

#[test]
fn set_dedup_array_idiom() {
    assert_eq!(run_str("[...new Set([1, 1, 2, 3, 3])].length"), "3");
    // Build a Set from a Map's values and dedup.
    assert_eq!(run_str("const m = new Map([['a', 1], ['b', 1], ['c', 2]]); new Set([...m.values()]).size"), "2");
}

#[test]
fn map_of_collections_buckets() {
    // Map-of-arrays "grab the bucket once, push into it" relies on reference
    // semantics: the bucket retrieved via get() is the *same* array stored.
    assert_eq!(
        run_str("const m = new Map(); m.set('k', []); const b = m.get('k'); b.push(1); b.push(2); m.get('k').join(',')"),
        "1,2"
    );
    // A Set stored as a map value is mutated through the shared handle.
    assert_eq!(
        run_str("const m = new Map(); m.set('s', new Set()); m.get('s').add(1); m.get('s').add(1); m.get('s').size"),
        "1"
    );
}

#[test]
fn weakmap_and_weakset_basic_operations() {
    // WeakMap: set/get/has/delete with object keys; set is chainable.
    assert_eq!(run_str("(function(){ const w = new WeakMap(); const k = {}; w.set(k, 1); return w.get(k); })()"), "1");
    assert_eq!(run_str("(function(){ const w = new WeakMap(); const k = {}; w.set(k, 5); return w.has(k) + ',' + w.has({}); })()"), "true,false");
    assert_eq!(run_str("(function(){ const w = new WeakMap(); const k = {}; w.set(k, 1); w.delete(k); return w.has(k); })()"), "false");
    assert_eq!(run_str("(function(){ const w = new WeakMap(); const k = {}; return w.set(k, 1) === w; })()"), "true");
    assert_eq!(run_str("(function(){ const w = new WeakMap(); return w.get({}) === undefined; })()"), "true");
    // Constructed from an iterable of [k, v] pairs.
    assert_eq!(run_str("(function(){ const k = {}; const w = new WeakMap([[k, 7]]); return w.get(k); })()"), "7");
    // WeakSet: add/has/delete; add is chainable.
    assert_eq!(run_str("(function(){ const s = new WeakSet(); const k = {}; s.add(k); return s.has(k); })()"), "true");
    assert_eq!(run_str("(function(){ const s = new WeakSet(); const k = {}; s.add(k); s.delete(k); return s.has(k); })()"), "false");
    assert_eq!(run_str("(function(){ const s = new WeakSet(); const k = {}; return s.add(k) === s; })()"), "true");
    // A common memoization pattern works.
    assert_eq!(
        run_str("(function(){ const cache = new WeakMap(); function memo(o){ if (cache.has(o)) return cache.get(o); const v = o.x * 2; cache.set(o, v); return v; } const o = { x: 5 }; return memo(o) + ',' + memo(o); })()"),
        "10,10"
    );
}
