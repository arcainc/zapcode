use zapcode_core::vm::eval_ts;
use zapcode_core::Value;

#[test]
fn test_array_literal() {
    let result = eval_ts("[1, 2, 3]").unwrap();
    match result {
        Value::Array(arr) => {
            assert_eq!(arr.len(), 3);
            assert_eq!(arr[0], Value::Int(1));
            assert_eq!(arr[1], Value::Int(2));
            assert_eq!(arr[2], Value::Int(3));
        }
        other => panic!("expected array, got {:?}", other),
    }
}

#[test]
fn test_array_length() {
    let result = eval_ts("const arr = [1, 2, 3]; arr.length").unwrap();
    assert_eq!(result, Value::Int(3));
}

#[test]
fn test_object_literal() {
    let result = eval_ts("const obj = { x: 1, y: 2 }; obj.x + obj.y").unwrap();
    assert_eq!(result, Value::Int(3));
}

#[test]
fn test_nested_object() {
    let result = eval_ts("const obj = { a: { b: 42 } }; obj.a.b").unwrap();
    assert_eq!(result, Value::Int(42));
}

#[test]
fn test_computed_property_access() {
    let result = eval_ts("const arr = [10, 20, 30]; arr[1]").unwrap();
    assert_eq!(result, Value::Int(20));
}

#[test]
fn test_object_shorthand() {
    let result = eval_ts("const x = 42; const obj = { x }; obj.x").unwrap();
    assert_eq!(result, Value::Int(42));
}

#[test]
fn test_destructuring_object() {
    let result = eval_ts("const obj = { a: 1, b: 2 }; const { a, b } = obj; a + b").unwrap();
    assert_eq!(result, Value::Int(3));
}

#[test]
fn test_destructuring_array() {
    let result = eval_ts("const arr = [10, 20]; const [a, b] = arr; a + b").unwrap();
    assert_eq!(result, Value::Int(30));
}

#[test]
fn test_template_literal() {
    let result = eval_ts("const name = \"world\"; `hello ${name}`").unwrap();
    assert_eq!(result, Value::String("hello world".into()));
}

#[test]
fn test_string_length() {
    let result = eval_ts("\"hello\".length").unwrap();
    assert_eq!(result, Value::Int(5));
}
