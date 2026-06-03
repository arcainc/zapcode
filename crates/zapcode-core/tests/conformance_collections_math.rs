//! Conformance breadth: Map, Set, Number, and Math.
//!
//! Map/Set construction, mutation, identity-keyed lookup, SameValueZero (NaN)
//! membership, iteration & spread; Number formatting (toFixed/toPrecision/
//! toExponential/toString-radix) and Number.* predicates; the Math surface
//! (rounding, sign/abs/trunc, pow/sqrt/cbrt/hypot, logs, min/max). All values
//! verified against Node; number-stringification stays in the
//! non-exponential range so it is byte-identical to Node's `String(...)`.

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
// Map
// ----------------------------------------------------------------------------

#[test]
fn map_basic_crud() {
    assert_eq!(run_str("const m = new Map(); m.set('a', 1); m.set('b', 2); `${m.get('a')},${m.size}`"), "1,2");
    assert_eq!(run_str("const m = new Map([['x', 1]]); `${m.has('x')},${m.has('y')}`"), "true,false");
    assert_eq!(run_str("const m = new Map([['x', 1]]); const d = m.delete('x'); `${d},${m.size}`"), "true,0");
    assert_eq!(run_str("const m = new Map([['x', 1]]); m.clear(); m.size"), "0");
    assert_eq!(run_str("const m = new Map(); String(m.get('missing'))"), "undefined");
    // update existing key
    assert_eq!(run_str("const m = new Map([['k', 1]]); m.set('k', 9); m.get('k')"), "9");
}

#[test]
fn map_set_returns_map_for_chaining() {
    assert_eq!(run_str("const m = new Map(); m.set(1, 'a').set(2, 'b'); m.size"), "2");
}

#[test]
fn map_identity_and_special_keys() {
    // Object keys are by identity (reference semantics).
    assert_eq!(run_str("const k = {}; const m = new Map(); m.set(k, 9); m.get(k)"), "9");
    assert_eq!(run_str("const m = new Map(); m.set({}, 1); String(m.get({}))"), "undefined"); // distinct objects
    // NaN is a usable key (SameValueZero).
    assert_eq!(run_str("const m = new Map(); m.set(NaN, 1); m.get(NaN)"), "1");
    // distinct type keys don't collide
    assert_eq!(run_str("const m = new Map(); m.set(1, 'n'); m.set('1', 's'); `${m.get(1)},${m.get('1')}`"), "n,s");
}

#[test]
fn map_iteration_preserves_insertion_order() {
    assert_eq!(run_str("const m = new Map([['a', 1], ['b', 2]]); let s = ''; for (const [k, v] of m) s += k + v; s"), "a1b2");
    assert_eq!(run_str("JSON.stringify([...new Map([['a', 1], ['b', 2]]).keys()])"), "[\"a\",\"b\"]");
    assert_eq!(run_str("JSON.stringify([...new Map([['a', 1], ['b', 2]]).values()])"), "[1,2]");
    assert_eq!(run_str("JSON.stringify([...new Map([['a', 1]]).entries()])"), "[[\"a\",1]]");
    assert_eq!(run_str("const m = new Map([['a', 1], ['b', 2]]); let s = 0; m.forEach(v => s += v); s"), "3");
    assert_eq!(run_str("JSON.stringify([...new Map([['a', 1], ['b', 2]])])"), "[[\"a\",1],[\"b\",2]]");
}

#[test]
fn map_copy_constructor() {
    assert_eq!(run_str("const a = new Map([['x', 1]]); const b = new Map(a); b.get('x')"), "1");
}

// ----------------------------------------------------------------------------
// Set
// ----------------------------------------------------------------------------

#[test]
fn set_basic_crud_and_dedup() {
    assert_eq!(run_str("new Set([1, 2, 2, 3]).size"), "3");
    assert_eq!(run_str("const s = new Set(); s.add(1).add(2).add(1); s.size"), "2"); // add chains + dedups
    assert_eq!(run_str("const s = new Set([1, 2]); `${s.has(2)},${s.has(9)}`"), "true,false");
    assert_eq!(run_str("const s = new Set([1, 2]); s.delete(1); s.size"), "1");
    assert_eq!(run_str("const s = new Set([1, 2, 3]); s.clear(); s.size"), "0");
    // NaN dedups (SameValueZero)
    assert_eq!(run_str("new Set([NaN, NaN]).size"), "1");
    // -0 and 0 are the same Set element
    assert_eq!(run_str("new Set([0, -0]).size"), "1");
}

#[test]
fn set_iteration_and_spread() {
    assert_eq!(run_str("JSON.stringify([...new Set('aabbc')])"), "[\"a\",\"b\",\"c\"]");
    assert_eq!(run_str("let r = []; for (const x of new Set([1, 2, 3])) r.push(x); JSON.stringify(r)"), "[1,2,3]");
    assert_eq!(run_str("JSON.stringify([...new Set([3, 1, 2, 1])])"), "[3,1,2]"); // insertion order, deduped
    assert_eq!(run_str("const s = new Set([1, 2, 3]); let sum = 0; s.forEach(x => sum += x); sum"), "6");
    // dedup an array via Set round-trip
    assert_eq!(run_str("JSON.stringify([...new Set([1, 1, 2, 3, 3, 3])])"), "[1,2,3]");
}

