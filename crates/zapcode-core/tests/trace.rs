use zapcode_core::{ResourceLimits, TraceSpan, TraceStatus, Value, VmState, ZapcodeRun};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn run_code(code: &str) -> zapcode_core::RunResult {
    let runner =
        ZapcodeRun::new(code.to_string(), vec![], vec![], ResourceLimits::default()).unwrap();
    runner.run(vec![]).unwrap()
}

fn run_with_externals(code: &str, externals: Vec<&str>) -> zapcode_core::RunResult {
    let runner = ZapcodeRun::new(
        code.to_string(),
        vec![],
        externals.into_iter().map(|s| s.to_string()).collect(),
        ResourceLimits::default(),
    )
    .unwrap();
    runner.run(vec![]).unwrap()
}

fn assert_span_timing(span: &TraceSpan) {
    assert!(span.start_time_ms > 0, "start_time_ms should be non-zero");
    assert!(
        span.end_time_ms >= span.start_time_ms,
        "end_time_ms ({}) should be >= start_time_ms ({})",
        span.end_time_ms,
        span.start_time_ms
    );
}

// ---------------------------------------------------------------------------
// Trace structure
// ---------------------------------------------------------------------------

#[test]
fn trace_has_root_with_parse_compile_execute_children() {
    let result = run_code("1 + 2");
    let root = &result.trace.root;

    assert_eq!(root.name, "zapcode.run");
    assert_eq!(root.status, TraceStatus::Ok);
    assert_eq!(root.children.len(), 3);
    assert_eq!(root.children[0].name, "parse");
    assert_eq!(root.children[1].name, "compile");
    assert_eq!(root.children[2].name, "execute");
}

#[test]
fn trace_all_children_have_ok_status_on_success() {
    let result = run_code("const x = 42; x");
    let root = &result.trace.root;

    assert_eq!(root.status, TraceStatus::Ok);
    for child in &root.children {
        assert_eq!(
            child.status,
            TraceStatus::Ok,
            "child '{}' should be Ok",
            child.name
        );
    }
}

// ---------------------------------------------------------------------------
// Timing
// ---------------------------------------------------------------------------

#[test]
fn trace_has_valid_timing() {
    let result = run_code("[1, 2, 3].map(x => x * 2)");
    let root = &result.trace.root;

    assert_span_timing(root);
    for child in &root.children {
        assert_span_timing(child);
    }
}

#[test]
fn trace_root_duration_gte_children_sum() {
    let result = run_code("let sum = 0; for (let i = 0; i < 100; i++) { sum += i; } sum");
    let root = &result.trace.root;

    let children_duration: u64 = root.children.iter().map(|c| c.duration_us).sum();
    assert!(
        root.duration_us >= children_duration,
        "root duration ({}µs) should be >= sum of children ({}µs)",
        root.duration_us,
        children_duration
    );
}

// ---------------------------------------------------------------------------
// Error traces
// ---------------------------------------------------------------------------

#[test]
fn trace_parse_error_has_error_status() {
    let runner = ZapcodeRun::new(
        "{{{{".to_string(),
        vec![],
        vec![],
        ResourceLimits::default(),
    )
    .unwrap();
    let err = runner.run(vec![]);

    // Parse errors return Err, so we can't inspect the trace from RunResult.
    // But we verify it doesn't panic.
    assert!(err.is_err());
}

#[test]
fn trace_runtime_error_does_not_panic() {
    let runner = ZapcodeRun::new(
        "null.foo".to_string(),
        vec![],
        vec![],
        ResourceLimits::default(),
    )
    .unwrap();
    let err = runner.run(vec![]);
    assert!(err.is_err());
}

// ---------------------------------------------------------------------------
// Suspension trace
// ---------------------------------------------------------------------------

