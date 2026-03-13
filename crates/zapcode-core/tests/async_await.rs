use zapcode_core::vm::eval_ts;
use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, Value, ZapcodeRun};

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

// ── async function declaration ──────────────────────────────────────

#[test]
fn test_async_function_basic() {
    // An async function that doesn't await anything should work like a regular function.
    let result = eval_ts(
        r#"
        async function greet(name: string): Promise<string> {
            return "hello " + name;
        }
        greet("world")
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::String("hello world".into()));
}

#[test]
fn test_async_arrow_function() {
    let result = eval_ts(
        r#"
        const add = async (a: number, b: number) => a + b;
        add(3, 4)
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Int(7));
}

// ── await on literal / non-promise values ───────────────────────────

#[test]
fn test_await_number_passthrough() {
    let result = eval_ts(
        r#"
        async function f() {
            const x = await 42;
            return x;
        }
        f()
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Int(42));
}

#[test]
fn test_await_string_passthrough() {
    let result = eval_ts(
        r#"
        async function f() {
            const x = await "hello";
            return x;
        }
        f()
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::String("hello".into()));
}

#[test]
fn test_await_undefined_passthrough() {
    let result = eval_ts(
        r#"
        async function f() {
            const x = await undefined;
            return x;
        }
        f()
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Undefined);
}

#[test]
fn test_await_null_passthrough() {
    let result = eval_ts(
        r#"
        async function f() {
            const x = await null;
            return x;
        }
        f()
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Null);
}

#[test]
fn test_await_bool_passthrough() {
    let result = eval_ts(
        r#"
        async function f() {
            const x = await true;
            return x;
        }
        f()
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Bool(true));
}

// ── Promise.resolve ─────────────────────────────────────────────────

#[test]
fn test_promise_resolve_basic() {
    let result = eval_ts(
        r#"
        const p = Promise.resolve(42);
        const val = await p;
        val
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Int(42));
}

#[test]
fn test_promise_resolve_string() {
    let result = eval_ts(
        r#"
        const p = Promise.resolve("hello");
        const val = await p;
        val
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::String("hello".into()));
}

#[test]
fn test_promise_resolve_undefined() {
    let result = eval_ts(
        r#"
        const val = await Promise.resolve(undefined);
        val
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Undefined);
}

#[test]
fn test_promise_resolve_object() {
    let result = eval_ts(
        r#"
        const p = Promise.resolve({ name: "test", value: 123 });
        const obj = await p;
        obj.name
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::String("test".into()));
}

#[test]
fn test_promise_resolve_no_args() {
    let result = eval_ts(
        r#"
        const val = await Promise.resolve();
        val
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Undefined);
}

// ── Promise.reject ──────────────────────────────────────────────────

#[test]
fn test_promise_reject_throws() {
    let result = eval_ts(
        r#"
        async function f() {
            const val = await Promise.reject("oops");
            return val;
        }
        f()
    "#,
    );
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("oops"),
        "error should contain rejection reason, got: {}",
        err
    );
}

#[test]
fn test_promise_reject_caught() {
    let result = eval_ts(
        r#"
        async function f() {
            try {
                const val = await Promise.reject("oops");
                return val;
            } catch (e) {
                return "caught: " + e;
            }
        }
        f()
    "#,
    )
    .unwrap();
    // The error message should contain "oops"
    match result {
        Value::String(s) => assert!(
            s.contains("oops"),
            "caught error should contain 'oops', got: {}",
            s
        ),
        other => panic!("expected string, got {:?}", other),
    }
}

// ── Promise.all ─────────────────────────────────────────────────────

#[test]
fn test_promise_all_resolved() {
    let result = eval_ts(
        r#"
        const p1 = Promise.resolve(1);
        const p2 = Promise.resolve(2);
        const p3 = Promise.resolve(3);
        const all = await Promise.all([p1, p2, p3]);
        all
    "#,
    )
    .unwrap();
    match result {
        Value::Array(arr) => {
            assert_eq!(arr.len(), 3);
            assert_eq!(arr[0], Value::Int(1));
            assert_eq!(arr[1], Value::Int(2));
            assert_eq!(arr[2], Value::Int(3));
        }
        other => panic!("expected array, got {:?}", other),
    }
}

#[test]
fn test_promise_all_with_plain_values() {
    let result = eval_ts(
        r#"
        const all = await Promise.all([1, 2, 3]);
        all
    "#,
    )
    .unwrap();
    match result {
        Value::Array(arr) => {
            assert_eq!(arr.len(), 3);
            assert_eq!(arr[0], Value::Int(1));
            assert_eq!(arr[1], Value::Int(2));
            assert_eq!(arr[2], Value::Int(3));
        }
        other => panic!("expected array, got {:?}", other),
    }
}

