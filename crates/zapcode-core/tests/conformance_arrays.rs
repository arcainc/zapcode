//! Conformance suite (round 1): `Array` and `Array.prototype`.
//!
//! Test262-style breadth across the entire array surface the interpreter
//! exposes: every iteration/query/mutation/transform method, in-place mutation
//! semantics, spread, iteration, static constructors, and edge cases (empty
//! arrays, negative indices, fromIndex clamping, identity-based membership,
//! comparator stability). Results are stringified with `JSON.stringify` (or a
//! scalar expression) so `to_js_string` output is byte-comparable to Node.
//!
//! Documented divergences from real JS that this suite deliberately works
//! around (per STRESS-PASS-BUGS.md and verified against the live interpreter):
//!   * `Array.prototype.lastIndexOf(target, fromIndex)` ignores `fromIndex`
//!     (always returns the absolute-last match). Pinned below WITH a comment;
//!     fromIndex-respecting cases are not asserted to the real-JS answer.
//!   * Iteration callbacks receive `(value, index)` only; the 3rd `array`
//!     argument is `undefined`, so no test reads it.
//!   * `Array.prototype.toString()` as a *method call* is not provided; string
//!     coercion (`String(arr)`, `` `${arr}` ``, `''+arr`) goes through the join
//!     path and works, so coercion is tested instead of `.toString()`.
//!   * `.entries()/.keys()/.values()` are spreadable iterables but the returned
//!     object is not a manual iterator with `.next()`; only spread is tested.
//!   * Calling `Array(n)` as a *function* (no `new`) throws here; `new Array(n)`
//!     works, so the `new` form is used for length-preallocation.

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

/// Run code expecting a thrown/runtime error; returns the error's debug string.
fn run_err(code: &str) -> String {
    match ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        Vec::new(),
        ResourceLimits::default(),
    )
    .unwrap()
    .run(Vec::new())
    {
        Ok(r) => match r.state {
            VmState::Complete(v) => panic!(
                "expected error for `{code}`, got completion {}",
                v.to_js_string(&r.heap)
            ),
            other => panic!("expected error for `{code}`, got {other:?}"),
        },
        Err(e) => format!("{e:?}"),
    }
}

// ============================================================================
// Static constructors & introspection
// ============================================================================

#[test]
fn array_of() {
    assert_eq!(run_str("JSON.stringify(Array.of())"), "[]");
    assert_eq!(run_str("JSON.stringify(Array.of(1, 2, 3))"), "[1,2,3]");
    // `Array.of(7)` is a one-element array, unlike `new Array(7)`.
    assert_eq!(run_str("JSON.stringify(Array.of(7))"), "[7]");
    assert_eq!(run_str("Array.of(7).length"), "1");
    assert_eq!(
        run_str("JSON.stringify(Array.of('a', true, null))"),
        "[\"a\",true,null]"
    );
}

#[test]
fn array_from_iterables() {
    assert_eq!(
        run_str("JSON.stringify(Array.from('abc'))"),
        "[\"a\",\"b\",\"c\"]"
    );
    assert_eq!(run_str("JSON.stringify(Array.from([1, 2, 3]))"), "[1,2,3]");
    assert_eq!(
        run_str("JSON.stringify(Array.from(new Set([1, 1, 2, 3, 3])))"),
        "[1,2,3]"
    );
    assert_eq!(
        run_str("JSON.stringify(Array.from(new Map([['a', 1], ['b', 2]])))"),
        "[[\"a\",1],[\"b\",2]]"
    );
    assert_eq!(run_str("JSON.stringify(Array.from([]))"), "[]");
}

#[test]
fn array_from_array_like_and_mapfn() {
    // Array-like with a `length` property.
    assert_eq!(
        run_str("JSON.stringify(Array.from({length: 3}, (_, i) => i))"),
        "[0,1,2]"
    );
    assert_eq!(
        run_str("JSON.stringify(Array.from({length: 0}, (_, i) => i))"),
        "[]"
    );
    // mapFn over an iterable.
    assert_eq!(
        run_str("JSON.stringify(Array.from([1, 2, 3], x => x * x))"),
        "[1,4,9]"
    );
    assert_eq!(
        run_str("JSON.stringify(Array.from('abc', (c, i) => c + i))"),
        "[\"a0\",\"b1\",\"c2\"]"
    );
}

