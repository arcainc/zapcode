//! Conformance suite — `String`, `String.prototype`, and template-literal coercion.
//!
//! Goal: test262-style breadth for the string surface of the zapcode TypeScript
//! subset. Every assertion encodes the CORRECT (real-Node) result UNLESS it sits on
//! a documented residual, in which case it asserts zapcode's ACTUAL behavior with an
//! explicit `DIVERGENCE` comment so the suite stays green and the gap stays visible.
//!
//! Documented residuals exercised here (see STRESS-PASS-BUGS.md):
//!   * G9 — strings are indexed by Unicode *code point*, not UTF-16 code unit. To
//!     stay clear of it every input is BMP/ASCII, so `length`/index/`charCodeAt`/
//!     `slice`/`match.index` all agree with JS for the text used here.
//!   * Function replacers for `replace`/`replaceAll` are NOT invoked — the function
//!     value is string-coerced to "function" and inserted literally. Pinned below.
//!   * A handful of small `replace`/`includes`/`at`/`lastIndexOf` argument-edge
//!     divergences, each pinned to actual behavior with a `DIVERGENCE` comment.
//!   * Tagged templates (`String.raw\`…\``) are not provided — exercised as
//!     "unsupported" so a future fix flips a known check. `String.prototype.normalize`
//!     is now implemented (NFC/NFD/NFKC/NFKD).

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, ZapcodeRun};

/// Run `code` and stringify the completion value via the heap, as the stress suites do.
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

/// Run `code` expecting a runtime/parse error; return its `Debug` string. Used to pin
/// "feature is not provided" residuals so a future implementation makes them fail loudly.
fn run_err(code: &str) -> String {
    match ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        Vec::new(),
        ResourceLimits::default(),
    ) {
        Ok(run) => match run.run(Vec::new()) {
            Ok(result) => panic!(
                "expected an error for `{code}`, got completion {:?}",
                match result.state {
                    VmState::Complete(v) => v.to_js_string(&result.heap),
                    other => format!("{other:?}"),
                }
            ),
            Err(e) => format!("{e:?}"),
        },
        Err(e) => format!("{e:?}"),
    }
}

// ============================================================================
// length / indexing / character access  (BMP-only per G9)
// ============================================================================

#[test]
fn length_basics() {
    assert_eq!(run_str("'hello'.length"), "5");
    assert_eq!(run_str("''.length"), "0");
    assert_eq!(run_str("'a b c'.length"), "5");
    assert_eq!(run_str("'\\t\\n'.length"), "2");
    assert_eq!(run_str("'aA1!_'.length"), "5");
}

#[test]
fn bracket_indexing() {
    assert_eq!(run_str("'hello'[0]"), "h");
    assert_eq!(run_str("'hello'[4]"), "o");
    assert_eq!(run_str("String('hello'[5])"), "undefined");
    assert_eq!(run_str("String('hello'[-1])"), "undefined");
    // DIVERGENCE: a STRING-typed index ('1') is not coerced to a numeric character
    // index; only numeric subscripts read a character. Asserting ACTUAL behavior.
    assert_eq!(run_str("String('hello'['1'])"), "undefined"); // JS: "e"
}

#[test]
fn char_at() {
    assert_eq!(run_str("'hello'.charAt(0)"), "h");
    assert_eq!(run_str("'hello'.charAt(4)"), "o");
    assert_eq!(run_str("'hello'.charAt()"), "h"); // default 0
    assert_eq!(run_str("'hello'.charAt(10)"), ""); // out of range -> ''
    assert_eq!(run_str("'abc'.charAt(-1)"), ""); // negative -> ''
}

#[test]
fn at_method() {
    assert_eq!(run_str("'hello'.at(0)"), "h");
    assert_eq!(run_str("'hello'.at(4)"), "o");
    assert_eq!(run_str("'hello'.at(-1)"), "o");
    assert_eq!(run_str("'hello'.at(-2)"), "l");
    assert_eq!(run_str("String('hello'.at(10))"), "undefined"); // forward OOB -> undefined
    // DIVERGENCE: a negative index beyond the start should be `undefined` in JS,
    // but zapcode clamps and returns the first char. Asserting ACTUAL behavior.
    assert_eq!(run_str("'abc'.at(-10)"), "a"); // JS: undefined
}

