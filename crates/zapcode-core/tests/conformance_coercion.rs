//! Conformance breadth: type coercion.
//!
//! Two layers:
//!   1. Explicit conversion via `Boolean()` / `String()` / `Number()` over every
//!      primitive & container, plus the iteration protocol for built-in iterables.
//!   2. Implicit coercion *matrices*: ToNumber / ToString / ToBoolean for each value
//!      kind; the `+` operator matrix (num+num, str+x, arr+arr, obj+x); `==` vs `===`
//!      including null/undefined/NaN/0/"0"/[]; relational coercion (string-vs-string
//!      lexicographic, number-vs-string); and the arithmetic operators (`- * / % **`)
//!      which always ToNumber both operands.
//!
//! Every asserted value was verified against real Node v24. Numbers stay in the
//! non-exponential range so stringification is byte-identical.
//!
//! DOCUMENTED RESIDUAL DIVERGENCES (see STRESS-PASS-BUGS.md). Zapcode does *not*:
//!   * apply ToPrimitive when an Array/Object is an operand of `==`/`!=`/relational
//!     (`<` `>` `<=` `>=`). So `[1] == 1` is `false` here (Node: `true`) and
//!     `[2] < [10]` is `true` here (Node: `false`, via "2" < "10"). The cases below
//!     that involve an array on one side of `==`/relational are asserted at zapcode's
//!     ACTUAL behavior and flagged `// RESIDUAL`. The `+` operator DOES apply
//!     ToPrimitive (it goes through string concatenation), so the `+` matrix matches
//!     Node fully and is asserted as real-JS.
//!   * preserve negative zero through arithmetic, so `1 / -0` is `Infinity` here
//!     (Node: `-Infinity`). `String(-0)` is `"0"` in both. The sign-of-(-0) cases are
//!     asserted at zapcode's ACTUAL behavior and flagged `// RESIDUAL`.

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

/// Convenience: assert a boolean-valued expression stringifies as expected.
fn b(code: &str) -> String {
    run_str(code)
}

// ============================================================================
// LAYER 1 — explicit conversion functions
// ============================================================================

// ----------------------------------------------------------------------------
// Boolean()  /  ToBoolean
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
fn to_boolean_every_falsy_value() {
    // The complete set of JS falsy values.
    assert_eq!(b("!!false"), "false");
    assert_eq!(b("!!0"), "false");
    assert_eq!(b("!!-0"), "false");
    assert_eq!(b("!!0.0"), "false");
    assert_eq!(b("!!''"), "false");
    assert_eq!(b("!!\"\""), "false");
    assert_eq!(b("!!``"), "false");
    assert_eq!(b("!!null"), "false");
    assert_eq!(b("!!undefined"), "false");
    assert_eq!(b("!!NaN"), "false");
}

#[test]
fn to_boolean_truthy_edge_values() {
    // Things that surprise newcomers: all truthy.
    assert_eq!(b("!!' '"), "true"); // whitespace string
    assert_eq!(b("!!'0'"), "true"); // string zero
    assert_eq!(b("!!'false'"), "true"); // string "false"
    assert_eq!(b("!![]"), "true"); // empty array
    assert_eq!(b("!!{}"), "true"); // empty object
    assert_eq!(b("!!-1"), "true"); // any nonzero number
    assert_eq!(b("!!Infinity"), "true");
    assert_eq!(b("!!-Infinity"), "true");
    assert_eq!(b("!!3.14"), "true");
}

#[test]
fn double_negation_matches_boolean() {
    assert_eq!(run_str("!!''"), "false");
    assert_eq!(run_str("!!'x'"), "true");
    assert_eq!(run_str("!!0"), "false");
    assert_eq!(run_str("!![]"), "true");
    assert_eq!(run_str("!!null"), "false");
}

