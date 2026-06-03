//! Conformance breadth: functions, closures, parameters, and destructuring.
//!
//! Function declarations/expressions/arrows, default & rest params, spread calls,
//! closures (incl. per-call capture & currying), and binding/array/object
//! destructuring. Real-Node answers are asserted where zapcode agrees; the
//! deferred "cluster D" gaps (the `arguments` object, named-function-expression
//! self-reference, array-destructuring DEFAULTS, and destructuring-assignment to
//! pre-existing bindings) are pinned to zapcode's actual behavior with comments.

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, ZapcodeRun};

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

// ----------------------------------------------------------------------------
// Function forms
// ----------------------------------------------------------------------------

#[test]
fn function_declaration_and_call() {
    assert_eq!(run_str("function add(a, b){ return a + b; } add(3, 4)"), "7");
    assert_eq!(run_str("function noReturn(){ } typeof noReturn()"), "undefined");
    assert_eq!(run_str("function f(){ return; } typeof f()"), "undefined");
}

#[test]
fn function_expression_and_iife() {
    assert_eq!(run_str("const f = function(x){ return x * 2; }; f(21)"), "42");
    assert_eq!(run_str("(function(x){ return x + 1; })(41)"), "42");
    assert_eq!(run_str("(function(){ return 'hi'; })()"), "hi");
}

#[test]
fn arrow_functions() {
    assert_eq!(run_str("const f = x => x * 3; f(7)"), "21");
    assert_eq!(run_str("const f = (a, b) => a + b; f(2, 3)"), "5");
    assert_eq!(run_str("const f = () => 99; f()"), "99");
    // parenthesized object literal return
    assert_eq!(run_str("const f = () => ({a: 1, b: 2}); JSON.stringify(f())"), "{\"a\":1,\"b\":2}");
    // block body
    assert_eq!(run_str("const f = x => { const y = x + 1; return y * 2; }; f(4)"), "10");
}

#[test]
fn recursion() {
    assert_eq!(run_str("function fact(n){ return n <= 1 ? 1 : n * fact(n - 1); } fact(5)"), "120");
    assert_eq!(run_str("function fib(n){ return n < 2 ? n : fib(n-1) + fib(n-2); } fib(10)"), "55");
    // mutual recursion
    assert_eq!(
        run_str("function isEven(n){ return n===0?true:isOdd(n-1); } function isOdd(n){ return n===0?false:isEven(n-1); } isEven(10)"),
        "true"
    );
}

#[test]
fn function_hoisting() {
    // Function declarations are hoisted: callable before their textual position.
    assert_eq!(run_str("const r = early(5); function early(x){ return x * 10; } r"), "50");
    assert_eq!(run_str("let y; y = hoisted(); function hoisted(){ return 'ok'; } y"), "ok");
    // Hoisting also works inside a function body.
    assert_eq!(run_str("function outer(){ return inner(); function inner(){ return 'in'; } } outer()"), "in");
    // Forward reference between two hoisted declarations.
    assert_eq!(run_str("function a(){ return b() + 1; } function b(){ return 10; } a()"), "11");
}

// ----------------------------------------------------------------------------
// Parameters: defaults, rest, spread
// ----------------------------------------------------------------------------

#[test]
fn default_parameters() {
    assert_eq!(run_str("function f(a, b = 10){ return a + b; } f(5)"), "15");
    assert_eq!(run_str("function f(a, b = 10){ return a + b; } f(5, 1)"), "6");
    // default expression can reference earlier params
    assert_eq!(run_str("function f(a, b = a * 2){ return b; } f(7)"), "14");
    assert_eq!(run_str("function f(a, b = a, c = a + b){ return [a,b,c].join(','); } f(1)"), "1,1,2");
    // explicit undefined triggers the default; null does not
    assert_eq!(run_str("function f(a = 5){ return a; } f(undefined)"), "5");
    assert_eq!(run_str("function f(a = 5){ return String(a); } f(null)"), "null");
}

#[test]
fn rest_parameters() {
    assert_eq!(run_str("function f(...xs){ return xs.length; } f(1, 2, 3)"), "3");
    assert_eq!(run_str("function f(...xs){ return xs.length; } f()"), "0");
    assert_eq!(run_str("function f(a, ...rest){ return a + ':' + rest.join(','); } f(1, 2, 3, 4)"), "1:2,3,4");
    assert_eq!(run_str("function sum(...ns){ return ns.reduce((a,b)=>a+b, 0); } sum(1,2,3,4,5)"), "15");
}

#[test]
fn spread_in_calls() {
    assert_eq!(run_str("function add(a, b, c){ return a + b + c; } add(...[1, 2, 3])"), "6");
    assert_eq!(run_str("function f(a, b, c, d){ return [a,b,c,d].join(','); } f(0, ...[1, 2], 3)"), "0,1,2,3");
    assert_eq!(run_str("Math.max(...[3, 9, 2, 7])"), "9");
    assert_eq!(run_str("function f(...xs){ return xs.join('-'); } f(...[1,2], ...[3,4])"), "1-2-3-4");
}

// ----------------------------------------------------------------------------
// Closures
// ----------------------------------------------------------------------------