#[test]
fn char_code_at() {
    assert_eq!(run_str("'ABC'.charCodeAt(0)"), "65");
    assert_eq!(run_str("'ABC'.charCodeAt(2)"), "67");
    assert_eq!(run_str("'ABC'.charCodeAt()"), "65"); // default 0
    assert_eq!(run_str("'abc'.charCodeAt(10)"), "NaN"); // OOB -> NaN
    assert_eq!(run_str("'0'.charCodeAt(0)"), "48");
}

#[test]
fn code_point_at() {
    assert_eq!(run_str("'ABC'.codePointAt(0)"), "65");
    assert_eq!(run_str("'ABC'.codePointAt(1)"), "66");
    assert_eq!(run_str("'a'.codePointAt(0)"), "97");
    assert_eq!(run_str("String('abc'.codePointAt(10))"), "undefined"); // OOB -> undefined
}

// ============================================================================
// String.fromCharCode / String.fromCodePoint
// ============================================================================

#[test]
fn from_char_code() {
    assert_eq!(run_str("String.fromCharCode(72, 105)"), "Hi");
    assert_eq!(run_str("String.fromCharCode(65)"), "A");
    assert_eq!(run_str("JSON.stringify(String.fromCharCode())"), "\"\""); // no args -> ''
    assert_eq!(run_str("String.fromCharCode(65, 0x2042)"), "A\u{2042}"); // BMP code unit
}

#[test]
fn from_code_point() {
    assert_eq!(run_str("String.fromCodePoint(65, 66, 67)"), "ABC");
    assert_eq!(run_str("String.fromCodePoint(97, 98)"), "ab");
    assert_eq!(run_str("String.fromCodePoint(0x48, 0x69)"), "Hi");
    assert_eq!(run_str("JSON.stringify(String.fromCodePoint())"), "\"\"");
}

// ============================================================================
// slice / substring / substr
// ============================================================================

#[test]
fn slice_basics() {
    assert_eq!(run_str("'hello world'.slice(6)"), "world");
    assert_eq!(run_str("'hello'.slice(1, 3)"), "el");
    assert_eq!(run_str("'hello'.slice(0)"), "hello");
    assert_eq!(run_str("'hello'.slice(0, 100)"), "hello"); // end clamps to length
    assert_eq!(run_str("JSON.stringify('hello'.slice(3, 1))"), "\"\""); // start>end -> ''
}

#[test]
fn slice_negative_offsets() {
    assert_eq!(run_str("'hello'.slice(-3)"), "llo");
    assert_eq!(run_str("'hello'.slice(-3, -1)"), "ll");
    assert_eq!(run_str("'hello'.slice(-100)"), "hello"); // clamps to 0
    assert_eq!(run_str("'hello'.slice(1, -1)"), "ell");
}

#[test]
fn substring_basics() {
    assert_eq!(run_str("'hello'.substring(1, 3)"), "el");
    assert_eq!(run_str("'hello'.substring(3)"), "lo");
    assert_eq!(run_str("'hello'.substring(3, 1)"), "el"); // swaps args
    assert_eq!(run_str("'hello'.substring(-5)"), "hello"); // negatives -> 0
    assert_eq!(run_str("'hello'.substring(-2, 2)"), "he"); // negative clamps to 0
}

#[test]
fn substr_basics() {
    assert_eq!(run_str("'hello'.substr(1, 3)"), "ell"); // (start, length)
    assert_eq!(run_str("'hello'.substr(2)"), "llo"); // to end
    assert_eq!(run_str("'hello'.substr(-2)"), "lo"); // negative start from end
    assert_eq!(run_str("'hello'.substr(-100, 2)"), "he"); // clamps start to 0
    assert_eq!(run_str("JSON.stringify('hello'.substr(1, 0))"), "\"\""); // zero length
}

// ============================================================================
// indexOf / lastIndexOf / includes / startsWith / endsWith / search
// ============================================================================

#[test]
fn index_of() {
    assert_eq!(run_str("'abcabc'.indexOf('bc')"), "1");
    assert_eq!(run_str("'abcabc'.indexOf('bc', 2)"), "4"); // from position
    assert_eq!(run_str("'abc'.indexOf('z')"), "-1");
    assert_eq!(run_str("'abc'.indexOf('')"), "0"); // empty match at start
    assert_eq!(run_str("'abc'.indexOf('a', 1)"), "-1"); // start past it
}

