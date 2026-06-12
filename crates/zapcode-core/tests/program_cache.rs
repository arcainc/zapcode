//! Compile-once / run-many program caching (`ZapcodeProgram`).

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, Value, ZapcodeProgram, ZapcodeRun, ZapcodeSnapshot};

fn complete_value(state: VmState) -> Value {
    match state {
        VmState::Complete(v) => v,
        VmState::Suspended { function_name, .. } => {
            panic!("unexpected suspension on '{function_name}'")
        }
        VmState::SuspendedMany { .. } => panic!("unexpected batch suspension"),
    }
}

#[test]
fn compile_once_run_twice_identical_results() {
    let program = ZapcodeProgram::compile(
        r#"
        console.log("hi");
        const xs = [1, 2, 3];
        xs.map((x) => x * 2).join("-")
        "#,
        Vec::new(),
    )
    .unwrap();

    let r1 = program.run(Vec::new(), ResourceLimits::default()).unwrap();
    let r2 = program.run(Vec::new(), ResourceLimits::default()).unwrap();

    assert_eq!(complete_value(r1.state), Value::String("2-4-6".into()));
    assert_eq!(complete_value(r2.state), Value::String("2-4-6".into()));
    assert_eq!(r1.stdout, "hi\n");
    assert_eq!(r1.stdout, r2.stdout);
}

#[test]
fn cached_program_sees_fresh_inputs_each_run() {
    let program = ZapcodeProgram::compile("n * 10 + 1", Vec::new()).unwrap();
    for n in [1i64, 7, 42] {
        let result = program
            .run(
                vec![("n".to_string(), Value::Int(n))],
                ResourceLimits::default(),
            )
            .unwrap();
        assert_eq!(complete_value(result.state), Value::Int(n * 10 + 1));
    }
}

#[test]
fn cached_program_matches_zapcode_run() {
    // The pre-compiled path must produce exactly what the parse-every-time
    // path produces.
    let source = r#"
        let total = 0;
        for (let i = 1; i <= 10; i++) { total += i; }
        console.log(`total=${total}`);
        total
    "#;
    let runner = ZapcodeRun::new(
        source.to_string(),
        Vec::new(),
        Vec::new(),
        ResourceLimits::default(),
    )
    .unwrap();
    let via_run = runner.run(Vec::new()).unwrap();

    let program = ZapcodeProgram::compile(source, Vec::new()).unwrap();
    let via_program = program.run(Vec::new(), ResourceLimits::default()).unwrap();

    assert_eq!(
        complete_value(via_run.state),
        complete_value(via_program.state)
    );
    assert_eq!(via_run.stdout, via_program.stdout);
}

#[test]
fn dump_load_run_identical() {
    let program =
        ZapcodeProgram::compile(r#"["a", "b", "c"].map((s) => s.toUpperCase()).join("")"#, Vec::new())
            .unwrap();
    let direct = program.run(Vec::new(), ResourceLimits::default()).unwrap();

    let bytes = program.dump().unwrap();
    let loaded = ZapcodeProgram::load(&bytes).unwrap();
    assert_eq!(loaded.external_functions(), program.external_functions());
    let roundtripped = loaded.run(Vec::new(), ResourceLimits::default()).unwrap();

    assert_eq!(
        complete_value(direct.state),
        complete_value(roundtripped.state)
    );
    assert_eq!(complete_value(
        loaded.run(Vec::new(), ResourceLimits::default()).unwrap().state
    ), Value::String("ABC".into()));
}

#[test]
fn dump_emits_magic_header() {
    let program = ZapcodeProgram::compile("1 + 1", Vec::new()).unwrap();
    let bytes = program.dump().unwrap();
    assert_eq!(&bytes[0..4], b"ZPC1");
}

#[test]
fn load_rejects_version_mismatch() {
    let program = ZapcodeProgram::compile("1 + 1", Vec::new()).unwrap();
    let mut bytes = program.dump().unwrap();
    // Bump the format version (bytes 4..6, little-endian u16) to a future value.
    bytes[4] = bytes[4].wrapping_add(1);
    let err = ZapcodeProgram::load(&bytes).unwrap_err().to_string();
    assert!(err.contains("format version"), "unexpected error: {err}");
}

#[test]
fn load_rejects_tampered_payload() {
    let program = ZapcodeProgram::compile("1 + 1", Vec::new()).unwrap();
    let mut bytes = program.dump().unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    let err = ZapcodeProgram::load(&bytes).unwrap_err().to_string();
    assert!(err.contains("integrity"), "unexpected error: {err}");
}

#[test]
fn program_blob_does_not_load_as_snapshot_and_vice_versa() {
    let program = ZapcodeProgram::compile("1 + 1", Vec::new()).unwrap();
    let program_bytes = program.dump().unwrap();
    let err = ZapcodeSnapshot::load(&program_bytes).unwrap_err().to_string();
    assert!(
        err.contains("expected a snapshot blob but got a program blob"),
        "unexpected error: {err}"
    );

    // And a suspension snapshot must not load as a program.
    let fetcher = ZapcodeProgram::compile(
        r#"const r = await fetch("https://example.com"); r"#,
        vec!["fetch".to_string()],
    )
    .unwrap();
    let snapshot_bytes = match fetcher.start(Vec::new(), ResourceLimits::default()).unwrap() {
        VmState::Suspended { snapshot, .. } => snapshot.dump().unwrap(),
        other => panic!("expected suspension, got {other:?}"),
    };
    let err = ZapcodeProgram::load(&snapshot_bytes).unwrap_err().to_string();
    assert!(
        err.contains("expected a program blob but got a snapshot blob"),
        "unexpected error: {err}"
    );
}

#[test]
fn tool_suspension_flow_from_loaded_program() {
    let program = ZapcodeProgram::compile(
        r#"const r = await fetch("https://example.com"); r + 1"#,
        vec!["fetch".to_string()],
    )
    .unwrap();

    // Persist the compiled program, reload it in (conceptually) another
    // process, and drive a full suspend → snapshot → resume cycle from it.
    let bytes = program.dump().unwrap();
    let loaded = ZapcodeProgram::load(&bytes).unwrap();

    let state = loaded.start(Vec::new(), ResourceLimits::default()).unwrap();
    match state {
        VmState::Suspended {
            function_name,
            args,
            snapshot,
        } => {
            assert_eq!(function_name, "fetch");
            assert_eq!(args.len(), 1);
            // The suspension snapshot itself still round-trips through bytes.
            let snap_bytes = snapshot.dump().unwrap();
            let restored = ZapcodeSnapshot::load(&snap_bytes).unwrap();
            let result = restored.resume(Value::Int(41)).unwrap();
            assert_eq!(complete_value(result.state), Value::Int(42));
        }
        other => panic!("expected suspension on fetch, got {other:?}"),
    }

    // The loaded program is reusable: a second start suspends again.
    match loaded.start(Vec::new(), ResourceLimits::default()).unwrap() {
        VmState::Suspended { function_name, .. } => assert_eq!(function_name, "fetch"),
        other => panic!("expected suspension on fetch, got {other:?}"),
    }
}

#[test]
fn unregistered_external_function_still_fails_from_cached_program() {
    // Compiling without registering `fetch` must not let a cached program
    // suspend on it — same sandbox behavior as ZapcodeRun.
    let program = ZapcodeProgram::compile(
        r#"const r = await fetch("https://example.com"); r"#,
        Vec::new(),
    )
    .unwrap();
    let err = program.run(Vec::new(), ResourceLimits::default());
    assert!(err.is_err(), "expected an error for unregistered fetch");
}
