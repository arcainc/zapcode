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
    // Code that awaits an external function, causing suspension.
    let code = r#"
        const result = await fetch("https://example.com");
    "#;

    let state = start_with_externals(code, vec!["fetch"], Vec::new());

    let snapshot = match state {
        VmState::Suspended { snapshot, .. } => snapshot,
        VmState::Complete(_) => panic!("expected suspension"),
        VmState::SuspendedMany { .. } => panic!("unexpected batch suspension"),
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
    // Code: await external, use return value
    let code = r#"
        const data = await fetch("https://example.com");
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
                .unwrap()
                .state;

            match result {
                VmState::Complete(v) => {
                    assert_eq!(v, Value::String("response body".into()));
                }
                VmState::Suspended { .. } => panic!("expected completion after resume"),
                VmState::SuspendedMany { .. } => panic!("unexpected batch suspension"),
            }
        }
        VmState::Complete(_) => panic!("expected suspension"),
        VmState::SuspendedMany { .. } => panic!("unexpected batch suspension"),
    }
}

#[test]
fn test_snapshot_resume_with_computation_after() {
    // Code: await external, then do computation with the result
    let code = r#"
        const x = await fetch("url");
        x + " processed"
    "#;

    let state = start_with_externals(code, vec!["fetch"], Vec::new());

    match state {
        VmState::Suspended { snapshot, .. } => {
            let result = snapshot.resume(Value::String("data".into())).unwrap().state;

            match result {
                VmState::Complete(v) => {
                    assert_eq!(v, Value::String("data processed".into()));
                }
                VmState::Suspended { .. } => panic!("expected completion"),
                VmState::SuspendedMany { .. } => panic!("unexpected batch suspension"),
            }
        }
        VmState::Complete(_) => panic!("expected suspension"),
        VmState::SuspendedMany { .. } => panic!("unexpected batch suspension"),
    }
}

