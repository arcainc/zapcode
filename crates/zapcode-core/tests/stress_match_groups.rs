//! Regression tests for non-global String.prototype.match / matchAll results
//! carrying `.index`, `.input`, named-capture `.groups`, indexed groups, and
//! `.length` (G4). Non-global match results are array-like heap objects (a Vec-
//! backed array can't hold extra named props), so `Array.isArray(m)` is false —
//! an accepted trade-off documented in STRESS-PASS-BUGS.md.

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
        other => panic!("expected completion, got {other:?}"),
    }
}

#[test]
fn match_result_carries_index() {
    // Match starts at char 3.
    assert_eq!(run_str(r#""abc123".match(/\d+/).index"#), "3");
    // Index at the very start.
    assert_eq!(run_str(r#""hello".match(/h/).index"#), "0");
}

#[test]
fn match_result_index_is_in_chars_not_bytes() {
    // A multi-byte prefix: "é" is 2 bytes but 1 char. The match for "x" should
    // report char index 2, not a byte offset.
    assert_eq!(run_str(r#""éé x".match(/x/).index"#), "3");
}

#[test]
fn match_result_carries_input() {
    assert_eq!(run_str(r#""abc123".match(/\d+/).input"#), "abc123");
}

#[test]
fn match_result_indexed_groups_and_length() {
    assert_eq!(run_str(r#""a1".match(/([a-z])(\d)/)[0]"#), "a1");
    assert_eq!(run_str(r#""a1".match(/([a-z])(\d)/)[1]"#), "a");
    assert_eq!(run_str(r#""a1".match(/([a-z])(\d)/)[2]"#), "1");
    // length is whole-match + group count.
    assert_eq!(run_str(r#""a1".match(/([a-z])(\d)/).length"#), "3");
}

#[test]
fn match_result_named_groups() {
    let code = r#"
        const m = "2026-06-02".match(/(?<year>\d{4})-(?<month>\d{2})-(?<day>\d{2})/);
        `${m.groups.year}/${m.groups.month}/${m.groups.day}`
    "#;
    assert_eq!(run_str(code), "2026/06/02");
}

#[test]
fn match_result_groups_undefined_without_named() {
    // No named groups -> .groups is undefined (matching JS).
    assert_eq!(run_str(r#"String("a1".match(/([a-z])(\d)/).groups)"#), "undefined");
}

#[test]
fn match_result_named_and_indexed_coexist() {
    let code = r#"
        const m = "key=val".match(/(?<k>\w+)=(?<v>\w+)/);
        `${m[0]}|${m[1]}|${m[2]}|${m.groups.k}|${m.groups.v}`
    "#;
    assert_eq!(run_str(code), "key=val|key|val|key|val");
}

#[test]
fn no_match_is_null() {
    assert_eq!(run_str(r#"String("abc".match(/\d+/))"#), "null");
}

#[test]
fn global_match_still_returns_plain_array_of_strings() {
    // /g keeps the plain-array-of-matched-strings behavior (JS does too).
    assert_eq!(run_str(r#""cat bat sat".match(/at/g).join(",")"#), "at,at,at");
    assert_eq!(run_str(r#""cat bat sat".match(/at/g).length"#), "3");
    assert_eq!(run_str(r#"Array.isArray("cat bat".match(/at/g))"#), "true");
}

#[test]
fn match_all_yields_array_like_objects() {
    assert_eq!(run_str(r#""x1 y2 z3".matchAll(/([a-z])(\d)/g).length"#), "3");
    // Each yielded result carries indexed groups, .index, .input, .length.
    let code = r#"
        const ms = [..."x1 y2 z3".matchAll(/([a-z])(\d)/g)];
        ms.map(m => `${m[0]}@${m.index}:${m[1]}${m[2]}`).join(",")
    "#;
    assert_eq!(run_str(code), "x1@0:x1,y2@3:y2,z3@6:z3");
    // .input is the full subject on each result.
    let code2 = r#"
        const ms = [..."x1 y2".matchAll(/([a-z])(\d)/g)];
        ms.map(m => m.input).join("|")
    "#;
    assert_eq!(run_str(code2), "x1 y2|x1 y2");
}

#[test]
fn match_all_named_groups() {
    let code = r#"
        const ms = [..."a1 b2".matchAll(/(?<letter>[a-z])(?<num>\d)/g)];
        ms.map(m => `${m.groups.letter}${m.groups.num}`).join(",")
    "#;
    assert_eq!(run_str(code), "a1,b2");
}

#[test]
fn match_all_iteration_via_for_of() {
    let code = r#"
        let out = "";
        for (const m of "p1 q2".matchAll(/([a-z])(\d)/g)) {
            out += m[1] + m[2] + "/";
        }
        out
    "#;
    assert_eq!(run_str(code), "p1/q2/");
}

#[test]
fn match_result_is_not_an_array() {
    // Documented trade-off: the non-global array-like result is an object.
    assert_eq!(run_str(r#"Array.isArray("a1".match(/([a-z])(\d)/))"#), "false");
}
