use zapcode_core::vm::eval_ts;
use zapcode_core::ZapcodeError;

#[test]
fn test_import_blocked() {
    let result = eval_ts("import fs from 'fs'");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, ZapcodeError::SandboxViolation(_)));
}

#[test]
fn test_require_blocked() {
    let result = eval_ts("require('fs')");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, ZapcodeError::SandboxViolation(_)));
}

#[test]
fn test_eval_blocked() {
    let result = eval_ts("eval('1 + 1')");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, ZapcodeError::SandboxViolation(_)));
}

#[test]
fn test_function_constructor_blocked() {
    let result = eval_ts("Function('return 1')");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, ZapcodeError::SandboxViolation(_)));
}

#[test]
fn test_new_function_constructor_blocked() {
    // `new Function(...)` is rejected the same way the bare call is — a
    // sandbox violation, raised at runtime so it is catchable by the guest.
    let result = eval_ts(r#"new Function("return 1")"#);
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), ZapcodeError::SandboxViolation(_)));
}

#[test]
fn test_function_call_violation_is_catchable_not_fatal() {
    // Referencing `Function` must NOT abort the program at parse time. A guest
    // try/catch around a forbidden call recovers cleanly (the violation
    // surfaces as a catchable Error inside the guest), and the program runs to
    // completion returning a normal value.
    let result = eval_ts(r#"let ok=false; try{ Function("return 1") }catch(e){ ok=true } ok"#);
    assert_eq!(result.unwrap(), zapcode_core::Value::Bool(true));

    let result = eval_ts(r#"let ok=false; try{ new Function("return 1") }catch(e){ ok=true } ok"#);
    assert_eq!(result.unwrap(), zapcode_core::Value::Bool(true));
}

#[test]
fn test_function_global_is_typeof_function() {
    // The `Function` global itself exists as a non-constructible value, so it
    // can be inspected without being called.
    let result = eval_ts("typeof Function");
    assert_eq!(
        result.unwrap(),
        zapcode_core::Value::String(zapcode_core::JsString::from("function"))
    );
}

#[test]
fn test_process_blocked() {
    let result = eval_ts("process.exit(1)");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, ZapcodeError::SandboxViolation(_)));
}

#[test]
fn test_globalthis_blocked() {
    let result = eval_ts("globalThis.x = 1");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, ZapcodeError::SandboxViolation(_)));
}

#[test]
fn test_dynamic_import_blocked() {
    let result = eval_ts("import('fs')");
    assert!(result.is_err());
}

#[test]
fn test_export_blocked() {
    let result = eval_ts("export default 42");
    assert!(result.is_err());
}