#[test]
fn last_index_of() {
    assert_eq!(run_str("'abcabc'.lastIndexOf('bc')"), "4");
    assert_eq!(run_str("'abc'.lastIndexOf('z')"), "-1");
    assert_eq!(run_str("'abc'.lastIndexOf('')"), "3"); // empty -> length
    assert_eq!(run_str("'aXaXa'.lastIndexOf('a')"), "4");
    // DIVERGENCE: with a `fromIndex` the JS semantics search backward from that
    // index ("abcabc".lastIndexOf("bc",3) === 1); zapcode reports the last overall
    // occurrence instead. Asserting ACTUAL behavior.
    assert_eq!(run_str("'abcabc'.lastIndexOf('bc', 3)"), "4"); // JS: 1
}

#[test]
fn includes() {
    assert_eq!(run_str("'hello'.includes('ell')"), "true");
    assert_eq!(run_str("'hello'.includes('xyz')"), "false");
    assert_eq!(run_str("'hello'.includes('')"), "true"); // empty always present
    assert_eq!(run_str("'hello'.includes('h')"), "true");
    // DIVERGENCE: a search-from `position` argument is not honored, so a needle
    // before `position` is still found. Asserting ACTUAL behavior.
    assert_eq!(run_str("'hello'.includes('he', 1)"), "true"); // JS: false
}

#[test]
fn starts_with() {
    assert_eq!(run_str("'hello'.startsWith('he')"), "true");
    assert_eq!(run_str("'hello'.startsWith('lo')"), "false");
    assert_eq!(run_str("'hello'.startsWith('')"), "true");
    assert_eq!(run_str("'hello'.startsWith('llo', 2)"), "true"); // with position
    assert_eq!(run_str("'hello'.startsWith('he', 1)"), "false");
}

#[test]
fn ends_with() {
    assert_eq!(run_str("'hello'.endsWith('lo')"), "true");
    assert_eq!(run_str("'hello'.endsWith('he')"), "false");
    assert_eq!(run_str("'hello'.endsWith('')"), "true");
    assert_eq!(run_str("'hello'.endsWith('hel', 3)"), "true"); // endPosition
    assert_eq!(run_str("'hello'.endsWith('hello')"), "true");
}

#[test]
fn search_method() {
    assert_eq!(run_str("'hello'.search(/l/)"), "2");
    assert_eq!(run_str("'hello'.search(/z/)"), "-1");
    assert_eq!(run_str("'abc123'.search(/\\d/)"), "3");
    assert_eq!(run_str("'abc'.search(/^a/)"), "0");
}

// ============================================================================
// padStart / padEnd / repeat
// ============================================================================

#[test]
fn pad_start() {
    assert_eq!(run_str("'5'.padStart(3, '0')"), "005");
    assert_eq!(run_str("'5'.padStart(4)"), "   5"); // default space fill
    assert_eq!(run_str("'abc'.padStart(2)"), "abc"); // already long enough
    assert_eq!(run_str("'ab'.padStart(4, 'xy')"), "xyab"); // exact fill
    assert_eq!(run_str("'ab'.padStart(7, '123')"), "12312ab"); // fill repeats+truncates
    assert_eq!(run_str("'x'.padStart(5, '').length"), "1"); // empty fill: no padding
}

#[test]
fn pad_end() {
    assert_eq!(run_str("'5'.padEnd(3, '-')"), "5--");
    assert_eq!(run_str("'5'.padEnd(4)"), "5   ");
    assert_eq!(run_str("'abc'.padEnd(2)"), "abc");
    assert_eq!(run_str("'7'.padEnd(5, 'ab')"), "7abab"); // fill repeats+truncates
}

#[test]
fn repeat() {
    assert_eq!(run_str("'ab'.repeat(3)"), "ababab");
    assert_eq!(run_str("'x'.repeat(1)"), "x");
    assert_eq!(run_str("JSON.stringify('x'.repeat(0))"), "\"\""); // zero -> ''
    assert_eq!(run_str("'-'.repeat(5)"), "-----");
}

// ============================================================================
// trim / trimStart / trimEnd
// ============================================================================