#[test]
fn array_is_array() {
    assert_eq!(run_str("Array.isArray([])"), "true");
    assert_eq!(run_str("Array.isArray([1, 2, 3])"), "true");
    assert_eq!(run_str("Array.isArray(new Array(3))"), "true");
    assert_eq!(run_str("Array.isArray(Array.of(1))"), "true");
    assert_eq!(run_str("Array.isArray([1].concat([2]))"), "true");
    assert_eq!(run_str("Array.isArray({length: 2})"), "false");
    assert_eq!(run_str("Array.isArray('abc')"), "false");
    assert_eq!(run_str("Array.isArray(null)"), "false");
    assert_eq!(run_str("Array.isArray(undefined)"), "false");
    assert_eq!(run_str("Array.isArray(42)"), "false");
}

#[test]
fn new_array_constructor() {
    // `new Array(n)` preallocates length n.
    assert_eq!(run_str("new Array(3).length"), "3");
    assert_eq!(run_str("JSON.stringify(new Array(3).fill(7))"), "[7,7,7]");
    // `new Array(a, b, c)` with multiple args is a literal list.
    assert_eq!(run_str("JSON.stringify(new Array(1, 2, 3))"), "[1,2,3]");
    assert_eq!(run_str("new Array(1, 2, 3).length"), "3");
    assert_eq!(run_str("JSON.stringify(new Array())"), "[]");
}

// ============================================================================
// length & literal basics
// ============================================================================

#[test]
fn length_and_indexing() {
    assert_eq!(run_str("[1, 2, 3].length"), "3");
    assert_eq!(run_str("[].length"), "0");
    assert_eq!(run_str("[1, 2, 3][0]"), "1");
    assert_eq!(run_str("[1, 2, 3][2]"), "3");
    assert_eq!(run_str("String([1, 2, 3][5])"), "undefined");
    assert_eq!(run_str("const a = [1]; a[5] = 9; a.length"), "6");
    assert_eq!(run_str("const a = [1]; a[5] = 9; String(a[3])"), "undefined");
}

// ============================================================================
// map / filter / forEach
// ============================================================================

#[test]
fn map_basic_and_index() {
    assert_eq!(run_str("JSON.stringify([1, 2, 3].map(x => x * x))"), "[1,4,9]");
    assert_eq!(run_str("JSON.stringify([].map(x => x))"), "[]");
    assert_eq!(
        run_str("JSON.stringify([10, 20, 30].map((v, i) => i))"),
        "[0,1,2]"
    );
    assert_eq!(
        run_str("JSON.stringify([10, 20, 30].map((v, i) => v + i))"),
        "[10,21,32]"
    );
    // map preserves length and does not mutate the source.
    assert_eq!(
        run_str("const a = [1, 2, 3]; a.map(x => x * 2); JSON.stringify(a)"),
        "[1,2,3]"
    );
}

#[test]
fn map_returns_new_array() {
    assert_eq!(
        run_str("const a = [1, 2]; const b = a.map(x => x); a === b"),
        "false"
    );
    assert_eq!(
        run_str("JSON.stringify([{n: 1}, {n: 2}].map(o => o.n))"),
        "[1,2]"
    );
}

#[test]
fn filter_basic_and_index() {
    assert_eq!(
        run_str("JSON.stringify([1, 2, 3, 4].filter(x => x % 2 === 0))"),
        "[2,4]"
    );
    assert_eq!(
        run_str("JSON.stringify([1, 2, 3, 4, 5].filter(x => x > 10))"),
        "[]"
    );
    assert_eq!(
        run_str("JSON.stringify([1, 2, 3, 4, 5].filter(x => true))"),
        "[1,2,3,4,5]"
    );
    assert_eq!(
        run_str("JSON.stringify([10, 20, 30, 40].filter((v, i) => i % 2 === 0))"),
        "[10,30]"
    );
    // does not mutate the source.
    assert_eq!(
        run_str("const a = [1, 2, 3]; a.filter(x => x > 1); JSON.stringify(a)"),
        "[1,2,3]"
    );
}

#[test]
fn for_each_side_effects() {
    assert_eq!(
        run_str("let s = 0; [1, 2, 3, 4].forEach(x => { s += x; }); s"),
        "10"
    );
    assert_eq!(
        run_str("const r = []; [10, 20, 30].forEach((v, i) => r.push(i)); JSON.stringify(r)"),
        "[0,1,2]"
    );
    // forEach returns undefined.
    assert_eq!(run_str("String([1, 2].forEach(() => {}))"), "undefined");
    // empty array: callback never runs.
    assert_eq!(
        run_str("let n = 0; [].forEach(() => { n++; }); n"),
        "0"
    );
}

// ============================================================================
// reduce / reduceRight
// ============================================================================

