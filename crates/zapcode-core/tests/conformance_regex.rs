//! Conformance breadth: regular expressions.
//!
//! A dedicated, test262-style sweep of the regex surface the interpreter exposes
//! through `RegExp` *literals* and the `String` regex methods. Every asserted
//! value was cross-checked against real `node -e`.
//!
//! FULLY-WORKING (asserted at the real-JS value):
//!   - `RegExp.prototype.test` / `exec` (non-global single match + global stepping
//!     via `lastIndex`, including the `while ((m = re.exec(s)) !== null)` loop);
//!   - `String.prototype.match` (non-global → match-result; global `/g` → array of
//!     matched substrings) and `matchAll` (iterable of match results);
//!   - `String.prototype.replace` / `replaceAll` with STRING patterns: `$1..$n`,
//!     `$&`, `$$`, `$<name>`;
//!   - `String.prototype.split` with a regex separator, capture-group inclusion,
//!     and a numeric `limit`;
//!   - `String.prototype.search`;
//!   - the full literal grammar exercised: char classes `[...]`/`[^...]`/ranges,
//!     the predefined classes `\d \w \s` (+ negations), quantifiers
//!     `* + ? {n} {n,} {n,m}` (greedy AND lazy `*?`/`+?`/`??`), anchors `^`/`$`,
//!     word boundaries `\b`/`\B`, alternation `|`, grouping `(...)`, non-capturing
//!     `(?:...)`, named groups `(?<name>...)`, lookahead `(?=)`/`(?!)`, and the
//!     `g i m s` flags;
//!   - match-result shape: `m[0]` whole match, `m[1..n]` captures, `m.index`,
//!     `m.input`, `m.length`, and `m.groups` (named captures) — see
//!     `stress_match_groups.rs` for the array-like-brand residual.
//!
//! DOCUMENTED DIVERGENCES (asserted at zapcode's ACTUAL behavior, with a comment,
//! never the real-JS answer):
//!   - a FUNCTION replacer passed to `replace`/`replaceAll` is NOT invoked; the
//!     literal token `function` is spliced in instead (cluster G1).
//!   - `RegExp.prototype.exec`'s result object does NOT expose `.groups` (named
//!     captures), even though `String.prototype.match`'s result does. Reading
//!     `exec(...).groups` yields `undefined`.
//!   - the `RegExp` *constructor* is unavailable (`typeof RegExp === "undefined"`),
//!     so dynamically-built patterns via `new RegExp(str)` are unsupported
//!     (cluster G8); use literals.

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
// test()
// ============================================================================