#[test]
fn trim_variants() {
    assert_eq!(run_str("'  hi  '.trim()"), "hi");
    assert_eq!(run_str("'\\t\\n hi \\n'.trim()"), "hi");
    assert_eq!(run_str("'noedges'.trim()"), "noedges");
    assert_eq!(run_str("JSON.stringify('   '.trim())"), "\"\"");
    // Unicode whitespace (NBSP) is trimmed too.
    assert_eq!(run_str("'\\u00a0hi\\u00a0'.trim().length"), "2");
}

#[test]
fn trim_start_end() {
    assert_eq!(run_str("'  hi  '.trimStart() + '|'"), "hi  |");
    assert_eq!(run_str("'|' + '  hi  '.trimEnd()"), "|  hi");
    assert_eq!(run_str("'xx  '.trimStart()"), "xx  "); // nothing to trim on left
    assert_eq!(run_str("'  xx'.trimEnd()"), "  xx"); // nothing to trim on right
}

// ============================================================================
// toUpperCase / toLowerCase
// ============================================================================

#[test]
fn case_conversion() {
    assert_eq!(run_str("'hello'.toUpperCase()"), "HELLO");
    assert_eq!(run_str("'HeLLo'.toLowerCase()"), "hello");
    assert_eq!(run_str("'Hello World 123'.toUpperCase()"), "HELLO WORLD 123");
    assert_eq!(run_str("'MiXeD'.toLowerCase()"), "mixed");
    assert_eq!(run_str("'already upper'.toUpperCase()"), "ALREADY UPPER");
    assert_eq!(run_str("''.toUpperCase()"), "");
}

// ============================================================================
// concat / localeCompare
// ============================================================================

#[test]
fn concat() {
    assert_eq!(run_str("'foo'.concat('bar', 'baz')"), "foobarbaz");
    assert_eq!(run_str("'a'.concat('b')"), "ab");
    assert_eq!(run_str("'x'.concat()"), "x"); // no args
    assert_eq!(run_str("'x'.concat(1, 2)"), "x12"); // number args coerced
    assert_eq!(run_str("''.concat('only')"), "only");
}

#[test]
fn locale_compare() {
    assert_eq!(run_str("'a'.localeCompare('a')"), "0");
    assert_eq!(run_str("'a'.localeCompare('b')"), "-1");
    assert_eq!(run_str("'b'.localeCompare('a')"), "1");
    assert_eq!(run_str("'apple'.localeCompare('banana')"), "-1");
    assert_eq!(run_str("'z'.localeCompare('a')"), "1");
}

// ============================================================================
// split — string sep, char split, limit, regex, capture groups
// ============================================================================

#[test]
fn split_string_separator() {
    assert_eq!(run_str("JSON.stringify('a,b,c'.split(','))"), "[\"a\",\"b\",\"c\"]");
    assert_eq!(run_str("JSON.stringify('no-delim'.split(','))"), "[\"no-delim\"]");
    assert_eq!(run_str("JSON.stringify('a::b'.split('::'))"), "[\"a\",\"b\"]");
    assert_eq!(run_str("JSON.stringify('a,b,'.split(','))"), "[\"a\",\"b\",\"\"]"); // trailing empty
}

#[test]
fn split_char_and_default() {
    assert_eq!(run_str("JSON.stringify('abc'.split(''))"), "[\"a\",\"b\",\"c\"]"); // char split
    // DIVERGENCE: a missing separator argument is treated like the empty string
    // (char split) rather than returning the whole string in a single-element array.
    // Asserting ACTUAL behavior. (`split(undefined)` below DOES match JS.)
    assert_eq!(run_str("JSON.stringify('abc'.split())"), "[\"a\",\"b\",\"c\"]"); // JS: ["abc"]
    assert_eq!(run_str("JSON.stringify('abc'.split(undefined))"), "[\"abc\"]");
}

#[test]
fn split_limit() {
    assert_eq!(run_str("JSON.stringify('a,b,c,d'.split(',', 2))"), "[\"a\",\"b\"]");
    assert_eq!(run_str("'a,b,c'.split(',', 0).length"), "0"); // limit 0 -> empty
    assert_eq!(run_str("JSON.stringify('a,b'.split(',', 10))"), "[\"a\",\"b\"]"); // limit > parts
    assert_eq!(run_str("'a,b,c'.split(',', 1).join('|')"), "a");
}

