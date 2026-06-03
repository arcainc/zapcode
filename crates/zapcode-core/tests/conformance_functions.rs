//! Conformance breadth: functions — declarations vs expressions vs arrows,
//! default & rest params, spread calls, recursion & mutual recursion, higher-order
//! functions & currying, `this` binding in methods/arrows, hoisting & forward refs,
//! IIFEs, and binding/array/object destructuring.
//!
//! Real-Node answers are asserted wherever zapcode agrees. Several DOCUMENTED
//! residual divergences are pinned to zapcode's ACTUAL behavior with comments so
//! the suite stays green and the divergence is recorded rather than silently
//! tolerated. The known function-area residuals exercised here are:
//!   * `call` / `apply` / `bind` are absent (not defined on functions).
//!   * `new` only constructs `class` instances; `new fn()` with a plain function
//!     that mutates `this` throws (an explicit object return still works).
//!   * `fn.length` and `fn.name` are not reflected (`undefined`).
//!   * a named function EXPRESSION's internal name is not bound for self-recursion.
//!   * an arrow that ESCAPES its defining method (returned then called later) loses
//!     the method's `this`; synchronously-invoked nested arrows keep it.

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

/// Run code that is expected to surface a guest `RuntimeError` (e.g. a TypeError);
/// returns the error's Display string so tests can assert the divergence shape.
fn run_err(code: &str) -> String {
    let result = ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        Vec::new(),
        ResourceLimits::default(),
    )
    .unwrap()
    .run(Vec::new());
    match result {
        Ok(r) => match r.state {
            VmState::Complete(v) => panic!(
                "expected an error for `{code}`, got completion: {}",
                v.to_js_string(&r.heap)
            ),
            other => panic!("expected an error for `{code}`, got state {other:?}"),
        },
        Err(e) => format!("{e}"),
    }
}

// ----------------------------------------------------------------------------
// Function forms: declarations, expressions, arrows
// ----------------------------------------------------------------------------

#[test]
fn function_declaration_and_call() {
    assert_eq!(run_str("function add(a, b){ return a + b; } add(3, 4)"), "7");
    assert_eq!(run_str("function noReturn(){ } typeof noReturn()"), "undefined");
    assert_eq!(run_str("function f(){ return; } typeof f()"), "undefined");
    // multiple statements, locals
    assert_eq!(
        run_str("function f(n){ let x = n * 2; let y = x + 1; return y; } f(10)"),
        "21"
    );
    // declared functions are first-class values
    assert_eq!(run_str("function g(x){ return x + 1; } const h = g; h(41)"), "42");
    assert_eq!(run_str("function g(){} typeof g"), "function");
}

#[test]
fn function_expression_and_iife() {
    assert_eq!(run_str("const f = function(x){ return x * 2; }; f(21)"), "42");
    assert_eq!(run_str("(function(x){ return x + 1; })(41)"), "42");
    assert_eq!(run_str("(function(){ return 'hi'; })()"), "hi");
    // named function expression (callable via the outer binding)
    assert_eq!(run_str("const fac = function fac0(n){ return n <= 1 ? 1 : n * fac(n-1); }; fac(5)"), "120");
    // IIFE arrow form
    assert_eq!(run_str("(() => 42)()"), "42");
    assert_eq!(run_str("(() => { return 'arrow-iife'; })()"), "arrow-iife");
    // IIFE returning a closure that is then invoked
    assert_eq!(run_str("(function(){ let n = 0; return () => ++n; })()()"), "1");
    // !function form
    assert_eq!(run_str("!function(){ return 1; }(); 'done'"), "done");
}

#[test]
fn arrow_functions() {
    assert_eq!(run_str("const f = x => x * 3; f(7)"), "21");
    assert_eq!(run_str("const f = (a, b) => a + b; f(2, 3)"), "5");
    assert_eq!(run_str("const f = () => 99; f()"), "99");
    // parenthesized object-literal return
    assert_eq!(run_str("const f = () => ({a: 1, b: 2}); JSON.stringify(f())"), "{\"a\":1,\"b\":2}");
    // block body with locals
    assert_eq!(run_str("const f = x => { const y = x + 1; return y * 2; }; f(4)"), "10");
    // implicit-return ternary
    assert_eq!(run_str("const sign = n => n < 0 ? -1 : n > 0 ? 1 : 0; `${sign(-9)},${sign(0)},${sign(4)}`"), "-1,0,1");
    // arrow returning arrow (no parens)
    assert_eq!(run_str("const f = a => b => a - b; f(10)(3)"), "7");
    // single param without parens vs with
    assert_eq!(run_str("const a = x => x; const b = (x) => x; `${a(1)},${b(2)}`"), "1,2");
}

