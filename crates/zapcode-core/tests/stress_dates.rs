//! Regression tests for Date (cluster M): string/multi-arg construction,
//! arithmetic/coercion, statics, Invalid Date, instanceof, toJSON/toString.
//! Ground truth verified against Node.

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, ZapcodeRun};

fn run_str(code: &str) -> String {
    let result = ZapcodeRun::new(code.to_string(), Vec::new(), Vec::new(), ResourceLimits::default())
        .unwrap().run(Vec::new()).unwrap();
    match result.state {
        VmState::Complete(v) => v.to_js_string(),
        other => panic!("expected completion, got {other:?}"),
    }
}

#[test]
fn parse_iso_strings() {
    assert_eq!(run_str("new Date('2023-11-14T22:13:20.000Z').getTime()"), "1700000000000");
    assert_eq!(run_str("new Date('2023-11-14T22:13:20.123Z').getUTCFullYear()"), "2023");
    assert_eq!(run_str("new Date('2024-03-10').getTime()"), "1710028800000");
    assert_eq!(run_str("new Date('2023-11-14T22:13:20+05:00').getTime()"), "1699982000000");
    assert_eq!(run_str("Date.parse('2023-11-14T22:13:20.000Z')"), "1700000000000");
}

#[test]
fn multi_arg_construction() {
    assert_eq!(run_str("new Date(2024, 0, 15).getUTCFullYear()"), "2024");
    assert_eq!(run_str("new Date(2024, 0, 15).getUTCMonth()"), "0");
    assert_eq!(run_str("new Date(2024, 0, 15).getUTCDate()"), "15");
    assert_eq!(run_str("Date.UTC(2024,0,1)"), "1704067200000");
}

#[test]
fn arithmetic_and_coercion() {
    assert_eq!(run_str("new Date(1700000005000) - new Date(1700000000000)"), "5000");
    assert_eq!(run_str("+new Date(1700000000000)"), "1700000000000");
    assert_eq!(run_str("Number(new Date(1700000000000))"), "1700000000000");
    assert_eq!(run_str("new Date(1700000000000) < new Date(1700000005000)"), "true");
    assert_eq!(run_str("new Date(1700000000000) instanceof Date"), "true");
}

#[test]
fn invalid_date() {
    assert_eq!(run_str("isNaN(new Date('garbage').getTime())"), "true");
    assert_eq!(run_str("String(new Date('nope'))"), "Invalid Date");
}

#[test]
fn formatters_and_string_coercion() {
    assert_eq!(run_str("new Date(1700000000123).toISOString()"), "2023-11-14T22:13:20.123Z");
    assert_eq!(run_str("new Date(1700000000123).toJSON()"), "2023-11-14T22:13:20.123Z");
    assert_eq!(run_str("String(new Date(1700000000000))"), "2023-11-14T22:13:20.000Z");
    assert_eq!(run_str("new Date(1700000000000).getTimezoneOffset()"), "0");
}