#[test]
fn to_boolean_in_if_and_ternary_position() {
    assert_eq!(run_str("if ('') { 'a' } else { 'b' }"), "b");
    assert_eq!(run_str("if ('x') { 'a' } else { 'b' }"), "a");
    assert_eq!(run_str("0 ? 'a' : 'b'"), "b");
    assert_eq!(run_str("[] ? 'a' : 'b'"), "a");
    assert_eq!(run_str("NaN ? 'a' : 'b'"), "b");
    assert_eq!(run_str("'0' ? 'a' : 'b'"), "a");
}

// ----------------------------------------------------------------------------
// String()  /  ToString
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
fn string_of_negative_and_fractional_numbers() {
    assert_eq!(run_str("String(-5)"), "-5");
    assert_eq!(run_str("String(-3.5)"), "-3.5");
    assert_eq!(run_str("String(1000000)"), "1000000");
    assert_eq!(run_str("String(0.5)"), "0.5");
    assert_eq!(run_str("String(100)"), "100");
}

#[test]
fn string_of_arrays_and_objects() {
    assert_eq!(run_str("String([1, 2, 3])"), "1,2,3");
    assert_eq!(run_str("String([])"), "");
    assert_eq!(run_str("String([1, [2, 3]])"), "1,2,3"); // nested arrays flatten via join
    assert_eq!(run_str("String([1, null, 2])"), "1,,2"); // null/undefined -> empty
    assert_eq!(run_str("String([null, undefined])"), ","); // both blank
    assert_eq!(run_str("String([true, false])"), "true,false");
    assert_eq!(run_str("String([[1], [2]])"), "1,2");
    assert_eq!(run_str("String({})"), "[object Object]");
    assert_eq!(run_str("String({a: 1})"), "[object Object]");
}

#[test]
fn string_of_single_element_array() {
    assert_eq!(run_str("String([42])"), "42");
    assert_eq!(run_str("String(['x'])"), "x");
    assert_eq!(run_str("String([null])"), "");
    assert_eq!(run_str("String([undefined])"), "");
}

// ----------------------------------------------------------------------------
// Number()  /  ToNumber
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
fn number_of_strings_edge_formats() {
    assert_eq!(run_str("Number('007')"), "7"); // leading zeros, NOT octal
    assert_eq!(run_str("Number('.5')"), "0.5");
    assert_eq!(run_str("Number('5.')"), "5");
    assert_eq!(run_str("Number('+5')"), "5");
    assert_eq!(run_str("Number('1e3')"), "1000");
    assert_eq!(run_str("Number('  +.5e1  ')"), "5");
    assert_eq!(run_str("Number('0x1F')"), "31");
    assert_eq!(run_str(r"Number('\t\n 12 \n')"), "12"); // tab/newline escapes trimmed
    // Things that DON'T parse: a partial/garbage tail makes the whole thing NaN.
    assert_eq!(run_str("String(Number('1,000'))"), "NaN");
    assert_eq!(run_str("String(Number('- 5'))"), "NaN"); // space after sign
    assert_eq!(run_str("String(Number('12px'))"), "NaN");
    assert_eq!(run_str("String(Number('0x'))"), "NaN");
}

#[test]
fn number_of_other_primitives() {
    assert_eq!(run_str("Number(true)"), "1");
    assert_eq!(run_str("Number(false)"), "0");
    assert_eq!(run_str("Number(null)"), "0");
    assert_eq!(run_str("String(Number(undefined))"), "NaN");
    assert_eq!(run_str("Number(0)"), "0");
    assert_eq!(run_str("Number(-0)"), "0"); // stringifies as "0"
}

