//! Conformance breadth: Array & Object built-in methods.
//!
//! Wide coverage of the array iteration/mutation/query surface and object
//! construction/reflection. Results are stringified with `JSON.stringify` (or a
//! scalar expression) so the harness's `to_js_string` output is deterministic and
//! byte-comparable to Node. `Object.freeze` is enforcing and
//! `Object.getOwnPropertyNames` is implemented.

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

// ----------------------------------------------------------------------------
// Array construction / static methods
// ----------------------------------------------------------------------------

#[test]
fn array_static_constructors() {
    assert_eq!(run_str("JSON.stringify(Array.of(1, 2, 3))"), "[1,2,3]");
    assert_eq!(run_str("JSON.stringify(Array.of(7))"), "[7]");
    assert_eq!(run_str("JSON.stringify(Array.from('abc'))"), "[\"a\",\"b\",\"c\"]");
    assert_eq!(run_str("JSON.stringify(Array.from([1, 2, 3], x => x * x))"), "[1,4,9]");
    assert_eq!(run_str("JSON.stringify(Array.from({length: 3}, (_, i) => i))"), "[0,1,2]");
    assert_eq!(run_str("JSON.stringify(Array.from(new Set([1, 1, 2, 3, 3])))"), "[1,2,3]");
}

#[test]
fn array_is_array() {
    assert_eq!(run_str("Array.isArray([1, 2])"), "true");
    assert_eq!(run_str("Array.isArray([])"), "true");
    assert_eq!(run_str("Array.isArray({length: 2})"), "false");
    assert_eq!(run_str("Array.isArray('abc')"), "false");
    assert_eq!(run_str("Array.isArray(null)"), "false");
}

// ----------------------------------------------------------------------------
// Array mutation methods
// ----------------------------------------------------------------------------

#[test]
fn push_pop_shift_unshift() {
    assert_eq!(run_str("let a = [1]; a.push(2, 3)"), "3"); // returns new length
    assert_eq!(run_str("let a = [1, 2, 3]; const p = a.pop(); `${p}:${JSON.stringify(a)}`"), "3:[1,2]");
    assert_eq!(run_str("let a = [1, 2, 3]; const s = a.shift(); `${s}:${JSON.stringify(a)}`"), "1:[2,3]");
    assert_eq!(run_str("let a = [3]; const n = a.unshift(1, 2); `${n}:${JSON.stringify(a)}`"), "3:[1,2,3]");
    assert_eq!(run_str("let a = []; a.pop(); JSON.stringify(a)"), "[]"); // pop empty is safe
}

#[test]
fn splice() {
    assert_eq!(run_str("let a = [1,2,3,4,5]; const r = a.splice(1, 2); `${JSON.stringify(r)}|${JSON.stringify(a)}`"), "[2,3]|[1,4,5]");
    assert_eq!(run_str("let a = [1,2,3]; a.splice(1, 0, 'x', 'y'); JSON.stringify(a)"), "[1,\"x\",\"y\",2,3]");
    assert_eq!(run_str("let a = [1,2,3,4]; a.splice(-2); JSON.stringify(a)"), "[1,2]"); // negative start
    assert_eq!(run_str("let a = [1,2,3]; a.splice(1, 1, 'z'); JSON.stringify(a)"), "[1,\"z\",3]"); // replace
}

#[test]
fn reverse_fill_copy_within() {
    assert_eq!(run_str("JSON.stringify([1, 2, 3].reverse())"), "[3,2,1]");
    assert_eq!(run_str("JSON.stringify([1, 2, 3, 4].fill(0, 1, 3))"), "[1,0,0,4]");
    assert_eq!(run_str("JSON.stringify([1, 2, 3].fill(9))"), "[9,9,9]");
    assert_eq!(run_str("JSON.stringify([1, 2, 3, 4, 5].copyWithin(0, 3))"), "[4,5,3,4,5]");
}

#[test]
fn sort_default_and_comparator() {
    assert_eq!(run_str("JSON.stringify([3, 1, 2].sort())"), "[1,2,3]");
    // default sort is lexicographic (string) order
    assert_eq!(run_str("JSON.stringify([10, 9, 2, 1].sort())"), "[1,10,2,9]");
    assert_eq!(run_str("JSON.stringify([10, 9, 2, 1].sort((a, b) => a - b))"), "[1,2,9,10]");
    assert_eq!(run_str("JSON.stringify([10, 9, 2, 1].sort((a, b) => b - a))"), "[10,9,2,1]");
    assert_eq!(run_str("JSON.stringify(['banana', 'apple', 'cherry'].sort())"), "[\"apple\",\"banana\",\"cherry\"]");
}

