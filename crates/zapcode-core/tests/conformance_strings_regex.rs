//! Conformance breadth: String methods, template literals, and RegExp.
//!
//! All inputs are BMP/ASCII to stay clear of the documented G9 UTF-16-vs-code-point
//! indexing divergence (astral characters only). Covers query/slice/case/pad/trim,
//! split (incl. capture groups & limit), replace with STRING patterns (`$1`,
//! `$<name>`), match/matchAll/exec/test, and template literals. Two documented gaps
//! are pinned to actual behavior: a FUNCTION replacer for `replace`/`replaceAll`
//! is not invoked, and `String.prototype.normalize` is not provided.

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
// Indexing / access (BMP only)
// ----------------------------------------------------------------------------

#[test]
fn length_and_indexing() {
    assert_eq!(run_str("'hello'.length"), "5");
    assert_eq!(run_str("''.length"), "0");
    assert_eq!(run_str("'hello'[0]"), "h");
    assert_eq!(run_str("'hello'[4]"), "o");
    assert_eq!(run_str("String('hello'[9])"), "undefined"); // out of range
    assert_eq!(run_str("'hello'.charAt(1)"), "e");
    assert_eq!(run_str("'hello'.charAt(10)"), ""); // out of range -> ''
    assert_eq!(run_str("'hello'.at(-1)"), "o");
    assert_eq!(run_str("'hello'.at(0)"), "h");
}

#[test]
fn char_code_and_from() {
    assert_eq!(run_str("'ABC'.charCodeAt(0)"), "65");
    assert_eq!(run_str("'ABC'.charCodeAt(2)"), "67");
    assert_eq!(run_str("'ABC'.codePointAt(1)"), "66");
    assert_eq!(run_str("String.fromCharCode(72, 105)"), "Hi");
    assert_eq!(run_str("String.fromCodePoint(65, 66, 67)"), "ABC");
}

// ----------------------------------------------------------------------------
// Case / trim / pad / repeat
// ----------------------------------------------------------------------------

#[test]
fn case_conversion() {
    assert_eq!(run_str("'hello'.toUpperCase()"), "HELLO");
    assert_eq!(run_str("'HeLLo'.toLowerCase()"), "hello");
    assert_eq!(run_str("'Hello World'.toUpperCase()"), "HELLO WORLD");
}

#[test]
fn trim_variants() {
    assert_eq!(run_str("'  hi  '.trim()"), "hi");
    assert_eq!(run_str("'  hi  '.trimStart() + '|'"), "hi  |");
    assert_eq!(run_str("'|' + '  hi  '.trimEnd()"), "|  hi");
    assert_eq!(run_str("'\\t\\n hi \\n'.trim()"), "hi");
}

#[test]
fn pad_and_repeat() {
    assert_eq!(run_str("'5'.padStart(3, '0')"), "005");
    assert_eq!(run_str("'5'.padEnd(3, '-')"), "5--");
    assert_eq!(run_str("'abc'.padStart(2)"), "abc"); // already long enough
    assert_eq!(run_str("'7'.padStart(5, 'ab')"), "abab7"); // pad repeats/truncates
    assert_eq!(run_str("'ab'.repeat(3)"), "ababab");
    assert_eq!(run_str("'x'.repeat(0)"), "");
}

// ----------------------------------------------------------------------------
// Slice / substring / substr
// ----------------------------------------------------------------------------

#[test]
fn slice_substring_substr() {
    assert_eq!(run_str("'hello world'.slice(6)"), "world");
    assert_eq!(run_str("'hello'.slice(1, 3)"), "el");
    assert_eq!(run_str("'hello'.slice(-3, -1)"), "ll"); // negatives from end
    assert_eq!(run_str("'hello'.substring(1, 3)"), "el");
    assert_eq!(run_str("'hello'.substring(3, 1)"), "el"); // substring swaps args
    assert_eq!(run_str("'hello'.substring(-2, 2)"), "he"); // negatives clamp to 0
    assert_eq!(run_str("'hello'.substr(1, 3)"), "ell"); // (start, length)
}