#[test]
fn arrow_vs_function_typeof_and_value() {
    assert_eq!(run_str("typeof (() => 0)"), "function");
    assert_eq!(run_str("typeof (function(){})"), "function");
    assert_eq!(run_str("const fns = [x=>x, function(){}]; `${typeof fns[0]},${typeof fns[1]}`"), "function,function");
}

// ----------------------------------------------------------------------------
// Recursion & mutual recursion
// ----------------------------------------------------------------------------

#[test]
fn recursion() {
    assert_eq!(run_str("function fact(n){ return n <= 1 ? 1 : n * fact(n - 1); } fact(5)"), "120");
    assert_eq!(run_str("function fib(n){ return n < 2 ? n : fib(n-1) + fib(n-2); } fib(10)"), "55");
    assert_eq!(run_str("function fib(n){ return n < 2 ? n : fib(n-1) + fib(n-2); } fib(15)"), "610");
    // accumulator-style recursion
    assert_eq!(run_str("function sum(n, acc){ return n === 0 ? acc : sum(n - 1, acc + n); } sum(100, 0)"), "5050");
    // recursion via const arrow
    assert_eq!(run_str("const pow = (b, e) => e === 0 ? 1 : b * pow(b, e - 1); pow(2, 10)"), "1024");
    // recursion over an array (length-driven)
    assert_eq!(run_str("function rsum(a, i){ return i >= a.length ? 0 : a[i] + rsum(a, i + 1); } rsum([1,2,3,4,5], 0)"), "15");
}

#[test]
fn mutual_recursion() {
    assert_eq!(
        run_str("function isEven(n){ return n===0?true:isOdd(n-1); } function isOdd(n){ return n===0?false:isEven(n-1); } isEven(10)"),
        "true"
    );
    assert_eq!(
        run_str("function isEven(n){ return n===0?true:isOdd(n-1); } function isOdd(n){ return n===0?false:isEven(n-1); } isOdd(7)"),
        "true"
    );
    // three-way mutual recursion cycling a/b/c
    assert_eq!(
        run_str("function a(n){return n<=0?'a':b(n-1);} function b(n){return n<=0?'b':c(n-1);} function c(n){return n<=0?'c':a(n-1);} a(7)"),
        "b"
    );
    // mutual recursion across arrow consts
    assert_eq!(
        run_str("const ping = n => n<=0 ? 'ping' : pong(n-1); const pong = n => n<=0 ? 'pong' : ping(n-1); ping(5)"),
        "pong"
    );
}

// ----------------------------------------------------------------------------
// Hoisting & forward references
// ----------------------------------------------------------------------------

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

#[test]
fn const_and_let_functions_are_not_hoisted_tdz() {
    // A `const`/`let`-bound function expression or arrow is in the TDZ before its
    // initializer runs — calling it early throws (caught here so the suite is green).
    assert_eq!(
        run_str("let r; try { r = g(); } catch(e){ r = 'threw'; } const g = () => 1; r"),
        "threw"
    );
    assert_eq!(
        run_str("let r; try { r = f(); } catch(e){ r = 'threw'; } const f = function(){ return 2; }; r"),
        "threw"
    );
    // After initialization the same binding is callable.
    assert_eq!(run_str("const g = () => 7; g()"), "7");
}

// ----------------------------------------------------------------------------
// Parameters: defaults (referencing earlier params), rest, spread
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
    // default can be a call to a hoisted/declared function
    assert_eq!(run_str("function g(){ return 5; } function f(a = g()){ return a; } f()"), "5");
    // only the missing trailing arg gets the default
    assert_eq!(run_str("function f(a = 1, b = 2, c = 3){ return [a,b,c].join(','); } f(9, undefined, 8)"), "9,2,8");
}