#[test]
fn sort_is_stable() {
    // Stable sort: equal keys preserve original relative order.
    assert_eq!(
        run_str("JSON.stringify([{k:1,v:'a'},{k:1,v:'b'},{k:0,v:'c'}].sort((a,b)=>a.k-b.k).map(o=>o.v))"),
        "[\"c\",\"a\",\"b\"]"
    );
}

// ----------------------------------------------------------------------------
// Array query / iteration methods
// ----------------------------------------------------------------------------

#[test]
fn map_filter_reduce() {
    assert_eq!(run_str("JSON.stringify([1, 2, 3].map(x => x * 2))"), "[2,4,6]");
    assert_eq!(run_str("JSON.stringify([1, 2, 3, 4].filter(x => x % 2 === 0))"), "[2,4]");
    assert_eq!(run_str("[1, 2, 3, 4].reduce((a, b) => a + b, 0)"), "10");
    assert_eq!(run_str("[1, 2, 3, 4].reduce((a, b) => a + b)"), "10"); // no init
    assert_eq!(run_str("[1, 2, 3].reduceRight((a, b) => `${a}${b}`, '')"), "321");
    // index argument
    assert_eq!(run_str("JSON.stringify(['a', 'b'].map((x, i) => `${i}${x}`))"), "[\"0a\",\"1b\"]");
}

#[test]
fn flat_and_flat_map() {
    assert_eq!(run_str("JSON.stringify([1, [2, 3], [4]].flat())"), "[1,2,3,4]");
    assert_eq!(run_str("JSON.stringify([1, [2, [3, [4]]]].flat(2))"), "[1,2,3,[4]]");
    assert_eq!(run_str("JSON.stringify([1, [2, [3, [4]]]].flat(Infinity))"), "[1,2,3,4]");
    assert_eq!(run_str("JSON.stringify([[1, 2], [3, 4]].flatMap(x => x))"), "[1,2,3,4]");
    assert_eq!(run_str("JSON.stringify([1, 2, 3].flatMap(x => [x, x * 10]))"), "[1,10,2,20,3,30]");
}

#[test]
fn predicates_every_some_find() {
    assert_eq!(run_str("[2, 4, 6].every(x => x % 2 === 0)"), "true");
    assert_eq!(run_str("[2, 4, 5].every(x => x % 2 === 0)"), "false");
    assert_eq!(run_str("[1, 3, 5].some(x => x % 2 === 0)"), "false");
    assert_eq!(run_str("[1, 3, 4].some(x => x % 2 === 0)"), "true");
    assert_eq!(run_str("[1, 2, 3, 4].find(x => x > 2)"), "3");
    assert_eq!(run_str("[1, 2, 3, 4].findIndex(x => x > 2)"), "2");
    assert_eq!(run_str("String([1, 2, 3].find(x => x > 9))"), "undefined");
}

#[test]
fn index_of_and_includes() {
    assert_eq!(run_str("[1, 2, 3, 2].indexOf(2)"), "1");
    assert_eq!(run_str("[1, 2, 3, 2].lastIndexOf(2)"), "3");
    assert_eq!(run_str("[1, 2, 3].indexOf(9)"), "-1");
    assert_eq!(run_str("[1, 2, 3].includes(2)"), "true");
    assert_eq!(run_str("[1, 2, 3].includes(9)"), "false");
    // SameValueZero: includes finds NaN, indexOf does not
    assert_eq!(run_str("[NaN].includes(NaN)"), "true");
    assert_eq!(run_str("[NaN].indexOf(NaN)"), "-1");
    // fromIndex
    assert_eq!(run_str("[1, 2, 1, 2].indexOf(2, 2)"), "3");
}

#[test]
fn slice_at_join_concat() {
    assert_eq!(run_str("JSON.stringify([1, 2, 3, 4, 5].slice(1, 3))"), "[2,3]");
    assert_eq!(run_str("JSON.stringify([1, 2, 3, 4, 5].slice(-2))"), "[4,5]");
    assert_eq!(run_str("JSON.stringify([1, 2, 3].slice())"), "[1,2,3]"); // shallow copy
    assert_eq!(run_str("[1, 2, 3].at(-1)"), "3");
    assert_eq!(run_str("[1, 2, 3].at(0)"), "1");
    assert_eq!(run_str("[1, 2, 3].join('-')"), "1-2-3");
    assert_eq!(run_str("[1, 2, 3].join()"), "1,2,3"); // default comma
    assert_eq!(run_str("[1, null, undefined, 2].join('-')"), "1---2"); // null/undefined -> ''
    assert_eq!(run_str("JSON.stringify([1, 2].concat([3, 4], 5))"), "[1,2,3,4,5]");
}