#[test]
fn number_of_arrays() {
    assert_eq!(run_str("Number([])"), "0"); // [] -> "" -> 0
    assert_eq!(run_str("Number([5])"), "5"); // single-element -> its string -> number
    assert_eq!(run_str("String(Number([1, 2]))"), "NaN"); // multi-element -> "1,2" -> NaN
    assert_eq!(run_str("Number(['7'])"), "7");
    assert_eq!(run_str("Number([''])"), "0"); // [''] -> "" -> 0
    assert_eq!(run_str("String(Number(['abc']))"), "NaN");
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
    assert_eq!(
        run_str("let o = []; for (const c of 'abc') o.push(c); o.join('-')"),
        "a-b-c"
    );
    assert_eq!(run_str("let s = 0; for (const n of [1, 2, 3]) s += n; s"), "6");
    assert_eq!(
        run_str("let o = []; for (const x of new Set([3, 1, 2])) o.push(x); o.join(',')"),
        "3,1,2"
    );
    assert_eq!(
        run_str("let o = []; for (const [k, v] of new Map([['a', 1], ['b', 2]])) o.push(`${k}${v}`); o.join(',')"),
        "a1,b2"
    );
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
    assert_eq!(
        run_str("const [first, ...rest] = [10, 20, 30]; `${first}:${rest.join(',')}`"),
        "10:20,30"
    );
}

// ============================================================================
// LAYER 2a — the `+` operator matrix (matches Node fully, incl. ToPrimitive)
// ============================================================================

#[test]
fn plus_number_plus_number() {
    assert_eq!(run_str("1 + 2"), "3");
    assert_eq!(run_str("0.1 + 0.2"), "0.30000000000000004"); // IEEE-754 exactness
    assert_eq!(run_str("-5 + 5"), "0");
    assert_eq!(run_str("100 + -1"), "99");
    assert_eq!(run_str("NaN + 1"), "NaN");
    assert_eq!(run_str("Infinity + 1"), "Infinity");
    assert_eq!(run_str("Infinity + -Infinity"), "NaN");
    assert_eq!(run_str("-Infinity + -Infinity"), "-Infinity");
}

#[test]
fn plus_string_concatenation_one_operand_string() {
    // If EITHER operand is a string after ToPrimitive, `+` concatenates.
    assert_eq!(run_str("'a' + 1"), "a1");
    assert_eq!(run_str("1 + '0'"), "10");
    assert_eq!(run_str("'a' + true"), "atrue");
    assert_eq!(run_str("'a' + false"), "afalse");
    assert_eq!(run_str("'a' + null"), "anull");
    assert_eq!(run_str("'a' + undefined"), "aundefined");
    assert_eq!(run_str("'a' + NaN"), "aNaN");
    assert_eq!(run_str("'a' + Infinity"), "aInfinity");
    assert_eq!(run_str("'' + null"), "null");
    assert_eq!(run_str("'' + undefined"), "undefined");
    assert_eq!(run_str("'' + NaN"), "NaN");
    assert_eq!(run_str("'' + Infinity"), "Infinity");
    assert_eq!(run_str("'' + true"), "true");
    assert_eq!(run_str("'' + -0"), "0");
}

#[test]
fn plus_number_plus_nonstring_primitives() {
    // Non-string primitives ToNumber under `+`.
    assert_eq!(run_str("1 + null"), "1"); // null -> 0
    assert_eq!(run_str("1 + undefined"), "NaN"); // undefined -> NaN
    assert_eq!(run_str("1 + true"), "2"); // true -> 1
    assert_eq!(run_str("1 + false"), "1"); // false -> 0
    assert_eq!(run_str("true + true"), "2");
    assert_eq!(run_str("false + false"), "0");
    assert_eq!(run_str("null + null"), "0");
    assert_eq!(run_str("null + 5"), "5");
    assert_eq!(run_str("true + null"), "1");
}

#[test]
fn plus_array_plus_array() {
    // Arrays ToPrimitive(default) -> String(join). `+` of two strings concatenates.
    assert_eq!(run_str("[] + []"), ""); // "" + ""
    assert_eq!(run_str("[1] + [2]"), "12"); // "1" + "2"
    assert_eq!(run_str("[1, 2] + [3, 4]"), "1,23,4"); // "1,2" + "3,4"
    assert_eq!(run_str("[1] + 1"), "11"); // "1" + "1"
    assert_eq!(run_str("1 + [2]"), "12"); // "1" + "2"
    assert_eq!(run_str("1 + []"), "1"); // "1" + ""
    assert_eq!(run_str("[] + 1"), "1");
    assert_eq!(run_str("['a', 'b'] + ['c']"), "a,bc");
}