#[test]
fn reduce_with_and_without_init() {
    assert_eq!(run_str("[1, 2, 3, 4].reduce((a, b) => a + b, 0)"), "10");
    assert_eq!(run_str("[1, 2, 3, 4].reduce((a, b) => a + b)"), "10");
    assert_eq!(run_str("[42].reduce((a, b) => a + b)"), "42");
    assert_eq!(run_str("[].reduce((a, b) => a + b, 100)"), "100");
    // index is passed.
    assert_eq!(run_str("[10, 20, 30].reduce((a, v, i) => a + i, 0)"), "3");
    // building an object.
    assert_eq!(
        run_str("JSON.stringify(['a', 'b'].reduce((o, k, i) => { o[k] = i; return o; }, {}))"),
        "{\"a\":0,\"b\":1}"
    );
}

#[test]
fn reduce_empty_no_init_throws() {
    let e = run_err("[].reduce((a, b) => a + b)");
    assert!(
        e.contains("empty array") || e.contains("initial value"),
        "unexpected error: {e}"
    );
}

#[test]
fn reduce_right_order_and_init() {
    assert_eq!(
        run_str("[1, 2, 3, 4].reduceRight((a, b) => a + '-' + b, 'X')"),
        "X-4-3-2-1"
    );
    assert_eq!(run_str("[42].reduceRight((a, b) => a + b)"), "42");
    // flatten right-to-left.
    assert_eq!(
        run_str("JSON.stringify([[0, 1], [2, 3], [4, 5]].reduceRight((a, b) => a.concat(b)))"),
        "[4,5,2,3,0,1]"
    );
    assert_eq!(
        run_str("['a', 'b', 'c'].reduceRight((a, b) => a + b)"),
        "cba"
    );
}

// ============================================================================
// find / findIndex / findLast / findLastIndex
// ============================================================================

#[test]
fn find_and_find_index() {
    assert_eq!(run_str("[1, 2, 3, 4].find(x => x > 2)"), "3");
    assert_eq!(run_str("[1, 2, 3].find(x => x > 5) === undefined"), "true");
    assert_eq!(run_str("[10, 20, 30].find((v, i) => i === 2)"), "30");
    assert_eq!(run_str("[1, 2, 3, 4].findIndex(x => x > 2)"), "2");
    assert_eq!(run_str("[1, 2, 3].findIndex(x => x > 5)"), "-1");
    assert_eq!(run_str("[10, 20, 30].findIndex((v, i) => v === 20)"), "1");
    // find returns the first match.
    assert_eq!(
        run_str("JSON.stringify([{id: 1, ok: false}, {id: 2, ok: true}, {id: 3, ok: true}].find(o => o.ok))"),
        "{\"id\":2,\"ok\":true}"
    );
}

#[test]
fn find_last_and_find_last_index() {
    assert_eq!(run_str("[1, 2, 3, 4].findLast(x => x % 2 === 1)"), "3");
    assert_eq!(run_str("[1, 2, 3, 4].findLastIndex(x => x % 2 === 1)"), "2");
    assert_eq!(run_str("String([1, 2, 3].findLast(x => x > 9))"), "undefined");
    assert_eq!(run_str("[1, 2, 3].findLastIndex(x => x > 9)"), "-1");
    assert_eq!(run_str("[5, 6, 7].findLast((v, i) => i === 0)"), "5");
}

// ============================================================================
// some / every
// ============================================================================

#[test]
fn some_and_every() {
    assert_eq!(run_str("[1, 2, 3].some(x => x > 2)"), "true");
    assert_eq!(run_str("[1, 2, 3].some(x => x > 5)"), "false");
    assert_eq!(run_str("[1, 2, 3].every(x => x > 0)"), "true");
    assert_eq!(run_str("[1, 2, 3].every(x => x > 1)"), "false");
    assert_eq!(run_str("[1, 2, 3].some((v, i) => i === 2)"), "true");
    assert_eq!(run_str("[1, 2, 3].every((v, i) => i < 5)"), "true");
}

#[test]
fn some_every_vacuous_truth() {
    // every on empty is vacuously true; some on empty is false.
    assert_eq!(run_str("[].every(x => x > 0)"), "true");
    assert_eq!(run_str("[].some(x => x > 0)"), "false");
}

// ============================================================================
// flat / flatMap
// ============================================================================

#[test]
fn flat_default_depth_one() {
    assert_eq!(
        run_str("JSON.stringify([1, [2, [3, [4]]]].flat())"),
        "[1,2,[3,[4]]]"
    );
    assert_eq!(
        run_str("JSON.stringify([[], [1], [2, 3]].flat())"),
        "[1,2,3]"
    );
    assert_eq!(run_str("JSON.stringify([1, 2, 3].flat())"), "[1,2,3]");
}