#[test]
fn split_regex_and_groups() {
    // Trailing digit produces a trailing empty segment, exactly like JS:
    // "a1b2c3".split(/\d/) === ["a","b","c",""].
    assert_eq!(run_str("'a1b2c3'.split(/\\d/).join('|')"), "a|b|c|"); // regex sep dropped
    // A capturing-group regex KEEPS the captured separators interleaved.
    assert_eq!(run_str("'a1b2c'.split(/(\\d)/).join('|')"), "a|1|b|2|c");
    assert_eq!(run_str("JSON.stringify('a1b2c'.split(/(\\d)/))"), "[\"a\",\"1\",\"b\",\"2\",\"c\"]");
    // limit applies after group interleaving
    assert_eq!(run_str("'a1b2c'.split(/(\\d)/, 3).join('|')"), "a|1|b");
    assert_eq!(run_str("'a1b2c3'.split(/\\d/, 2).join('|')"), "a|b");
}

// ============================================================================
// replace — string pattern, regex, $-substitution patterns
// ============================================================================

#[test]
fn replace_string_pattern() {
    assert_eq!(run_str("'hello'.replace('l', 'L')"), "heLlo"); // first only
    assert_eq!(run_str("'a.b.c'.replace('.', '-')"), "a-b.c"); // literal dot, first only
    assert_eq!(run_str("'abc'.replace('z', '!')"), "abc"); // no match -> unchanged
    assert_eq!(run_str("'abc'.replace('', '>')"), ">abc"); // empty matches at start
}

#[test]
fn replace_regex_global() {
    assert_eq!(run_str("'a1b2'.replace(/\\d/g, '#')"), "a#b#");
    assert_eq!(run_str("'aaa'.replace(/a/g, 'b')"), "bbb");
    assert_eq!(run_str("'a1b2'.replace(/\\d/, '#')"), "a#b2"); // non-global: first only
    assert_eq!(run_str("'Hello'.replace(/hello/i, 'hi')"), "hi"); // case-insensitive flag
}

#[test]
fn replace_dollar_patterns() {
    assert_eq!(run_str("'abc'.replace(/b/, '[$&]')"), "a[b]c"); // $& whole match
    assert_eq!(run_str("'2020-01'.replace(/(\\d+)-(\\d+)/, '$2/$1')"), "01/2020"); // numbered
    assert_eq!(
        run_str("'2020-01'.replace(/(?<y>\\d+)-(?<m>\\d+)/, '$<m>/$<y>')"),
        "01/2020"
    ); // named
    assert_eq!(run_str("'a'.replace(/(?<x>a)/, '$<y>')"), ""); // missing named -> empty
}

#[test]
fn replace_dollar_edge_divergences() {
    // DIVERGENCE: `$$` should yield a single literal `$` in JS; zapcode leaves it as
    // `$$`. Asserting ACTUAL behavior.
    assert_eq!(run_str("'ab'.replace('a', '$$')"), "$$b"); // JS: "$b"
    // DIVERGENCE: `$1` with no corresponding capture group is left LITERAL in JS
    // ("a$1c"); zapcode drops it. Asserting ACTUAL behavior.
    assert_eq!(run_str("'abc'.replace(/b/, '$1')"), "ac"); // JS: "a$1c"
    // DIVERGENCE: `` $` `` (prefix) and `$'` (suffix) substitutions are not
    // implemented; the tokens are inserted literally. Asserting ACTUAL behavior.
    assert_eq!(run_str("'abc'.replace('b', '$`')"), "a$`c"); // JS: "aac"
    assert_eq!(run_str("'abc'.replace('b', \"$'\")"), "a$'c"); // JS: "acc"
}

#[test]
fn replace_function_replacer() {
    // A FUNCTION replacer is invoked per match with (match, ...groups, offset,
    // string); its return value is string-coerced and substituted.
    assert_eq!(
        run_str("'a1b2'.replace(/\\d/g, m => '[' + m + ']')"),
        "a[1]b[2]"
    );
    assert_eq!(
        run_str("'hello'.replace('l', m => m.toUpperCase())"),
        "heLlo"
    );
    assert_eq!(
        run_str("'abcabc'.replace(/b/g, (m, off) => off)"),
        "a1ca4c"
    );
}

