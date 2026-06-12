//! `console.error` / `console.warn` are routed to a separate `stderr` stream,
//! while `console.log` / `info` / `debug` stay on `stdout` (matching Node).
//! The split is threaded through `RunResult` and survives a snapshot
//! dump/load/resume round-trip with both streams in flight.

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, Value, ZapcodeRun, ZapcodeSnapshot};

fn run(code: &str) -> (String, String) {
    let runner = ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        Vec::new(),
        ResourceLimits::default(),
    )
    .unwrap();
    let result = runner.run(Vec::new()).unwrap();
    (result.stdout, result.stderr)
}

#[test]
fn error_and_warn_go_to_stderr_log_to_stdout() {
    // Ground-truthed against Node: log/info/debug -> stdout; error/warn -> stderr.
    let (stdout, stderr) = run(
        r#"
        console.log("out-log");
        console.info("out-info");
        console.debug("out-debug");
        console.error("err-error");
        console.warn("err-warn");
        "#,
    );
    assert_eq!(stdout, "out-log\nout-info\nout-debug\n");
    assert_eq!(stderr, "err-error\nerr-warn\n");
}

#[test]
fn stderr_is_empty_when_nothing_written_to_it() {
    let (stdout, stderr) = run(r#"console.log("only stdout");"#);
    assert_eq!(stdout, "only stdout\n");
    assert_eq!(stderr, "");
}

#[test]
fn error_args_are_joined_like_log() {
    let (stdout, stderr) = run(r#"console.error("a", 1, true);"#);
    assert_eq!(stdout, "");
    assert_eq!(stderr, "a 1 true\n");
}

#[test]
fn stderr_survives_snapshot_dump_load_resume() {
    // Write to both streams, then suspend on an external call. The snapshot must
    // carry the pre-suspension stderr (and stdout); writes after resume append.
    let runner = ZapcodeRun::new(
        r#"
        console.log("before-out");
        console.error("before-err");
        const r = await fetch("https://example.com");
        console.warn("after-warn");
        console.log("after-out");
        r
        "#
        .to_string(),
        Vec::new(),
        vec!["fetch".to_string()],
        ResourceLimits::default(),
    )
    .unwrap();

    let snapshot = match runner.start(Vec::new()).unwrap() {
        VmState::Suspended { snapshot, .. } => snapshot,
        other => panic!("expected suspension, got {other:?}"),
    };

    // Round-trip the snapshot through bytes — the stderr must serialize.
    let bytes = snapshot.dump().unwrap();
    let loaded = ZapcodeSnapshot::load(&bytes).unwrap();

    let result = loaded.resume(Value::String("body".into())).unwrap();
    match &result.state {
        VmState::Complete(Value::String(s)) => assert_eq!(s.as_str(), "body"),
        other => panic!("expected completion with the resumed value, got {other:?}"),
    }
    // The pre-suspension and post-resume writes are present on each stream,
    // in order, with nothing crossed over.
    assert_eq!(result.stdout, "before-out\nafter-out\n");
    assert_eq!(result.stderr, "before-err\nafter-warn\n");
}

#[test]
fn console_assert_writes_to_stderr_only_on_failure() {
    let (out, err) = run(
        "console.assert(1 > 2, 'x exceeds y'); \
         console.assert(2 > 1, 'never shown'); \
         console.log('after'); 0",
    );
    // Passing assert is silent; failing one lands in stderr (not stdout),
    // and execution continues (no throw).
    assert_eq!(out, "after\n");
    assert_eq!(err, "Assertion failed: x exceeds y\n");
}

#[test]
fn console_assert_without_message_uses_default_text() {
    let (_out, err) = run("console.assert(false); 0");
    assert_eq!(err, "Assertion failed\n");
}
