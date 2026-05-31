use zapcode_core::{
    ResourceLimits, Value, ZapcodeError, ZapcodeSessionSnapshot, ZapcodeSessionState,
};

fn session() -> ZapcodeSessionSnapshot {
    ZapcodeSessionSnapshot::new(Vec::new(), ResourceLimits::default()).unwrap()
}

fn session_with_lookup() -> ZapcodeSessionSnapshot {
    ZapcodeSessionSnapshot::new(vec!["lookup".to_string()], ResourceLimits::default()).unwrap()
}

fn suspended_session(state: ZapcodeSessionState) -> ZapcodeSessionSnapshot {
    match state {
        ZapcodeSessionState::Suspended { session, .. } => session,
        ZapcodeSessionState::Complete { output, .. } => {
            panic!("expected suspension, completed with {output:?}")
        }
    }
}

#[test]
fn session_persists_top_level_bindings_across_dump_load() {
    let state = session()
        .run_chunk(
            r#"
            let count = 1;
            function inc() {
                count = count + 1;
                return count;
            }
            count
            "#
            .to_string(),
            Vec::new(),
        )
        .unwrap();

    let session = match state {
        ZapcodeSessionState::Complete {
            output, session, ..
        } => {
            assert_eq!(output, Value::Int(1));
            session
        }
        ZapcodeSessionState::Suspended { .. } => panic!("expected completion"),
    };

    let dumped = session.dump().unwrap();
    let restored = ZapcodeSessionSnapshot::load(&dumped).unwrap();

    let state = restored.run_chunk("inc()".to_string(), Vec::new()).unwrap();
    match state {
        ZapcodeSessionState::Complete { output, stdout, .. } => {
            assert_eq!(output, Value::Int(2));
            assert_eq!(stdout, "");
        }
        ZapcodeSessionState::Suspended { .. } => panic!("expected completion"),
    }
}

#[test]
fn session_persists_classes_and_mutated_instances() {
    let state = session()
        .run_chunk(
            r#"
            class Counter {
                constructor(start) {
                    this.count = start;
                }
                inc() {
                    this.count = this.count + 1;
                    return this.count;
                }
            }
            const counter = new Counter(10);
            counter.inc()
            "#
            .to_string(),
            Vec::new(),
        )
        .unwrap();

    let session = match state {
        ZapcodeSessionState::Complete {
            output, session, ..
        } => {
            assert_eq!(output, Value::Int(11));
            session
        }
        ZapcodeSessionState::Suspended { .. } => panic!("expected completion"),
    };

    let state = session
        .run_chunk("counter.inc()".to_string(), Vec::new())
        .unwrap();
    match state {
        ZapcodeSessionState::Complete { output, .. } => {
            assert_eq!(output, Value::Int(12));
        }
        ZapcodeSessionState::Suspended { .. } => panic!("expected completion"),
    }
}

#[test]
fn session_resume_then_continue_with_new_chunk() {
    let session =
        ZapcodeSessionSnapshot::new(vec!["fetch".to_string()], ResourceLimits::default()).unwrap();

    let state = session
        .run_chunk(
            r#"
            const prefix = "hello ";
            const data = fetch(url);
            prefix + data
            "#
            .to_string(),
            vec![(
                "url".to_string(),
                Value::String("https://example.com".into()),
            )],
        )
        .unwrap();

    let session = match state {
        ZapcodeSessionState::Suspended {
            function_name,
            args,
            stdout,
            session,
        } => {
            assert_eq!(function_name, "fetch");
            assert_eq!(args, vec![Value::String("https://example.com".into())]);
            assert_eq!(stdout, "");
            session
        }
        ZapcodeSessionState::Complete { .. } => panic!("expected suspension"),
    };

    let dumped = session.dump().unwrap();
    let restored = ZapcodeSessionSnapshot::load(&dumped).unwrap();

    let state = restored.resume(Value::String("world".into())).unwrap();
    let session = match state {
        ZapcodeSessionState::Complete {
            output, session, ..
        } => {
            assert_eq!(output, Value::String("hello world".into()));
            session
        }
        ZapcodeSessionState::Suspended { .. } => panic!("expected completion after resume"),
    };

    let state = session
        .run_chunk("prefix + data + \"!\"".to_string(), Vec::new())
        .unwrap();
    match state {
        ZapcodeSessionState::Complete { output, .. } => {
            assert_eq!(output, Value::String("hello world!".into()));
        }
        ZapcodeSessionState::Suspended { .. } => panic!("expected completion"),
    }
}

