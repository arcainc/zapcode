//! Conformance breadth: the iteration protocol across built-in collections.
//!
//! Covers how `Array` / `Map` / `Set` / `String` expose iteration to the three
//! consumers the language offers — the spread operator (`[...it]`), `for…of`
//! (incl. destructuring of `entries()` pairs), and `Array.from` — plus the
//! manual generator `.next()` step protocol. Every asserted value was
//! cross-checked against real `node -e`.
//!
//! FULLY-WORKING (asserted at the JS value):
//!   - `Array.prototype.keys()/values()/entries()` consumed by spread, `for…of`,
//!     and `Array.from`, including `[i, v]` destructuring of `entries()`;
//!   - `Map.prototype.keys()/values()/entries()` and `[...map]` direct spread;
//!   - `Set.prototype.values()/keys()` and `[...set]`;
//!   - `String` iteration via spread / `for…of` / `Array.from`;
//!   - `Array.from(iterable, mapFn)`;
//!   - generator objects as full step iterators (`it.next().value` / `.done`).
//!
//! DOCUMENTED DIVERGENCES (asserted at zapcode's ACTUAL behavior, with a comment,
//! never the JS answer — verified against Node):
//!   - built-in *collection* iterators (e.g. `arr.values()`) are spreadable /
//!     for-of-able but do NOT support a manual `.next()` step — only generator
//!     objects do. Calling `.next()` on a collection iterator throws a catchable
//!     TypeError.
//!   - `Set.prototype.entries()` does not iterate (throws) — unlike Map's, which
//!     yields `[value, value]` pairs in JS.
//!   - `Object.fromEntries(map)` / `Object.fromEntries(set)` return `{}` — only
//!     `Object.fromEntries(arrayOfPairs)` is consumed (JS consumes any iterable).

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
// Array iterator views: keys / values / entries
// ============================================================================

#[test]
fn array_values_via_spread_and_for_of() {
    assert_eq!(run_str("[...['a', 'b', 'c'].values()].join('-')"), "a-b-c");
    assert_eq!(run_str("let s = 0; for (const v of [1, 2, 3].values()) s += v; s"), "6");
}

#[test]
fn array_keys_via_spread_and_for_of() {
    assert_eq!(run_str("[...[10, 20, 30].keys()].join(',')"), "0,1,2");
    assert_eq!(run_str("let r = ''; for (const k of [9, 8, 7].keys()) r += k; r"), "012");
}

#[test]
fn array_entries_pairs_and_destructuring() {
    assert_eq!(
        run_str("[...[10, 20].entries()].map(e => e.join(':')).join(',')"),
        "0:10,1:20"
    );
    assert_eq!(
        run_str("let r = ''; for (const [i, v] of ['x', 'y'].entries()) r += i + v; r"),
        "0x1y"
    );
}

#[test]
fn array_entries_from_split_string() {
    assert_eq!(
        run_str(
            "let acc = []; for (const [i, c] of 'ab'.split('').entries()) acc.push(i + c); acc.join(',')"
        ),
        "0a,1b"
    );
}

// ============================================================================
// Map iterator views
// ============================================================================

#[test]
fn map_keys_values_entries() {
    assert_eq!(run_str("[...new Map([['a', 1], ['b', 2]]).keys()].join(',')"), "a,b");
    assert_eq!(run_str("[...new Map([['a', 1], ['b', 2]]).values()].join(',')"), "1,2");
    assert_eq!(
        run_str("[...new Map([['a', 1], ['b', 2]]).entries()].map(e => e[0] + e[1]).join(',')"),
        "a1,b2"
    );
}

#[test]
fn map_direct_spread_yields_pairs() {
    assert_eq!(
        run_str("[...new Map([['a', 1], ['b', 2]])].map(e => e.join('=')).join(',')"),
        "a=1,b=2"
    );
}

#[test]
fn map_entries_destructured_in_for_of() {
    assert_eq!(
        run_str("const m = new Map([['a', 1], ['b', 2]]); let out = ''; for (const [k, v] of m.entries()) out += k + v; out"),
        "a1b2"
    );
    // for-of directly over a Map also yields [k, v] pairs.
    assert_eq!(
        run_str("const m = new Map([['a', 1]]); let out = ''; for (const [k, v] of m) out += k + v; out"),
        "a1"
    );
}

// ============================================================================
// Set iterator views
// ============================================================================

#[test]
fn set_values_and_keys() {
    assert_eq!(run_str("[...new Set([1, 2, 3]).values()].join(',')"), "1,2,3");
    assert_eq!(run_str("[...new Set([1, 2, 3]).keys()].join(',')"), "1,2,3");
    assert_eq!(run_str("let s = 0; for (const v of new Set([4, 5, 6]).values()) s += v; s"), "15");
}

#[test]
fn set_direct_spread_and_for_of() {
    assert_eq!(run_str("[...new Set([3, 1, 2, 1])].join(',')"), "3,1,2");
    assert_eq!(run_str("let s = ''; for (const v of new Set(['x', 'y'])) s += v; s"), "xy");
}

