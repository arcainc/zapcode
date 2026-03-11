use zapcode_core::vm::eval_ts;
use zapcode_core::Value;

#[test]
fn test_number_literal() {
    let result = eval_ts("42").unwrap();
    assert_eq!(result, Value::Int(42));
}

#[test]
fn test_addition() {
    let result = eval_ts("1 + 2").unwrap();
    assert_eq!(result, Value::Int(3));
}

#[test]
fn test_subtraction() {
    let result = eval_ts("10 - 3").unwrap();
    assert_eq!(result, Value::Int(7));
}

#[test]
fn test_multiplication() {
    let result = eval_ts("6 * 7").unwrap();
    assert_eq!(result, Value::Int(42));
}

#[test]
fn test_division() {
    let result = eval_ts("10 / 3").unwrap();
    match result {
        Value::Float(f) => assert!((f - 3.3333333333333335).abs() < 1e-10),
        other => panic!("expected float, got {:?}", other),
    }
}

#[test]
fn test_string_literal() {
    let result = eval_ts("\"hello\"").unwrap();
    assert_eq!(result, Value::String("hello".into()));
}

#[test]
fn test_string_concatenation() {
    let result = eval_ts("\"hello\" + \" \" + \"world\"").unwrap();
    assert_eq!(result, Value::String("hello world".into()));
}

#[test]
fn test_boolean_true() {
    let result = eval_ts("true").unwrap();
    assert_eq!(result, Value::Bool(true));
}

#[test]
fn test_boolean_false() {
    let result = eval_ts("false").unwrap();
    assert_eq!(result, Value::Bool(false));
}

#[test]
fn test_null_literal() {
    let result = eval_ts("null").unwrap();
    assert_eq!(result, Value::Null);
}

#[test]
fn test_undefined_literal() {
    let result = eval_ts("undefined").unwrap();
    assert_eq!(result, Value::Undefined);
}

#[test]
fn test_comparison_lt() {
    let result = eval_ts("1 < 2").unwrap();
    assert_eq!(result, Value::Bool(true));
}

#[test]
fn test_comparison_gt() {
    let result = eval_ts("2 > 1").unwrap();
    assert_eq!(result, Value::Bool(true));
}

#[test]
fn test_strict_equality() {
    let result = eval_ts("42 === 42").unwrap();
    assert_eq!(result, Value::Bool(true));
}

#[test]
fn test_strict_inequality() {
    let result = eval_ts("42 !== 43").unwrap();
    assert_eq!(result, Value::Bool(true));
}

#[test]
fn test_negation() {
    let result = eval_ts("-42").unwrap();
    assert_eq!(result, Value::Int(-42));
}

#[test]
fn test_logical_not() {
    let result = eval_ts("!true").unwrap();
    assert_eq!(result, Value::Bool(false));
}

#[test]
fn test_typeof_number() {
    let result = eval_ts("typeof 42").unwrap();
    assert_eq!(result, Value::String("number".into()));
}

#[test]
fn test_typeof_string() {
    let result = eval_ts("typeof \"hello\"").unwrap();
    assert_eq!(result, Value::String("string".into()));
}

#[test]
fn test_ternary() {
    let result = eval_ts("true ? 1 : 2").unwrap();
    assert_eq!(result, Value::Int(1));
}

#[test]
fn test_ternary_false() {
    let result = eval_ts("false ? 1 : 2").unwrap();
    assert_eq!(result, Value::Int(2));
}

#[test]
fn test_modulo() {
    let result = eval_ts("10 % 3").unwrap();
    assert_eq!(result, Value::Int(1));
}

#[test]
fn test_power() {
    let result = eval_ts("2 ** 10").unwrap();
    match result {
        Value::Float(f) => assert_eq!(f, 1024.0),
        other => panic!("expected float, got {:?}", other),
    }
}

#[test]
fn test_complex_expression() {
    let result = eval_ts("(1 + 2) * 3 - 4").unwrap();
    assert_eq!(result, Value::Int(5));
}