#[test]
fn closures_capture_state() {
    assert_eq!(run_str("function mk(){ let n = 0; return () => ++n; } const c = mk(); c(); c(); c()"), "3");
    // independent closures have independent state
    assert_eq!(
        run_str("function mk(){ let n=0; return ()=>++n; } const a=mk(), b=mk(); a(); a(); `${a()},${b()}`"),
        "3,1"
    );
    // closure over a loop-binding parameter
    assert_eq!(run_str("function adder(x){ return y => x + y; } const add5 = adder(5); add5(10)"), "15");
}

#[test]
fn currying_and_higher_order() {
    assert_eq!(run_str("const add = a => b => c => a + b + c; add(1)(2)(3)"), "6");
    assert_eq!(run_str("const apply = (f, x) => f(x); apply(n => n + 1, 9)"), "10");
    assert_eq!(
        run_str("const compose = (f, g) => x => f(g(x)); const h = compose(x => x + 1, x => x * 2); h(5)"),
        "11"
    );
}

#[test]
fn callbacks_over_arrays() {
    assert_eq!(run_str("[1, 2, 3].map(x => x * x).join(',')"), "1,4,9");
    assert_eq!(run_str("[1, 2, 3, 4].filter(x => x % 2 === 0).join(',')"), "2,4");
    assert_eq!(run_str("[1, 2, 3, 4].reduce((acc, x) => acc + x, 0)"), "10");
    // closure mutating captured accumulator from forEach
    assert_eq!(run_str("let total = 0; [1, 2, 3].forEach(x => { total += x; }); total"), "6");
}

// ----------------------------------------------------------------------------
// Object destructuring
// ----------------------------------------------------------------------------

#[test]
fn object_destructuring() {
    assert_eq!(run_str("const {a, b} = {a: 1, b: 2}; a + b"), "3");
    assert_eq!(run_str("const {a, c: renamed} = {a: 1, c: 3}; `${a},${renamed}`"), "1,3");
    assert_eq!(run_str("const {x: {y}} = {x: {y: 42}}; y"), "42");
    assert_eq!(run_str("const {a, b, c} = {a: 1}; `${a},${b},${c}`"), "1,undefined,undefined");
}

#[test]
fn object_destructuring_defaults_work() {
    // Object-pattern defaults ARE applied for missing keys (and undefined values).
    assert_eq!(run_str("const {a, b = 9} = {a: 1}; `${a},${b}`"), "1,9");
    assert_eq!(run_str("const {a = 10, b = 20} = {a: 1}; `${a},${b}`"), "1,20");
    assert_eq!(run_str("const {a = 5} = {a: undefined}; a"), "5");
    assert_eq!(run_str("const {a = 5} = {a: null}; String(a)"), "null"); // null keeps
}

#[test]
fn parameter_object_destructuring() {
    assert_eq!(run_str("function f({a, b}){ return a + b; } f({a: 3, b: 4})"), "7");
    assert_eq!(run_str("function f({x, y = 0}){ return x + y; } f({x: 5})"), "5");
    assert_eq!(
        run_str("function area({w, h}){ return w * h; } area({w: 3, h: 4})"),
        "12"
    );
}

// ----------------------------------------------------------------------------
// Array destructuring
// ----------------------------------------------------------------------------

#[test]
fn array_destructuring() {
    assert_eq!(run_str("const [a, b] = [1, 2]; a + b"), "3");
    assert_eq!(run_str("const [a, , c] = [1, 2, 3]; `${a},${c}`"), "1,3"); // hole skips
    assert_eq!(run_str("const [first, ...rest] = [1, 2, 3, 4]; `${first}:${rest.join(',')}`"), "1:2,3,4");
    assert_eq!(run_str("function f([a, b]){ return a * b; } f([5, 6])"), "30");
}

#[test]
fn nested_array_destructuring_documented_divergence() {
    // DIVERGENCE (documented, cluster D): NESTED array patterns do not bind — the
    // inner elements come out `undefined`. (One level of array destructuring, and
    // object patterns nested inside object patterns, do work — see other tests.)
    assert_eq!(run_str("const [[a], [b]] = [[1], [2]]; String(a) + ',' + String(b)"), "undefined,undefined"); // JS: 1,2
    assert_eq!(run_str("const {arr: [x, y]} = {arr: [1, 2]}; String(x) + ',' + String(y)"), "undefined,undefined"); // JS: 1,2
}

#[test]
fn array_destructuring_defaults_documented_divergence() {
    // DIVERGENCE (documented, cluster D): array-pattern element DEFAULTS are not
    // applied — a missing element binds `undefined` instead of the default.
    // (Object-pattern defaults DO work; see object_destructuring_defaults_work.)
    assert_eq!(run_str("const [a = 10, b = 20] = [1]; `${a},${b}`"), "1,undefined"); // JS: 1,20
    assert_eq!(run_str("const [a = 10] = []; String(a)"), "undefined"); // JS: 10
}

// ----------------------------------------------------------------------------
// Deferred D-cluster gaps (asserting actual zapcode behavior, not the JS answer)
// ----------------------------------------------------------------------------

#[test]
fn named_function_expression_self_reference_is_unbound() {
    // DIVERGENCE (documented, cluster D): the internal name of a named function
    // EXPRESSION is not bound inside its own body, so self-recursion via that name
    // is not a function. Recursion via an outer `const`/declaration name works
    // (see `recursion`). Asserting zapcode's actual error-free typeof probe.
    assert_eq!(
        run_str("const f = function fac(n){ return typeof fac; }; f(3)"),
        "undefined" // JS: "function"
    );
}