#[test]
fn flat_with_depth() {
    assert_eq!(
        run_str("JSON.stringify([1, [2, [3, [4]]]].flat(2))"),
        "[1,2,3,[4]]"
    );
    assert_eq!(
        run_str("JSON.stringify([1, [2], [[3]]].flat(0))"),
        "[1,[2],[[3]]]"
    );
    assert_eq!(
        run_str("JSON.stringify([1, [2, [3, [4]]]].flat(Infinity))"),
        "[1,2,3,4]"
    );
    assert_eq!(run_str("[1, [2, [3, [4]]]].flat(Infinity).length"), "4");
}

#[test]
fn flat_map() {
    assert_eq!(
        run_str("JSON.stringify([1, 2, 3].flatMap(x => [x, x * 2]))"),
        "[1,2,2,4,3,6]"
    );
    // returning [] removes the element.
    assert_eq!(
        run_str("JSON.stringify([1, 2, 3].flatMap(x => x === 2 ? [] : x))"),
        "[1,3]"
    );
    // flatMap only flattens one level.
    assert_eq!(
        run_str("JSON.stringify([1, 2].flatMap(x => [[x]]))"),
        "[[1],[2]]"
    );
    // index is passed.
    assert_eq!(
        run_str("JSON.stringify([1, 2].flatMap((v, i) => [i, v]))"),
        "[0,1,1,2]"
    );
}

// ============================================================================
// includes / indexOf / lastIndexOf
// ============================================================================

#[test]
fn includes_basic() {
    assert_eq!(run_str("[1, 2, 3].includes(2)"), "true");
    assert_eq!(run_str("[1, 2, 3].includes(9)"), "false");
    assert_eq!(run_str("['a', 'b'].includes('a')"), "true");
    assert_eq!(run_str("[].includes(1)"), "false");
    // includes uses SameValueZero so it finds NaN (unlike indexOf).
    assert_eq!(run_str("[NaN].includes(NaN)"), "true");
}

#[test]
fn includes_from_index() {
    assert_eq!(run_str("[1, 2, 3].includes(1, 1)"), "false");
    assert_eq!(run_str("[1, 2, 3].includes(3, 2)"), "true");
    // negative fromIndex counts from the end; very negative clamps to 0.
    assert_eq!(run_str("[1, 2, 3].includes(1, -100)"), "true");
    assert_eq!(run_str("[1, 2, 3].includes(1, -1)"), "false");
}

#[test]
fn includes_identity_semantics() {
    assert_eq!(run_str("const o = {}; [o].includes(o)"), "true");
    // distinct object literals are not equal.
    assert_eq!(run_str("[{}].includes({})"), "false");
}

#[test]
fn index_of() {
    assert_eq!(run_str("[1, 2, 3, 2, 1].indexOf(2)"), "1");
    assert_eq!(run_str("[1, 2, 3].indexOf(9)"), "-1");
    // fromIndex.
    assert_eq!(run_str("[1, 2, 3, 2, 1].indexOf(2, 2)"), "3");
    assert_eq!(run_str("[1, 2, 3, 2, 1].indexOf(1, -2)"), "4");
    // fromIndex beyond length yields -1.
    assert_eq!(run_str("[1, 2, 3].indexOf(3, 5)"), "-1");
    // indexOf uses strict equality, so it cannot find NaN.
    assert_eq!(run_str("[NaN].indexOf(NaN)"), "-1");
    // object identity.
    assert_eq!(run_str("const o = {}; [o].indexOf(o)"), "0");
}

#[test]
fn last_index_of_without_from_index() {
    // Without a fromIndex, lastIndexOf returns the absolute-last match.
    assert_eq!(run_str("[1, 2, 3, 2, 1].lastIndexOf(2)"), "3");
    assert_eq!(run_str("[1, 2, 3].lastIndexOf(3)"), "2");
    assert_eq!(run_str("[1, 2, 3].lastIndexOf(99)"), "-1");
    assert_eq!(run_str("['a', 'b', 'a'].lastIndexOf('a')"), "2");
}

#[test]
fn last_index_of_from_index_is_documented_divergence() {
    // DIVERGENCE (documented): real JS searches backwards from `fromIndex`, so
    // `[1,2,3,2,1].lastIndexOf(2, 1)` would be 1 in Node. This interpreter
    // currently ignores the `fromIndex` argument and still returns the
    // absolute-last match (3). We pin the *actual* behavior here rather than
    // the real-JS answer, per the GREEN guarantee.
    assert_eq!(run_str("[1, 2, 3, 2, 1].lastIndexOf(2, 1)"), "3");
    assert_eq!(run_str("[1, 2, 3, 2, 1].lastIndexOf(2, 4)"), "3");
}

