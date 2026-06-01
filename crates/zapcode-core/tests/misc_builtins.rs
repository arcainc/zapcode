//! Regression tests for Date decomposition, String.fromCharCode,
//! structuredClone, and Array.from over Set/Map.

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, ZapcodeRun};

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
        VmState::Complete(v) => v.to_js_string(),
        other => panic!("expected completion, got {other:?}"),
    }
}

#[test]
fn date_decomposition() {
    // 2026-06-01T12:30:45Z = 1780317045000 ms (June 1 2026 is a Monday).
    let base = "const d = new Date(1780317045000);";
    assert_eq!(run_str(&format!("{base} d.getUTCFullYear()")), "2026");
    assert_eq!(run_str(&format!("{base} d.getUTCMonth()")), "5"); // 0-indexed June
    assert_eq!(run_str(&format!("{base} d.getUTCDate()")), "1");
    assert_eq!(run_str(&format!("{base} d.getUTCDay()")), "1"); // Monday
    assert_eq!(run_str(&format!("{base} d.getUTCHours()")), "12");
    assert_eq!(run_str(&format!("{base} d.getUTCMinutes()")), "30");
    assert_eq!(run_str(&format!("{base} d.getUTCSeconds()")), "45");
}

#[test]
fn string_from_char_code() {
    assert_eq!(run_str("String.fromCharCode(72, 105, 33)"), "Hi!");
}

#[test]
fn structured_clone_is_deep() {
    assert_eq!(
        run_str(
            "const o={a:[1,2]}; const c=structuredClone(o); c.a.push(3); JSON.stringify([o, c])"
        ),
        r#"[{"a":[1,2]},{"a":[1,2,3]}]"#
    );
}

#[test]
fn array_from_collections() {
    assert_eq!(
        run_str("Array.from(new Set([1,1,2,3])).join(\",\")"),
        "1,2,3"
    );
    assert_eq!(
        run_str("JSON.stringify(Array.from(new Map([[\"a\",1],[\"b\",2]])))"),
        r#"[["a",1],["b",2]]"#
    );
}
