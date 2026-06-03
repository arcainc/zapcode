//! N9: `for await (const x of iterable)` — await each iterated value before
//! binding. Covers arrays of promises, plain values, mixed, rejection, and a
//! suspend/resume across an external call inside the loop body. Also asserts
//! that an `async function*` declaration still parses (its for-await
//! *consumption* is documented as a gap in STRESS-PASS-BUGS.md).

use zapcode_core::vm::{eval_ts, VmState};
use zapcode_core::{ResourceLimits, Value, ZapcodeRun};

fn run_str(code: &str) -> String {
    let result = ZapcodeRun::new(code.to_string(), Vec::new(), Vec::new(), ResourceLimits::default())
        .unwrap()
        .run(Vec::new())
        .unwrap();
    match result.state {
        VmState::Complete(v) => v.to_js_string(&result.heap),
        other => panic!("expected completion, got {other:?}"),
    }
}

fn start_with_externals(code: &str, external_fns: Vec<&str>) -> VmState {
    ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        external_fns.into_iter().map(|s| s.to_string()).collect(),
        ResourceLimits::default(),
    )
    .unwrap()
    .start(Vec::new())
    .unwrap()
}

#[test]
fn for_await_over_array_of_promises_sums() {
    assert_eq!(
        run_str(
            r#"
            async function main() {
                let sum = 0;
                for await (const x of [Promise.resolve(1), Promise.resolve(2)]) {
                    sum += x;
                }
                return sum;
            }
            main();
        "#
        ),
        "3"
    );
}

#[test]
fn for_await_over_plain_values() {
    // Awaiting a non-promise yields the value unchanged.
    assert_eq!(
        run_str(
            r#"
            async function main() {
                const out = [];
                for await (const x of [10, 20, 30]) {
                    out.push(x * 2);
                }
                return out.join(",");
            }
            main();
        "#
        ),
        "20,40,60"
    );
}

#[test]
fn for_await_over_mixed_promises_and_values() {
    assert_eq!(
        run_str(
            r#"
            async function main() {
                let total = 0;
                for await (const x of [1, Promise.resolve(2), 3, Promise.resolve(4)]) {
                    total += x;
                }
                return total;
            }
            main();
        "#
        ),
        "10"
    );
}

#[test]
fn for_await_destructuring_binding() {
    assert_eq!(
        run_str(
            r#"
            async function main() {
                let sum = 0;
                for await (const [a, b] of [Promise.resolve([1, 2]), Promise.resolve([3, 4])]) {
                    sum += a + b;
                }
                return sum;
            }
            main();
        "#
        ),
        "10"
    );
}

#[test]
fn for_await_break_and_continue() {
    assert_eq!(
        run_str(
            r#"
            async function main() {
                const out = [];
                for await (const x of [1, 2, 3, 4, 5]) {
                    if (x === 2) { continue; }
                    if (x === 4) { break; }
                    out.push(x);
                }
                return out.join(",");
            }
            main();
        "#
        ),
        "1,3"
    );
}

#[test]
fn for_await_nested_does_not_leak_outer_iterator() {
    assert_eq!(
        run_str(
            r#"
            async function main() {
                const out = [];
                for await (const a of [Promise.resolve("a"), Promise.resolve("b")]) {
                    for await (const n of [Promise.resolve(1), Promise.resolve(2)]) {
                        out.push(a + n);
                    }
                }
                return out.join(",");
            }
            main();
        "#
        ),
        "a1,a2,b1,b2"
    );
}

#[test]
fn for_await_rejected_promise_throws_and_is_catchable() {
    assert_eq!(
        run_str(
            r#"
            async function main() {
                try {
                    for await (const x of [Promise.resolve(1), Promise.reject("boom")]) {
                        // first element resolves, second rejects
                    }
                    return "no-throw";
                } catch (e) {
                    return "caught:" + e;
                }
            }
            main();
        "#
        ),
        // Awaiting a rejected promise throws the same RuntimeError that a bare
        // `await Promise.reject(...)` does; for-await inherits that behavior.
        "caught:Error: Unhandled promise rejection: boom"
    );
}