// ----------------------------------------------------------------------------
// Search
// ----------------------------------------------------------------------------

#[test]
fn index_of_and_search() {
    assert_eq!(run_str("'abcabc'.indexOf('bc')"), "1");
    assert_eq!(run_str("'abcabc'.indexOf('bc', 2)"), "4");
    assert_eq!(run_str("'abc'.indexOf('z')"), "-1");
    assert_eq!(run_str("'abcabc'.lastIndexOf('bc')"), "4");
    assert_eq!(run_str("'hello'.includes('ell')"), "true");
    assert_eq!(run_str("'hello'.includes('xyz')"), "false");
}

#[test]
fn starts_ends_with() {
    assert_eq!(run_str("'hello'.startsWith('he')"), "true");
    assert_eq!(run_str("'hello'.startsWith('llo', 2)"), "true"); // with position
    assert_eq!(run_str("'hello'.endsWith('lo')"), "true");
    assert_eq!(run_str("'hello'.endsWith('hel', 3)"), "true"); // endPosition
    assert_eq!(run_str("'hello'.startsWith('lo')"), "false");
}

// ----------------------------------------------------------------------------
// Split
// ----------------------------------------------------------------------------

#[test]
fn split_variants() {
    assert_eq!(run_str("JSON.stringify('a,b,c'.split(','))"), "[\"a\",\"b\",\"c\"]");
    assert_eq!(run_str("JSON.stringify('a,b,c,d'.split(',', 2))"), "[\"a\",\"b\"]"); // limit
    assert_eq!(run_str("'a,b,c'.split(',', 0).length"), "0");
    assert_eq!(run_str("JSON.stringify('abc'.split(''))"), "[\"a\",\"b\",\"c\"]"); // char split
    assert_eq!(run_str("JSON.stringify('no-delim'.split(','))"), "[\"no-delim\"]");
    // split with a capturing-group regex keeps the captures
    assert_eq!(run_str("'a1b2c'.split(/(\\d)/).join('|')"), "a|1|b|2|c");
}

// ----------------------------------------------------------------------------
// Replace (string patterns)
// ----------------------------------------------------------------------------

#[test]
fn replace_with_string_replacement() {
    assert_eq!(run_str("'hello'.replace('l', 'L')"), "heLlo"); // first only
    assert_eq!(run_str("'hello'.replaceAll('l', 'L')"), "heLLo");
    assert_eq!(run_str("'a1b2'.replace(/\\d/g, '#')"), "a#b#"); // global regex
    assert_eq!(run_str("'2020-01'.replace(/(\\d+)-(\\d+)/, '$2/$1')"), "01/2020"); // numbered groups
    assert_eq!(
        run_str("'2020-01'.replace(/(?<y>\\d+)-(?<m>\\d+)/, '$<m>/$<y>')"),
        "01/2020"
    ); // named groups
    assert_eq!(run_str("'abc'.replace(/b/, '[$&]')"), "a[b]c"); // $& whole match
}

#[test]
fn replace_with_function_replacer_documented_divergence() {
    // DIVERGENCE (documented): a FUNCTION replacer is not invoked — the function
    // value is coerced to the string "function" and inserted literally. Only
    // string replacements (with $-substitutions) are supported. Asserting actual.
    assert_eq!(run_str("'a1b2'.replace(/\\d/g, m => '[' + m + ']')"), "afunctionbfunction"); // JS: a[1]b[2]
}

// ----------------------------------------------------------------------------
// RegExp: match / matchAll / exec / test
// ----------------------------------------------------------------------------

#[test]
fn match_global_returns_array_of_strings() {
    assert_eq!(run_str("JSON.stringify('a1b2c3'.match(/\\d/g))"), "[\"1\",\"2\",\"3\"]");
    assert_eq!(run_str("String('abc'.match(/\\d/))"), "null"); // no match -> null
    assert_eq!(run_str("JSON.stringify('cat hat bat'.match(/\\w+at/g))"), "[\"cat\",\"hat\",\"bat\"]");
}