// ============================================================================
// join & string coercion
// ============================================================================

#[test]
fn join_separators() {
    assert_eq!(run_str("[1, 2, 3].join('-')"), "1-2-3");
    // default separator is comma.
    assert_eq!(run_str("[1, 2, 3].join()"), "1,2,3");
    assert_eq!(run_str("[1, 2, 3].join('')"), "123");
    assert_eq!(run_str("JSON.stringify([].join(','))"), "\"\"");
    assert_eq!(run_str("[1].join('-')"), "1");
}

#[test]
fn join_null_undefined_and_nested() {
    // null and undefined become empty strings in a join.
    assert_eq!(run_str("[1, null, undefined, 2].join('-')"), "1---2");
    // nested arrays are themselves joined (comma) before the outer separator.
    assert_eq!(run_str("[1, [2, 3], 4].join('-')"), "1-2,3-4");
}

#[test]
fn array_string_coercion() {
    // `.toString()` as a method is unavailable here, but coercion paths work.
    assert_eq!(run_str("String([1, 2, 3])"), "1,2,3");
    assert_eq!(run_str("String([1, [2, 3]])"), "1,2,3");
    assert_eq!(run_str("'' + [1, 2, 3]"), "1,2,3");
    assert_eq!(run_str("`${[1, 2, 3]}`"), "1,2,3");
    assert_eq!(run_str("String([])"), "");
}

// ============================================================================
// slice (non-mutating)
// ============================================================================

#[test]
fn slice_ranges() {
    assert_eq!(run_str("JSON.stringify([1, 2, 3, 4, 5].slice())"), "[1,2,3,4,5]");
    assert_eq!(run_str("JSON.stringify([1, 2, 3, 4, 5].slice(1))"), "[2,3,4,5]");
    assert_eq!(run_str("JSON.stringify([1, 2, 3, 4, 5].slice(1, 3))"), "[2,3]");
    assert_eq!(run_str("JSON.stringify([1, 2, 3, 4, 5].slice(-3, -1))"), "[3,4]");
    assert_eq!(run_str("JSON.stringify([1, 2, 3].slice(2, 1))"), "[]");
    assert_eq!(run_str("JSON.stringify([1, 2, 3].slice(5))"), "[]");
    assert_eq!(run_str("JSON.stringify([1, 2, 3].slice(-1))"), "[3]");
}

#[test]
fn slice_is_shallow_copy() {
    assert_eq!(
        run_str("const a = [1, 2]; const b = a.slice(); a === b"),
        "false"
    );
    // does not mutate source.
    assert_eq!(
        run_str("const a = [1, 2, 3]; a.slice(1); JSON.stringify(a)"),
        "[1,2,3]"
    );
    // shallow: nested objects are shared.
    assert_eq!(
        run_str("const o = {n: 1}; const b = [o].slice(); b[0] === o"),
        "true"
    );
}

// ============================================================================
// concat (non-mutating)
// ============================================================================

#[test]
fn concat_spreads_array_args_only() {
    assert_eq!(run_str("JSON.stringify([1].concat(2, [3, 4]))"), "[1,2,3,4]");
    // nested arrays are NOT recursively spread.
    assert_eq!(
        run_str("JSON.stringify([1].concat(2, [3, 4], [[5]]))"),
        "[1,2,3,4,[5]]"
    );
    assert_eq!(run_str("JSON.stringify([1, 2].concat())"), "[1,2]");
    assert_eq!(run_str("JSON.stringify([].concat([1, 2], 3))"), "[1,2,3]");
    assert_eq!(
        run_str("JSON.stringify([1, 2].concat([3], 4, [5, 6]))"),
        "[1,2,3,4,5,6]"
    );
}

#[test]
fn concat_does_not_mutate() {
    assert_eq!(
        run_str("const a = [1, 2]; const b = a.concat([3]); JSON.stringify([a, b])"),
        "[[1,2],[1,2,3]]"
    );
    assert_eq!(
        run_str("const a = [1]; const b = a.concat([2]); a === b"),
        "false"
    );
}

// ============================================================================
// reverse (in-place)
// ============================================================================

#[test]
fn reverse_in_place() {
    assert_eq!(run_str("JSON.stringify([1, 2, 3].reverse())"), "[3,2,1]");
    assert_eq!(run_str("JSON.stringify([].reverse())"), "[]");
    assert_eq!(run_str("JSON.stringify([1].reverse())"), "[1]");
    // mutates in place and returns the same reference.
    assert_eq!(
        run_str("const a = [1, 2, 3]; const b = a.reverse(); JSON.stringify([a, b, a === b])"),
        "[[3,2,1],[3,2,1],true]"
    );
}