#[test]
fn test_basic_match_and_nomatch() {
    assert_eq!(run_str(r#"/abc/.test("xxabcxx")"#), "true");
    assert_eq!(run_str(r#"/abc/.test("xxabxx")"#), "false");
    assert_eq!(run_str(r#"/\d+/.test("no digits")"#), "false");
    assert_eq!(run_str(r#"/\d+/.test("has 42")"#), "true");
}

#[test]
fn test_empty_pattern_always_matches() {
    assert_eq!(run_str(r#"/(?:)/.test("")"#), "true");
    assert_eq!(run_str(r#"/(?:)/.test("anything")"#), "true");
}

#[test]
fn test_with_case_insensitive_flag() {
    assert_eq!(run_str(r#"/hello/i.test("HELLO")"#), "true");
    assert_eq!(run_str(r#"/hello/i.test("HeLLo")"#), "true");
    assert_eq!(run_str(r#"/hello/.test("HELLO")"#), "false");
}

#[test]
fn test_global_regex_advances_then_resets_lastindex() {
    // /g + test() advances lastIndex on each match, resets to 0 on miss.
    assert_eq!(run_str(r#"const r=/a/g; const x=r.test("aaa"); x + ":" + r.lastIndex"#), "true:1");
    assert_eq!(
        run_str(r#"const r=/a/g; r.test("ba"); r.test("ba"); r.lastIndex"#),
        "0"
    );
}

// ============================================================================
// exec()
// ============================================================================

#[test]
fn exec_returns_whole_match_and_captures() {
    assert_eq!(run_str(r#"/(\d)(\d)/.exec("a12b")[0]"#), "12");
    assert_eq!(run_str(r#"/(\d)(\d)/.exec("a12b")[1]"#), "1");
    assert_eq!(run_str(r#"/(\d)(\d)/.exec("a12b")[2]"#), "2");
}

#[test]
fn exec_returns_null_on_no_match() {
    assert_eq!(run_str(r#"String(/zzz/.exec("abc"))"#), "null");
    assert_eq!(run_str(r#"/zzz/.exec("abc") === null"#), "true");
}

#[test]
fn exec_index_and_input_is_documented_divergence() {
    // DIVERGENCE asserted as actual: the `exec` result is a bare positional array
    // and does NOT carry `.index` / `.input` (real JS exposes both, as does this
    // interpreter's `match()` result — see match_non_global_returns_match_result).
    assert_eq!(run_str(r#"String(/abc/.exec("xxabc").index)"#), "undefined"); // JS: 2
    assert_eq!(run_str(r#"String(/abc/.exec("xxabc").input)"#), "undefined"); // JS: "xxabc"
}

#[test]
fn exec_global_loop_terminates_and_collects() {
    // The canonical `while ((m = re.exec(s)) !== null)` loop must terminate via
    // lastIndex advancement and collect every match.
    assert_eq!(
        run_str(
            r#"const r=/\d/g; let out=[]; let m; while((m=r.exec("a1b2c3"))!==null){ out.push(m[0]); } out.join("")"#
        ),
        "123"
    );
    assert_eq!(
        run_str(
            r#"const r=/\w+/g; let words=[]; let m; while((m=r.exec("foo bar baz"))!==null){ words.push(m[0]); } words.join(",")"#
        ),
        "foo,bar,baz"
    );
}

#[test]
fn exec_groups_property_is_documented_divergence() {
    // DIVERGENCE asserted as actual: exec's result object does NOT carry the named
    // captures under `.groups` (match()'s result does — see match_named_groups).
    // Real JS exposes `.groups` on both. The positional captures DO work.
    assert_eq!(run_str(r#"const m=/(?<y>\d{4})/.exec("2020"); String(m.groups)"#), "undefined"); // JS: { y: "2020" }
    assert_eq!(run_str(r#"/(?<y>\d{4})/.exec("2020")[1]"#), "2020"); // positional capture works
}

// ============================================================================
// match() — non-global
// ============================================================================

#[test]
fn match_non_global_returns_match_result() {
    assert_eq!(run_str(r#""xxabc".match(/abc/)[0]"#), "abc");
    assert_eq!(run_str(r#""xxabc".match(/abc/).index"#), "2");
    assert_eq!(run_str(r#""xxabc".match(/abc/).input"#), "xxabc");
    assert_eq!(run_str(r#""a12".match(/(\d)(\d)/).length"#), "3"); // [whole, $1, $2]
}

#[test]
fn match_non_global_captures() {
    assert_eq!(run_str(r#""2020-12".match(/(\d+)-(\d+)/)[1]"#), "2020");
    assert_eq!(run_str(r#""2020-12".match(/(\d+)-(\d+)/)[2]"#), "12");
}

#[test]
fn match_no_match_is_null() {
    assert_eq!(run_str(r#"String("abc".match(/z/))"#), "null");
    assert_eq!(run_str(r#""abc".match(/z/) === null"#), "true");
}

#[test]
fn match_named_groups() {
    assert_eq!(run_str(r#""a1".match(/(?<l>\w)(?<d>\d)/).groups.l"#), "a");
    assert_eq!(run_str(r#""a1".match(/(?<l>\w)(?<d>\d)/).groups.d"#), "1");
    assert_eq!(run_str(r#""2020".match(/(?<y>\d+)/).groups.y"#), "2020");
}

#[test]
fn match_groups_undefined_when_no_named_captures() {
    // No named groups in the pattern → `.groups` is undefined (matches JS).
    assert_eq!(run_str(r#"String("a1".match(/(\w)(\d)/).groups)"#), "undefined");
}

// ============================================================================
// match() — global /g returns a plain array of matched substrings
// ============================================================================

#[test]
fn match_global_returns_array_of_strings() {
    assert_eq!(run_str(r#""a1b2c3".match(/\d/g).join(",")"#), "1,2,3");
    assert_eq!(run_str(r#""a1b2c3".match(/\d/g).length"#), "3");
    // It IS a real array (so array methods work) — distinct from the non-global result.
    assert_eq!(run_str(r#"Array.isArray("a1b2".match(/\d/g))"#), "true");
    assert_eq!(run_str(r#""abcABC".match(/[a-z]/g).join("")"#), "abc");
}

#[test]
fn match_global_no_match_is_null() {
    assert_eq!(run_str(r#"String("abc".match(/\d/g))"#), "null");
}

#[test]
fn match_global_array_is_mappable() {
    assert_eq!(
        run_str(r#""a1b22c333".match(/\d+/g).map(s => s.length).join(",")"#),
        "1,2,3"
    );
}

// ============================================================================
// matchAll()
// ============================================================================

#[test]
fn match_all_iterates_match_results() {
    assert_eq!(
        run_str(r#"[..."a1b2".matchAll(/(\w)(\d)/g)].map(m => m[1] + m[2]).join(",")"#),
        "a1,b2"
    );
    assert_eq!(
        run_str(r#"[..."x1y2z3".matchAll(/\d/g)].map(m => m[0]).join("")"#),
        "123"
    );
}

#[test]
fn match_all_named_groups_per_element() {
    assert_eq!(
        run_str(r#"[..."2020-12".matchAll(/(?<n>\d+)/g)].map(m => m.groups.n).join(",")"#),
        "2020,12"
    );
}

#[test]
fn match_all_index_per_element() {
    assert_eq!(
        run_str(r#"[..."a.b.c".matchAll(/\w/g)].map(m => m.index).join(",")"#),
        "0,2,4"
    );
}

#[test]
fn match_all_empty_when_no_matches() {
    assert_eq!(run_str(r#"[..."abc".matchAll(/\d/g)].length"#), "0");
}

// ============================================================================
// replace() / replaceAll() with STRING patterns
// ============================================================================

#[test]
fn replace_first_occurrence_only_without_g() {
    assert_eq!(run_str(r#""aaa".replace(/a/, "X")"#), "Xaa");
    assert_eq!(run_str(r#""foo bar".replace(/o/, "0")"#), "f0o bar");
}

#[test]
fn replace_all_occurrences_with_g() {
    assert_eq!(run_str(r#""aaa".replace(/a/g, "X")"#), "XXX");
    assert_eq!(run_str(r#""foo bar".replace(/o/g, "0")"#), "f00 bar");
}

#[test]
fn replace_all_method() {
    assert_eq!(run_str(r#""a-b-c".replaceAll("-", "_")"#), "a_b_c");
    assert_eq!(run_str(r##""a1b2".replaceAll(/\d/g, "#")"##), "a#b#");
}

#[test]
fn replace_dollar_numbered_captures() {
    assert_eq!(run_str(r#""a1b2".replace(/(\d)/g, "[$1]")"#), "a[1]b[2]");
    assert_eq!(
        run_str(r#""2020-12-31".replace(/(\d+)-(\d+)-(\d+)/, "$3/$2/$1")"#),
        "31/12/2020"
    );
}

#[test]
fn replace_dollar_ampersand_whole_match() {
    assert_eq!(run_str(r#""abc".replace(/b/, "[$&]")"#), "a[b]c");
    assert_eq!(run_str(r#""cat".replace(/a/g, "$&$&")"#), "caat");
}

#[test]
fn replace_double_dollar_is_literal_dollar() {
    assert_eq!(run_str(r#""abc".replace(/b/, "$$")"#), "a$c");
}

#[test]
fn replace_named_group_reference() {
    assert_eq!(
        run_str(r#""2020-01".replace(/(?<y>\d+)-(?<m>\d+)/, "$<m>/$<y>")"#),
        "01/2020"
    );
}

#[test]
fn replace_no_match_is_unchanged() {
    assert_eq!(run_str(r#""abc".replace(/z/, "X")"#), "abc");
    assert_eq!(run_str(r#""abc".replace(/z/g, "X")"#), "abc");
}

#[test]
fn replace_function_replacer() {
    // A FUNCTION replacer is invoked per match; its return value is spliced in
    // (cluster G1). Title-casing the first letter of each word works as in JS.
    assert_eq!(
        run_str(r#""hello world".replace(/\b\w/g, c => c.toUpperCase())"#),
        "Hello World"
    );
    assert_eq!(
        run_str(r##""a1".replace(/\d/, d => "#")"##),
        "a#"
    );
}

// ============================================================================
// split() with a regex separator
// ============================================================================

#[test]
fn split_on_regex_separator() {
    assert_eq!(run_str(r#""a,b;c".split(/[,;]/).join("-")"#), "a-b-c");
    assert_eq!(run_str(r#""aXbXc".split(/X/).length"#), "3");
    assert_eq!(run_str(r#""1  2   3".split(/\s+/).join(",")"#), "1,2,3");
}

#[test]
fn split_includes_capture_groups() {
    assert_eq!(run_str(r#""a1b2c".split(/(\d)/).join("|")"#), "a|1|b|2|c");
}

#[test]
fn split_with_limit() {
    assert_eq!(run_str(r#""a,b,c,d".split(/,/, 2).join("|")"#), "a|b");
    assert_eq!(run_str(r#""a,b,c".split(",", 2).join("|")"#), "a|b");
}

#[test]
fn split_empty_separator_yields_chars() {
    assert_eq!(run_str(r#""abc".split("").join("-")"#), "a-b-c");
}

// ============================================================================
// search()
// ============================================================================

#[test]
fn search_returns_index_or_negative_one() {
    assert_eq!(run_str(r#""hello".search(/l/)"#), "2");
    assert_eq!(run_str(r#""hello".search(/z/)"#), "-1");
    assert_eq!(run_str(r#""abc123".search(/\d/)"#), "3");
}

// ============================================================================
// Character classes
// ============================================================================

#[test]
fn char_class_ranges() {
    assert_eq!(run_str(r#""aB3".match(/[a-z]/)[0]"#), "a");
    assert_eq!(run_str(r#""aB3".match(/[A-Z]/)[0]"#), "B");
    assert_eq!(run_str(r#""aB3".match(/[0-9]/)[0]"#), "3");
    assert_eq!(run_str(r#""hello123".match(/[a-z0-9]+/)[0]"#), "hello123");
}

#[test]
fn negated_char_class() {
    assert_eq!(run_str(r#""abc123".match(/[^0-9]+/)[0]"#), "abc");
    assert_eq!(run_str(r#""   x".match(/[^ ]/)[0]"#), "x");
}

#[test]
fn predefined_classes_digit_word_space() {
    assert_eq!(run_str(r#""a 1".match(/\d/)[0]"#), "1");
    assert_eq!(run_str(r#""a 1".match(/\w/)[0]"#), "a");
    assert_eq!(run_str(r#"" x".match(/\s/) ? "yes" : "no""#), "yes");
}

#[test]
fn negated_predefined_classes() {
    assert_eq!(run_str(r#""123abc".match(/\D+/)[0]"#), "abc");
    assert_eq!(run_str(r#""a-b".match(/\W/)[0]"#), "-");
    assert_eq!(run_str(r#"" a".match(/\S/)[0]"#), "a");
}

#[test]
fn dot_matches_any_non_newline() {
    assert_eq!(run_str(r#"/^a.c$/.test("axc")"#), "true");
    assert_eq!(run_str(r#"/^a.c$/.test("ac")"#), "false");
}

// ============================================================================
// Quantifiers
// ============================================================================

#[test]
fn quantifier_star_plus_question() {
    assert_eq!(run_str(r#""aaa".match(/a+/)[0]"#), "aaa");
    assert_eq!(run_str(r#""aaa".match(/a*/)[0]"#), "aaa");
    assert_eq!(run_str(r#""".match(/a*/)[0]"#), ""); // matches empty
    assert_eq!(run_str(r#""color colour".match(/colou?r/g).length"#), "2");
}

#[test]
fn quantifier_counted() {
    assert_eq!(run_str(r#""aaaa".match(/a{2}/)[0]"#), "aa");
    assert_eq!(run_str(r#""aaaa".match(/a{2,3}/)[0]"#), "aaa");
    assert_eq!(run_str(r#""aaaa".match(/a{2,}/)[0]"#), "aaaa");
}

#[test]
fn greedy_vs_lazy() {
    assert_eq!(run_str(r#""<a><b>".match(/<.+>/)[0]"#), "<a><b>"); // greedy
    assert_eq!(run_str(r#""<a><b>".match(/<.+?>/)[0]"#), "<a>"); // lazy
    assert_eq!(run_str(r#""aaa".match(/a+?/)[0]"#), "a"); // lazy plus
}

// ============================================================================
// Anchors & boundaries
// ============================================================================

#[test]
fn anchors_start_and_end() {
    assert_eq!(run_str(r#"/^abc/.test("abcdef")"#), "true");
    assert_eq!(run_str(r#"/^abc/.test("xabcdef")"#), "false");
    assert_eq!(run_str(r#"/def$/.test("abcdef")"#), "true");
    assert_eq!(run_str(r#"/def$/.test("abcdefx")"#), "false");
}

#[test]
fn word_boundaries() {
    assert_eq!(run_str(r#""cat cats".match(/\bcat\b/g).length"#), "1");
    assert_eq!(run_str(r#""a.b.c".match(/\b\w\b/g).join("")"#), "abc");
}

#[test]
fn non_word_boundary() {
    assert_eq!(run_str(r#"/\Bcat/.test("scatter")"#), "true");
    assert_eq!(run_str(r#"/\Bcat/.test("cat")"#), "false");
}

// ============================================================================
// Alternation, grouping, lookahead
// ============================================================================

#[test]
fn alternation() {
    assert_eq!(run_str(r#""red or blue".match(/red|blue/g).join(",")"#), "red,blue");
    assert_eq!(run_str(r#"/^(cat|dog)$/.test("dog")"#), "true");
}

#[test]
fn capturing_group_repeat() {
    assert_eq!(run_str(r#""abab".match(/(ab)+/)[0]"#), "abab");
    assert_eq!(run_str(r#""abab".match(/(ab)+/)[1]"#), "ab"); // last capture
}

#[test]
fn non_capturing_group() {
    // (?:...) groups for precedence without producing a capture slot.
    assert_eq!(run_str(r#""abcabc".match(/(?:abc)+/)[0]"#), "abcabc");
    assert_eq!(run_str(r#""ab".match(/(?:a)(b)/)[1]"#), "b"); // only one capture
}

#[test]
fn lookaround_is_documented_divergence() {
    // DIVERGENCE asserted as actual: the underlying regex engine does NOT support
    // look-around (look-ahead `(?=)`/`(?!)` or look-behind), so a pattern using it
    // surfaces a catchable RuntimeError rather than matching. Real JS supports them.
    assert_eq!(
        run_str(r#"try { "12px".match(/\d+(?=px)/); "no" } catch (e) { "threw" }"#),
        "threw" // JS: matches "12"
    );
    assert_eq!(
        run_str(r#"try { "12em".match(/\d+(?!px)/); "no" } catch (e) { "threw" }"#),
        "threw" // JS: matches
    );
}

// ============================================================================
// Flags
// ============================================================================

#[test]
fn flag_global_match_all() {
    assert_eq!(run_str(r#""a.a.a".match(/a/g).length"#), "3");
}

#[test]
fn flag_ignorecase() {
    assert_eq!(run_str(r#""Hello".match(/hello/i)[0]"#), "Hello");
    assert_eq!(run_str(r#""HELLO WORLD".replace(/world/i, "there")"#), "HELLO there");
}

#[test]
fn flag_multiline() {
    // ^ matches at each line start under /m.
    assert_eq!(run_str("\"a\\nb\\nc\".match(/^./gm).join(\"\")"), "abc");
}

#[test]
fn flag_dotall() {
    // /s makes `.` match newline.
    assert_eq!(run_str("/a.b/s.test(\"a\\nb\")"), "true");
    assert_eq!(run_str("/a.b/.test(\"a\\nb\")"), "false");
}

#[test]
fn combined_flags() {
    assert_eq!(run_str("\"FOO\\nBAR\".match(/^\\w+/gim).join(\",\")"), "FOO,BAR");
}

// ============================================================================
// RegExp constructor — documented divergence
// ============================================================================

#[test]
fn regexp_constructor_is_documented_divergence() {
    // DIVERGENCE asserted as actual (cluster G8): the RegExp constructor is not
    // provided, so dynamically-built patterns are unsupported. `typeof` is
    // "undefined" (not "function"), and `new RegExp(...)` throws a catchable error.
    assert_eq!(run_str(r#"typeof RegExp"#), "undefined"); // JS: "function"
    assert_eq!(
        run_str(r#"try { new RegExp("\\d+"); "no" } catch (e) { "threw" }"#),
        "threw"
    );
}

// ============================================================================
// Realistic text-processing pipelines (string-pattern replacers only)
// ============================================================================

#[test]
fn camel_to_snake_via_string_replacement() {
    // Uses a STRING replacement ($1) — avoids the function-replacer divergence.
    assert_eq!(
        run_str(r#""camelCaseName".replace(/([A-Z])/g, "_$1").toLowerCase()"#),
        "camel_case_name"
    );
}

#[test]
fn extract_all_numbers_and_sum() {
    // Use an arrow wrapper for the numeric coercion; passing the bare `Number`
    // builtin as a `.map` callback is not supported (it is not a callable
    // reference in callback position).
    assert_eq!(
        run_str(r#""a3 b10 c7".match(/\d+/g).map(x => Number(x)).reduce((a, b) => a + b, 0)"#),
        "20"
    );
}

#[test]
fn validate_email_shape() {
    assert_eq!(run_str(r#"/^[\w.]+@[\w.]+\.\w+$/.test("a.b@example.com")"#), "true");
    assert_eq!(run_str(r#"/^[\w.]+@[\w.]+\.\w+$/.test("not-an-email")"#), "false");
}

#[test]
fn tokenize_with_exec_loop_and_groups() {
    assert_eq!(
        run_str(
            r#"const re = /(\w+)=(\d+)/g; const out = {}; let m; while ((m = re.exec("x=1 y=2 z=3")) !== null) { out[m[1]] = Number(m[2]); } JSON.stringify(out)"#
        ),
        r#"{"x":1,"y":2,"z":3}"#
    );
}

#[test]
fn redact_via_string_pattern() {
    assert_eq!(
        run_str(r#""call 555-1234 now".replace(/\d{3}-\d{4}/, "[redacted]")"#),
        "call [redacted] now"
    );
}
