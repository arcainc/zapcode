//! Regression tests for statement completion values (B1): a program ending in
//! try/catch, if, or a block yields that block's value, not null.

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, ZapcodeRun};

fn run_str(code: &str) -> String {
    let result = ZapcodeRun::new(code.to_string(), Vec::new(), Vec::new(), ResourceLimits::default())
        .unwrap().run(Vec::new()).unwrap();
    match result.state {
        VmState::Complete(v) => v.to_js_string(&result.heap),
        other => panic!("expected completion, got {other:?}"),
    }
}

#[test]
fn trailing_try_catch_value() {
    assert_eq!(run_str("try { null.x; } catch (e) { e.name }"), "TypeError");
    assert_eq!(run_str("try { throw new Error('x'); } catch (e) { 'handled:' + e.message }"), "handled:x");
    assert_eq!(run_str("try { 1 + 1 } catch (e) { 0 }"), "2");
}

#[test]
fn trailing_if_value() {
    assert_eq!(run_str("if (true) { 42 }"), "42");
    assert_eq!(run_str("if (false) { 1 } else { 2 }"), "2");
    assert_eq!(run_str("const x = 5; if (x > 3) { 'big' } else { 'small' }"), "big");
    assert_eq!(run_str("if (false) { 1 }"), "undefined");
}

#[test]
fn trailing_block_value() {
    assert_eq!(run_str("{ const a = 1; a + 2 }"), "3");
}

#[test]
fn try_with_finally_preserves_value() {
    assert_eq!(run_str("let log=[]; try { 'try-val' } finally { log.push('f'); }"), "try-val");
}

#[test]
fn existing_trailing_expression_unchanged() {
    assert_eq!(run_str("const x = 10; x * 2"), "20");
    assert_eq!(run_str("[1,2,3].map(n => n * 2)"), "2,4,6");
}
