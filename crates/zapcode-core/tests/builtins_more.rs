//! Regression tests for the builtins added in the second stress pass:
//! array findLast/findLastIndex/reduceRight/entries/keys, Math.hypot,
//! Number.isSafeInteger, new Array(n), Array.from(arrayLike, mapFn),
//! instanceof Array/Object, Object.hasOwn, hasOwnProperty, localeCompare,
//! Promise.allSettled/race/any, and the >>> (unsigned shift) fix.

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
        VmState::Complete(v) => v.to_js_string(),
        other => panic!("expected completion, got {other:?}"),
    }
}

#[test]
fn array_find_last_and_reduce_right() {
    assert_eq!(run_str("[1,2,3,4].findLast(x => x % 2 === 1)"), "3");
    assert_eq!(run_str("[1,2,3,4].findLastIndex(x => x % 2 === 1)"), "2");
    assert_eq!(run_str("[1,2,3].findLast(x => x > 9)"), "undefined");
    assert_eq!(run_str("[1,2,3].findLastIndex(x => x > 9)"), "-1");
    assert_eq!(
        run_str("['a','b','c'].reduceRight((acc, x) => acc + x, '')"),
        "cba"
    );
    assert_eq!(run_str("[1,2,3].reduceRight((a, b) => a + b)"), "6");
}

#[test]
fn array_entries_keys_values() {
    assert_eq!(
        run_str("[...['a','b'].entries()].map(e => e.join(':')).join(',')"),
        "0:a,1:b"
    );
    assert_eq!(run_str("[...['a','b'].keys()].join(',')"), "0,1");
    assert_eq!(run_str("[...[10,20].values()].join(',')"), "10,20");
}

#[test]
fn math_and_number_additions() {
    assert_eq!(run_str("Math.hypot(3, 4)"), "5");
    assert_eq!(run_str("Number.isSafeInteger(9007199254740991)"), "true");
    assert_eq!(run_str("Number.isSafeInteger(9007199254740992)"), "false");
    assert_eq!(run_str("Number.isSafeInteger(1.5)"), "false");
}

#[test]
fn array_constructor_and_from_mapfn() {
    assert_eq!(run_str("new Array(3).length"), "3");
    assert_eq!(run_str("new Array(1, 2, 3).join(',')"), "1,2,3");
    assert_eq!(run_str("new Array('x').join(',')"), "x");
    assert_eq!(
        run_str("Array.from({length: 3}, (_, i) => i * 2).join(',')"),
        "0,2,4"
    );
    assert_eq!(
        run_str("Array.from('abc', c => c.toUpperCase()).join('')"),
        "ABC"
    );
}

#[test]
fn instanceof_array_and_object() {
    assert_eq!(run_str("[] instanceof Array"), "true");
    assert_eq!(run_str("[] instanceof Object"), "true");
    assert_eq!(run_str("({}) instanceof Object"), "true");
    assert_eq!(run_str("({}) instanceof Array"), "false");
    assert_eq!(run_str("'x' instanceof Object"), "false");
}

#[test]
fn object_has_own_variants() {
    assert_eq!(run_str("Object.hasOwn({a: 1}, 'a')"), "true");
    assert_eq!(run_str("Object.hasOwn({a: 1}, 'b')"), "false");
    assert_eq!(run_str("({a: 1}).hasOwnProperty('a')"), "true");
    assert_eq!(run_str("({a: 1}).hasOwnProperty('toString')"), "false");
}

#[test]
fn string_locale_compare() {
    assert_eq!(run_str("'a'.localeCompare('b')"), "-1");
    assert_eq!(run_str("'b'.localeCompare('a')"), "1");
    assert_eq!(run_str("'a'.localeCompare('a')"), "0");
}

#[test]
fn promise_combinators() {
    assert_eq!(
        run_str("(await Promise.allSettled([Promise.resolve(1), Promise.reject('e')])).map(r => r.status).join(',')"),
        "fulfilled,rejected"
    );
    assert_eq!(run_str("await Promise.race([Promise.resolve('a')])"), "a");
    assert_eq!(
        run_str("await Promise.any([Promise.reject('x'), Promise.resolve('y')])"),
        "y"
    );
}

#[test]
fn unsigned_right_shift() {
    assert_eq!(run_str("-1 >>> 0"), "4294967295");
    assert_eq!(run_str("4 >>> 1"), "2");
    assert_eq!(run_str("-8 >>> 28"), "15");
}