// ============================================================================
// replaceAll — string + regex (must be /g) + same $-patterns
// ============================================================================

#[test]
fn replace_all_string() {
    assert_eq!(run_str("'hello'.replaceAll('l', 'L')"), "heLLo");
    assert_eq!(run_str("'a.b.c'.replaceAll('.', '-')"), "a-b-c");
    assert_eq!(run_str("'aaa'.replaceAll('a', 'bb')"), "bbbbbb");
    assert_eq!(run_str("'abc'.replaceAll('z', '!')"), "abc"); // no match
}

#[test]
fn replace_all_regex_and_patterns() {
    assert_eq!(run_str("'a1b2'.replaceAll(/\\d/g, '#')"), "a#b#");
    assert_eq!(run_str("'x1x2x3'.replaceAll(/x(\\d)/g, '[$1]')"), "[1][2][3]");
    // A function replacer is invoked per match here too.
    assert_eq!(
        run_str("'a1b2'.replaceAll(/\\d/g, m => '<' + m + '>')"),
        "a<1>b<2>"
    );
}

// ============================================================================
// match — global (array of strings) vs non-global (array-like with .index/.groups)
// ============================================================================

#[test]
fn match_global_array_of_strings() {
    assert_eq!(run_str("JSON.stringify('a1b2c3'.match(/\\d/g))"), "[\"1\",\"2\",\"3\"]");
    assert_eq!(
        run_str("JSON.stringify('cat hat bat'.match(/\\w+at/g))"),
        "[\"cat\",\"hat\",\"bat\"]"
    );
    assert_eq!(run_str("String('abc'.match(/\\d/g))"), "null"); // no match -> null
    assert_eq!(run_str("Array.isArray('a1b2'.match(/\\d/g))"), "true"); // real Array
}

#[test]
fn match_non_global_result() {
    // Non-global match result is an array-like heap object (documented G4): indexed
    // access, .length, .index, and .groups all work; only the brand differs.
    assert_eq!(run_str("'a1'.match(/([a-z])(\\d)/)[0]"), "a1"); // whole match
    assert_eq!(run_str("'a1'.match(/([a-z])(\\d)/)[1]"), "a"); // group 1
    assert_eq!(run_str("'a1'.match(/([a-z])(\\d)/)[2]"), "1"); // group 2
    assert_eq!(run_str("'a1b'.match(/(\\d)/).length"), "2"); // match + 1 group
    assert_eq!(run_str("'xy1'.match(/\\d/).index"), "2"); // match position
    assert_eq!(run_str("String('abc'.match(/\\d/))"), "null"); // no match -> null
    // Documented G4 residual: non-global result is NOT branded as a real Array.
    assert_eq!(run_str("Array.isArray('a1'.match(/\\d/))"), "false"); // JS: true
}

#[test]
fn match_named_groups() {
    assert_eq!(run_str("'2020-01'.match(/(?<y>\\d+)-(?<m>\\d+)/).groups.y"), "2020");
    assert_eq!(run_str("'2020-01'.match(/(?<y>\\d+)-(?<m>\\d+)/).groups.m"), "01");
}

// ============================================================================
// matchAll — iterator of array-like results with groups/index/input
// ============================================================================

#[test]
fn match_all_basics() {
    assert_eq!(
        run_str("[...'a1b2'.matchAll(/([a-z])(\\d)/g)].map(m => m[1] + m[2]).join('|')"),
        "a1|b2"
    );
    assert_eq!(
        run_str("[...'k1 k2 k3'.matchAll(/k(\\d)/g)].map(m => m[1]).join(',')"),
        "1,2,3"
    );
    assert_eq!(run_str("[...'k1 k2 k3'.matchAll(/k(\\d)/g)].length"), "3");
}

#[test]
fn match_all_index_input_groups() {
    assert_eq!(run_str("[...'a1b2'.matchAll(/\\d/g)].map(m => m.index).join(',')"), "1,3");
    assert_eq!(run_str("[...'a1'.matchAll(/(\\d)/g)][0].input"), "a1");
    assert_eq!(run_str("[...'a1'.matchAll(/(\\d)/g)][0].length"), "2"); // match + group
    assert_eq!(
        run_str("[...'2020-01'.matchAll(/(?<y>\\d+)-(?<m>\\d+)/g)].map(m => m.groups.y + ':' + m.groups.m).join(',')"),
        "2020:01"
    );
}