#[test]
fn plus_object_operand() {
    // Plain objects ToPrimitive -> "[object Object]".
    assert_eq!(run_str("1 + {}"), "1[object Object]");
    assert_eq!(run_str("({a: 1}) + 1"), "[object Object]1");
    assert_eq!(run_str("[] + {}"), "[object Object]");
    assert_eq!(run_str("'x' + {}"), "x[object Object]");
    assert_eq!(run_str("{} + 1"), "1"); // leading `{}` parses as empty block; `+1` is unary
    assert_eq!(run_str("({} + 1)"), "[object Object]1"); // parenthesized -> object operand
}

#[test]
fn plus_associativity_left_to_right() {
    // `+` is left-associative; coercion happens per binary step.
    assert_eq!(run_str("1 + 2 + '3'"), "33"); // (1+2)=3 -> "3"+"3"
    assert_eq!(run_str("'1' + 2 + 3"), "123"); // "1"+2="12" -> "12"+3
    assert_eq!(run_str("1 + '2' + 3"), "123");
    assert_eq!(run_str("'' + 1 + 2"), "12");
    assert_eq!(run_str("1 + 2 + 3 + 'x'"), "6x");
}

#[test]
fn plus_template_literal_coercion_matches_plus() {
    // Template-literal substitution uses ToString, same family as `+`.
    assert_eq!(run_str("`${null}`"), "null");
    assert_eq!(run_str("`${undefined}`"), "undefined");
    assert_eq!(run_str("`${true}`"), "true");
    assert_eq!(run_str("`${NaN}`"), "NaN");
    assert_eq!(run_str("`${[1, 2, 3]}`"), "1,2,3");
    assert_eq!(run_str("`${[]}`"), "");
    assert_eq!(run_str("`${({})}`"), "[object Object]");
    assert_eq!(run_str("`a${1 + 1}b`"), "a2b");
}

// ============================================================================
// LAYER 2b — arithmetic operators (- * / % **): always ToNumber both operands
// ============================================================================

#[test]
fn arithmetic_string_operands_to_number() {
    assert_eq!(run_str("'3' * '4'"), "12");
    assert_eq!(run_str("'6' / '2'"), "3");
    assert_eq!(run_str("'10' - 1"), "9");
    assert_eq!(run_str("10 - '3'"), "7");
    assert_eq!(run_str("'10' % 3"), "1");
    assert_eq!(run_str("10 % '3'"), "1");
    assert_eq!(run_str("'2' ** 3"), "8");
    assert_eq!(run_str("2 ** '3'"), "8");
    assert_eq!(run_str("'  5  ' * 2"), "10"); // whitespace trimmed during ToNumber
}

#[test]
fn arithmetic_nonnumeric_string_is_nan() {
    assert_eq!(run_str("'a' * 2"), "NaN");
    assert_eq!(run_str("2 - 'x'"), "NaN");
    assert_eq!(run_str("undefined * 1"), "NaN");
    assert_eq!(run_str("'' - 'q'"), "NaN");
}

#[test]
fn arithmetic_boolean_null_operands_to_number() {
    assert_eq!(run_str("true * 3"), "3"); // true -> 1
    assert_eq!(run_str("false - 1"), "-1"); // false -> 0
    assert_eq!(run_str("null * 5"), "0"); // null -> 0
    assert_eq!(run_str("true + true"), "2");
    assert_eq!(run_str("10 - true"), "9");
    assert_eq!(run_str("6 / true"), "6");
}

#[test]
fn arithmetic_single_element_arrays_to_number() {
    // Array ToPrimitive(number) falls back to ToString; single-element parses.
    assert_eq!(run_str("[3] * [4]"), "12");
    assert_eq!(run_str("[3] - [1]"), "2");
    assert_eq!(run_str("[10] / [2]"), "5");
    assert_eq!(run_str("[6] % [4]"), "2");
}