#[test]
fn match_non_global_exposes_groups() {
    // Non-global match result is array-like (documented G4): indexed access works.
    assert_eq!(run_str("'a1'.match(/([a-z])(\\d)/)[0]"), "a1");
    assert_eq!(run_str("'a1'.match(/([a-z])(\\d)/)[1]"), "a");
    assert_eq!(run_str("'a1'.match(/([a-z])(\\d)/)[2]"), "1");
    assert_eq!(run_str("'2020-01'.match(/(?<y>\\d+)-(?<m>\\d+)/).groups.y"), "2020");
    assert_eq!(run_str("'2020-01'.match(/(?<y>\\d+)-(?<m>\\d+)/).groups.m"), "01");
    assert_eq!(run_str("'xy1'.match(/\\d/).index"), "2");
}

#[test]
fn match_all() {
    assert_eq!(
        run_str("JSON.stringify([...'a1b2'.matchAll(/([a-z])(\\d)/g)].map(m => m[1] + m[2]))"),
        "[\"a1\",\"b2\"]"
    );
    assert_eq!(
        run_str("[...'k1 k2 k3'.matchAll(/k(\\d)/g)].map(m => m[1]).join(',')"),
        "1,2,3"
    );
}

#[test]
fn regex_test_and_exec() {
    assert_eq!(run_str("/\\d/.test('abc123')"), "true");
    assert_eq!(run_str("/\\d/.test('abc')"), "false");
    assert_eq!(run_str("/^a.*z$/.test('abcz')"), "true");
    assert_eq!(run_str("JSON.stringify(/(\\w)(\\d)/.exec('a1').slice(0, 3))"), "[\"a1\",\"a\",\"1\"]");
    assert_eq!(run_str("String(/\\d/.exec('abc'))"), "null");
}

#[test]
fn regex_global_exec_loop_advances_lastindex() {
    // A /g regex maintains lastIndex so a classic exec loop terminates.
    assert_eq!(
        run_str("const re = /\\d/g; const s = 'a1b2c3'; let out = []; let m; while ((m = re.exec(s)) !== null) out.push(m[0]); out.join(',')"),
        "1,2,3"
    );
}

#[test]
fn regex_flags_case_insensitive_and_multiline() {
    assert_eq!(run_str("/hello/i.test('HELLO')"), "true");
    assert_eq!(run_str("'Hello'.replace(/hello/i, 'hi')"), "hi");
    assert_eq!(run_str("JSON.stringify('a\\nb\\nc'.match(/^\\w/gm))"), "[\"a\",\"b\",\"c\"]");
}

// ----------------------------------------------------------------------------
// Template literals
// ----------------------------------------------------------------------------

#[test]
fn template_literals() {
    assert_eq!(run_str("const n = 5; `n is ${n * 2}`"), "n is 10");
    assert_eq!(run_str("const a = 'x', b = 'y'; `${a}-${b}`"), "x-y");
    assert_eq!(run_str("`sum: ${1 + 2 + 3}`"), "sum: 6");
    assert_eq!(run_str("const o = {name: 'Ada'}; `Hi ${o.name}!`"), "Hi Ada!");
    // nested template
    assert_eq!(run_str("const x = 2; `outer ${`inner ${x}`}`"), "outer inner 2");
    // cooked escapes
    assert_eq!(run_str("`a\\nb`.length"), "3");
    assert_eq!(run_str("`tab\\there`.includes('\\t')"), "true");
    assert_eq!(run_str("`a\\u0041b`"), "aAb");
}

// ----------------------------------------------------------------------------
// concat / misc
// ----------------------------------------------------------------------------

#[test]
fn concat_and_relational() {
    assert_eq!(run_str("'foo'.concat('bar', 'baz')"), "foobarbaz");
    assert_eq!(run_str("'a'.localeCompare('b')"), "-1");
    assert_eq!(run_str("'b'.localeCompare('a')"), "1");
    assert_eq!(run_str("'a'.localeCompare('a')"), "0");
}
