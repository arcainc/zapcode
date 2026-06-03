//! Regression tests for regex-backed String methods + RegExp methods.

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
fn replace_with_regex_and_groups() {
    assert_eq!(run_str(r#""foo foo".replace(/foo/g, "qux")"#), "qux qux");
    assert_eq!(run_str(r#""abc".replace(/b/, "X")"#), "aXc");
    assert_eq!(
        run_str(r#""2026-06-01".replace(/(\d+)-(\d+)-(\d+)/, "$3/$2/$1")"#),
        "01/06/2026"
    );
    assert_eq!(run_str(r##""a1b2c3".replaceAll(/\d/g, "#")"##), "a#b#c#");
    // string-literal replace stays literal (not regex)
    assert_eq!(run_str(r#""a.b.c".replace(".", "-")"#), "a-b.c");
}

#[test]
fn match_with_metacharacters_and_flags() {
    // Non-global match: whole match is m[0] on the array-like object (G4).
    assert_eq!(run_str(r#""abc123".match(/[a-z]+/)[0]"#), "abc");
    // Global match keeps the plain-array behavior, so .join still works.
    assert_eq!(
        run_str(r#""cat bat sat".match(/at/g).join(",")"#),
        "at,at,at"
    );
    assert_eq!(run_str(r#""x".match(/z+/)"#), "null");
    // Capture groups (first match) are now exposed on an array-like object
    // (G4): m[0] is the whole match, m[1]/m[2] are the groups, m.length is the
    // group count. (The non-global result is no longer a plain Array so it can
    // also carry .index/.input/.groups — see STRESS-PASS-BUGS.md.)
    assert_eq!(
        run_str(r#"const m = "a1".match(/([a-z])(\d)/); `${m[0]},${m[1]},${m[2]}`"#),
        "a1,a,1"
    );
    assert_eq!(run_str(r#""a1".match(/([a-z])(\d)/).length"#), "3");
}

#[test]
fn test_method() {
    assert_eq!(run_str(r#"/^\d{4}$/.test("2026")"#), "true");
    assert_eq!(run_str(r#"/^\d{4}$/.test("20x6")"#), "false");
    assert_eq!(
        run_str(r#"/^[^@]+@[^@]+\.[a-z]+$/i.test("A@b.COM")"#),
        "true"
    );
}

#[test]
fn split_and_search_with_regex() {
    assert_eq!(run_str(r#""a  b   c".split(/\s+/).join("|")"#), "a|b|c");
    assert_eq!(run_str(r#""abc123".search(/\d/)"#), "3");
    assert_eq!(run_str(r#""abc".search(/\d/)"#), "-1");
}

#[test]
fn match_all() {
    assert_eq!(
        run_str(r#""x1 y2 z3".matchAll(/([a-z])(\d)/g).length"#),
        "3"
    );
}