#[test]
fn session_preserves_hardened_external_call_patterns() {
    let state = session_with_lookup()
        .run_chunk(
            r#"
            const keys = ["a", "b"];
            const values = await Promise.all(keys.map(async key => await lookup(key)));
            values.join(",")
            "#
            .to_string(),
            Vec::new(),
        )
        .unwrap();

    let state = suspended_session(state)
        .resume(Value::String("A".into()))
        .unwrap();
    let state = suspended_session(state)
        .resume(Value::String("B".into()))
        .unwrap();

    let session = match state {
        ZapcodeSessionState::Complete {
            output, session, ..
        } => {
            assert_eq!(output, Value::String("A,B".into()));
            session
        }
        ZapcodeSessionState::Suspended { .. } => panic!("expected completion after both lookups"),
    };

    let state = session
        .run_chunk(
            r#"
            const more = [];
            for (const key of keys) {
                more.push(await lookup(key));
            }
            more.join(",")
            "#
            .to_string(),
            Vec::new(),
        )
        .unwrap();

    let state = suspended_session(state)
        .resume(Value::String("AA".into()))
        .unwrap();
    let state = suspended_session(state)
        .resume(Value::String("BB".into()))
        .unwrap();

    match state {
        ZapcodeSessionState::Complete { output, .. } => {
            assert_eq!(output, Value::String("AA,BB".into()));
        }
        ZapcodeSessionState::Suspended { .. } => panic!("expected completion after both lookups"),
    }
}

#[test]
fn session_persists_nested_destructuring_and_object_rest() {
    let state = session()
        .run_chunk(
            r#"
            const obj = { a: { b: 3 }, c: 4, d: 5 };
            const { a: { b }, ...rest } = obj;
            b
            "#
            .to_string(),
            Vec::new(),
        )
        .unwrap();

    let session = match state {
        ZapcodeSessionState::Complete {
            output, session, ..
        } => {
            assert_eq!(output, Value::Int(3));
            session
        }
        ZapcodeSessionState::Suspended { .. } => panic!("expected completion"),
    };

    let state = session
        .run_chunk("rest.c + rest.d + b".to_string(), Vec::new())
        .unwrap();
    match state {
        ZapcodeSessionState::Complete { output, .. } => {
            assert_eq!(output, Value::Int(12));
        }
        ZapcodeSessionState::Suspended { .. } => panic!("expected completion"),
    }
}

#[test]
fn session_rejects_top_level_redeclaration() {
    let state = session()
        .run_chunk("const value = 1; value".to_string(), Vec::new())
        .unwrap();

    let session = match state {
        ZapcodeSessionState::Complete { session, .. } => session,
        ZapcodeSessionState::Suspended { .. } => panic!("expected completion"),
    };

    let err = session
        .run_chunk("const value = 2; value".to_string(), Vec::new())
        .unwrap_err();
    match err {
        ZapcodeError::CompileError(message) => {
            assert!(message.contains("value"));
            assert!(message.contains("already been declared"));
        }
        other => panic!("expected compile error, got {:?}", other),
    }
}

#[test]
fn session_rejects_inputs_that_shadow_session_or_reserved_bindings() {
    let state = session()
        .run_chunk("let count = 1; count".to_string(), Vec::new())
        .unwrap();

    let session = match state {
        ZapcodeSessionState::Complete { session, .. } => session,
        ZapcodeSessionState::Suspended { .. } => panic!("expected completion"),
    };

    let err = session
        .run_chunk(
            "count".to_string(),
            vec![("count".to_string(), Value::Int(99))],
        )
        .unwrap_err();
    assert!(matches!(err, ZapcodeError::RuntimeError(message) if message.contains("conflicts")));

    let reserved_err = session
        .run_chunk(
            "console".to_string(),
            vec![("console".to_string(), Value::Int(1))],
        )
        .unwrap_err();
    assert!(
        matches!(reserved_err, ZapcodeError::RuntimeError(message) if message.contains("conflicts"))
    );
}

#[test]
fn session_survives_failed_chunk_attempts() {
    let state = session()
        .run_chunk("let count = 1; count".to_string(), Vec::new())
        .unwrap();

    let session = match state {
        ZapcodeSessionState::Complete { session, .. } => session,
        ZapcodeSessionState::Suspended { .. } => panic!("expected completion"),
    };

    let err = session
        .run_chunk("const count = 2; count".to_string(), Vec::new())
        .unwrap_err();
    assert!(matches!(err, ZapcodeError::CompileError(_)));

    let state = session
        .run_chunk("count + 1".to_string(), Vec::new())
        .unwrap();
    match state {
        ZapcodeSessionState::Complete { output, .. } => {
            assert_eq!(output, Value::Int(2));
        }
        ZapcodeSessionState::Suspended { .. } => panic!("expected completion"),
    }
}

#[test]
fn session_rejects_generators_and_builtin_methods_in_persisted_state() {
    let gen_err = session()
        .run_chunk(
            r#"
            function* makeGen() {
                yield 1;
            }
            const gen = makeGen();
            1
            "#
            .to_string(),
            Vec::new(),
        )
        .unwrap_err();
    assert!(matches!(gen_err, ZapcodeError::SnapshotError(_)));

    let builtin_err = session()
        .run_chunk(
            r#"
            const log = console.log;
            1
            "#
            .to_string(),
            Vec::new(),
        )
        .unwrap_err();
    assert!(matches!(builtin_err, ZapcodeError::SnapshotError(_)));
}
