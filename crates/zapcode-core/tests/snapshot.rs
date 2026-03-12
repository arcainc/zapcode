use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, Value, ZapcodeRun, ZapcodeSnapshot};

/// Helper: create a ZapcodeRun with external functions and run start().
fn start_with_externals(
    code: &str,
    external_fns: Vec<&str>,
    inputs: Vec<(String, Value)>,
) -> VmState {
    let runner = ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        external_fns.into_iter().map(|s| s.to_string()).collect(),
        ResourceLimits::default(),
    )
    .unwrap();
    runner.start(inputs).unwrap()
}

#[test]
fn test_snapshot_dump_load_roundtrip() {
    // Code that calls an external function, causing suspension.
    let code = r#"
        const result = fetch("https://example.com");
    "#;

    let state = start_with_externals(code, vec!["fetch"], Vec::new());

    let snapshot = match state {
        VmState::Suspended { snapshot, .. } => snapshot,
        VmState::Complete(_) => panic!("expected suspension"),
    };

    // Dump to bytes
    let bytes = snapshot.dump().unwrap();
    assert!(!bytes.is_empty());

    // Load from bytes
    let loaded = ZapcodeSnapshot::load(&bytes).unwrap();

    // Dump again and verify deterministic
    let bytes2 = loaded.dump().unwrap();
    assert_eq!(bytes, bytes2);
}

#[test]
fn test_snapshot_resume_simple() {
    // Code: call external, use return value
    let code = r#"
        const data = fetch("https://example.com");
        data
    "#;

    let state = start_with_externals(code, vec!["fetch"], Vec::new());

    match state {
        VmState::Suspended {
            function_name,
            args,
            snapshot,
        } => {
            assert_eq!(function_name, "fetch");
            assert_eq!(args.len(), 1);
            assert_eq!(args[0], Value::String("https://example.com".into()));

            // Resume with a return value
            let result = snapshot
                .resume(Value::String("response body".into()))
                .unwrap();

            match result {
                VmState::Complete(v) => {
                    assert_eq!(v, Value::String("response body".into()));
                }
                VmState::Suspended { .. } => panic!("expected completion after resume"),
            }
        }
        VmState::Complete(_) => panic!("expected suspension"),
    }
}

#[test]
fn test_snapshot_resume_with_computation_after() {
    // Code: call external, then do computation with the result
    let code = r#"
        const x = fetch("url");
        x + " processed"
    "#;

    let state = start_with_externals(code, vec!["fetch"], Vec::new());

    match state {
        VmState::Suspended { snapshot, .. } => {
            let result = snapshot.resume(Value::String("data".into())).unwrap();

            match result {
                VmState::Complete(v) => {
                    assert_eq!(v, Value::String("data processed".into()));
                }
                VmState::Suspended { .. } => panic!("expected completion"),
            }
        }
        VmState::Complete(_) => panic!("expected suspension"),
    }
}

#[test]
fn test_snapshot_resume_chain() {
    // Code that calls two external functions in sequence
    let code = r#"
        const a = fetch("url1");
        const b = db(a);
        b
    "#;

    let state = start_with_externals(code, vec!["fetch", "db"], Vec::new());

    // First suspension: fetch
    let snapshot1 = match state {
        VmState::Suspended {
            function_name,
            snapshot,
            ..
        } => {
            assert_eq!(function_name, "fetch");
            snapshot
        }
        _ => panic!("expected first suspension"),
    };

    // Resume fetch with a value, should suspend again at db
    let state2 = snapshot1.resume(Value::String("fetched".into())).unwrap();

    let snapshot2 = match state2 {
        VmState::Suspended {
            function_name,
            args,
            snapshot,
            ..
        } => {
            assert_eq!(function_name, "db");
            assert_eq!(args[0], Value::String("fetched".into()));
            snapshot
        }
        _ => panic!("expected second suspension"),
    };

    // Resume db with final value
    let state3 = snapshot2.resume(Value::String("db result".into())).unwrap();

    match state3 {
        VmState::Complete(v) => {
            assert_eq!(v, Value::String("db result".into()));
        }
        _ => panic!("expected completion"),
    }
}

#[test]
fn test_snapshot_preserves_locals_and_globals() {
    // Verify that local variables survive snapshot/resume
    let code = r#"
        const prefix = "hello";
        const suffix = fetch("url");
        prefix + " " + suffix
    "#;

    let state = start_with_externals(code, vec!["fetch"], Vec::new());

    match state {
        VmState::Suspended { snapshot, .. } => {
            let result = snapshot.resume(Value::String("world".into())).unwrap();
            match result {
                VmState::Complete(v) => {
                    assert_eq!(v, Value::String("hello world".into()));
                }
                _ => panic!("expected completion"),
            }
        }
        _ => panic!("expected suspension"),
    }
}

#[test]
fn test_snapshot_with_inputs() {
    let code = r#"
        const result = fetch(url);
        result
    "#;

    let inputs = vec![("url".to_string(), Value::String("https://test.com".into()))];
    let state = start_with_externals(code, vec!["fetch"], inputs);

    match state {
        VmState::Suspended { args, snapshot, .. } => {
            assert_eq!(args[0], Value::String("https://test.com".into()));
            let result = snapshot.resume(Value::String("ok".into())).unwrap();
            match result {
                VmState::Complete(v) => assert_eq!(v, Value::String("ok".into())),
                _ => panic!("expected completion"),
            }
        }
        _ => panic!("expected suspension"),
    }
}