#[test]
fn test_snapshot_resume_chain() {
    // Code that awaits two external functions in sequence
    let code = r#"
        const a = await fetch("url1");
        const b = await db(a);
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
    let state2 = snapshot1.resume(Value::String("fetched".into())).unwrap().state;

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
    let state3 = snapshot2.resume(Value::String("db result".into())).unwrap().state;

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
        const suffix = await fetch("url");
        prefix + " " + suffix
    "#;

    let state = start_with_externals(code, vec!["fetch"], Vec::new());

    match state {
        VmState::Suspended { snapshot, .. } => {
            let result = snapshot.resume(Value::String("world".into())).unwrap().state;
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
        const result = await fetch(url);
        result
    "#;

    let inputs = vec![("url".to_string(), Value::String("https://test.com".into()))];
    let state = start_with_externals(code, vec!["fetch"], inputs);

    match state {
        VmState::Suspended { args, snapshot, .. } => {
            assert_eq!(args[0], Value::String("https://test.com".into()));
            let result = snapshot.resume(Value::String("ok".into())).unwrap().state;
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
        const a = await fetch("url1");
        const b = await db(a);
        const c = await fetch("url2");
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
        const data = await fetch("https://example.com");
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
    let result = loaded.resume(Value::String("response".into())).unwrap().state;

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
        const count = await getCount();
        count * 2 + 1
    "#;

    let state = start_with_externals(code, vec!["getCount"], Vec::new());

    match state {
        VmState::Suspended { snapshot, .. } => {
            let result = snapshot.resume(Value::Int(21)).unwrap().state;
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
    let mut snap1 = match state {
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

    // Build the compound return value directly in the snapshot's heap so its
    // handle is valid when resumed.
    let weather1 = Value::Object(snap1.heap_mut().alloc_object(
        vec![
            ("condition".into(), Value::String("Clear".into())),
            ("temp".into(), Value::Int(26)),
        ]
        .into_iter()
        .collect(),
    ));
    let state2 = snap1.resume(weather1).unwrap().state;

    // Second suspend: getWeather("Paris")
    let mut snap2 = match state2 {
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

    let weather2 = Value::Object(snap2.heap_mut().alloc_object(
        vec![
            ("condition".into(), Value::String("Sunny".into())),
            ("temp".into(), Value::Int(22)),
        ]
        .into_iter()
        .collect(),
    ));
    let state3 = snap2.resume(weather2).unwrap().state;

    // Third suspend: searchFlights
    let mut snap3 = match state3 {
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

    let flight = Value::Object(snap3.heap_mut().alloc_object(
        vec![
            ("airline".into(), Value::String("BA".into())),
            ("price".into(), Value::Int(450)),
        ]
        .into_iter()
        .collect(),
    ));
    let flights = Value::Array(snap3.heap_mut().alloc_array(vec![flight]));
    let final_state = snap3.resume(flights).unwrap();

    match final_state.state {
        VmState::Complete(v) => {
            // Should have all fields
            if let Value::Object(h) = &v {
                let map = final_state.heap.object_map(*h);
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

// ════════════════════════════════════════════════════════════════════════════
//  Builtin-template elision (wire v12): snapshots stay small and STAY small
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn snapshot_size_constant_across_hops() {
    // Re-registration used to append ~40 duplicate builtin objects per
    // resume, growing every subsequent snapshot. With the template prefix
    // reused on restore, only the guest's own state can grow a snapshot.
    let code = "async function main() { \
                    let n = 0; \
                    for (const id of ['a', 'b', 'c', 'd', 'e']) { \
                        await callTool(id); n += 1; \
                    } \
                    return n; \
                } \
                main();";
    let runner = ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        vec!["callTool".to_string()],
        ResourceLimits::default(),
    )
    .unwrap();
    let mut sizes = Vec::new();
    let mut state = runner.start(Vec::new()).unwrap();
    loop {
        match state {
            VmState::Suspended { snapshot, .. } => {
                let bytes = snapshot.dump().unwrap();
                sizes.push(bytes.len());
                state = ZapcodeSnapshot::load(&bytes)
                    .unwrap()
                    .resume(Value::Int(1))
                    .unwrap()
                    .state;
            }
            VmState::Complete(v) => {
                assert_eq!(v, Value::Int(5));
                break;
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(sizes.len(), 5);
    // Each iteration leaves a few dead heap slots (the arena has no GC), so
    // a couple of bytes per hop is inherent. The re-registration leak this
    // guards against appended ~40 builtin objects (~70 deflated bytes) per
    // hop — assert growth stays an order of magnitude below that.
    let per_hop = sizes[4].saturating_sub(sizes[1]) / 3;
    assert!(
        per_hop < 10,
        "snapshot grew {per_hop} bytes/hop: {sizes:?} (builtin re-registration leak?)"
    );
    // And the whole snapshot stays comfortably under the 2 KB target.
    assert!(sizes.iter().all(|s| *s < 2048), "sizes: {sizes:?}");
}

#[test]
fn mutated_builtin_object_survives_hop_and_disables_elision() {
    // Guest code CAN write to a builtin object. The mutation must survive a
    // dump/load (the restored heap keeps the mutated prefix slot — the run
    // behaves exactly like the in-memory run), which forces the snapshot to
    // carry the full heap instead of eliding the template.
    let code = "Math.tag = 41; \
                async function main() { \
                    Math.tag += 1; \
                    await callTool('x'); \
                    return Math.tag; \
                } \
                main();";
    let drive = |hop: bool| {
        let runner = ZapcodeRun::new(
            code.to_string(),
            Vec::new(),
            vec!["callTool".to_string()],
            ResourceLimits::default(),
        )
        .unwrap();
        match runner.start(Vec::new()).unwrap() {
            VmState::Suspended { snapshot, .. } => {
                let snapshot = if hop {
                    ZapcodeSnapshot::load(&snapshot.dump().unwrap()).unwrap()
                } else {
                    snapshot
                };
                match snapshot.resume(Value::Int(0)).unwrap().state {
                    VmState::Complete(v) => v,
                    other => panic!("unexpected {other:?}"),
                }
            }
            other => panic!("expected suspension, got {other:?}"),
        }
    };
    assert_eq!(drive(false), Value::Int(42));
    assert_eq!(drive(true), Value::Int(42), "hop diverged from in-memory run");
}

/// Object-key interning is a pure runtime accelerator: it must not change
/// snapshot wire bytes (postcard writes each key's bytes regardless of whether
/// the backing `Arc` is shared) and must not introduce any nondeterminism.
/// Two captures of the same object-heavy run must dump byte-identically, and a
/// dump/load round-trip must be byte-stable.
#[test]
fn interned_keys_do_not_change_snapshot_bytes_or_determinism() {
    let code = r#"
        const records = [];
        for (let i = 0; i < 50; i++) {
            records.push({ id: i, name: "u" + i, status: "ok", meta: { source: "api" } });
        }
        async function main() { return records.length + ":" + (await callTool("x")); }
        main();
    "#;
    let dump_once = || -> Vec<u8> {
        match start_with_externals(code, vec!["callTool"], Vec::new()) {
            VmState::Suspended { snapshot, .. } => snapshot.dump().unwrap(),
            other => panic!("expected suspension, got {other:?}"),
        }
    };
    // Two independent runs of the same program produce identical bytes (the
    // interner introduces no ordering or pointer-dependent nondeterminism).
    let a = dump_once();
    let b = dump_once();
    assert_eq!(a, b, "interning made snapshot bytes nondeterministic");

    // Load (which runs the post-load reintern pass) then re-dump: byte-stable.
    let loaded = ZapcodeSnapshot::load(&a).unwrap();
    let c = loaded.dump().unwrap();
    assert_eq!(a, c, "reintern pass perturbed snapshot bytes on load");
}

/// A parked->resumed VM must regain correct behavior after the post-load
/// reintern pass — keys re-shared in place must still read back the right
/// values and the resumed computation must produce the in-memory result.
#[test]
fn resume_after_reintern_preserves_object_behavior() {
    let code = r#"
        const rows = [];
        for (let i = 0; i < 30; i++) {
            rows.push({ id: i, tag: "t" + (i % 3) });
        }
        async function main() {
            const extra = await callTool("x");
            // Read back interned keys after resume; mutate via computed key too.
            rows[0]["tag"] = "mutated";
            return rows.length + ":" + rows[0].id + ":" + rows[0].tag + ":" + extra;
        }
        main();
    "#;
    let drive = |hop: bool| -> Value {
        match start_with_externals(code, vec!["callTool"], Vec::new()) {
            VmState::Suspended { snapshot, .. } => {
                let snapshot = if hop {
                    ZapcodeSnapshot::load(&snapshot.dump().unwrap()).unwrap()
                } else {
                    snapshot
                };
                match snapshot.resume(Value::String("done".into())).unwrap().state {
                    VmState::Complete(v) => v,
                    other => panic!("unexpected {other:?}"),
                }
            }
            other => panic!("expected suspension, got {other:?}"),
        }
    };
    let expected = Value::String("30:0:mutated:done".into());
    assert_eq!(drive(false), expected);
    assert_eq!(drive(true), expected, "resume after reintern diverged");
}