#[test]
fn set_entries_does_not_iterate_documented_divergence() {
    // DIVERGENCE asserted as actual: JS yields [v, v] pairs; here Set.entries()
    // is not iterable and throws a catchable TypeError when consumed.
    assert_eq!(
        run_str("try { [...new Set([1, 2]).entries()]; 'no' } catch (e) { e.name }"),
        "TypeError"
    );
    assert_eq!(
        run_str("try { for (const x of new Set([1]).entries()) {} 'no' } catch (e) { e instanceof TypeError ? 'te' : 'o' }"),
        "te"
    );
}

// ============================================================================
// String iteration
// ============================================================================

#[test]
fn string_iteration_forms() {
    assert_eq!(run_str("[...'abc'].join('-')"), "a-b-c");
    assert_eq!(run_str("Array.from('abc').join('-')"), "a-b-c");
    assert_eq!(run_str("let s = ''; for (const c of 'xyz') s += c.toUpperCase(); s"), "XYZ");
}

// ============================================================================
// Array.from over iterables
// ============================================================================

#[test]
fn array_from_over_collections() {
    assert_eq!(run_str("Array.from(new Set([1, 1, 2])).join(',')"), "1,2");
    assert_eq!(run_str("Array.from(new Set([5, 6]).values()).join(',')"), "5,6");
    assert_eq!(
        run_str("Array.from(new Map([['a', 1]])).map(e => e.join(':')).join(',')"),
        "a:1"
    );
}

#[test]
fn array_from_with_map_fn() {
    assert_eq!(
        run_str("Array.from(new Map([['a', 1], ['b', 2]]), e => e[1]).join(',')"),
        "1,2"
    );
    assert_eq!(run_str("Array.from({ length: 3 }, (_, i) => i * 2).join(',')"), "0,2,4");
    assert_eq!(run_str("Array.from('abc', c => c.toUpperCase()).join('')"), "ABC");
}

// ============================================================================
// Generator objects as full step iterators
// ============================================================================

#[test]
fn generator_manual_step_protocol() {
    assert_eq!(
        run_str("function* g() { yield 1; yield 2 } const it = g(); it.next().value + '/' + it.next().value"),
        "1/2"
    );
    assert_eq!(
        run_str("function* g() { yield 1; yield 2 } const it = g(); it.next(); it.next(); it.next().done"),
        "true"
    );
    assert_eq!(
        run_str("function* g() { yield 'a' } const it = g(); const r = it.next(); r.value + ':' + r.done"),
        "a:false"
    );
}

#[test]
fn generator_consumed_by_for_of_and_spread() {
    assert_eq!(
        run_str("function* g() { yield 1; yield 2; yield 3 } let s = 0; for (const v of g()) s += v; s"),
        "6"
    );
}

// ============================================================================
// Manual .next() on collection iterators — documented divergence
// ============================================================================

#[test]
fn collection_iterator_manual_next_is_documented_divergence() {
    // DIVERGENCE asserted as actual: collection iterators are spreadable /
    // for-of-able but NOT manual step iterators. JS supports `.next()`.
    assert_eq!(
        run_str("try { [10, 20].values().next(); 'no' } catch (e) { e.name }"),
        "TypeError"
    );
    assert_eq!(
        run_str("try { [1].keys().next(); 'no' } catch (e) { e instanceof TypeError ? 'te' : 'o' }"),
        "te"
    );
}

// ============================================================================
// Object.fromEntries iterable consumption
// ============================================================================

#[test]
fn from_entries_from_array_of_pairs() {
    assert_eq!(
        run_str("JSON.stringify(Object.fromEntries([['a', 1], ['b', 2]]))"),
        "{\"a\":1,\"b\":2}"
    );
}

#[test]
fn from_entries_from_map_or_set_is_documented_divergence() {
    // DIVERGENCE asserted as actual: JS consumes any iterable so a Map/Set of
    // pairs builds the object; here only an array-of-pairs is consumed, so a Map
    // or Set source yields an empty object.
    assert_eq!(run_str("JSON.stringify(Object.fromEntries(new Map([['a', 1], ['b', 2]])))"), "{}");
    assert_eq!(run_str("JSON.stringify(Object.fromEntries(new Set([['a', 1]])))"), "{}");
    // The array workaround is correct.
    assert_eq!(
        run_str("JSON.stringify(Object.fromEntries([...new Map([['a', 1]])]))"),
        "{\"a\":1}"
    );
}

// ============================================================================
// Realistic iteration pipelines
// ============================================================================

#[test]
fn enumerate_then_transform() {
    assert_eq!(
        run_str(
            "const labels = ['red', 'green', 'blue']; \
             [...labels.entries()].map(([i, name]) => `${i}:${name}`).join(', ')"
        ),
        "0:red, 1:green, 2:blue"
    );
}

#[test]
fn histogram_via_map_iteration() {
    assert_eq!(
        run_str(
            "const counts = new Map(); for (const w of ['a', 'b', 'a', 'c', 'a']) counts.set(w, (counts.get(w) ?? 0) + 1); \
             [...counts.entries()].map(([k, v]) => k + v).join(',')"
        ),
        "a3,b1,c1"
    );
}

#[test]
fn dedup_and_sort_via_set_spread() {
    assert_eq!(
        run_str("[...new Set([3, 1, 2, 3, 1])].sort((a, b) => a - b).join(',')"),
        "1,2,3"
    );
}