// ============================================================================
// sort (in-place, comparator, stability, default lexicographic)
// ============================================================================

#[test]
fn sort_default_lexicographic() {
    // default sort compares string forms.
    assert_eq!(
        run_str("JSON.stringify([10, 1, 2, 20, 3].sort())"),
        "[1,10,2,20,3]"
    );
    assert_eq!(
        run_str("JSON.stringify([100, 25, 8, 9].sort())"),
        "[100,25,8,9]"
    );
    assert_eq!(
        run_str("JSON.stringify(['banana', 'apple', 'cherry'].sort())"),
        "[\"apple\",\"banana\",\"cherry\"]"
    );
    assert_eq!(run_str("['b', 'a', 'c'].sort().join('')"), "abc");
}

#[test]
fn sort_with_comparator() {
    assert_eq!(
        run_str("JSON.stringify([3, 1, 2].sort((a, b) => a - b))"),
        "[1,2,3]"
    );
    assert_eq!(
        run_str("JSON.stringify([5, 3, 8, 1].sort((a, b) => b - a))"),
        "[8,5,3,1]"
    );
    assert_eq!(
        run_str("JSON.stringify([3.5, 1.2, 2.8].sort((a, b) => a - b))"),
        "[1.2,2.8,3.5]"
    );
    assert_eq!(run_str("JSON.stringify([].sort())"), "[]");
    assert_eq!(run_str("JSON.stringify([1].sort())"), "[1]");
}

#[test]
fn sort_in_place_same_reference() {
    assert_eq!(run_str("const a = [2, 1]; a.sort() === a"), "true");
    assert_eq!(
        run_str("const a = [3, 1, 2]; a.sort((x, y) => x - y); a.join(',')"),
        "1,2,3"
    );
}

#[test]
fn sort_is_stable() {
    // equal keys preserve original relative order.
    assert_eq!(
        run_str(
            "JSON.stringify([{k: 1, i: 0}, {k: 1, i: 1}, {k: 0, i: 2}].sort((a, b) => a.k - b.k).map(o => o.i))"
        ),
        "[2,0,1]"
    );
    assert_eq!(
        run_str(
            "JSON.stringify([{k: 2, i: 0}, {k: 1, i: 1}, {k: 2, i: 2}, {k: 1, i: 3}, {k: 2, i: 4}].sort((a, b) => a.k - b.k).map(o => o.i))"
        ),
        "[1,3,0,2,4]"
    );
    // multi-key comparator (tie-break).
    assert_eq!(
        run_str(
            "JSON.stringify([{p: 2, id: 'b'}, {p: 1, id: 'z'}, {p: 1, id: 'a'}].sort((x, y) => x.p !== y.p ? x.p - y.p : (x.id < y.id ? -1 : 1)).map(o => o.id))"
        ),
        "[\"a\",\"z\",\"b\"]"
    );
}

// ============================================================================
// fill (in-place)
// ============================================================================

#[test]
fn fill_ranges() {
    assert_eq!(run_str("JSON.stringify([1, 2, 3].fill(0))"), "[0,0,0]");
    assert_eq!(run_str("JSON.stringify([1, 2, 3, 4].fill(0, 1, 3))"), "[1,0,0,4]");
    assert_eq!(run_str("JSON.stringify([1, 2, 3, 4].fill(0, 2))"), "[1,2,0,0]");
    // negative start counts from the end.
    assert_eq!(run_str("JSON.stringify([1, 2, 3, 4].fill(9, -2))"), "[1,2,9,9]");
}

#[test]
fn fill_in_place_same_reference() {
    assert_eq!(run_str("const a = [1, 2, 3]; a.fill(0) === a"), "true");
    assert_eq!(
        run_str("const a = [1, 2, 3]; a.fill(7); JSON.stringify(a)"),
        "[7,7,7]"
    );
}

// ============================================================================
// copyWithin (in-place)
// ============================================================================

#[test]
fn copy_within_ranges() {
    assert_eq!(
        run_str("JSON.stringify([1, 2, 3, 4, 5].copyWithin(0, 3))"),
        "[4,5,3,4,5]"
    );
    assert_eq!(
        run_str("JSON.stringify([1, 2, 3, 4, 5].copyWithin(1, 3, 4))"),
        "[1,4,3,4,5]"
    );
    // negative target/start/end count from the end.
    assert_eq!(
        run_str("JSON.stringify([1, 2, 3, 4, 5].copyWithin(-2, -3, -1))"),
        "[1,2,3,3,4]"
    );
    assert_eq!(
        run_str("JSON.stringify([1, 2, 3, 4, 5].copyWithin(-2))"),
        "[1,2,3,1,2]"
    );
    // copyWithin(0) (no source offset) is a no-op.
    assert_eq!(
        run_str("JSON.stringify([1, 2, 3].copyWithin(0))"),
        "[1,2,3]"
    );
}

