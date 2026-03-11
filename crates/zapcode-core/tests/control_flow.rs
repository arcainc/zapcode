use zapcode_core::vm::eval_ts;
use zapcode_core::Value;

#[test]
fn test_if_true() {
    let result = eval_ts("let x = 0; if (true) { x = 1; } x").unwrap();
    assert_eq!(result, Value::Int(1));
}

#[test]
fn test_if_false() {
    let result = eval_ts("let x = 0; if (false) { x = 1; } x").unwrap();
    assert_eq!(result, Value::Int(0));
}

#[test]
fn test_if_else() {
    let result = eval_ts("let x = 0; if (false) { x = 1; } else { x = 2; } x").unwrap();
    assert_eq!(result, Value::Int(2));
}

#[test]
fn test_while_loop() {
    let result = eval_ts("let sum = 0; let i = 1; while (i <= 10) { sum += i; i++; } sum").unwrap();
    assert_eq!(result, Value::Int(55));
}

#[test]
fn test_for_loop() {
    let result = eval_ts(
        "let sum = 0; for (let i = 0; i < 5; i++) { sum += i; } sum",
    )
    .unwrap();
    assert_eq!(result, Value::Int(10));
}

#[test]
fn test_break_in_loop() {
    let result = eval_ts(
        "let x = 0; while (true) { x++; if (x === 5) { break; } } x",
    )
    .unwrap();
    assert_eq!(result, Value::Int(5));
}

#[test]
fn test_continue_in_loop() {
    let result = eval_ts(
        "let sum = 0; for (let i = 0; i < 10; i++) { if (i % 2 === 0) { continue; } sum += i; } sum",
    )
    .unwrap();
    assert_eq!(result, Value::Int(25));
}

#[test]
fn test_nested_if() {
    let result = eval_ts(
        "let x = 0; if (true) { if (true) { x = 42; } } x",
    )
    .unwrap();
    assert_eq!(result, Value::Int(42));
}

#[test]
fn test_logical_and() {
    let result = eval_ts("true && 42").unwrap();
    assert_eq!(result, Value::Int(42));
}

#[test]
fn test_logical_or() {
    let result = eval_ts("false || 42").unwrap();
    assert_eq!(result, Value::Int(42));
}

#[test]
fn test_nullish_coalescing() {
    let result = eval_ts("null ?? 42").unwrap();
    assert_eq!(result, Value::Int(42));
}

#[test]
fn test_nullish_coalescing_defined() {
    let result = eval_ts("0 ?? 42").unwrap();
    assert_eq!(result, Value::Int(0));
}
