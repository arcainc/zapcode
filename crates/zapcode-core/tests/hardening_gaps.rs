use zapcode_core::vm::{eval_ts, VmState};
use zapcode_core::{ResourceLimits, Value, ZapcodeRun, ZapcodeSnapshot};

fn start_with_lookup(code: &str) -> VmState {
    let runner = ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        vec!["lookup".to_string()],
        ResourceLimits::default(),
    )
    .unwrap();
    runner.start(Vec::new()).unwrap()
}

fn lookup_snapshot(state: VmState) -> ZapcodeSnapshot {
    match state {
        VmState::Suspended { snapshot, .. } => snapshot,
        VmState::Complete(output) => {
            panic!("expected suspension on lookup, completed with {output:?}")
        }
        VmState::SuspendedMany { .. } => panic!("unexpected batch suspension"),
    }
}

#[test]
fn promise_all_can_snapshot_external_calls() {
    let state = start_with_lookup(
        r#"
        const keys = ["a", "b"];
        const values = await Promise.all(keys.map(async key => await lookup(key)));
        values.join(",")
        "#,
    );

    let state = lookup_snapshot(state)
        .resume(Value::String("A".into()))
        .unwrap()
        .state;
    let state = lookup_snapshot(state)
        .resume(Value::String("B".into()))
        .unwrap()
        .state;

    match state {
        VmState::Complete(output) => assert_eq!(output, Value::String("A,B".into())),
        VmState::Suspended { .. } => panic!("expected completion after both lookups"),
        VmState::SuspendedMany { .. } => panic!("unexpected batch suspension"),
    }
}

#[test]
fn for_of_can_snapshot_awaited_external_calls() {
    let state = start_with_lookup(
        r#"
        const keys = ["a", "b"];
        const values = [];
        for (const key of keys) {
            values.push(await lookup(key));
        }
        values.join(",")
        "#,
    );

    let state = lookup_snapshot(state)
        .resume(Value::String("A".into()))
        .unwrap()
        .state;
    let state = lookup_snapshot(state)
        .resume(Value::String("B".into()))
        .unwrap()
        .state;

    match state {
        VmState::Complete(output) => assert_eq!(output, Value::String("A,B".into())),
        VmState::Suspended { .. } => panic!("expected completion after both lookups"),
        VmState::SuspendedMany { .. } => panic!("unexpected batch suspension"),
    }
}

#[test]
fn nested_object_destructuring_binds_nested_values() {
    let result = eval_ts(
        r#"
        const obj = { a: { b: 3 } };
        const { a: { b } } = obj;
        b
        "#,
    )
    .unwrap();

    assert_eq!(result, Value::Int(3));
}

#[test]
fn object_rest_destructuring_binds_remaining_properties() {
    let result = eval_ts(
        r#"
        const obj = { a: 1, b: 2, c: 3 };
        const { a, ...rest } = obj;
        rest.b + rest.c
        "#,
    )
    .unwrap();

    assert_eq!(result, Value::Int(5));
}

#[test]
fn map_builtin_supports_basic_get_set() {
    let result = eval_ts(
        r#"
        const map = new Map();
        map.set("a", 1);
        map.get("a")
        "#,
    )
    .unwrap();

    assert_eq!(result, Value::Int(1));
}

#[test]
fn date_builtin_supports_epoch_iso_string() {
    let result = eval_ts("new Date(0).toISOString()").unwrap();

    assert_eq!(result, Value::String("1970-01-01T00:00:00.000Z".into()));
}

#[test]
fn string_match_supports_regex_literals() {
    let result = eval_ts(r#""abc".match(/b/)[0]"#).unwrap();

    assert_eq!(result, Value::String("b".into()));
}

#[test]
fn infinite_loop_reports_time_limit() {
    let runner = ZapcodeRun::new(
        "while (true) {}".to_string(),
        Vec::new(),
        Vec::new(),
        ResourceLimits {
            time_limit_ms: 10,
            ..ResourceLimits::default()
        },
    )
    .unwrap();

    let err = runner.run_simple().unwrap_err().to_string();
    assert!(
        err.contains("time limit"),
        "expected time limit error, got {err}"
    );
}

#[test]
fn indexed_for_loop_can_sequence_external_calls() {
    let state = start_with_lookup(
        r#"
        const keys = ["a", "b"];
        const values = [];
        for (let i = 0; i < keys.length; i++) {
            values.push(await lookup(keys[i]));
        }
        values.join(",")
        "#,
    );

    let state = lookup_snapshot(state)
        .resume(Value::String("A".into()))
        .unwrap()
        .state;
    let state = lookup_snapshot(state)
        .resume(Value::String("B".into()))
        .unwrap()
        .state;

    match state {
        VmState::Complete(output) => assert_eq!(output, Value::String("A,B".into())),
        VmState::Suspended { .. } => panic!("expected completion after both lookups"),
        VmState::SuspendedMany { .. } => panic!("unexpected batch suspension"),
    }
}

#[test]
fn direct_sequential_external_calls_can_snapshot_today() {
    let state = start_with_lookup(
        r#"
        const a = await lookup("a");
        const b = await lookup("b");
        a + "," + b
        "#,
    );

    let state = lookup_snapshot(state)
        .resume(Value::String("A".into()))
        .unwrap()
        .state;
    let state = lookup_snapshot(state)
        .resume(Value::String("B".into()))
        .unwrap()
        .state;

    match state {
        VmState::Complete(output) => assert_eq!(output, Value::String("A,B".into())),
        VmState::Suspended { .. } => panic!("expected completion after both lookups"),
        VmState::SuspendedMany { .. } => panic!("unexpected batch suspension"),
    }
}
