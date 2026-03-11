use zapcode_core::vm::eval_ts;
use zapcode_core::Value;

#[test]
fn test_const_declaration() {
    let result = eval_ts("const x = 42; x").unwrap();
    assert_eq!(result, Value::Int(42));
}

#[test]
fn test_let_declaration() {
    let result = eval_ts("let x = 10; x").unwrap();
    assert_eq!(result, Value::Int(10));
}

#[test]
fn test_variable_reassignment() {
    let result = eval_ts("let x = 1; x = 2; x").unwrap();
    assert_eq!(result, Value::Int(2));
}

#[test]
fn test_multiple_variables() {
    let result = eval_ts("const a = 10; const b = 20; a + b").unwrap();
    assert_eq!(result, Value::Int(30));
}

#[test]
fn test_variable_in_expression() {
    let result = eval_ts("const x = 5; const y = 3; x * y + 1").unwrap();
    assert_eq!(result, Value::Int(16));
}

#[test]
fn test_compound_assignment_add() {
    let result = eval_ts("let x = 10; x += 5; x").unwrap();
    assert_eq!(result, Value::Int(15));
}

#[test]
fn test_compound_assignment_sub() {
    let result = eval_ts("let x = 10; x -= 3; x").unwrap();
    assert_eq!(result, Value::Int(7));
}

#[test]
fn test_increment() {
    let result = eval_ts("let x = 5; x++; x").unwrap();
    assert_eq!(result, Value::Int(6));
}

#[test]
fn test_decrement() {
    let result = eval_ts("let x = 5; x--; x").unwrap();
    assert_eq!(result, Value::Int(4));
}
