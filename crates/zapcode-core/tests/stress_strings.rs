//! Regression tests for string/template divergences (cluster G):
//! template-literal escapes (G2), split limit + capture groups (G5),
//! $<name> replacement (G6), startsWith/endsWith position (G10),
//! substr/codePointAt/String.fromCodePoint (G11).

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
fn template_literal_escapes_are_cooked() {
    assert_eq!(run_str("`a\\nb`.length"), "3");
    assert_eq!(run_str("`a\\tb`.length"), "3");
    assert_eq!(run_str("`a\\u0041b`"), "aAb");
    assert_eq!(run_str("`a\\\\b`.length"), "3");
    // Interpolation still works alongside escapes.
    assert_eq!(run_str("const x = 2; `v=${x}\\n`.length"), "4");
}

#[test]
fn split_honors_limit_and_capture_groups() {
    assert_eq!(run_str("'a,b,c,d'.split(',',2).join('|')"), "a|b");
    assert_eq!(run_str("'a,b'.split(',',0).length"), "0");
    assert_eq!(run_str("'a1b2c'.split(/(\\d)/).join('|')"), "a|1|b|2|c");
    // No limit still returns everything.
    assert_eq!(run_str("'a,b,c'.split(',').length"), "3");
}

#[test]
fn replacement_named_group() {
    assert_eq!(
        run_str("'2020-01'.replace(/(?<y>\\d+)-(?<m>\\d+)/, '$<m>/$<y>')"),
        "01/2020"
    );
}

#[test]
fn starts_ends_with_position() {
    assert_eq!(run_str("'hello'.startsWith('llo', 2)"), "true");
    assert_eq!(run_str("'hello'.endsWith('hel', 3)"), "true");
    assert_eq!(run_str("'hello'.startsWith('he')"), "true");
    assert_eq!(run_str("'hello'.endsWith('lo')"), "true");
}

#[test]
fn substr_and_codepoint() {
    assert_eq!(run_str("'hello'.substr(1,3)"), "ell");
    assert_eq!(run_str("'hello'.substr(-2)"), "lo");
    assert_eq!(run_str("'abc'.codePointAt(0)"), "97");
    assert_eq!(run_str("String.fromCodePoint(97,98)"), "ab");
}
