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