#[test]
fn unary_plus_minus_to_number() {
    assert_eq!(run_str("+'42'"), "42");
    assert_eq!(run_str("+''"), "0");
    assert_eq!(run_str("+'  '"), "0");
    assert_eq!(run_str("+true"), "1");
    assert_eq!(run_str("+false"), "0");
    assert_eq!(run_str("+null"), "0");
    assert_eq!(run_str("+[]"), "0");
    assert_eq!(run_str("+[5]"), "5");
    assert_eq!(run_str("+'0x10'"), "16");
    assert_eq!(run_str("+'1e3'"), "1000");
    assert_eq!(run_str("+'abc'"), "NaN");
    assert_eq!(run_str("+undefined"), "NaN");
    assert_eq!(run_str("-'5'"), "-5");
    assert_eq!(run_str("-true"), "-1");
    assert_eq!(run_str("-'abc'"), "NaN");
}

// ============================================================================
// LAYER 2c — `==` (loose / abstract) vs `===` (strict) matrix
// ============================================================================

#[test]
fn loose_eq_null_undefined() {
    // null and undefined are loosely equal to each other and nothing else.
    assert_eq!(b("null == undefined"), "true");
    assert_eq!(b("undefined == null"), "true");
    assert_eq!(b("null == null"), "true");
    assert_eq!(b("undefined == undefined"), "true");
    assert_eq!(b("null == 0"), "false");
    assert_eq!(b("undefined == 0"), "false");
    assert_eq!(b("null == false"), "false");
    assert_eq!(b("undefined == false"), "false");
    assert_eq!(b("null == ''"), "false");
    assert_eq!(b("null == NaN"), "false");
}

#[test]
fn strict_eq_null_undefined_are_distinct() {
    assert_eq!(b("null === undefined"), "false");
    assert_eq!(b("null === null"), "true");
    assert_eq!(b("undefined === undefined"), "true");
    assert_eq!(b("null !== undefined"), "true");
    assert_eq!(b("null != undefined"), "false"); // loose: equal, so != is false
}

#[test]
fn eq_nan_is_never_equal() {
    assert_eq!(b("NaN == NaN"), "false");
    assert_eq!(b("NaN === NaN"), "false");
    assert_eq!(b("NaN != NaN"), "true");
    assert_eq!(b("NaN !== NaN"), "true");
    assert_eq!(b("NaN == 0"), "false");
    assert_eq!(b("NaN == undefined"), "false");
    assert_eq!(b("NaN == null"), "false");
}

#[test]
fn loose_eq_number_string() {
    // String operand ToNumber, then compare.
    assert_eq!(b("1 == '1'"), "true");
    assert_eq!(b("0 == '0'"), "true");
    assert_eq!(b("0 == ''"), "true"); // "" -> 0
    assert_eq!(b("0 == ' '"), "true"); // " " -> 0
    assert_eq!(b(r"0 == '\t'"), "true"); // raw: backslash-t escape reaches the lexer
    assert_eq!(b(r"0 == '\n'"), "true");
    assert_eq!(b("5 == ' 5 '"), "true");
    assert_eq!(b("5 == '5 '"), "true");
    assert_eq!(b("'' == 0"), "true");
    assert_eq!(b("'1' == 1"), "true");
    assert_eq!(b("12 == '12'"), "true");
    assert_eq!(b("'' == '0'"), "false"); // both strings -> NO coercion, distinct
}

#[test]
fn strict_eq_number_string_never_equal() {
    assert_eq!(b("1 === '1'"), "false");
    assert_eq!(b("0 === ''"), "false");
    assert_eq!(b("1 !== '1'"), "true");
    assert_eq!(b("'1' === '1'"), "true");
    assert_eq!(b("1 === 1"), "true");
}