// ── await on external function (via CallExternal → suspend) ────────

#[test]
fn test_await_external_function_suspends() {
    let code = r#"
        async function fetchData(url: string) {
            const response = await fetch(url);
            return response;
        }
        fetchData("https://example.com")
    "#;

    let state = start_with_externals(code, vec!["fetch"], Vec::new());

    match state {
        VmState::Suspended {
            function_name,
            args,
            ..
        } => {
            assert_eq!(function_name, "fetch");
            assert_eq!(args.len(), 1);
            assert_eq!(args[0], Value::String("https://example.com".into()));
        }
        VmState::Complete(_) => panic!("expected suspension at external call"),
    }
}

#[test]
fn test_await_external_function_resume() {
    let code = r#"
        async function fetchData(url: string) {
            const response = await fetch(url);
            return response + " processed";
        }
        fetchData("https://example.com")
    "#;

    let state = start_with_externals(code, vec!["fetch"], Vec::new());

    match state {
        VmState::Suspended { snapshot, .. } => {
            let result = snapshot
                .resume(Value::String("response body".into()))
                .unwrap();

            match result {
                VmState::Complete(v) => {
                    assert_eq!(v, Value::String("response body processed".into()));
                }
                VmState::Suspended { .. } => panic!("expected completion after resume"),
            }
        }
        VmState::Complete(_) => panic!("expected suspension at external call"),
    }
}

// ── async function with multiple awaits ─────────────────────────────

#[test]
fn test_multiple_awaits_in_async_function() {
    let result = eval_ts(
        r#"
        async function f() {
            const a = await 10;
            const b = await Promise.resolve(20);
            const c = await 30;
            return a + b + c;
        }
        f()
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Int(60));
}

#[test]
fn test_multiple_external_awaits_suspend_resume() {
    let code = r#"
        async function compute() {
            const a = await getA();
            const b = await getB();
            return a + b;
        }
        compute()
    "#;

    let state = start_with_externals(code, vec!["getA", "getB"], Vec::new());

    // First suspension: getA()
    let snapshot = match state {
        VmState::Suspended {
            function_name,
            snapshot,
            ..
        } => {
            assert_eq!(function_name, "getA");
            snapshot
        }
        VmState::Complete(_) => panic!("expected first suspension"),
    };

    // Resume with result of getA
    let state2 = snapshot.resume(Value::Int(100)).unwrap();

    // Second suspension: getB()
    let snapshot2 = match state2 {
        VmState::Suspended {
            function_name,
            snapshot,
            ..
        } => {
            assert_eq!(function_name, "getB");
            snapshot
        }
        VmState::Complete(_) => panic!("expected second suspension"),
    };

    // Resume with result of getB
    let final_state = snapshot2.resume(Value::Int(200)).unwrap();

    match final_state {
        VmState::Complete(v) => {
            assert_eq!(v, Value::Int(300));
        }
        VmState::Suspended { .. } => panic!("expected completion"),
    }
}

// ── Promise.resolve chaining ────────────────────────────────────────

#[test]
fn test_promise_resolve_of_promise() {
    // Promise.resolve on an already-resolved promise should return it as-is
    let result = eval_ts(
        r#"
        const p1 = Promise.resolve(42);
        const p2 = Promise.resolve(p1);
        const val = await p2;
        val
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Int(42));
}

// ── async function returning a value used by caller ─────────────────

#[test]
fn test_async_function_return_value_used() {
    let result = eval_ts(
        r#"
        async function double(n: number) {
            return n * 2;
        }
        const result = double(21);
        result
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Int(42));
}

// ── await in expression context ─────────────────────────────────────

#[test]
fn test_await_in_expression() {
    let result = eval_ts(
        r#"
        async function f() {
            const result = (await Promise.resolve(10)) + (await Promise.resolve(20));
            return result;
        }
        f()
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Int(30));
}

// ── Promise object without await (just creation) ────────────────────

#[test]
fn test_promise_resolve_creates_object() {
    let result = eval_ts(
        r#"
        const p = Promise.resolve(42);
        p
    "#,
    )
    .unwrap();
    // Should be a promise object (not unwrapped)
    match result {
        Value::Object(map) => {
            assert_eq!(map.get("__promise__"), Some(&Value::Bool(true)));
            assert_eq!(map.get("status"), Some(&Value::String("resolved".into())));
            assert_eq!(map.get("value"), Some(&Value::Int(42)));
        }
        other => panic!("expected object, got {:?}", other),
    }
}