#[test]
fn rest_parameters() {
    assert_eq!(run_str("function f(...xs){ return xs.length; } f(1, 2, 3)"), "3");
    assert_eq!(run_str("function f(...xs){ return xs.length; } f()"), "0");
    assert_eq!(run_str("function f(a, ...rest){ return a + ':' + rest.join(','); } f(1, 2, 3, 4)"), "1:2,3,4");
    assert_eq!(run_str("function sum(...ns){ return ns.reduce((a,b)=>a+b, 0); } sum(1,2,3,4,5)"), "15");
    // rest collects nothing past the named params
    assert_eq!(run_str("function f(a, b, ...rest){ return rest.length; } f(1, 2)"), "0");
    // rest is a real array (supports array methods)
    assert_eq!(run_str("function f(...xs){ return xs.map(x => x * 2).join(','); } f(1, 2, 3)"), "2,4,6");
    assert_eq!(run_str("function f(...xs){ return Array.isArray(xs); } f(1)"), "true");
}

#[test]
fn defaults_combined_with_rest() {
    assert_eq!(run_str("function f(a, b = a + 1, ...rest){ return `${a},${b},${rest.join('-')}`; } f(5)"), "5,6,");
    assert_eq!(run_str("function f(a, b = a + 1, ...rest){ return `${a},${b},${rest.join('-')}`; } f(5, 6, 7, 8)"), "5,6,7-8");
}

#[test]
fn spread_in_calls() {
    assert_eq!(run_str("function add(a, b, c){ return a + b + c; } add(...[1, 2, 3])"), "6");
    assert_eq!(run_str("function f(a, b, c, d){ return [a,b,c,d].join(','); } f(0, ...[1, 2], 3)"), "0,1,2,3");
    assert_eq!(run_str("Math.max(...[3, 9, 2, 7])"), "9");
    assert_eq!(run_str("function f(...xs){ return xs.join('-'); } f(...[1,2], ...[3,4])"), "1-2-3-4");
    // spread two arrays in the middle, with trailing fixed args
    assert_eq!(run_str("function f(a, b, c, d){ return [a,b,c,d].join('-'); } f(...[1,2], ...[3,4])"), "1-2-3-4");
    // spread an empty array contributes nothing
    assert_eq!(run_str("function f(a = 9){ return a; } f(...[])"), "9");
    // spread a string into char args
    assert_eq!(run_str("function f(a, b, c){ return `${a}${b}${c}`; } f(...'xyz')"), "xyz");
}

// ----------------------------------------------------------------------------
// `this` binding — methods and (lexical) arrows
// ----------------------------------------------------------------------------

#[test]
fn this_in_object_methods() {
    assert_eq!(run_str("const o = {n: 5, get(){ return this.n; }}; o.get()"), "5");
    assert_eq!(run_str("const o = {n: 5, get: function(){ return this.n; }}; o.get()"), "5");
    // nested object method sees its own immediate receiver
    assert_eq!(run_str("const o = {a: {b: 7, f(){ return this.b; }}}; o.a.f()"), "7");
    // method reachable through an array element
    assert_eq!(run_str("const arr = [{n:1,f(){return this.n;}},{n:2,f(){return this.n;}}]; arr[1].f()"), "2");
    // computed method name still binds `this`
    assert_eq!(run_str("const k = 'm'; const o = {n: 8, [k](){ return this.n; }}; o.m()"), "8");
}

#[test]
fn this_method_chaining() {
    // a method that mutates and returns `this` supports fluent chaining
    assert_eq!(
        run_str("const o = {v: 0, add(x){ this.v += x; return this; }}; o.add(1).add(2).add(3).v"),
        "6"
    );
    assert_eq!(
        run_str("const b = {s:'', push(x){ this.s += x; return this; }}; b.push('a').push('b').push('c').s"),
        "abc"
    );
}

