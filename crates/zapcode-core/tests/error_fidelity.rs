//! Runtime error fidelity: uncaught errors escaping to the host carry the
//! throw site ("    at <line>:<col>") and — when the source is at hand — a
//! one-line code frame, so an agent can self-correct. The first line of every
//! error message is preserved exactly; the location is appended on new lines.

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, Value, ZapcodeRun, ZapcodeSnapshot};

fn run_err(code: &str) -> String {
    ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        Vec::new(),
        ResourceLimits::default(),
    )
    .unwrap()
    .run(Vec::new())
    .unwrap_err()
    .to_string()
}

#[test]
fn sync_throw_reports_line_and_code_frame() {
    let e = run_err("const a = 1;\nthrow new Error(\"boom\");");
    // First line preserved exactly; location + code frame appended.
    assert!(
        e.starts_with("runtime error: Error: boom\n"),
        "first line changed: {e}"
    );
    assert!(e.contains("\n    at 2:1"), "missing location: {e}");
    assert!(
        e.contains("\n    throw new Error(\"boom\");\n    ^"),
        "missing code frame: {e}"
    );
}

#[test]
fn throw_inside_function_reports_the_throw_line() {
    let e = run_err("function f() {\n  const x = 1;\n  throw new Error(\"inner\");\n}\nf();");
    // Line 3 is the `throw`, not the call site (line 5) or the decl (line 1).
    assert!(e.contains("\n    at 3:3"), "wrong line: {e}");
    assert!(e.contains("throw new Error(\"inner\");"), "missing frame: {e}");
}

#[test]
fn null_property_access_reports_line() {
    let e = run_err("const obj = null;\nconst x = 1;\nobj.field;");
    assert!(
        e.starts_with("type error: Cannot read properties of null"),
        "first line changed: {e}"
    );
    assert!(e.contains("\n    at 3:1"), "wrong line: {e}");
    assert!(e.contains("\n    obj.field;"), "missing frame: {e}");
}

#[test]
fn undefined_call_reports_line() {
    let e = run_err("const a = 5;\nnoSuchFn(a);");
    assert!(
        e.starts_with("type error: undefined is not a function"),
        "first line changed: {e}"
    );
    assert!(e.contains("\n    at 2:1"), "wrong line: {e}");
}

#[test]
fn caret_points_at_the_statement_column() {
    let e = run_err("function g() {\n    throw new Error(\"deep\");\n}\ng();");
    // 4-space indent in the source → caret padded 4 columns under the frame.
    assert!(e.contains("\n    at 2:5"), "wrong column: {e}");
    assert!(
        e.contains("\n        throw new Error(\"deep\");\n        ^"),
        "caret misaligned: {e}"
    );
}

#[test]
fn caught_errors_are_not_annotated() {
    // An error handled inside the guest never escapes, so the catch binding
    // (and anything built from it) must not contain location lines.
    let runner = ZapcodeRun::new(
        "let m;\ntry {\n  null.x;\n} catch (e) {\n  m = String(e);\n}\nm".to_string(),
        Vec::new(),
        Vec::new(),
        ResourceLimits::default(),
    )
    .unwrap();
    let result = runner.run(Vec::new()).unwrap();
    match result.state {
        VmState::Complete(Value::String(s)) => {
            let s = s.to_string();
            assert!(!s.contains("    at "), "caught error was annotated: {s}");
        }
        other => panic!("expected completion, got {other:?}"),
    }
}

#[test]
fn error_after_snapshot_hop_reports_same_line() {
    let code = "const a = await callTool(\"x\");\nconst b = null;\nb.field;";

    // Establish the line the non-hopping run reports.
    let direct = ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        vec!["callTool".to_string()],
        ResourceLimits::default(),
    )
    .unwrap();
    let state = direct.start(Vec::new()).unwrap();
    let snapshot = match state {
        VmState::Suspended { snapshot, .. } => snapshot,
        other => panic!("expected suspension, got {other:?}"),
    };

    // Hop: dump to bytes, reload in a "different process", resume.
    let bytes = snapshot.dump().unwrap();
    let restored = ZapcodeSnapshot::load(&bytes).unwrap();
    let e = restored
        .resume(Value::String("ok".into()))
        .unwrap_err()
        .to_string();

    assert!(
        e.starts_with("type error: Cannot read properties of null"),
        "first line changed: {e}"
    );
    // The line table travels inside the snapshot's CompiledProgram, so the
    // resumed run reports the same `b.field;` line (3) as a direct run would.
    assert!(e.contains("\n    at 3:1"), "wrong line after hop: {e}");
}

#[test]
fn uncaught_resume_error_reports_the_await_line() {
    let code = "const x = 1;\nconst v = await callTool(\"x\");\nv";
    let runner = ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        vec!["callTool".to_string()],
        ResourceLimits::default(),
    )
    .unwrap();
    let snapshot = match runner.start(Vec::new()).unwrap() {
        VmState::Suspended { snapshot, .. } => snapshot,
        other => panic!("expected suspension, got {other:?}"),
    };
    let e = snapshot
        .resume_with_error(Value::String("upstream 500".into()))
        .unwrap_err()
        .to_string();
    assert!(
        e.starts_with("external function error: upstream 500"),
        "first line changed: {e}"
    );
    assert!(e.contains("\n    at 2:"), "wrong line: {e}");
}

#[test]
fn uncaught_error_through_finally_keeps_the_throw_site() {
    // The re-raise after a finally body must stay attributed to the ORIGINAL
    // throw, not the finally's last line — pointing the agent at an innocent
    // cleanup statement is worse than no location at all.
    let err = run_err(
        "try {\n  throw new Error(\"boom\");\n} finally {\n  const cleanup = 1;\n}",
    );
    assert!(err.starts_with("runtime error: Error: boom"), "first line: {err}");
    assert!(err.contains("at 2:3"), "expected the throw's line, got: {err}");
    assert!(
        err.contains("throw new Error"),
        "code frame should show the throw, got: {err}"
    );
    assert!(
        !err.contains("cleanup"),
        "must not point at the finally body, got: {err}"
    );
}

#[test]
fn nested_finally_chain_keeps_the_throw_site() {
    let err = run_err(
        "try {\n  try {\n    throw new TypeError(\"inner\");\n  } finally {\n    const a = 1;\n  }\n} finally {\n  const b = 2;\n}",
    );
    assert!(err.contains("at 3:5"), "expected the inner throw line, got: {err}");
    assert!(!err.contains("const b"), "got: {err}");
}