// ── Promise .then() ──────────────────────────────────────────────────

#[test]
fn test_promise_then_resolved() {
    let result = eval_ts(
        r#"
        const p = Promise.resolve(10);
        const p2 = p.then(x => x * 2);
        await p2
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Int(20));
}

#[test]
fn test_promise_then_chain() {
    let result = eval_ts(
        r#"
        const result = await Promise.resolve(5)
            .then(x => x + 1)
            .then(x => x * 3);
        result
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Int(18));
}

#[test]
fn test_promise_then_no_callback() {
    let result = eval_ts(
        r#"
        const p = Promise.resolve(42);
        const p2 = p.then();
        await p2
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Int(42));
}

#[test]
fn test_promise_then_on_rejected() {
    let result = eval_ts(
        r#"
        const p = Promise.reject("oops");
        const p2 = p.then(null, err => "caught: " + err);
        await p2
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::String("caught: oops".into()));
}

#[test]
fn test_promise_then_rejected_passthrough() {
    // .then with only onFulfilled should pass through the rejection
    let result = eval_ts(
        r#"
        async function test() {
            try {
                const p = Promise.reject("fail");
                const p2 = p.then(x => x);
                return await p2;
            } catch (e) {
                return "error: " + e;
            }
        }
        test()
    "#,
    )
    .unwrap();
    match result {
        Value::String(s) => assert!(s.contains("fail"), "should contain 'fail', got: {}", s),
        other => panic!("expected string, got {:?}", other),
    }
}

// ── Promise .catch() ─────────────────────────────────────────────────

#[test]
fn test_promise_catch_rejected() {
    let result = eval_ts(
        r#"
        const p = Promise.reject("bad");
        const p2 = p.catch(err => "recovered: " + err);
        await p2
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::String("recovered: bad".into()));
}

#[test]
fn test_promise_catch_resolved_passthrough() {
    let result = eval_ts(
        r#"
        const p = Promise.resolve(99);
        const p2 = p.catch(err => 0);
        await p2
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Int(99));
}

// ── Promise .finally() ───────────────────────────────────────────────

#[test]
fn test_promise_finally_resolved() {
    // finally runs the callback but preserves the original promise value
    let result = eval_ts(
        r#"
        const p = Promise.resolve(42).finally(() => 999);
        await p
    "#,
    )
    .unwrap();
    // finally does not change the resolved value
    assert_eq!(result, Value::Int(42));
}

#[test]
fn test_promise_finally_does_not_change_value() {
    let result = eval_ts(
        r#"
        const val = await Promise.resolve("original").finally(() => "ignored");
        val
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::String("original".into()));
}

// ── Combined patterns (model-generated style) ────────────────────────

#[test]
fn test_promise_then_with_resolve_pattern() {
    // Pattern: Promise.resolve().then() — common in model-generated code
    let result = eval_ts(
        r#"
        const result = await Promise.resolve(42).then(x => x + 8);
        result
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Int(50));
}

#[test]
fn test_promise_then_catch_chain() {
    let result = eval_ts(
        r#"
        const val = await Promise.resolve(10)
            .then(x => x * 2)
            .catch(e => 0)
            .then(x => x + 5);
        val
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Int(25));
}

// ── Promise.all with async map (external calls) ────────────────────

#[test]
fn test_sequential_external_calls_in_loop() {
    // Sequential external calls using a regular loop (not .map)
    // This pattern already worked before continuations.
    let code = r#"
        const a = await getWeather("London");
        const b = await getWeather("Tokyo");
        const c = await getWeather("Paris");
        [a, b, c]
    "#;

    let state = start_with_externals(code, vec!["getWeather"], Vec::new());

    let snap = match state {
        VmState::Suspended {
            function_name,
            args,
            snapshot,
        } => {
            assert_eq!(function_name, "getWeather");
            assert_eq!(args[0], Value::String("London".into()));
            snapshot
        }
        VmState::Complete(_) => panic!("expected suspension"),
    };

    let state2 = snap.resume(Value::String("rainy".into())).unwrap();
    let snap2 = match state2 {
        VmState::Suspended {
            function_name,
            args,
            snapshot,
        } => {
            assert_eq!(function_name, "getWeather");
            assert_eq!(args[0], Value::String("Tokyo".into()));
            snapshot
        }
        VmState::Complete(_) => panic!("expected second suspension"),
    };

    let state3 = snap2.resume(Value::String("sunny".into())).unwrap();
    let snap3 = match state3 {
        VmState::Suspended {
            function_name,
            args,
            snapshot,
        } => {
            assert_eq!(function_name, "getWeather");
            assert_eq!(args[0], Value::String("Paris".into()));
            snapshot
        }
        VmState::Complete(_) => panic!("expected third suspension"),
    };

    let final_state = snap3.resume(Value::String("cloudy".into())).unwrap();
    match final_state {
        VmState::Complete(Value::Array(arr)) => {
            assert_eq!(arr.len(), 3);
            assert_eq!(arr[0], Value::String("rainy".into()));
            assert_eq!(arr[1], Value::String("sunny".into()));
            assert_eq!(arr[2], Value::String("cloudy".into()));
        }
        other => panic!("expected array, got {:?}", other),
    }
}