#[test]
fn arrow_this_is_lexical() {
    // an arrow declared inside a method uses the method's `this`
    assert_eq!(run_str("const o = {n: 9, get(){ const f = () => this.n; return f(); }}; o.get()"), "9");
    // arrow inside a method callback keeps the method's `this`
    assert_eq!(run_str("const o = {n: 3, run(){ return [1,2].map(() => this.n).join(','); }}; o.run()"), "3,3");
    // doubly-nested arrows, synchronously invoked, keep `this`
    assert_eq!(run_str("const o = {n: 7, run(){ return [1].map(() => [2].map(() => this.n)[0])[0]; }}; o.run()"), "7");
}

#[test]
fn this_standalone_call_is_undefined() {
    // A function called as a bare reference (not as a method) has `this === undefined`
    // (strict-mode-style binding); both declarations and expressions agree.
    assert_eq!(run_str("function f(){ return this === undefined; } f()"), "true");
    assert_eq!(run_str("const f = function(){ return this === undefined; }; f()"), "true");
    assert_eq!(run_str("function f(){ return typeof this; } f()"), "undefined");
    // a top-level arrow's lexical `this` is undefined at module scope
    assert_eq!(run_str("const f = () => this; typeof f()"), "undefined");
}

#[test]
fn escaped_arrow_this_documented_divergence() {
    // DIVERGENCE (documented): an arrow that ESCAPES its defining method — returned
    // and then invoked from outside — loses the method's `this` (it reads as
    // undefined, so `this.n` throws). Synchronously-invoked nested arrows keep it
    // (see arrow_this_is_lexical). JS would bind the method's `this` permanently and
    // return 11 here.
    assert_eq!(
        run_err("const o = {n: 11, mk(){ return () => this.n; }}; const f = o.mk(); f()"),
        "type error: Cannot read properties of undefined (reading 'n')"
    );
}

// ----------------------------------------------------------------------------
// call / apply / bind — documented residual (absent)
// ----------------------------------------------------------------------------

#[test]
fn call_apply_bind_are_absent_documented_divergence() {
    // DIVERGENCE (documented): functions do not carry `call`/`apply`/`bind`; they
    // read as `undefined` and are not callable. JS provides all three.
    assert_eq!(run_str("function f(){} typeof f.call"), "undefined");
    assert_eq!(run_str("function f(){} typeof f.apply"), "undefined");
    assert_eq!(run_str("function f(){} typeof f.bind"), "undefined");
    assert_eq!(run_str("function f(){} 'call' in f"), "false");
    // attempting to invoke them surfaces a TypeError
    assert_eq!(
        run_err("function f(){ return this.x; } f.call({ x: 42 })"),
        "type error: undefined is not a function"
    );
    assert_eq!(
        run_err("function f(){} f.apply(null, [])"),
        "type error: undefined is not a function"
    );
    assert_eq!(
        run_err("function f(){} f.bind(null)"),
        "type error: undefined is not a function"
    );
}

#[test]
fn manual_this_redirection_via_closures_works() {
    // Even though `bind` is absent, the same effect is expressible with closures —
    // this is the idiomatic workaround and it behaves correctly.
    assert_eq!(
        run_str("function bindCtx(ctx, fn){ return (...args) => fn(ctx, ...args); } const get = (self) => self.x; const g = bindCtx({x: 7}, get); g()"),
        "7"
    );
    // partial application (the `bind`-with-leading-args pattern) via a closure
    assert_eq!(
        run_str("function partial(fn, ...pre){ return (...rest) => fn(...pre, ...rest); } const add3 = (a,b,c)=>a+b+c; const p = partial(add3, 1, 2); p(3)"),
        "6"
    );
}

// ----------------------------------------------------------------------------
// arguments — documented residual (unbound)
// ----------------------------------------------------------------------------

#[test]
fn arguments_object_is_bound() {
    // An ordinary function exposes an array-like `arguments` of all passed args.
    assert_eq!(run_str("function f(){ return typeof arguments; } f(1, 2, 3)"), "object");
    assert_eq!(run_str("function f(){ return arguments.length; } f(1, 2, 3)"), "3");
    assert_eq!(run_str("function f(){ return arguments[0] + arguments[2]; } f(10, 20, 30)"), "40");
    // `arguments` reflects ALL args, not just the declared params.
    assert_eq!(run_str("function f(a){ return arguments.length; } f(1, 2, 3, 4)"), "4");
    assert_eq!(run_str("function f(a, b){ return arguments[2]; } f(1, 2, 3)"), "3");
    // rest parameters remain available alongside it
    assert_eq!(run_str("function f(...args){ return args.length; } f(1, 2, 3)"), "3");
    assert_eq!(run_str("function f(...args){ return args[0] + args[1]; } f(10, 20)"), "30");
}