#[test]
fn test_snapshot_size() {
    // Verify snapshot is compact — should be well under 10KB for simple code
    let code = r#"
        const a = fetch("url1");
        const b = db(a);
        const c = fetch("url2");
        c
    "#;

    let state = start_with_externals(code, vec!["fetch", "db"], Vec::new());

    match state {
        VmState::Suspended { snapshot, .. } => {
            let bytes = snapshot.dump().unwrap();
            assert!(
                bytes.len() < 10_000,
                "snapshot too large: {} bytes (limit 10KB)",
                bytes.len()
            );
            // For typical simple code, should be well under 1KB
            assert!(
                bytes.len() < 2_000,
                "snapshot unexpectedly large: {} bytes",
                bytes.len()
            );
        }
        _ => panic!("expected suspension"),
    }
}

#[test]
fn test_snapshot_dump_load_resume() {
    // Full round-trip: capture → dump → load → resume
    let code = r#"
        const data = fetch("https://example.com");
        data + "!"
    "#;

    let state = start_with_externals(code, vec!["fetch"], Vec::new());

    let snapshot = match state {
        VmState::Suspended { snapshot, .. } => snapshot,
        _ => panic!("expected suspension"),
    };

    // Serialize to bytes (simulating storage / network transfer)
    let bytes = snapshot.dump().unwrap();

    // Deserialize (simulating a different process loading the snapshot)
    let loaded = ZapcodeSnapshot::load(&bytes).unwrap();

    // Resume execution
    let result = loaded.resume(Value::String("response".into())).unwrap();

    match result {
        VmState::Complete(v) => {
            assert_eq!(v, Value::String("response!".into()));
        }
        _ => panic!("expected completion"),
    }
}

#[test]
fn test_snapshot_resume_with_numeric_result() {
    let code = r#"
        const count = getCount();
        count * 2 + 1
    "#;

    let state = start_with_externals(code, vec!["getCount"], Vec::new());

    match state {
        VmState::Suspended { snapshot, .. } => {
            let result = snapshot.resume(Value::Int(21)).unwrap();
            match result {
                VmState::Complete(v) => assert_eq!(v, Value::Int(43)),
                _ => panic!("expected completion"),
            }
        }
        _ => panic!("expected suspension"),
    }
}

#[test]
fn test_snapshot_multi_await_with_let_and_console() {
    // Reproduces the pattern AI models generate: multiple awaits with let bindings
    // and console.log calls between them.
    let code = r#"
        const w1 = await getWeather("Tokyo");
        console.log("Tokyo weather:", w1);
        const w2 = await getWeather("Paris");
        console.log("Paris weather:", w2);
        const tokyoTemp = w1.temp;
        const parisTemp = w2.temp;
        let colderCity = "Paris";
        let warmerCity = "Tokyo";
        if (tokyoTemp < parisTemp) {
            colderCity = "Tokyo";
            warmerCity = "Paris";
        }
        const flights = await searchFlights(colderCity, warmerCity);
        ({ tokyo: w1, paris: w2, colderCity, warmerCity, flights })
    "#;

    let state = start_with_externals(code, vec!["getWeather", "searchFlights"], Vec::new());

    // First suspend: getWeather("Tokyo")
    let snap1 = match state {
        VmState::Suspended {
            function_name,
            snapshot,
            ..
        } => {
            assert_eq!(function_name, "getWeather");
            snapshot
        }
        _ => panic!("expected first suspension"),
    };

    let weather1 = Value::Object(
        vec![
            ("condition".into(), Value::String("Clear".into())),
            ("temp".into(), Value::Int(26)),
        ]
        .into_iter()
        .collect(),
    );
    let state2 = snap1.resume(weather1).unwrap();

    // Second suspend: getWeather("Paris")
    let snap2 = match state2 {
        VmState::Suspended {
            function_name,
            snapshot,
            ..
        } => {
            assert_eq!(function_name, "getWeather");
            snapshot
        }
        _ => panic!("expected second suspension"),
    };

    let weather2 = Value::Object(
        vec![
            ("condition".into(), Value::String("Sunny".into())),
            ("temp".into(), Value::Int(22)),
        ]
        .into_iter()
        .collect(),
    );
    let state3 = snap2.resume(weather2).unwrap();

    // Third suspend: searchFlights
    let snap3 = match state3 {
        VmState::Suspended {
            function_name,
            snapshot,
            ..
        } => {
            assert_eq!(function_name, "searchFlights");
            snapshot
        }
        _ => panic!("expected third suspension"),
    };

    let flights = Value::Array(vec![Value::Object(
        vec![
            ("airline".into(), Value::String("BA".into())),
            ("price".into(), Value::Int(450)),
        ]
        .into_iter()
        .collect(),
    )]);
    let final_state = snap3.resume(flights).unwrap();

    match final_state {
        VmState::Complete(v) => {
            // Should have all fields
            if let Value::Object(map) = &v {
                assert!(map.contains_key("tokyo"));
                assert!(map.contains_key("paris"));
                assert!(map.contains_key("flights"));
            } else {
                panic!("expected object result, got {:?}", v);
            }
        }
        _ => panic!("expected completion"),
    }
}