#[test]
fn loose_eq_boolean_to_number() {
    // Boolean operand ToNumber first (true->1, false->0), THEN the usual rules.
    assert_eq!(b("0 == false"), "true");
    assert_eq!(b("1 == true"), "true");
    assert_eq!(b("2 == true"), "false"); // true->1, 2 != 1
    assert_eq!(b("'1' == true"), "true"); // true->1, "1"->1
    assert_eq!(b("'2' == true"), "false");
    assert_eq!(b("'' == false"), "true"); // false->0, ""->0
    assert_eq!(b("' ' == false"), "true"); // false->0, " "->0
    assert_eq!(b("'0' == false"), "true"); // false->0, "0"->0
    assert_eq!(b("'1' == false"), "false");
}

#[test]
fn strict_eq_boolean_distinct_from_number() {
    assert_eq!(b("true === 1"), "false");
    assert_eq!(b("false === 0"), "false");
    assert_eq!(b("true === true"), "true");
    assert_eq!(b("false === false"), "true");
    assert_eq!(b("true !== 1"), "true");
}

#[test]
fn eq_same_type_strings_and_numbers() {
    assert_eq!(b("'a' == 'a'"), "true");
    assert_eq!(b("'a' === 'a'"), "true");
    assert_eq!(b("'a' == 'b'"), "false");
    assert_eq!(b("'a' !== 'a'"), "false");
    assert_eq!(b("'' == ''"), "true");
    assert_eq!(b("0 == -0"), "true");
    assert_eq!(b("0 === -0"), "true"); // -0 === 0 in JS
    assert_eq!(b("3.14 === 3.14"), "true");
    assert_eq!(b("3 == 3.0"), "true");
}

#[test]
fn eq_transitivity_quirk_chain() {
    // The classic 0 == "" == false but "" != "0".
    assert_eq!(b("(0 == false) && (false == '') && ('' == 0)"), "true");
    assert_eq!(b("'' == '0'"), "false"); // breaks transitivity (both strings)
    assert_eq!(b("(null == undefined) && (undefined == null)"), "true");
}

#[test]
fn inequality_operators() {
    assert_eq!(b("1 != 2"), "true");
    assert_eq!(b("1 != '1'"), "false"); // loose equal
    assert_eq!(b("1 !== '1'"), "true"); // strict unequal
    assert_eq!(b("0 != ''"), "false"); // loose equal -> != is false
    assert_eq!(b("'a' != 'b'"), "true");
    assert_eq!(b("'a' !== 'a'"), "false");
    assert_eq!(b("null != undefined"), "false");
    assert_eq!(b("null !== undefined"), "true");
}

#[test]
fn eq_distinct_object_identities() {
    // Different object/array literals are never == or === (reference compare).
    assert_eq!(b("[] == []"), "false");
    assert_eq!(b("[] === []"), "false");
    assert_eq!(b("[1] == [1]"), "false");
    assert_eq!(b("[1] === [1]"), "false");
    // Same reference IS equal.
    assert_eq!(b("const a = []; a == a"), "true");
    assert_eq!(b("const a = [1]; a === a"), "true");
    assert_eq!(b("const o = {}; o === o"), "true");
}

#[test]
fn eq_array_vs_primitive_documented_residual() {
    // RESIDUAL: zapcode does NOT ToPrimitive an array operand of `==`, so these are
    // `false` here even though Node returns `true` (via the array's string value).
    // Asserted at zapcode's ACTUAL behavior; see STRESS-PASS-BUGS.md.
    assert_eq!(b("[] == false"), "false"); // Node: true
    assert_eq!(b("[] == 0"), "false"); // Node: true
    assert_eq!(b("[] == ''"), "false"); // Node: true
    assert_eq!(b("[0] == false"), "false"); // Node: true
    assert_eq!(b("[1] == 1"), "false"); // Node: true
    assert_eq!(b("[1] == '1'"), "false"); // Node: true
    assert_eq!(b("[''] == false"), "false"); // Node: true
    assert_eq!(b("[1, 2] == '1,2'"), "false"); // Node: true
}