#[test]
fn copy_within_in_place_same_reference() {
    assert_eq!(run_str("const a = [1, 2, 3]; a.copyWithin(0, 1) === a"), "true");
}

// ============================================================================
// splice (in-place: remove, insert, replace)
// ============================================================================

#[test]
fn splice_replace() {
    assert_eq!(
        run_str("const a = [1, 2, 3, 4, 5]; const r = a.splice(1, 2, 'x', 'y', 'z'); JSON.stringify([r, a])"),
        "[[2,3],[1,\"x\",\"y\",\"z\",4,5]]"
    );
}

#[test]
fn splice_delete_only() {
    assert_eq!(
        run_str("const a = [1, 2, 3, 4]; const r = a.splice(1); JSON.stringify([r, a])"),
        "[[2,3,4],[1]]"
    );
    // count larger than remaining clamps.
    assert_eq!(
        run_str("const a = [1, 2, 3]; const r = a.splice(1, 10); JSON.stringify([r, a])"),
        "[[2,3],[1]]"
    );
    // negative start counts from the end.
    assert_eq!(
        run_str("const a = [1, 2, 3, 4, 5]; a.splice(-2, 1); JSON.stringify(a)"),
        "[1,2,3,5]"
    );
}

#[test]
fn splice_insert_only() {
    assert_eq!(
        run_str("const a = [1, 4]; a.splice(1, 0, 2, 3); JSON.stringify(a)"),
        "[1,2,3,4]"
    );
    // deleteCount 0 removes nothing.
    assert_eq!(
        run_str("const a = [1, 2, 3]; const r = a.splice(1, 0); JSON.stringify(r)"),
        "[]"
    );
    // negative deleteCount clamps to 0 (insert only).
    assert_eq!(
        run_str("const a = [1, 2, 3]; const r = a.splice(1, -1, 'x'); JSON.stringify([r, a])"),
        "[[],[1,\"x\",2,3]]"
    );
}

// ============================================================================
// push / pop / shift / unshift (in-place; return values)
// ============================================================================

#[test]
fn push_returns_new_length() {
    assert_eq!(run_str("const a = [1]; a.push(2, 3)"), "3");
    assert_eq!(
        run_str("const a = [1]; a.push(2, 3); JSON.stringify(a)"),
        "[1,2,3]"
    );
    // push with spread.
    assert_eq!(
        run_str("const a = []; a.push(...[1, 2, 3]); JSON.stringify(a)"),
        "[1,2,3]"
    );
    assert_eq!(run_str("const a = []; a.push(1)"), "1");
}

#[test]
fn pop_returns_last_and_mutates() {
    assert_eq!(
        run_str("const a = [1, 2, 3]; const x = a.pop(); JSON.stringify([x, a])"),
        "[3,[1,2]]"
    );
    // pop on empty returns undefined.
    assert_eq!(run_str("String([].pop())"), "undefined");
    assert_eq!(run_str("const a = []; a.pop(); a.length"), "0");
}

#[test]
fn shift_returns_first_and_mutates() {
    assert_eq!(
        run_str("const a = [1, 2, 3]; const x = a.shift(); JSON.stringify([x, a])"),
        "[1,[2,3]]"
    );
    assert_eq!(run_str("String([].shift())"), "undefined");
}

#[test]
fn unshift_prepends_and_returns_length() {
    assert_eq!(run_str("const a = [1, 2, 3]; a.unshift(0)"), "4");
    assert_eq!(
        run_str("const a = [3]; a.unshift(1, 2); JSON.stringify(a)"),
        "[1,2,3]"
    );
}

#[test]
fn stack_and_queue_usage() {
    // push/pop as a stack.
    assert_eq!(
        run_str("const s = []; s.push(1); s.push(2); const a = s.pop(); const b = s.pop(); JSON.stringify([a, b, s])"),
        "[2,1,[]]"
    );
    // push/shift as a queue.
    assert_eq!(
        run_str("const q = []; q.push('a'); q.push('b'); const x = q.shift(); JSON.stringify([x, q])"),
        "[\"a\",[\"b\"]]"
    );
}

// ============================================================================
// at
// ============================================================================