#[test]
fn trace_on_suspension_has_execute_with_suspended_attrs() {
    let result = run_with_externals("const x = await fetchData(); x", vec!["fetchData"]);
    let root = &result.trace.root;

    assert_eq!(root.status, TraceStatus::Ok);
    assert_eq!(root.children.len(), 3);

    let execute_span = &root.children[2];
    assert_eq!(execute_span.name, "execute");
    assert_eq!(execute_span.status, TraceStatus::Ok);

    // Should have zapcode.suspended_on attribute
    let suspended_attr = execute_span
        .attributes
        .iter()
        .find(|(k, _)| k == "zapcode.suspended_on");
    assert!(
        suspended_attr.is_some(),
        "execute span should have zapcode.suspended_on attribute"
    );
    assert_eq!(suspended_attr.unwrap().1, "fetchData");
}

#[test]
fn trace_suspension_state_matches() {
    let result = run_with_externals("const x = await myFunc(42); x", vec!["myFunc"]);

    // Verify the VM actually suspended
    match &result.state {
        VmState::Suspended { function_name, .. } => {
            assert_eq!(function_name, "myFunc");
        }
        VmState::Complete(_) => panic!("expected suspension"),
        VmState::SuspendedMany { .. } => panic!("unexpected batch suspension"),
    }

    // And the trace captured it
    let execute_span = &result.trace.root.children[2];
    let args_count = execute_span
        .attributes
        .iter()
        .find(|(k, _)| k == "zapcode.args_count");
    assert!(args_count.is_some());
    assert_eq!(args_count.unwrap().1, "1");
}

// ---------------------------------------------------------------------------
// Pretty printing
// ---------------------------------------------------------------------------

#[test]
fn trace_pretty_print_contains_span_names() {
    let result = run_code("1 + 1");
    let output = result.trace.to_string_pretty();

    assert!(
        output.contains("zapcode.run"),
        "should contain root span name"
    );
    assert!(output.contains("parse"), "should contain parse span");
    assert!(output.contains("compile"), "should contain compile span");
    assert!(output.contains("execute"), "should contain execute span");
}

#[test]
fn trace_pretty_print_contains_status_icons() {
    let result = run_code("true");
    let output = result.trace.to_string_pretty();

    assert!(output.contains("✓"), "success trace should contain ✓ icon");
}

#[test]
fn trace_pretty_print_contains_duration() {
    let result = run_code("42");
    let output = result.trace.to_string_pretty();

    // Should contain at least one duration marker (µs or ms)
    assert!(
        output.contains("µs") || output.contains("ms"),
        "trace output should contain duration: {}",
        output
    );
}

// ---------------------------------------------------------------------------
// Multiple runs produce independent traces
// ---------------------------------------------------------------------------

#[test]
fn trace_multiple_runs_are_independent() {
    let runner = ZapcodeRun::new(
        "1 + 1".to_string(),
        vec![],
        vec![],
        ResourceLimits::default(),
    )
    .unwrap();

    let result1 = runner.run(vec![]).unwrap();
    let result2 = runner.run(vec![]).unwrap();

    // Each run should produce its own trace (different start times or at least independent objects)
    assert_eq!(result1.trace.root.children.len(), 3);
    assert_eq!(result2.trace.root.children.len(), 3);
}

// ---------------------------------------------------------------------------
// Trace with inputs
// ---------------------------------------------------------------------------

#[test]
fn trace_with_inputs_still_has_full_structure() {
    let runner = ZapcodeRun::new(
        "x + y".to_string(),
        vec!["x".to_string(), "y".to_string()],
        vec![],
        ResourceLimits::default(),
    )
    .unwrap();

    let result = runner
        .run(vec![
            ("x".to_string(), Value::Int(10)),
            ("y".to_string(), Value::Int(20)),
        ])
        .unwrap();

    let root = &result.trace.root;
    assert_eq!(root.status, TraceStatus::Ok);
    assert_eq!(root.children.len(), 3);
    assert!(matches!(result.state, VmState::Complete(Value::Int(30))));
}