#[test]
fn test_array_map_async_callback_with_external() {
    // The core use case: arr.map(async fn => await external())
    let code = r#"
        const items = ["a", "b", "c"];
        const results = items.map(async (item) => {
            const data = await fetchData(item);
            return data;
        });
        results
    "#;

    let state = start_with_externals(code, vec!["fetchData"], Vec::new());

    // First suspension: fetchData("a")
    let snap = match state {
        VmState::Suspended {
            function_name,
            args,
            snapshot,
        } => {
            assert_eq!(function_name, "fetchData");
            assert_eq!(args[0], Value::String("a".into()));
            snapshot
        }
        VmState::Complete(_) => panic!("expected suspension for 'a'"),
    };

    // Resume with result for "a"
    let state2 = snap.resume(Value::String("data_a".into())).unwrap();
    let snap2 = match state2 {
        VmState::Suspended {
            function_name,
            args,
            snapshot,
        } => {
            assert_eq!(function_name, "fetchData");
            assert_eq!(args[0], Value::String("b".into()));
            snapshot
        }
        VmState::Complete(_) => panic!("expected suspension for 'b'"),
    };

    // Resume with result for "b"
    let state3 = snap2.resume(Value::String("data_b".into())).unwrap();
    let snap3 = match state3 {
        VmState::Suspended {
            function_name,
            args,
            snapshot,
        } => {
            assert_eq!(function_name, "fetchData");
            assert_eq!(args[0], Value::String("c".into()));
            snapshot
        }
        VmState::Complete(_) => panic!("expected suspension for 'c'"),
    };

    // Resume with result for "c"
    let final_state = snap3.resume(Value::String("data_c".into())).unwrap();
    match final_state {
        VmState::Complete(Value::Array(arr)) => {
            assert_eq!(arr.len(), 3);
            assert_eq!(arr[0], Value::String("data_a".into()));
            assert_eq!(arr[1], Value::String("data_b".into()));
            assert_eq!(arr[2], Value::String("data_c".into()));
        }
        other => panic!("expected array, got {:?}", other),
    }
}

#[test]
fn test_array_map_async_empty() {
    // Edge case: empty array with async map should return empty array immediately
    let code = r#"
        const items: string[] = [];
        const results = items.map(async (item) => {
            const data = await fetchData(item);
            return data;
        });
        results
    "#;

    let state = start_with_externals(code, vec!["fetchData"], Vec::new());
    match state {
        VmState::Complete(Value::Array(arr)) => {
            assert_eq!(arr.len(), 0);
        }
        other => panic!("expected empty array, got {:?}", other),
    }
}

#[test]
fn test_array_map_sync_still_works() {
    // Regression test: sync .map() must still work as before
    let result = eval_ts(
        r#"
        const nums = [1, 2, 3];
        const doubled = nums.map(x => x * 2);
        doubled
    "#,
    )
    .unwrap();
    match result {
        Value::Array(arr) => {
            assert_eq!(arr, vec![Value::Int(2), Value::Int(4), Value::Int(6)]);
        }
        other => panic!("expected array, got {:?}", other),
    }
}

#[test]
fn test_array_map_async_single_element() {
    // Edge case: single element
    let code = r#"
        const items = ["only"];
        const results = items.map(async (item) => {
            const data = await fetchData(item);
            return data;
        });
        results
    "#;

    let state = start_with_externals(code, vec!["fetchData"], Vec::new());
    let snap = match state {
        VmState::Suspended {
            function_name,
            snapshot,
            ..
        } => {
            assert_eq!(function_name, "fetchData");
            snapshot
        }
        VmState::Complete(_) => panic!("expected suspension"),
    };

    let final_state = snap.resume(Value::String("result".into())).unwrap();
    match final_state {
        VmState::Complete(Value::Array(arr)) => {
            assert_eq!(arr.len(), 1);
            assert_eq!(arr[0], Value::String("result".into()));
        }
        other => panic!("expected array with one element, got {:?}", other),
    }
}

