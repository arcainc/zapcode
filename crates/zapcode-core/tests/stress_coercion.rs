//! Regression tests for coercion/operator divergences found in the stress passes:
//! string relational comparison (O1), `+` ToPrimitive for arrays/objects (O3),
//! array->string/number coercion (O6/O10), bitwise ToInt32 wraparound (F2), and
//! Number() of hex/binary/octal/Infinity strings (F5).

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
fn string_relational_is_lexicographic() {
    // O1: string < string used to always return false.
    assert_eq!(run_str("'apple' < 'banana'"), "true");
    assert_eq!(run_str("'a' < 'b'"), "true");
    assert_eq!(run_str("'b' < 'a'"), "false");
    assert_eq!(run_str("'car' <= 'car'"), "true");
    assert_eq!(run_str("'car' >= 'car'"), "true");
    assert_eq!(run_str("'Z' < 'a'"), "true");
    assert_eq!(run_str("'10' < '2'"), "true");
    assert_eq!(run_str("'banana' > 'apple'"), "true");
    // Mixed number/string stays numeric.
    assert_eq!(run_str("10 < '9'"), "false");
    assert_eq!(run_str("'5' >= 5"), "true");
    // Sorting/filtering on strings now works.
    assert_eq!(
        run_str("['banana','apple','cherry'].filter(s => s < 'b').join(',')"),
        "apple"
    );
}

#[test]
fn plus_coerces_arrays_and_objects_to_string() {
    // O3: array/object operands ToPrimitive to a string for `+`.
    assert_eq!(run_str("[1,2]+[3]"), "1,23");
    assert_eq!(run_str("1+[2]"), "12");
    assert_eq!(run_str("[1]+1"), "11");
    assert_eq!(run_str("[]+{}"), "[object Object]");
    assert_eq!(run_str("[]+[]"), "");
    assert_eq!(run_str("1+{}"), "1[object Object]");
    // Numeric operands without a reference type stay numeric.
    assert_eq!(run_str("2+3"), "5");
    assert_eq!(run_str("'x'+1"), "x1");
}

#[test]
fn array_to_string_renders_null_and_undefined_as_empty() {
    // O6: join/toString render null/undefined as "".
    assert_eq!(run_str("[1,null,2,undefined,3].join(',')"), "1,,2,,3");
    assert_eq!(run_str("['a',null,'b'].join('/')"), "a//b");
    assert_eq!(run_str("String([1,null,3])"), "1,,3");
    assert_eq!(run_str("[1,null,3]+''"), "1,,3");
}

#[test]
fn number_of_array() {
    // O10: Number(array) coerces via toString.
    assert_eq!(run_str("Number([])"), "0");
    assert_eq!(run_str("Number([5])"), "5");
    assert_eq!(run_str("let x = Number([5]); x === 5"), "true");
    assert_eq!(run_str("Number([1,2])"), "NaN");
}

#[test]
fn bitwise_uses_to_int32_wraparound() {
    // F2: bitwise ops must wrap mod 2^32, not saturate at i32::MAX.
    assert_eq!(run_str("4294967296 | 0"), "0");
    assert_eq!(run_str("4294967295 | 0"), "-1");
    assert_eq!(run_str("2147483648 | 0"), "-2147483648");
    assert_eq!(run_str("3000000000 | 0"), "-1294967296");
    assert_eq!(run_str("0xFFFFFFFF ^ 0"), "-1");
    // Small operands still behave.
    assert_eq!(run_str("5 & 3"), "1");
    assert_eq!(run_str("1 << 4"), "16");
    assert_eq!(run_str("-1 >>> 0"), "4294967295");
}

#[test]
fn number_of_radix_and_infinity_strings() {
    // F5: Number("0x..")/"0b"/"0o"/"Infinity".
    assert_eq!(run_str("Number('0x1F')"), "31");
    assert_eq!(run_str("Number('0b101')"), "5");
    assert_eq!(run_str("Number('0o17')"), "15");
    assert_eq!(run_str("Number('Infinity')"), "Infinity");
    assert_eq!(run_str("+'0x10'"), "16");
}
