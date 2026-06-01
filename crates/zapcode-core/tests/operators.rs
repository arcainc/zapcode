//! Regression tests for loose equality, logical-assignment operators, and
//! nullish coalescing (including in call-argument position).

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
fn loose_equality_coerces() {
    assert_eq!(run_str("1 == '1'"), "true");
    assert_eq!(run_str("0 == ''"), "true");
    assert_eq!(run_str("0 == false"), "true");
    assert_eq!(run_str("1 == true"), "true");
    assert_eq!(run_str("'1' == true"), "true");
    assert_eq!(run_str("null == undefined"), "true");
    assert_eq!(run_str("null == 0"), "false");
    assert_eq!(run_str("undefined == 0"), "false");
    assert_eq!(run_str("'abc' == 0"), "false");
    assert_eq!(run_str("2 != '2'"), "false");
    assert_eq!(run_str("1 != 2"), "true");
}

#[test]
fn strict_equality_unchanged() {
    assert_eq!(run_str("1 === '1'"), "false");
    assert_eq!(run_str("0 === false"), "false");
    assert_eq!(run_str("null === undefined"), "false");
    assert_eq!(run_str("1 === 1"), "true");
    assert_eq!(run_str("1 !== '1'"), "true");
}

#[test]
fn or_assign() {
    assert_eq!(run_str("let a = 0; a ||= 5; a"), "5");
    assert_eq!(run_str("let a = 3; a ||= 5; a"), "3");
    assert_eq!(run_str("let a = ''; a ||= 'x'; a"), "x");
}

#[test]
fn and_assign() {
    assert_eq!(run_str("let a = 1; a &&= 5; a"), "5");
    assert_eq!(run_str("let a = 0; a &&= 5; a"), "0");
}

#[test]
fn nullish_assign() {
    assert_eq!(run_str("let a = null; a ??= 5; a"), "5");
    assert_eq!(run_str("let a = 0; a ??= 5; a"), "0");
    assert_eq!(run_str("let a = undefined; a ??= 'x'; a"), "x");
    // Object property target.
    assert_eq!(
        run_str("const o = { a: null, b: 2 }; o.a ??= 9; o.b ??= 9; JSON.stringify(o)"),
        r#"{"a":9,"b":2}"#
    );
}

#[test]
fn nullish_coalescing_value() {
    assert_eq!(run_str("null ?? 'd'"), "d");
    assert_eq!(run_str("0 ?? 'd'"), "0");
    assert_eq!(run_str("undefined ?? 42"), "42");
    assert_eq!(run_str("'x' ?? 'y'"), "x");
}

#[test]
fn nullish_coalescing_in_call_args() {
    // The original crash: `??` left an extra stack value, corrupting arg counts.
    let code = "function pick(a, b) { return a + '|' + b; } pick(null ?? 'x', 'y')";
    assert_eq!(run_str(code), "x|y");
    let code2 = "function f(x) { return x * 2; } f(undefined ?? 21)";
    assert_eq!(run_str(code2), "42");
    // Multiple nullish args in one call.
    let code3 = "function g(a, b, c) { return [a, b, c].join(','); } g(1 ?? 9, null ?? 2, 3 ?? 9)";
    assert_eq!(run_str(code3), "1,2,3");
}

#[test]
fn optional_member_does_not_leak_receiver_into_object_literals() {
    assert_eq!(
        run_str(
            r#"
            const owner = { ownerId: "owner_ava" };
            const payload = { ownerId: owner?.ownerId ?? "manager_pool", ticketId: "t1" };
            JSON.stringify(payload)
            "#
        ),
        r#"{"ownerId":"owner_ava","ticketId":"t1"}"#
    );
    assert_eq!(
        run_str(
            r#"
            const owner = null;
            const payload = { ownerId: owner?.ownerId ?? "manager_pool", ticketId: "t1" };
            JSON.stringify(payload)
            "#
        ),
        r#"{"ownerId":"manager_pool","ticketId":"t1"}"#
    );
    assert_eq!(
        run_str(
            r#"
            const owner = { nested: { ownerId: "owner_ava" } };
            const key = "nested";
            const payload = { ownerId: owner?.[key]?.ownerId ?? "manager_pool" };
            JSON.stringify(payload)
            "#
        ),
        r#"{"ownerId":"owner_ava"}"#
    );
}