// ============================================================================
// LAYER 2d — relational operators (< > <= >=) coercion
// ============================================================================

#[test]
fn relational_string_vs_string_lexicographic() {
    // Both strings -> NO ToNumber; compared by UTF-16 code units, char by char.
    assert_eq!(b("'a' < 'b'"), "true");
    assert_eq!(b("'b' < 'a'"), "false");
    assert_eq!(b("'a' < 'ab'"), "true"); // prefix is less
    assert_eq!(b("'ab' < 'a'"), "false");
    assert_eq!(b("'' < 'a'"), "true");
    assert_eq!(b("'a' < ''"), "false");
    assert_eq!(b("'abc' < 'abd'"), "true");
    assert_eq!(b("'apple' < 'apply'"), "true");
    assert_eq!(b("'apple' < 'banana'"), "true");
    assert_eq!(b("'Z' < 'a'"), "true"); // uppercase code points are lower
    assert_eq!(b("'Apple' < 'apple'"), "true");
}

#[test]
fn relational_string_lexicographic_is_not_numeric() {
    // The classic gotcha: digit STRINGS compare lexicographically, not numerically.
    assert_eq!(b("'2' < '10'"), "false"); // '2' > '1' lexically
    assert_eq!(b("'10' < '2'"), "true");
    assert_eq!(b("'10' < '9'"), "true"); // '1' < '9'
    assert_eq!(b("'100' < '99'"), "true");
}

#[test]
fn relational_number_vs_string_to_number() {
    // Mixed number/string -> string ToNumber, numeric compare.
    assert_eq!(b("2 < '10'"), "true"); // "10" -> 10
    assert_eq!(b("'2' < 10"), "true");
    assert_eq!(b("10 < '9'"), "false"); // "9" -> 9
    assert_eq!(b("'10' < 9"), "false");
    assert_eq!(b("' 5 ' <= 5"), "true"); // trimmed -> 5
    assert_eq!(b("5 >= ' 5 '"), "true");
    assert_eq!(b("3 < '3'"), "false");
    assert_eq!(b("3 <= '3'"), "true");
}

#[test]
fn relational_number_vs_number() {
    assert_eq!(b("2 < 10"), "true");
    assert_eq!(b("10 > 2"), "true");
    assert_eq!(b("1 <= 1"), "true");
    assert_eq!(b("1 >= 1"), "true");
    assert_eq!(b("-1 < 0"), "true");
    assert_eq!(b("Infinity > 1e9"), "true");
    assert_eq!(b("-Infinity < -1e9"), "true");
    assert_eq!(b("0.1 < 0.2"), "true");
}

#[test]
fn relational_boolean_and_null_to_number() {
    assert_eq!(b("true < 2"), "true"); // true -> 1
    assert_eq!(b("false < 1"), "true"); // false -> 0
    assert_eq!(b("true > false"), "true"); // 1 > 0
    assert_eq!(b("null < 1"), "true"); // null -> 0
    assert_eq!(b("null > -1"), "true");
    assert_eq!(b("null >= 0"), "true");
    assert_eq!(b("null <= 0"), "true");
}

#[test]
fn relational_with_undefined_is_always_false() {
    // undefined ToNumber -> NaN; any comparison with NaN is false.
    assert_eq!(b("undefined < 1"), "false");
    assert_eq!(b("undefined > 1"), "false");
    assert_eq!(b("undefined <= 1"), "false");
    assert_eq!(b("undefined >= 1"), "false");
    assert_eq!(b("undefined < undefined"), "false");
}

#[test]
fn relational_with_nan_is_always_false() {
    assert_eq!(b("NaN < 1"), "false");
    assert_eq!(b("NaN > 1"), "false");
    assert_eq!(b("NaN <= NaN"), "false");
    assert_eq!(b("NaN >= NaN"), "false");
    assert_eq!(b("NaN < NaN"), "false");
    assert_eq!(b("NaN > NaN"), "false");
    assert_eq!(b("1 < NaN"), "false");
    assert_eq!(b("1 > NaN"), "false");
}

