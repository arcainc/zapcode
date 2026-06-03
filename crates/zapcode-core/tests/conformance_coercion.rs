//! Conformance breadth: explicit type conversion & iteration protocols.
//!
//! `Boolean()`/`String()`/`Number()` over every primitive & container, plus the
//! iteration protocol for built-in iterables (spread & `for...of` over strings,
//! arrays, Map, Set; `Array.from`). All verified against Node. Number stays in the
//! non-exponential range so stringification is byte-identical.

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
// Boolean()
// ----------------------------------------------------------------------------

#[test]
fn boolean_of_primitives() {
    assert_eq!(run_str("Boolean(0)"), "false");
    assert_eq!(run_str("Boolean(1)"), "true");
    assert_eq!(run_str("Boolean(-0)"), "false");
    assert_eq!(run_str("Boolean(NaN)"), "false");
    assert_eq!(run_str("Boolean('')"), "false");
    assert_eq!(run_str("Boolean('x')"), "true");
    assert_eq!(run_str("Boolean('0')"), "true"); // non-empty string is truthy
    assert_eq!(run_str("Boolean(null)"), "false");
    assert_eq!(run_str("Boolean(undefined)"), "false");
}

#[test]
fn boolean_of_objects_is_always_true() {
    assert_eq!(run_str("Boolean([])"), "true");
    assert_eq!(run_str("Boolean({})"), "true");
    assert_eq!(run_str("Boolean([0])"), "true");
    assert_eq!(run_str("Boolean(new Map())"), "true");
}

#[test]
fn double_negation_matches_boolean() {
    assert_eq!(run_str("!!''"), "false");
    assert_eq!(run_str("!!'x'"), "true");
    assert_eq!(run_str("!!0"), "false");
    assert_eq!(run_str("!![]"), "true");
    assert_eq!(run_str("!!null"), "false");
}

// ----------------------------------------------------------------------------
// String()
// ----------------------------------------------------------------------------

#[test]
fn string_of_primitives() {
    assert_eq!(run_str("String(42)"), "42");
    assert_eq!(run_str("String(3.14)"), "3.14");
    assert_eq!(run_str("String(-0)"), "0"); // negative zero stringifies to "0"
    assert_eq!(run_str("String(true)"), "true");
    assert_eq!(run_str("String(false)"), "false");
    assert_eq!(run_str("String(null)"), "null");
    assert_eq!(run_str("String(undefined)"), "undefined");
    assert_eq!(run_str("String(NaN)"), "NaN");
    assert_eq!(run_str("String(Infinity)"), "Infinity");
    assert_eq!(run_str("String(-Infinity)"), "-Infinity");
}

#[test]
fn string_of_arrays_and_objects() {
    assert_eq!(run_str("String([1, 2, 3])"), "1,2,3");
    assert_eq!(run_str("String([])"), "");
    assert_eq!(run_str("String([1, [2, 3]])"), "1,2,3"); // nested arrays flatten via join
    assert_eq!(run_str("String([1, null, 2])"), "1,,2"); // null/undefined -> empty
    assert_eq!(run_str("String({})"), "[object Object]");
    assert_eq!(run_str("String({a: 1})"), "[object Object]");
}

// ----------------------------------------------------------------------------
// Number()
// ----------------------------------------------------------------------------

#[test]
fn number_of_strings() {
    assert_eq!(run_str("Number('42')"), "42");
    assert_eq!(run_str("Number('3.14')"), "3.14");
    assert_eq!(run_str("Number('  12  ')"), "12"); // trims whitespace
    assert_eq!(run_str("Number('')"), "0");
    assert_eq!(run_str("Number('  ')"), "0"); // whitespace only -> 0
    assert_eq!(run_str("String(Number('abc'))"), "NaN");
    assert_eq!(run_str("Number('0xff')"), "255"); // hex
    assert_eq!(run_str("Number('0b101')"), "5"); // binary
    assert_eq!(run_str("Number('0o17')"), "15"); // octal
    assert_eq!(run_str("Number('Infinity')"), "Infinity");
    assert_eq!(run_str("Number('-5')"), "-5");
}

#[test]
fn number_of_other_primitives() {
    assert_eq!(run_str("Number(true)"), "1");
    assert_eq!(run_str("Number(false)"), "0");
    assert_eq!(run_str("Number(null)"), "0");
    assert_eq!(run_str("String(Number(undefined))"), "NaN");
}

#[test]
fn number_of_arrays() {
    assert_eq!(run_str("Number([])"), "0"); // [] -> "" -> 0
    assert_eq!(run_str("Number([5])"), "5"); // single-element -> its string -> number
    assert_eq!(run_str("String(Number([1, 2]))"), "NaN"); // multi-element -> "1,2" -> NaN
    assert_eq!(run_str("Number(['7'])"), "7");
}

// ----------------------------------------------------------------------------
// Iteration protocol over built-in iterables
// ----------------------------------------------------------------------------

#[test]
fn spread_over_iterables() {
    assert_eq!(run_str("[...'abc'].join('-')"), "a-b-c");
    assert_eq!(run_str("[...[1, 2, 3]].length"), "3");
    assert_eq!(run_str("[...new Set([1, 1, 2])].join(',')"), "1,2");
    assert_eq!(run_str("[...new Map([['a', 1]])].length"), "1");
    assert_eq!(run_str("JSON.stringify([...new Map([['a', 1]])])"), "[[\"a\",1]]");
    // spread into a new array combining iterables
    assert_eq!(run_str("[...[1, 2], ...[3, 4]].join(',')"), "1,2,3,4");
    assert_eq!(run_str("['x', ...'yz'].join('')"), "xyz");
}

#[test]
fn for_of_over_iterables() {
    assert_eq!(run_str("let o = []; for (const c of 'abc') o.push(c); o.join('-')"), "a-b-c");
    assert_eq!(run_str("let s = 0; for (const n of [1, 2, 3]) s += n; s"), "6");
    assert_eq!(run_str("let o = []; for (const x of new Set([3, 1, 2])) o.push(x); o.join(',')"), "3,1,2");
    assert_eq!(run_str("let o = []; for (const [k, v] of new Map([['a', 1], ['b', 2]])) o.push(`${k}${v}`); o.join(',')"), "a1,b2");
}

#[test]
fn array_from_over_iterables() {
    assert_eq!(run_str("Array.from('xy').join(',')"), "x,y");
    assert_eq!(run_str("Array.from(new Set([1, 1, 2, 3])).join(',')"), "1,2,3");
    assert_eq!(run_str("Array.from([1, 2, 3], x => x * 10).join(',')"), "10,20,30");
    assert_eq!(run_str("Array.from({length: 4}, (_, i) => i * i).join(',')"), "0,1,4,9");
}

#[test]
fn destructuring_over_iterables() {
    assert_eq!(run_str("const [a, b, c] = 'xyz'; `${a}${b}${c}`"), "xyz");
    assert_eq!(run_str("const [first, ...rest] = [10, 20, 30]; `${first}:${rest.join(',')}`"), "10:20,30");
}