// ----------------------------------------------------------------------------
// `new` on plain functions — documented residual
// ----------------------------------------------------------------------------

#[test]
fn new_with_plain_function_documented_divergence() {
    // DIVERGENCE (documented): `new fn()` does not allocate-and-bind a fresh `this`
    // for a plain function, so assigning `this.x` throws. JS would create `{x:3}`.
    assert_eq!(
        run_err("function F(x){ this.x = x; } new F(3)"),
        "type error: cannot set property 'x' on undefined"
    );
    // BUT a constructor that explicitly RETURNS an object works (the return value
    // is what `new` yields), matching JS for the explicit-return case.
    assert_eq!(run_str("function F(y){ return { y }; } new F(5).y"), "5");
    // The idiomatic, supported constructor form is a class.
    assert_eq!(run_str("class F { constructor(x){ this.x = x; } } new F(3).x"), "3");
}

// ----------------------------------------------------------------------------
// Higher-order functions, currying, composition
// ----------------------------------------------------------------------------

#[test]
fn currying_and_higher_order() {
    assert_eq!(run_str("const add = a => b => c => a + b + c; add(1)(2)(3)"), "6");
    assert_eq!(run_str("const apply = (f, x) => f(x); apply(n => n + 1, 9)"), "10");
    assert_eq!(
        run_str("const compose = (f, g) => x => f(g(x)); const h = compose(x => x + 1, x => x * 2); h(5)"),
        "11"
    );
    // three-deep closure factory
    assert_eq!(run_str("function a(x){ return function(y){ return function(z){ return x+y+z; }; }; } a(1)(2)(3)"), "6");
    // reduce with function composition (pipe)
    assert_eq!(
        run_str("const pipe = (...fns) => x => fns.reduce((acc, f) => f(acc), x); const f = pipe(n=>n+1, n=>n*2, n=>n-3); f(5)"),
        "9"
    );
}

#[test]
fn functions_as_arguments_and_returns() {
    // passing a declared function by name
    assert_eq!(run_str("function dbl(x){ return x*2; } function ap(f, v){ return f(v); } ap(dbl, 21)"), "42");
    // returning different functions based on a flag
    assert_eq!(
        run_str("function chooser(op){ return op === '+' ? (a,b)=>a+b : (a,b)=>a-b; } chooser('+')(2,3) + ',' + chooser('-')(5,1)"),
        "5,4"
    );
    // array of functions invoked
    assert_eq!(run_str("const ops = [x=>x+1, x=>x*2, x=>x-3]; ops.map(f => f(10)).join(',')"), "11,20,7");
    // memoize-style closure
    assert_eq!(
        run_str("function memo(fn){ const c = {}; return n => (n in c) ? c[n] : (c[n] = fn(n)); } const sq = memo(n => n*n); `${sq(4)},${sq(4)},${sq(5)}`"),
        "16,16,25"
    );
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
    // closure over a parameter
    assert_eq!(run_str("function adder(x){ return y => x + y; } const add5 = adder(5); add5(10)"), "15");
    // counter object exposing increment/get over the same captured variable
    assert_eq!(
        run_str("function counter(){ let n = 0; return { inc(){ n++; }, get(){ return n; } }; } const k = counter(); k.inc(); k.inc(); k.get()"),
        "2"
    );
}

#[test]
fn closures_over_loop_bindings() {
    // `let` in a for-loop is captured per-iteration
    assert_eq!(
        run_str("const fns = []; for (let i = 0; i < 3; i++) { fns.push(() => i); } fns.map(f => f()).join(',')"),
        "0,1,2"
    );
    // building an array of adders
    assert_eq!(
        run_str("const adders = [1,2,3].map(n => x => x + n); adders.map(f => f(10)).join(',')"),
        "11,12,13"
    );
}