// ============================================================================
// RegExp surface used by the string methods: test / exec / flags / lastIndex
// ============================================================================

#[test]
fn regex_test() {
    assert_eq!(run_str("/\\d/.test('abc123')"), "true");
    assert_eq!(run_str("/\\d/.test('abc')"), "false");
    assert_eq!(run_str("/^a.*z$/.test('abcz')"), "true");
    assert_eq!(run_str("/a.c/.test('axc')"), "true");
    assert_eq!(run_str("/hello/i.test('HELLO')"), "true"); // case-insensitive flag
}

#[test]
fn regex_exec() {
    // The `exec` result is an array-LIKE object (it carries the extra `.index` /
    // `.input` / `.groups` props that a Vec-backed heap array can't hold — the same
    // documented trade-off as `match()`), so read groups by index, not `.slice`.
    assert_eq!(
        run_str("const m=/(\\w)(\\d)/.exec('a1'); JSON.stringify([m[0], m[1], m[2]])"),
        "[\"a1\",\"a\",\"1\"]"
    );
    assert_eq!(run_str("String(/\\d/.exec('abc'))"), "null"); // no match -> null
    assert_eq!(run_str("/(\\d+)/.exec('xy42')[0]"), "42"); // group 0 = whole match
    assert_eq!(run_str("/(\\d+)/.exec('xy42')[1]"), "42"); // group 1
    // The `exec` result carries `.index` (match start in chars), like JS.
    assert_eq!(run_str("String(/(\\d+)/.exec('xy42').index)"), "2");
}

#[test]
fn regex_global_exec_loop() {
    // A /g regex maintains lastIndex so a classic exec loop terminates correctly.
    assert_eq!(
        run_str("const re = /\\d/g; const s = 'a1b2c3'; let out = []; let m; while ((m = re.exec(s)) !== null) out.push(m[0]); out.join(',')"),
        "1,2,3"
    );
    assert_eq!(run_str("const r = /\\d/g; r.exec('a1b2'); r.lastIndex"), "2");
}

#[test]
fn regex_flags_in_methods() {
    assert_eq!(run_str("'Hello'.replace(/hello/i, 'hi')"), "hi");
    assert_eq!(run_str("JSON.stringify('a\\nb\\nc'.match(/^\\w/gm))"), "[\"a\",\"b\",\"c\"]"); // multiline
    assert_eq!(run_str("'AbAb'.replace(/a/gi, 'x')"), "xbxb"); // global+insensitive
}

// ============================================================================
// String() coercion of every primitive / reference kind
// ============================================================================

#[test]
fn string_of_primitives() {
    assert_eq!(run_str("String(null)"), "null");
    assert_eq!(run_str("String(undefined)"), "undefined");
    assert_eq!(run_str("String(true)"), "true");
    assert_eq!(run_str("String(false)"), "false");
    assert_eq!(run_str("String('x')"), "x");
}

#[test]
fn string_of_numbers() {
    assert_eq!(run_str("String(0)"), "0");
    assert_eq!(run_str("String(-0)"), "0"); // negative zero stringifies to "0"
    assert_eq!(run_str("String(0.5)"), "0.5");
    assert_eq!(run_str("String(42)"), "42");
    assert_eq!(run_str("String(-7)"), "-7");
    assert_eq!(run_str("String(NaN)"), "NaN");
    assert_eq!(run_str("String(Infinity)"), "Infinity");
    assert_eq!(run_str("String(-Infinity)"), "-Infinity");
    // DIVERGENCE: very large integers are printed in full positional notation; JS
    // switches to exponential at 1e21 (String(1e21) === "1e+21"). Asserting ACTUAL.
    assert_eq!(run_str("String(1e21)"), "1000000000000000000000"); // JS: "1e+21"
}

#[test]
fn string_of_references() {
    assert_eq!(run_str("String([1, 2, 3])"), "1,2,3"); // arrays join with comma
    assert_eq!(run_str("String([1, [2, 3], 4])"), "1,2,3,4"); // nested flattens via toString
    assert_eq!(run_str("String([])"), ""); // empty array -> ''
    assert_eq!(run_str("String([null, undefined, 1])"), ",,1"); // null/undef -> empty slots
    assert_eq!(run_str("String({a: 1})"), "[object Object]"); // plain object
}