#[test]
fn for_await_suspends_and_resumes_across_external_call_in_body() {
    // The loop body awaits an external call each iteration; the VM must suspend
    // at the external call and resume back into the same loop iteration.
    let code = r#"
        async function main() {
            let sum = 0;
            for await (const x of [Promise.resolve(1), Promise.resolve(2)]) {
                const doubled = await dbl(x);
                sum += doubled;
            }
            return sum;
        }
        main();
    "#;

    // Iteration 1: suspend at dbl(1)
    let state = start_with_externals(code, vec!["dbl"]);
    let snapshot = match state {
        VmState::Suspended {
            function_name,
            args,
            snapshot,
        } => {
            assert_eq!(function_name, "dbl");
            assert_eq!(args, vec![Value::Int(1)]);
            snapshot
        }
        other => panic!("expected first suspension at dbl(1), got {other:?}"),
    };

    // Resume dbl(1) -> 2. Iteration 2: suspend at dbl(2)
    let state2 = snapshot.resume(Value::Int(2)).unwrap().state;
    let snapshot2 = match state2 {
        VmState::Suspended {
            function_name,
            args,
            snapshot,
        } => {
            assert_eq!(function_name, "dbl");
            assert_eq!(args, vec![Value::Int(2)]);
            snapshot
        }
        other => panic!("expected second suspension at dbl(2), got {other:?}"),
    };

    // Resume dbl(2) -> 4. Loop completes: 2 + 4 = 6.
    let final_state = snapshot2.resume(Value::Int(4)).unwrap().state;
    match final_state {
        VmState::Complete(v) => assert_eq!(v, Value::Int(6)),
        other => panic!("expected completion, got {other:?}"),
    }
}

#[test]
fn sync_for_of_still_works_unchanged() {
    // The shared lowering path must not regress plain for-of.
    assert_eq!(
        run_str("const out=[]; for (const x of [1,2,3]) { out.push(x); } out.join(',')"),
        "1,2,3"
    );
}

#[test]
fn async_generator_declaration_parses() {
    // An `async function*` declaration must parse/lower without error.
    let result = eval_ts(
        r#"
        async function* gen() {
            yield 1;
            yield 2;
        }
        typeof gen
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::String("function".into()));
}

#[test]
fn for_await_consumes_async_generator() {
    // The generator machinery extends to async generators: for-await drives the
    // generator's iterator and awaits each yielded value.
    assert_eq!(
        run_str(
            r#"
            async function* gen() {
                yield Promise.resolve(10);
                yield Promise.resolve(20);
                const internal = await Promise.resolve(2);
                yield internal;
            }
            async function main() {
                let sum = 0;
                for await (const x of gen()) { sum += x; }
                return sum;
            }
            main();
        "#
        ),
        "32"
    );
}

#[test]
fn async_generator_external_suspension_is_the_documented_gap() {
    // GAP (STRESS-PASS-BUGS.md): an async generator that suspends on an *external*
    // host call mid-iteration is not supported — generators run synchronously via
    // generator_next, so a suspending CallExternal inside one errors. Internal
    // promise awaits and yielding promises both work; only host-call suspension
    // inside the generator body is unsupported. Pin the behavior so a future fix
    // is a deliberate change.
    let code = r#"
        async function* gen() {
            const a = await fetchVal();
            yield a;
        }
        async function main() {
            let sum = 0;
            for await (const x of gen()) { sum += x; }
            return sum;
        }
        main();
    "#;
    let result = ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        vec!["fetchVal".to_string()],
        ResourceLimits::default(),
    )
    .unwrap()
    .start(Vec::new());
    match result {
        Err(e) => assert!(
            e.to_string().contains("cannot suspend inside a generator"),
            "expected generator-suspension error, got: {e}"
        ),
        Ok(state) => panic!("expected suspension error, got state: {state:?}"),
    }
}
