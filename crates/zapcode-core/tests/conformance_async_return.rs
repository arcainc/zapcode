//! Conformance: an `async` function call evaluates to a Promise
//! (microtask-design Stage 2).
//!
//! Before this stage an `async` function returned its bare value, so
//! `f().then(cb)` threw (`5` has no `.then`). Now the call site receives a
//! resolved Promise: `.then`/`.catch`/`.finally` chain off it, `await f()`
//! unwraps it, and a returned promise is adopted rather than double-wrapped.
//!
//! Host-boundary contract (asserted here, intentionally different from a JS
//! REPL): the program result is implicitly awaited, so a *settled* promise as
//! the final value delivers its fulfilled value to the host — never the
//! internal promise object — and a rejected final value surfaces as an
//! "Unhandled promise rejection" error.
//!
//! Residual (NOT asserted to Node, see docs/microtask-design.md Stage 3): a
//! `throw` escaping an async body still propagates synchronously to the
//! caller instead of rejecting the returned promise, and `.then` callbacks
//! still run eagerly (no microtask deferral).

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, ZapcodeRun};

/// Run `code` to completion and stringify the result via `to_js_string`.
fn run_str(code: &str) -> String {
    let result = ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        Vec::new(),
        ResourceLimits::default(),
    )
    .unwrap()
    .run(Vec::new())
    .unwrap();
    match result.state {
        VmState::Complete(v) => v.to_js_string(&result.heap),
        other => panic!("expected completion for `{code}`, got {other:?}"),
    }
}

/// Run `code` expecting the program itself to fail; return the error text.
fn run_err(code: &str) -> String {
    let err = ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        Vec::new(),
        ResourceLimits::default(),
    )
    .unwrap()
    .run(Vec::new())
    .expect_err("expected the program to fail");
    err.to_string()
}

// ════════════════════════════════════════════════════════════════════════════
//  The call site receives a Promise
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn async_call_is_object_typed() {
    assert_eq!(
        run_str("async function f() { return 5 } typeof f()"),
        "object"
    );
    // Falling off the end still produces a promise, not undefined.
    assert_eq!(run_str("async function h() {} typeof h()"), "object");
}

#[test]
fn then_on_async_result() {
    // The headline Stage-2 fix: this used to throw (`5` has no `.then`).
    assert_eq!(
        run_str("async function f() { return 5 } await f().then(x => x * 2)"),
        "10"
    );
}

#[test]
fn then_chain_on_async_result() {
    assert_eq!(
        run_str(
            "async function f() { return 5 } \
             await f().then(x => x + 1).then(x => x * 2)"
        ),
        "12"
    );
}

#[test]
fn catch_and_finally_pass_a_fulfillment_through() {
    assert_eq!(
        run_str("async function f() { return 5 } await f().catch(e => 'nope')"),
        "5"
    );
    // `.finally` neither consumes nor replaces the value.
    assert_eq!(
        run_str(
            "let ran = false; \
             async function f() { return 5 } \
             const v = await f().finally(() => { ran = true }); \
             `${v}:${ran}`"
        ),
        "5:true"
    );
}

#[test]
fn async_arrow_and_method_results_chain() {
    assert_eq!(
        run_str("const add1 = async (x) => x + 1; await add1(1).then(x => x * 10)"),
        "20"
    );
    assert_eq!(
        run_str(
            "class A { async m() { return 3 } } \
             await new A().m().then(x => x * 2)"
        ),
        "6"
    );
}

#[test]
fn implicit_return_resolves_with_undefined() {
    assert_eq!(
        run_str("async function h() {} String(await h())"),
        "undefined"
    );
    assert_eq!(
        run_str("async function h() {} await h().then(x => `got:${x}`)"),
        "got:undefined"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  `await f()` and adoption are unchanged
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn await_of_async_call_unwraps() {
    assert_eq!(
        run_str("async function f() { return 5 } const x = await f(); x"),
        "5"
    );
    // Two independent calls settle independently.
    assert_eq!(
        run_str(
            "async function f() { return 5 } \
             const a = await f(); const b = await f(); a + b"
        ),
        "10"
    );
}

#[test]
fn returned_promise_is_adopted_not_double_wrapped() {
    assert_eq!(
        run_str("async function g() { return Promise.resolve(7) } await g()"),
        "7"
    );
    // The `.then` callback sees the adopted value, not a promise.
    assert_eq!(
        run_str(
            "async function g() { return Promise.resolve(7) } \
             await g().then(x => typeof x)"
        ),
        "number"
    );
    // An async function returning another async call adopts transitively.
    assert_eq!(
        run_str(
            "async function f() { return 5 } \
             async function outer() { return f() } \
             await outer()"
        ),
        "5"
    );
}

#[test]
fn async_results_interop_with_combinators() {
    assert_eq!(
        run_str(
            "async function f() { return 5 } \
             async function g() { return Promise.resolve(7) } \
             JSON.stringify(await Promise.all([f(), g()]))"
        ),
        "[5,7]"
    );
    assert_eq!(
        run_str(
            "async function f() { return 5 } \
             await Promise.race([f()])"
        ),
        "5"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  Host boundary: the program result is implicitly awaited
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn unawaited_async_call_as_final_value_delivers_the_value() {
    assert_eq!(run_str("async function f() { return 5 } f()"), "5");
    assert_eq!(
        run_str("async function h() {} String(h())"),
        "[object Promise]" // in-program coercion is still a promise…
    );
    // …but as the *final* value the host receives the fulfillment.
    assert_eq!(run_str("async function f() { return 'done' } f()"), "done");
}

#[test]
fn settled_plain_promise_as_final_value_unwraps() {
    assert_eq!(run_str("Promise.resolve(42)"), "42");
}

#[test]
fn rejected_final_value_surfaces_as_unhandled_rejection() {
    let err = run_err("Promise.reject('boom')");
    assert!(
        err.contains("Unhandled promise rejection") && err.contains("boom"),
        "unexpected error: {err}"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  Async generators are NOT shaped by the async-return path
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn async_generator_iteration_is_unaffected() {
    assert_eq!(
        run_str(
            "async function* gen() { yield 1; yield 2 } \
             async function main() { \
                 const out = []; \
                 for await (const v of gen()) { out.push(v) } \
                 return JSON.stringify(out); \
             } \
             main();"
        ),
        "[1,2]"
    );
}