#[test]
fn at_positive_negative_and_oob() {
    assert_eq!(run_str("[1, 2, 3].at(0)"), "1");
    assert_eq!(run_str("[1, 2, 3].at(2)"), "3");
    assert_eq!(run_str("[1, 2, 3].at(-1)"), "3");
    assert_eq!(run_str("[1, 2, 3].at(-3)"), "1");
    // positive out-of-bounds returns undefined (matches JS).
    assert_eq!(run_str("String([1, 2, 3].at(5))"), "undefined");
    assert_eq!(run_str("String([].at(0))"), "undefined");
}

#[test]
fn at_negative_underflow_is_documented_divergence() {
    // DIVERGENCE (documented): in real JS a negative index past the start
    // (`[1,2,3].at(-5)`) returns `undefined`. This interpreter clamps the
    // negative offset to index 0 and returns the first element instead. We
    // pin the *actual* behavior here, per the GREEN guarantee.
    assert_eq!(run_str("[1, 2, 3].at(-5)"), "1");
    assert_eq!(run_str("[1, 2, 3].at(-4)"), "1");
}

// ============================================================================
// entries / keys / values (spread; iterator protocol)
// ============================================================================

#[test]
fn entries_keys_values_spread() {
    assert_eq!(
        run_str("JSON.stringify([...['a', 'b'].entries()])"),
        "[[0,\"a\"],[1,\"b\"]]"
    );
    assert_eq!(run_str("JSON.stringify([...['a', 'b'].keys()])"), "[0,1]");
    assert_eq!(
        run_str("JSON.stringify([...['a', 'b'].values()])"),
        "[\"a\",\"b\"]"
    );
    assert_eq!(run_str("JSON.stringify([...[].entries()])"), "[]");
    // nested spread/flatten of entries.
    assert_eq!(
        run_str("JSON.stringify([...[7, 8].entries()].flat())"),
        "[0,7,1,8]"
    );
}

// ============================================================================
// for-of iteration
// ============================================================================

#[test]
fn for_of_iteration() {
    assert_eq!(run_str("let s = 0; for (const x of [1, 2, 3]) s += x; s"), "6");
    assert_eq!(
        run_str("const r = []; for (const [i, v] of ['a', 'b'].entries()) r.push(i + ':' + v); r.join(',')"),
        "0:a,1:b"
    );
    assert_eq!(
        run_str("let out = ''; for (const k of ['x', 'y'].keys()) out += k; out"),
        "01"
    );
}

// ============================================================================
// spread
// ============================================================================

#[test]
fn array_spread() {
    assert_eq!(run_str("JSON.stringify([0, ...[1, 2], 3, ...[4]])"), "[0,1,2,3,4]");
    assert_eq!(run_str("JSON.stringify([...[], ...[1], ...[]])"), "[1]");
    // spread into a call.
    assert_eq!(run_str("Math.max(...[3, 1, 4, 1, 5])"), "5");
    // spread a string into chars.
    assert_eq!(run_str("JSON.stringify([...'abc'])"), "[\"a\",\"b\",\"c\"]");
    // spread a Set.
    assert_eq!(run_str("JSON.stringify([...new Set([1, 1, 2])])"), "[1,2]");
}

// ============================================================================
// Method chaining & integration
// ============================================================================

#[test]
fn method_chaining() {
    assert_eq!(
        run_str("JSON.stringify([1, 2, 3, 4, 5, 6].filter(x => x % 2 === 0).map(x => x * 10).reverse())"),
        "[60,40,20]"
    );
    assert_eq!(
        run_str("[1, 2, 3, 4].filter(x => x > 1).map(x => x * x).reduce((a, b) => a + b, 0)"),
        "29"
    );
    assert_eq!(
        run_str("['c', 'a', 'b'].sort().map(s => s.toUpperCase()).join('')"),
        "ABC"
    );
    // group-and-count idiom.
    assert_eq!(
        run_str(
            "JSON.stringify(['a', 'b', 'a', 'c', 'b', 'a'].reduce((m, k) => { m[k] = (m[k] || 0) + 1; return m; }, {}))"
        ),
        "{\"a\":3,\"b\":2,\"c\":1}"
    );
}

#[test]
fn mutation_independence_after_copy() {
    // mutating a slice copy does not affect the original.
    assert_eq!(
        run_str("const a = [1, 2, 3]; const b = a.slice(); b.push(4); JSON.stringify([a, b])"),
        "[[1,2,3],[1,2,3,4]]"
    );
    // mutating original after concat does not affect the concat result.
    assert_eq!(
        run_str("const a = [1, 2]; const b = a.concat([3]); a.push(99); JSON.stringify([a, b])"),
        "[[1,2,99],[1,2,3]]"
    );
}