// ============================================================================
// Template-literal coercion of interpolated values
// ============================================================================

#[test]
fn template_basic_interpolation() {
    assert_eq!(run_str("const n = 5; `n is ${n * 2}`"), "n is 10");
    assert_eq!(run_str("const a = 'x', b = 'y'; `${a}-${b}`"), "x-y");
    assert_eq!(run_str("`sum: ${1 + 2 + 3}`"), "sum: 6");
    assert_eq!(run_str("`${1}${2}${3}`"), "123");
    assert_eq!(run_str("const o = {name: 'Ada'}; `Hi ${o.name}!`"), "Hi Ada!");
}

#[test]
fn template_coercion_of_values() {
    assert_eq!(run_str("`${null}`"), "null");
    assert_eq!(run_str("`${undefined}`"), "undefined");
    assert_eq!(run_str("`${true}`"), "true");
    assert_eq!(run_str("`${[1, 2, 3]}`"), "1,2,3"); // array -> comma join
    assert_eq!(run_str("`${{a: 1}}`"), "[object Object]"); // object -> [object Object]
    assert_eq!(run_str("`${NaN}`"), "NaN");
    assert_eq!(run_str("`${-0}`"), "0");
}

#[test]
fn template_nesting_and_escapes() {
    assert_eq!(run_str("const x = 2; `outer ${`inner ${x}`}`"), "outer inner 2"); // nested
    assert_eq!(run_str("`a\\nb`.length"), "3"); // cooked newline
    assert_eq!(run_str("`a\\tb`.length"), "3"); // cooked tab
    assert_eq!(run_str("`a\\\\b`.length"), "3"); // cooked backslash
    assert_eq!(run_str("`a\\u0041b`"), "aAb"); // cooked unicode escape
    assert_eq!(run_str("`tab\\there`.includes('\\t')"), "true");
    assert_eq!(run_str("const x = 2; `v=${x}\\n`.length"), "4");
}

// ============================================================================
// Unsupported-feature residuals — pinned so a future implementation flips them
// ============================================================================

#[test]
fn normalize_returns_normalized_string() {
    // String.prototype.normalize returns the (already-NFC) string unchanged.
    assert_eq!(run_str("'abc'.normalize()"), "abc");
    // A decomposed sequence composes under the default NFC form.
    assert_eq!(run_str("'cafe\\u0301'.normalize('NFC') === 'café'"), "true");
}

#[test]
fn tagged_template_is_unsupported_residual() {
    // DIVERGENCE: tagged template expressions (incl. String.raw`…`) are not
    // supported and surface as UnsupportedSyntax at compile time.
    assert!(
        run_err("String.raw`a\\nb`").contains("tagged template"),
        "expected an unsupported-tagged-template error"
    );
}

// ============================================================================
// Method chaining / realistic compositions (integration breadth)
// ============================================================================

#[test]
fn method_chaining() {
    assert_eq!(run_str("'  Hello World  '.trim().toLowerCase()"), "hello world");
    assert_eq!(run_str("'a,b,c'.split(',').map(s => s.toUpperCase()).join('-')"), "A-B-C");
    assert_eq!(run_str("'one two three'.split(' ').length"), "3");
    assert_eq!(run_str("'  x  '.trim().padStart(3, '.')"), "..x");
    assert_eq!(run_str("'Hello'.slice(0, 1).toLowerCase() + 'Hello'.slice(1)"), "hello");
    assert_eq!(
        run_str("'2020-01-15'.split('-').map(s => Number(s)).reduce((a, b) => a + b, 0)"),
        "2036"
    );
}

#[test]
fn realistic_string_processing() {
    // Word count via split.
    assert_eq!(run_str("'the quick brown fox'.split(' ').length"), "4");
    // CSV-ish reshaping.
    assert_eq!(
        run_str("'k1=v1;k2=v2'.split(';').map(p => p.split('=')[1]).join(',')"),
        "v1,v2"
    );
    // Replace all digits then collapse.
    assert_eq!(run_str("'a1b2c3'.replace(/\\d/g, '').toUpperCase()"), "ABC");
    // Title-case a single word.
    assert_eq!(
        run_str("const w = 'hello'; w[0].toUpperCase() + w.slice(1)"),
        "Hello"
    );
}