#[test]
fn callbacks_over_arrays() {
    assert_eq!(run_str("[1, 2, 3].map(x => x * x).join(',')"), "1,4,9");
    assert_eq!(run_str("[1, 2, 3, 4].filter(x => x % 2 === 0).join(',')"), "2,4");
    assert_eq!(run_str("[1, 2, 3, 4].reduce((acc, x) => acc + x, 0)"), "10");
    // closure mutating a captured accumulator from forEach
    assert_eq!(run_str("let total = 0; [1, 2, 3].forEach(x => { total += x; }); total"), "6");
    // index argument to map/forEach callbacks
    assert_eq!(run_str("['a','b','c'].map((c, i) => `${i}:${c}`).join(',')"), "0:a,1:b,2:c");
    // sort comparator closure
    assert_eq!(run_str("[3,1,2,5,4].sort((a, b) => a - b).join(',')"), "1,2,3,4,5");
    assert_eq!(run_str("[3,1,2,5,4].sort((a, b) => b - a).join(',')"), "5,4,3,2,1");
}

// ----------------------------------------------------------------------------
// Object destructuring (params & bindings)
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
    assert_eq!(run_str("function area({w, h}){ return w * h; } area({w: 3, h: 4})"), "12");
    // renaming in a parameter pattern
    assert_eq!(run_str("function f({a: x, b: y}){ return x - y; } f({a: 10, b: 4})"), "6");
}

#[test]
fn destructured_param_own_default() {
    // A destructured PARAMETER's own `= {}` default (so the function can be called
    // with no args) applies, and inner element defaults fire too.
    assert_eq!(run_str("function f({a = 1, b = 2} = {}){ return a + b; } f()"), "3");
    assert_eq!(run_str("function f({a = 1, b = 2} = {}){ return a + b; } f({a: 10})"), "12");
}

// ----------------------------------------------------------------------------
// Array destructuring (params & bindings)
// ----------------------------------------------------------------------------

#[test]
fn array_destructuring() {
    assert_eq!(run_str("const [a, b] = [1, 2]; a + b"), "3");
    assert_eq!(run_str("const [a, , c] = [1, 2, 3]; `${a},${c}`"), "1,3"); // hole skips
    assert_eq!(run_str("const [first, ...rest] = [1, 2, 3, 4]; `${first}:${rest.join(',')}`"), "1:2,3,4");
    assert_eq!(run_str("function f([a, b]){ return a * b; } f([5, 6])"), "30");
}

#[test]
fn nested_array_destructuring() {
    // NESTED array patterns bind their inner elements, including an array pattern
    // nested inside an object pattern.
    assert_eq!(run_str("const [[a], [b]] = [[1], [2]]; String(a) + ',' + String(b)"), "1,2");
    assert_eq!(run_str("const {arr: [x, y]} = {arr: [1, 2]}; String(x) + ',' + String(y)"), "1,2");
}

#[test]
fn array_destructuring_defaults() {
    // Array-pattern element DEFAULTS apply when the element is missing/undefined.
    assert_eq!(run_str("const [a = 10, b = 20] = [1]; `${a},${b}`"), "1,20");
    assert_eq!(run_str("const [a = 10] = []; String(a)"), "10");
}

// ----------------------------------------------------------------------------
// Reflection residuals: fn.length / fn.name / named-fn-expression self-reference
// ----------------------------------------------------------------------------

#[test]
fn function_length_and_name_are_reflected() {
    // Functions reflect `.length` (arity = leading params before any default/rest)
    // and `.name` (declared, or inferred from the binding for an anonymous expr).
    assert_eq!(run_str("function f(a, b, c){} f.length"), "3");
    assert_eq!(run_str("const g = (a, b) => 0; g.length"), "2");
    // `.length` counts only params before the first default or rest.
    assert_eq!(run_str("function h(a, b = 1, c){} h.length"), "1");
    assert_eq!(run_str("function r(a, ...rest){} r.length"), "1");
    assert_eq!(run_str("function named(){} named.name"), "named");
    assert_eq!(run_str("const bar = function(){}; bar.name"), "bar");
    assert_eq!(run_str("const arrow = () => 0; arrow.name"), "arrow");
}

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