#[test]
fn array_iterator_protocol() {
    assert_eq!(run_str("JSON.stringify([...[10, 20].keys()])"), "[0,1]");
    assert_eq!(run_str("JSON.stringify([...['a', 'b'].values()])"), "[\"a\",\"b\"]");
    assert_eq!(run_str("JSON.stringify([...['a', 'b'].entries()])"), "[[0,\"a\"],[1,\"b\"]]");
    assert_eq!(run_str("let s=''; for (const [i, v] of ['x', 'y'].entries()) s += `${i}${v}`; s"), "0x1y");
}

#[test]
fn for_each_visits_in_order() {
    assert_eq!(run_str("let out = []; [10, 20, 30].forEach((x, i) => out.push(`${i}:${x}`)); out.join(',')"), "0:10,1:20,2:30");
    assert_eq!(run_str("let sum = 0; [1, 2, 3].forEach(x => { sum += x; }); sum"), "6");
}

// ----------------------------------------------------------------------------
// Object construction / reflection
// ----------------------------------------------------------------------------

#[test]
fn object_literal_features() {
    assert_eq!(run_str("const a = 1, b = 2; JSON.stringify({a, b})"), "{\"a\":1,\"b\":2}"); // shorthand
    assert_eq!(run_str("const k = 'dyn'; JSON.stringify({[k]: 1, ['x' + 'y']: 2})"), "{\"dyn\":1,\"xy\":2}"); // computed
    assert_eq!(run_str("const o = {greet(){ return 'hi'; }}; o.greet()"), "hi"); // method shorthand
    assert_eq!(run_str("JSON.stringify({...{a: 1, b: 2}, b: 9})"), "{\"a\":1,\"b\":9}"); // spread + override
    assert_eq!(run_str("({a: {b: {c: 42}}}).a.b.c"), "42"); // nested access
}

#[test]
fn object_keys_values_entries() {
    assert_eq!(run_str("JSON.stringify(Object.keys({z: 1, a: 2, m: 3}))"), "[\"z\",\"a\",\"m\"]"); // insertion order
    assert_eq!(run_str("JSON.stringify(Object.values({a: 1, b: 2}))"), "[1,2]");
    assert_eq!(run_str("JSON.stringify(Object.entries({a: 1, b: 2}))"), "[[\"a\",1],[\"b\",2]]");
    assert_eq!(run_str("let s = ''; for (const [k, v] of Object.entries({a: 1, b: 2})) s += k + v; s"), "a1b2");
}

#[test]
fn object_assign_and_from_entries() {
    assert_eq!(run_str("JSON.stringify(Object.assign({}, {a: 1}, {b: 2}))"), "{\"a\":1,\"b\":2}");
    assert_eq!(run_str("JSON.stringify(Object.assign({a: 1}, {a: 9, b: 2}))"), "{\"a\":9,\"b\":2}"); // later wins
    assert_eq!(run_str("JSON.stringify(Object.fromEntries([['a', 1], ['b', 2]]))"), "{\"a\":1,\"b\":2}");
    // `Object.fromEntries` consumes an array of pairs; a Map must be spread first
    // (zapcode's `fromEntries` does not iterate a Map directly — documented).
    assert_eq!(run_str("JSON.stringify(Object.fromEntries([...new Map([['x', 1]])]))"), "{\"x\":1}");
    // round-trip entries -> fromEntries
    assert_eq!(
        run_str("const o = {a: 1, b: 2}; JSON.stringify(Object.fromEntries(Object.entries(o)))"),
        "{\"a\":1,\"b\":2}"
    );
}

#[test]
fn has_own_property() {
    assert_eq!(run_str("({a: 1}).hasOwnProperty('a')"), "true");
    assert_eq!(run_str("({a: 1}).hasOwnProperty('b')"), "false");
}

#[test]
fn object_freeze_is_enforcing() {
    // `Object.freeze` silently ignores the write in sloppy mode, leaving `a === 1`.
    assert_eq!(run_str("const o = Object.freeze({a: 1}); o.a = 2; o.a"), "1");
}
