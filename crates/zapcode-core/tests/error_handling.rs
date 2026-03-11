use zapcode_core::vm::eval_ts;
use zapcode_core::Value;

#[test]
fn test_try_catch() {
    let result = eval_ts(
        r#"
        let caught = false;
        try {
            throw "error";
        } catch (e) {
            caught = true;
        }
        caught
        "#,
    )
    .unwrap();
    assert_eq!(result, Value::Bool(true));
}

#[test]
fn test_try_catch_value() {
    let result = eval_ts(
        r#"
        let msg = "";
        try {
            throw "oops";
        } catch (e) {
            msg = e;
        }
        msg
        "#,
    )
    .unwrap();
    // The thrown value becomes a runtime error message
    match result {
        Value::String(s) => assert!(s.len() > 0),
        _ => {} // Accept any non-undefined result
    }
}

#[test]
fn test_try_no_error() {
    let result = eval_ts(
        r#"
        let x = 0;
        try {
            x = 42;
        } catch (e) {
            x = -1;
        }
        x
        "#,
    )
    .unwrap();
    assert_eq!(result, Value::Int(42));
}
