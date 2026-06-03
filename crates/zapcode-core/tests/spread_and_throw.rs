//! Regression tests for spread expansion in array/object literals and for
//! parsing `throw <expr>;` / control-flow blocks at end of program.

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, Value, ZapcodeRun};

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

fn run_int(code: &str) -> i64 {
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
        VmState::Complete(Value::Int(n)) => n,
        other => panic!("expected int, got {other:?}"),
    }
}

#[test]
fn array_spread_flattens() {
    assert_eq!(
        run_str("const a=[1,2]; const b=[3,4]; [...a, ...b].join(\",\")"),
        "1,2,3,4"
    );
    assert_eq!(
        run_str("const a=[2,3]; [1, ...a, 4].join(\",\")"),
        "1,2,3,4"
    );
    assert_eq!(run_str("const a=[1]; [...a].join(\",\")"), "1");
}

#[test]
fn string_spread_into_array() {
    assert_eq!(run_str("[...\"hi\", \"!\"].join(\"-\")"), "h-i-!");
}

#[test]
fn object_spread_merges_with_override() {
    assert_eq!(
        run_str("const a={x:1,y:2}; const b={y:9,z:3}; JSON.stringify({...a, ...b})"),
        r#"{"x":1,"y":9,"z":3}"#
    );
}

#[test]
fn object_spread_of_nullish_is_noop() {
    assert_eq!(
        run_str("JSON.stringify({...null, a:1, ...undefined})"),
        r#"{"a":1}"#
    );
}

#[test]
fn trailing_object_literal_still_parses_as_expression() {
    assert_eq!(run_int("({ a: 1, b: 2 }).a"), 1);
    assert_eq!(run_int("const x = 5; ({ doubled: x * 2 }).doubled"), 10);
}

#[test]
fn call_argument_spread() {
    assert_eq!(run_str("Math.min(...[5, 2, 9, 1])"), "1");
    assert_eq!(run_str("Math.max(...[5, 2, 9, 1])"), "9");
    assert_eq!(
        run_int("function add(a, b, c) { return a + b + c; } add(...[1, 2, 3])"),
        6
    );
    assert_eq!(
        run_str("function f(a, b, c, d) { return [a,b,c,d].join(\",\"); } f(1, ...[2, 3], 4)"),
        "1,2,3,4"
    );
    // array method with spread args
    assert_eq!(
        run_str("const a = [0]; a.push(...[1, 2, 3]); a.join(\",\")"),
        "0,1,2,3"
    );
}

#[test]
fn type_conversion_functions_are_callable() {
    assert_eq!(run_str("String(42)"), "42");
    assert_eq!(run_str("String(true)"), "true");
    assert_eq!(run_int("Number(\"42\") + 1"), 43);
    assert_eq!(run_str("Boolean(0) + \",\" + Boolean(\"x\")"), "false,true");
    assert_eq!(run_str("[1,2,3].map(n => String(n)).join(\"-\")"), "1-2-3");
}

#[test]
fn caught_value_preserves_type_and_content() {
    // A thrown string is caught verbatim (no "runtime error:" prefix).
    assert_eq!(
        run_str("let r; try { throw \"boom\"; } catch (e) { r = e; } r"),
        "boom"
    );
    // A thrown object is caught as an object, not a stringified error.
    assert_eq!(
        run_str(
            "let r; try { throw { code: 42 }; } catch (e) { r = typeof e + \":\" + e.code; } r"
        ),
        "object:42"
    );
}

#[test]
fn throw_string_literal_in_catch_is_caught() {
    assert_eq!(
        run_str("let r; try { throw \"x\"; } catch (e) { r = \"c:\" + e; } r"),
        "c:x"
    );
}

#[test]
fn rethrow_with_expression_from_catch_propagates() {
    let result = ZapcodeRun::new(
        "try { throw \"boom\"; } catch (e) { throw \"wrapped:\" + e; }".to_string(),
        Vec::new(),
        Vec::new(),
        ResourceLimits::default(),
    )
    .unwrap()
    .run(Vec::new());
    let err = match result {
        Ok(_) => panic!("expected the rethrow to propagate"),
        Err(e) => e.to_string(),
    };
    assert!(err.contains("wrapped:boom"), "unexpected error: {err}");
}

#[test]
fn control_flow_block_at_end_of_program_parses() {
    // A catch/if/for block ending the program must not be mis-wrapped as an
    // object literal.
    assert_eq!(
        run_str("let out = \"x\"; if (true) { out = \"taken\"; } out"),
        "taken"
    );
    assert_eq!(
        run_str("let n = 0; for (const i of [1,2,3]) { n += i; } String(n)"),
        "6"
    );
}