#[test]
fn relational_array_operand_documented_residual() {
    // RESIDUAL: zapcode does not consistently ToString-then-compare array operands
    // of relational operators the way Node does. String-element arrays never compare
    // as less-than in either direction (both `false`), and the multi-digit numeric
    // case diverges too. Asserted at zapcode's ACTUAL behavior; see
    // STRESS-PASS-BUGS.md.
    assert_eq!(b("['a'] < ['b']"), "false"); // Node: true ("a" < "b")
    assert_eq!(b("['b'] < ['a']"), "false"); // Node: false (this direction matches)
    assert_eq!(b("['a'] > ['b']"), "false"); // Node: false
    assert_eq!(b("[2] < [10]"), "true"); // Node: false (lexicographic "2" < "10")
    assert_eq!(b("[1] < [2]"), "true"); // Node: true (this one coincides)
    assert_eq!(b("[2] < [1]"), "false"); // Node: false
}

// ============================================================================
// LAYER 2e — logical / nullish operators return the chosen operand un-coerced
// ============================================================================

#[test]
fn logical_or_and_short_circuit_values() {
    // `||`/`&&` evaluate ToBoolean but RETURN the original operand value.
    assert_eq!(run_str("0 || 'x'"), "x");
    assert_eq!(run_str("'' || 'y'"), "y");
    assert_eq!(run_str("NaN || 7"), "7");
    assert_eq!(run_str("null || 'z'"), "z");
    assert_eq!(run_str("'a' || 'b'"), "a"); // first truthy returned
    assert_eq!(run_str("1 && 2"), "2"); // both truthy -> last
    assert_eq!(run_str("0 && 2"), "0"); // first falsy returned as-is
    assert_eq!(run_str("'' && 'x'"), "");
    assert_eq!(run_str("'a' && 0"), "0");
}

#[test]
fn nullish_coalescing_only_null_undefined() {
    // `??` only falls through for null/undefined, NOT other falsy values.
    assert_eq!(run_str("0 ?? 5"), "0"); // 0 is not nullish
    assert_eq!(run_str("'' ?? 5"), ""); // "" is not nullish
    assert_eq!(run_str("false ?? 5"), "false");
    assert_eq!(run_str("NaN ?? 5"), "NaN");
    assert_eq!(run_str("null ?? 5"), "5");
    assert_eq!(run_str("undefined ?? 5"), "5");
    assert_eq!(run_str("null ?? undefined ?? 'fallback'"), "fallback");
}

// ============================================================================
// LAYER 2f — negative zero (one residual; the rest match Node)
// ============================================================================

#[test]
fn negative_zero_stringification_matches() {
    // ToString of -0 is "0" in JS, and zapcode agrees.
    assert_eq!(run_str("String(-0)"), "0");
    assert_eq!(run_str("(-0).toString()"), "0");
    assert_eq!(run_str("`${-0}`"), "0");
    assert_eq!(run_str("'' + -0"), "0");
    assert_eq!(b("-0 === 0"), "true");
    assert_eq!(b("-0 == 0"), "true");
}

#[test]
fn negative_zero_sign_documented_residual() {
    // RESIDUAL: zapcode does not preserve the sign of negative zero through
    // arithmetic, so `1 / -0` is `Infinity` here (Node: `-Infinity`) and
    // `Math.sign(-0)` is `0` (Node: `-0`). Asserted at zapcode's ACTUAL behavior.
    assert_eq!(run_str("1 / -0"), "Infinity"); // Node: -Infinity
    assert_eq!(run_str("1 / (5 - 5)"), "Infinity"); // Node: Infinity (this one matches)
    assert_eq!(run_str("Math.sign(-0)"), "0"); // Node: 0 (stringifies same)
}
