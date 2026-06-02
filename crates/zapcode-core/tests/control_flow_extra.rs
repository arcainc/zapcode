//! Regression tests for top-level switch, switch/loop interaction, and misc
//! correctness fixes (Number(''), indexOf fromIndex, JSON.stringify indent).

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
        VmState::Complete(v) => v.to_js_string(&result.heap),
        other => panic!("expected completion, got {other:?}"),
    }
}

#[test]
fn top_level_switch_does_not_loop() {
    assert_eq!(
        run_str("const x=2; let r; switch (x) { case 1: r=\"one\"; break; case 2: r=\"two\"; break; default: r=\"other\"; } r"),
        "two"
    );
    assert_eq!(
        run_str("let r; switch (9) { case 1: r=\"a\"; break; default: r=\"d\"; } r"),
        "d"
    );
}

#[test]
fn switch_fallthrough_and_continue_target_enclosing_loop() {
    assert_eq!(
        run_str("let r=[]; switch (1) { case 1: r.push(\"a\"); case 2: r.push(\"b\"); break; case 3: r.push(\"c\"); } r.join(\",\")"),
        "a,b"
    );
    // `continue` inside a switch continues the enclosing for loop.
    assert_eq!(
        run_str("let out=[]; for (let i=0;i<3;i++) { switch(i){ case 1: continue; } out.push(i); } out.join(\",\")"),
        "0,2"
    );
}

#[test]
fn labeled_break_and_continue() {
    assert_eq!(
        run_str("const out=[]; outer: for (let i=0;i<3;i++){ for (let j=0;j<3;j++){ if (i===1&&j===1) break outer; out.push(i+\":\"+j);} } out.join(\",\")"),
        "0:0,0:1,0:2,1:0"
    );
    assert_eq!(
        run_str("const out=[]; outer: for (let i=0;i<3;i++){ for (let j=0;j<3;j++){ if (j===1) continue outer; out.push(i+\":\"+j);} } out.join(\",\")"),
        "0:0,1:0,2:0"
    );
    // Unlabeled break/continue unaffected.
    assert_eq!(
        run_str(
            "const out=[]; for (let i=0;i<5;i++){ if(i===2) break; out.push(i);} out.join(\",\")"
        ),
        "0,1"
    );
    assert_eq!(
        run_str("const out=[]; for (let i=0;i<4;i++){ if(i===1) continue; out.push(i);} out.join(\",\")"),
        "0,2,3"
    );
}

#[test]
fn number_of_empty_string_is_zero() {
    assert_eq!(run_str("Number(\"\")"), "0");
    assert_eq!(run_str("Number(\"  \") + 5"), "5");
    assert_eq!(run_str("Number(\"abc\")"), "NaN");
}

#[test]
fn index_of_with_from_index() {
    assert_eq!(run_str("\"abcabc\".indexOf(\"a\", 1)"), "3");
    assert_eq!(run_str("\"abcabc\".indexOf(\"a\", 4)"), "-1");
    assert_eq!(run_str("\"abcabc\".indexOf(\"a\")"), "0");
}

#[test]
fn json_stringify_pretty_prints() {
    assert_eq!(
        run_str("JSON.stringify({a:1, b:[2,3]}, null, 2)"),
        "{\n  \"a\": 1,\n  \"b\": [\n    2,\n    3\n  ]\n}"
    );
    // No space arg → compact.
    assert_eq!(run_str("JSON.stringify({a:1})"), r#"{"a":1}"#);
}