#[test]
fn test_array_for_each_async_callback_with_external() {
    // forEach with async callback should suspend for each external call
    // and complete with undefined (forEach's return value)
    let code = r#"
        const items = ["a", "b", "c"];
        items.forEach(async (item) => {
            await processItem(item);
        });
        "done"
    "#;

    let mut state = start_with_externals(code, vec!["processItem"], Vec::new());

    for expected in &["a", "b", "c"] {
        match state {
            VmState::Suspended {
                function_name,
                args,
                snapshot,
            } => {
                assert_eq!(function_name, "processItem");
                assert_eq!(args[0].to_js_string(), *expected);
                state = snapshot
                    .resume(Value::String(format!("processed_{}", expected).into()))
                    .unwrap();
            }
            VmState::Complete(_) => panic!("expected suspension for {}", expected),
        }
    }

    match state {
        VmState::Complete(val) => {
            assert_eq!(val, Value::String("done".into()));
        }
        other => panic!("expected completion with 'done', got {:?}", other),
    }
}

#[test]
fn test_array_for_each_async_empty() {
    // Empty array forEach should complete immediately
    let code = r#"
        const items: string[] = [];
        items.forEach(async (item) => {
            await processItem(item);
        });
        "done"
    "#;

    let state = start_with_externals(code, vec!["processItem"], Vec::new());
    match state {
        VmState::Complete(val) => {
            assert_eq!(val, Value::String("done".into()));
        }
        VmState::Suspended { .. } => panic!("expected immediate completion for empty array"),
    }
}

#[test]
fn test_array_async_unsupported_methods_filter() {
    // .filter() with async callback should return a clear error
    let code = r#"
        const items = [1, 2, 3];
        items.filter(async (item) => {
            const result = await check(item);
            return result;
        })
    "#;

    let runner = ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        vec!["check".to_string()],
        ResourceLimits::default(),
    )
    .unwrap();
    let result = runner.start(Vec::new());
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("does not support async callbacks"),
        "expected async guard error, got: {}",
        err_msg
    );
}

#[test]
fn test_array_async_unsupported_methods_reduce() {
    // .reduce() with async callback should return a clear error
    let code = r#"
        const items = [1, 2, 3];
        items.reduce(async (acc, item) => {
            const result = await transform(item);
            return acc + result;
        }, 0)
    "#;

    let runner = ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        vec!["transform".to_string()],
        ResourceLimits::default(),
    )
    .unwrap();
    let result = runner.start(Vec::new());
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("does not support async callbacks"),
        "expected async guard error, got: {}",
        err_msg
    );
}

#[test]
fn test_callback_result_user_object_not_unwrapped() {
    // A user object with {status: "resolved", value: ...} should NOT be
    // unwrapped as if it were an internal promise.
    let code = r#"
        const items = ["x"];
        items.map(async (item) => {
            const data = await fetchData(item);
            return data;
        })
    "#;

    let state = start_with_externals(code, vec!["fetchData"], Vec::new());
    let snap = match state {
        VmState::Suspended { snapshot, .. } => snapshot,
        VmState::Complete(_) => panic!("expected suspension"),
    };

    // Return a user object that looks like a promise but lacks __promise__
    let user_obj = Value::Object(indexmap::indexmap! {
        "status".into() => Value::String("resolved".into()),
        "value".into() => Value::Int(42),
    });
    let final_state = snap.resume(user_obj).unwrap();
    match final_state {
        VmState::Complete(Value::Array(arr)) => {
            assert_eq!(arr.len(), 1);
            // The user object should be preserved as-is, not unwrapped to 42
            match &arr[0] {
                Value::Object(map) => {
                    assert_eq!(map.get("status"), Some(&Value::String("resolved".into())));
                    assert_eq!(map.get("value"), Some(&Value::Int(42)));
                    // Must NOT have been unwrapped — it's still an object, not Int(42)
                    assert!(
                        map.get("__promise__").is_none(),
                        "user object should not have __promise__"
                    );
                }
                other => panic!("expected object, got {:?}", other),
            }
        }
        other => panic!("expected array with user object, got {:?}", other),
    }
}
