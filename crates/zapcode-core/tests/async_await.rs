use zapcode_core::vm::eval_ts;
use zapcode_core::{ZapcodeRun, ResourceLimits, Value};
use zapcode_core::vm::VmState;

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
    let result = eval_ts(r#"
        async function greet(name: string): Promise<string> {
            return "hello " + name;
        }
        greet("world")
    "#).unwrap();
    assert_eq!(result, Value::String("hello world".into()));
}

#[test]
fn test_async_arrow_function() {
    let result = eval_ts(r#"
        const add = async (a: number, b: number) => a + b;
        add(3, 4)
    "#).unwrap();
    assert_eq!(result, Value::Int(7));
}

// ── await on literal / non-promise values ───────────────────────────

#[test]
fn test_await_number_passthrough() {
    let result = eval_ts(r#"
        async function f() {
            const x = await 42;
            return x;
        }
        f()
    "#).unwrap();
    assert_eq!(result, Value::Int(42));
}

#[test]
fn test_await_string_passthrough() {
    let result = eval_ts(r#"
        async function f() {
            const x = await "hello";
            return x;
        }
        f()
    "#).unwrap();
    assert_eq!(result, Value::String("hello".into()));
}

#[test]
fn test_await_undefined_passthrough() {
    let result = eval_ts(r#"
        async function f() {
            const x = await undefined;
            return x;
        }
        f()
    "#).unwrap();
    assert_eq!(result, Value::Undefined);
}

#[test]
fn test_await_null_passthrough() {
    let result = eval_ts(r#"
        async function f() {
            const x = await null;
            return x;
        }
        f()
    "#).unwrap();
    assert_eq!(result, Value::Null);
}

#[test]
fn test_await_bool_passthrough() {
    let result = eval_ts(r#"
        async function f() {
            const x = await true;
            return x;
        }
        f()
    "#).unwrap();
    assert_eq!(result, Value::Bool(true));
}

// ── Promise.resolve ─────────────────────────────────────────────────

#[test]
fn test_promise_resolve_basic() {
    let result = eval_ts(r#"
        const p = Promise.resolve(42);
        const val = await p;
        val
    "#).unwrap();
    assert_eq!(result, Value::Int(42));
}

#[test]
fn test_promise_resolve_string() {
    let result = eval_ts(r#"
        const p = Promise.resolve("hello");
        const val = await p;
        val
    "#).unwrap();
    assert_eq!(result, Value::String("hello".into()));
}

#[test]
fn test_promise_resolve_undefined() {
    let result = eval_ts(r#"
        const val = await Promise.resolve(undefined);
        val
    "#).unwrap();
    assert_eq!(result, Value::Undefined);
}

#[test]
fn test_promise_resolve_object() {
    let result = eval_ts(r#"
        const p = Promise.resolve({ name: "test", value: 123 });
        const obj = await p;
        obj.name
    "#).unwrap();
    assert_eq!(result, Value::String("test".into()));
}

#[test]
fn test_promise_resolve_no_args() {
    let result = eval_ts(r#"
        const val = await Promise.resolve();
        val
    "#).unwrap();
    assert_eq!(result, Value::Undefined);
}

// ── Promise.reject ──────────────────────────────────────────────────

#[test]
fn test_promise_reject_throws() {
    let result = eval_ts(r#"
        async function f() {
            const val = await Promise.reject("oops");
            return val;
        }
        f()
    "#);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("oops"), "error should contain rejection reason, got: {}", err);
}

#[test]
fn test_promise_reject_caught() {
    let result = eval_ts(r#"
        async function f() {
            try {
                const val = await Promise.reject("oops");
                return val;
            } catch (e) {
                return "caught: " + e;
            }
        }
        f()
    "#).unwrap();
    // The error message should contain "oops"
    match result {
        Value::String(s) => assert!(s.contains("oops"), "caught error should contain 'oops', got: {}", s),
        other => panic!("expected string, got {:?}", other),
    }
}

// ── Promise.all ─────────────────────────────────────────────────────

#[test]
fn test_promise_all_resolved() {
    let result = eval_ts(r#"
        const p1 = Promise.resolve(1);
        const p2 = Promise.resolve(2);
        const p3 = Promise.resolve(3);
        const all = await Promise.all([p1, p2, p3]);
        all
    "#).unwrap();
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
    let result = eval_ts(r#"
        const all = await Promise.all([1, 2, 3]);
        all
    "#).unwrap();
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
        VmState::Suspended {
            snapshot,
            ..
        } => {
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
    let result = eval_ts(r#"
        async function f() {
            const a = await 10;
            const b = await Promise.resolve(20);
            const c = await 30;
            return a + b + c;
        }
        f()
    "#).unwrap();
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
        VmState::Suspended { function_name, snapshot, .. } => {
            assert_eq!(function_name, "getA");
            snapshot
        }
        VmState::Complete(_) => panic!("expected first suspension"),
    };

    // Resume with result of getA
    let state2 = snapshot.resume(Value::Int(100)).unwrap();

    // Second suspension: getB()
    let snapshot2 = match state2 {
        VmState::Suspended { function_name, snapshot, .. } => {
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
    let result = eval_ts(r#"
        const p1 = Promise.resolve(42);
        const p2 = Promise.resolve(p1);
        const val = await p2;
        val
    "#).unwrap();
    assert_eq!(result, Value::Int(42));
}

// ── async function returning a value used by caller ─────────────────

#[test]
fn test_async_function_return_value_used() {
    let result = eval_ts(r#"
        async function double(n: number) {
            return n * 2;
        }
        const result = double(21);
        result
    "#).unwrap();
    assert_eq!(result, Value::Int(42));
}

// ── await in expression context ─────────────────────────────────────

#[test]
fn test_await_in_expression() {
    let result = eval_ts(r#"
        async function f() {
            const result = (await Promise.resolve(10)) + (await Promise.resolve(20));
            return result;
        }
        f()
    "#).unwrap();
    assert_eq!(result, Value::Int(30));
}

// ── Promise object without await (just creation) ────────────────────

#[test]
fn test_promise_resolve_creates_object() {
    let result = eval_ts(r#"
        const p = Promise.resolve(42);
        p
    "#).unwrap();
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