// ----------------------------------------------------------------------------
// Number formatting & predicates
// ----------------------------------------------------------------------------

#[test]
fn number_to_fixed() {
    assert_eq!(run_str("(3.14159).toFixed(2)"), "3.14");
    assert_eq!(run_str("(3.14159).toFixed(0)"), "3");
    assert_eq!(run_str("(0.005).toFixed(2)"), "0.01");
    assert_eq!(run_str("(1).toFixed(3)"), "1.000");
    assert_eq!(run_str("(2.5).toFixed(0)"), "3");
}

#[test]
fn number_to_precision_and_exponential() {
    assert_eq!(run_str("(123.456).toPrecision(4)"), "123.5");
    assert_eq!(run_str("(0.0001234).toPrecision(2)"), "0.00012");
    assert_eq!(run_str("(12345).toExponential(2)"), "1.23e+4");
    assert_eq!(run_str("(0.5).toExponential(1)"), "5.0e-1");
}

#[test]
fn number_to_string_radix() {
    assert_eq!(run_str("(255).toString(16)"), "ff");
    assert_eq!(run_str("(5).toString(2)"), "101");
    assert_eq!(run_str("(255).toString(2)"), "11111111");
    assert_eq!(run_str("(35).toString(36)"), "z");
    assert_eq!(run_str("(255).toString()"), "255"); // default base 10
}

#[test]
fn number_predicates() {
    assert_eq!(run_str("Number.isInteger(5.0)"), "true");
    assert_eq!(run_str("Number.isInteger(5.5)"), "false");
    assert_eq!(run_str("Number.isNaN(NaN)"), "true");
    assert_eq!(run_str("Number.isNaN(5)"), "false");
    assert_eq!(run_str("Number.isFinite(Infinity)"), "false");
    assert_eq!(run_str("Number.isFinite(42)"), "true");
    assert_eq!(run_str("Number.isSafeInteger(2 ** 53)"), "false");
    assert_eq!(run_str("Number.isSafeInteger(2 ** 53 - 1)"), "true");
    assert_eq!(run_str("Number.MAX_SAFE_INTEGER"), "9007199254740991");
    assert_eq!(run_str("Number.EPSILON > 0"), "true");
}

#[test]
fn parse_int_and_float() {
    assert_eq!(run_str("parseInt('42')"), "42");
    assert_eq!(run_str("parseInt('42px')"), "42");
    assert_eq!(run_str("parseInt('0xff', 16)"), "255");
    assert_eq!(run_str("parseInt('0x1F')"), "31"); // auto-detect hex
    assert_eq!(run_str("parseInt('z', 36)"), "35");
    assert_eq!(run_str("String(parseInt('abc'))"), "NaN");
    assert_eq!(run_str("parseFloat('3.14xyz')"), "3.14");
    assert_eq!(run_str("parseFloat('.5')"), "0.5");
    assert_eq!(run_str("Number.parseInt('100')"), "100");
    assert_eq!(run_str("Number.parseFloat('2.5')"), "2.5");
}

// ----------------------------------------------------------------------------
// Math
// ----------------------------------------------------------------------------

#[test]
fn math_rounding() {
    assert_eq!(run_str("Math.round(2.5)"), "3");
    assert_eq!(run_str("Math.round(-2.5)"), "-2"); // rounds toward +Infinity
    assert_eq!(run_str("Math.round(2.4)"), "2");
    assert_eq!(run_str("Math.floor(-1.5)"), "-2");
    assert_eq!(run_str("Math.ceil(-1.5)"), "-1");
    assert_eq!(run_str("Math.trunc(-4.7)"), "-4");
    assert_eq!(run_str("Math.trunc(4.7)"), "4");
}

#[test]
fn math_abs_sign() {
    assert_eq!(run_str("Math.abs(-7)"), "7");
    assert_eq!(run_str("Math.abs(7)"), "7");
    assert_eq!(run_str("Math.sign(-3)"), "-1");
    assert_eq!(run_str("Math.sign(3)"), "1");
    assert_eq!(run_str("Math.sign(0)"), "0");
}

#[test]
fn math_power_and_roots() {
    assert_eq!(run_str("Math.pow(2, 10)"), "1024");
    assert_eq!(run_str("Math.sqrt(16)"), "4");
    assert_eq!(run_str("Math.cbrt(27)"), "3");
    assert_eq!(run_str("Math.hypot(3, 4)"), "5");
    assert_eq!(run_str("Math.hypot(5, 12)"), "13");
}

#[test]
fn math_min_max_and_logs() {
    assert_eq!(run_str("Math.max(1, 9, 3)"), "9");
    assert_eq!(run_str("Math.min(5, 2, 8)"), "2");
    assert_eq!(run_str("Math.max(...[3, 1, 4, 1, 5])"), "5");
    assert_eq!(run_str("Math.log2(8)"), "3");
    assert_eq!(run_str("Math.log10(1000)"), "3");
}

#[test]
fn math_constants() {
    assert_eq!(run_str("Math.PI > 3.14 && Math.PI < 3.15"), "true");
    assert_eq!(run_str("Math.E > 2.71 && Math.E < 2.72"), "true");
}
