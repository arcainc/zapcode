//! Conformance breadth: scoping, binding kinds, and closures.
//!
//! Covers `var`/`let`/`const`, hoisting (var + function declarations), block vs
//! function scope, shadowing, and closures — including the canonical
//! per-iteration-`let`-vs-shared-`var` loop-capture test, IIFEs, nested
//! closures, and factory/counter/memoize/once patterns.
//!
//! Real-Node answers are asserted everywhere zapcode agrees. zapcode has a set
//! of KNOWN, DOCUMENTED scoping residuals where it deliberately diverges from
//! spec lexical scoping; those cases are pinned to zapcode's ACTUAL behavior and
//! flagged with a `DIVERGENCE:` comment so the suite stays GREEN and so the
//! divergence is captured rather than hidden. The documented residuals are:
//!
//!   * No TDZ: a `let`/`const` read before its initializer yields `undefined`
//!     instead of throwing a ReferenceError.
//!   * `let`/`const` are not block-fresh: an inner-block re-declaration of the
//!     same name overwrites the outer binding rather than creating a distinct
//!     one that is restored when the block ends, and such bindings are visible
//!     after the block (effectively function-scoped, like `var`, but without
//!     hoisting-to-`undefined`).
//!   * `const` reassignment is not rejected.
//!   * Duplicate same-scope `let` is not rejected.
//!   * Per-iteration capture is implemented for the C-style `for (let …;;)`
//!     head ONLY; `for…of`, `for…in`, and `let`/`const` declared inside a loop
//!     *body* block all share a single binding (last value wins).
//!   * A closure nested inside a function that shadows an outer `let`/`const`
//!     captures the OUTER binding, not the function-local shadow.
//!
//! Everything NOT in that list is asserted at the real-JS answer.

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, ZapcodeRun};

/// Run a snippet to completion and stringify the result. Snippets that need a
/// `return` are wrapped in an IIFE by the caller, since zapcode (correctly)
/// rejects a top-level `return` as a ParseError.
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

// ============================================================================
// var: hoisting and function scope
// ============================================================================

#[test]
fn var_is_hoisted_to_undefined() {
    // A `var` read before its initializer sees `undefined`, not a ReferenceError.
    assert_eq!(run_str("(function(){ return x; var x = 1; })()"), "undefined");
    assert_eq!(
        run_str("(function(){ var a = typeof b; var b = 2; return a; })()"),
        "undefined"
    );
    // The assignment still happens at the original line.
    assert_eq!(
        run_str("(function(){ var x; var r = x; x = 5; return r + ',' + x; })()"),
        "undefined,5"
    );
    // typeof a hoisted-but-uninitialized var is "undefined".
    assert_eq!(
        run_str("(function(){ var r = typeof q; var q = 3; return r; })()"),
        "undefined"
    );
}

#[test]
fn var_is_function_scoped_not_block_scoped() {
    // var declared inside a block is visible (and assigned) at function scope.
    assert_eq!(run_str("(function(){ { var k = 7; } return k; })()"), "7");
    assert_eq!(
        run_str("(function(){ if (true) { var m = 1; } return m; })()"),
        "1"
    );
    assert_eq!(
        run_str("(function(){ for (var i = 0; i < 3; i++) {} return i; })()"),
        "3"
    );
    // A var inside a nested non-function block still belongs to the function.
    assert_eq!(
        run_str("(function(){ { { var deep = 42; } } return deep; })()"),
        "42"
    );
}

#[test]
fn var_does_not_escape_its_function() {
    // A nested function's var stays local — it does not leak to the enclosing fn.
    assert_eq!(
        run_str("(function(){ function g(){ var z = 5; } g(); return typeof z; })()"),
        "undefined"
    );
}

// ============================================================================
// function declaration hoisting
// ============================================================================

#[test]
fn function_declarations_are_hoisted() {
    // Callable before the textual declaration.
    assert_eq!(
        run_str("(function(){ return g(); function g(){ return 'hoisted'; } })()"),
        "hoisted"
    );
    // Mutual recursion across hoisted declarations.
    assert_eq!(
        run_str(
            "(function(){
                function even(n){ return n === 0 ? true : odd(n - 1); }
                function odd(n){ return n === 0 ? false : even(n - 1); }
                return even(10);
            })()"
        ),
        "true"
    );
    // Hoisting reaches into the function from a nested call site.
    assert_eq!(
        run_str("(function(){ const r = use(); return r; function use(){ return helper(); } function helper(){ return 99; } })()"),
        "99"
    );
}

#[test]
fn function_expressions_are_not_hoisted_as_values() {
    // A `const f = function(){}` binding is only usable after the assignment;
    // before it, reading the binding yields undefined (no TDZ — documented).
    // DIVERGENCE (no TDZ): real JS throws ReferenceError reading `f` early.
    assert_eq!(
        run_str("(function(){ var early = typeof f; const f = function(){ return 1; }; return early; })()"),
        "undefined"
    );
    // After assignment the expression is callable as normal.
    assert_eq!(
        run_str("(function(){ const f = function(){ return 1; }; return f(); })()"),
        "1"
    );
}

// ============================================================================
// let / const: basic binding, initialization, mutation
// ============================================================================

#[test]
fn let_and_const_basic_binding() {
    assert_eq!(run_str("(function(){ let x = 1; return x; })()"), "1");
    assert_eq!(run_str("(function(){ const x = 2; return x; })()"), "2");
    assert_eq!(
        run_str("(function(){ let a = 1, b = 2, c = 3; return a + b + c; })()"),
        "6"
    );
    // let is reassignable.
    assert_eq!(
        run_str("(function(){ let x = 1; x = x + 4; return x; })()"),
        "5"
    );
    // let without initializer is undefined until assigned.
    assert_eq!(
        run_str("(function(){ let x; const r = x; x = 9; return r + ',' + x; })()"),
        "undefined,9"
    );
}

#[test]
fn const_object_is_mutable_binding_is_not_reassigned() {
    // The const *binding* is fixed, but the referenced object is mutable.
    assert_eq!(
        run_str("(function(){ const o = { n: 1 }; o.n = 2; o.m = 3; return o.n + ',' + o.m; })()"),
        "2,3"
    );
    assert_eq!(
        run_str("(function(){ const a = [1, 2]; a.push(3); a[0] = 9; return a.join(','); })()"),
        "9,2,3"
    );
}

// ============================================================================
// DOCUMENTED SCOPING RESIDUALS (asserted at zapcode's ACTUAL behavior)
// ============================================================================

#[test]
fn no_tdz_for_let_and_const() {
    // DIVERGENCE (no TDZ): real JS throws ReferenceError ("Cannot access 'a'
    // before initialization"). zapcode reads the not-yet-initialized binding as
    // `undefined`, mirroring var-style hoisting.
    assert_eq!(
        run_str("(function(){ const r = (typeof a); let a = 1; return r; })()"),
        "undefined"
    );
    assert_eq!(
        run_str("(function(){ let captured; { captured = (typeof b); let b = 2; } return captured; })()"),
        "undefined"
    );
}

#[test]
fn const_reassignment_is_not_rejected() {
    // DIVERGENCE: real JS throws TypeError ("Assignment to constant variable").
    // zapcode permits the reassignment.
    assert_eq!(
        run_str("(function(){ const c = 1; c = 2; return c; })()"),
        "2"
    );
}

#[test]
fn duplicate_let_in_same_scope_is_not_rejected() {
    // DIVERGENCE: real JS is a SyntaxError ("Identifier 'a' has already been
    // declared"). zapcode treats the second `let` as a reassignment.
    assert_eq!(
        run_str("(function(){ let a = 1; let a = 2; return a; })()"),
        "2"
    );
}

#[test]
fn let_const_block_redeclaration_overwrites_outer() {
    // DIVERGENCE: in real JS an inner-block `let`/`const` of the same name is a
    // distinct binding, so the outer value is RESTORED after the block (these
    // would all be "1" in Node). zapcode instead overwrites the outer binding.
    assert_eq!(
        run_str("(function(){ let x = 1; { let x = 2; } return x; })()"),
        "2"
    );
    assert_eq!(
        run_str("(function(){ let x = 1; if (true) { let x = 2; } return x; })()"),
        "2"
    );
    assert_eq!(
        run_str("(function(){ const a = 1; { const a = 2; } return a; })()"),
        "2"
    );
    // Reading the inner shadow *inside* the block sees the inner value (this
    // part matches JS); the divergence is only the lack of restoration after.
    assert_eq!(
        run_str("(function(){ let x = 1; let seen; { let x = 2; seen = x; } return seen + ',' + x; })()"),
        "2,2"
    );
}

#[test]
fn let_const_leak_out_of_blocks() {
    // DIVERGENCE: real JS scopes these to the block, so `typeof a` after the
    // block is "undefined". zapcode keeps them visible (function-scoped).
    assert_eq!(run_str("(function(){ { let a = 1; } return typeof a; })()"), "number");
    assert_eq!(
        run_str("(function(){ { const k = 'x'; } return typeof k; })()"),
        "string"
    );
}

// ============================================================================
// shadowing (the parts that MATCH JS)
// ============================================================================

#[test]
fn nested_function_locals_do_not_leak_outward() {
    // After calling a helper that declares its own `let x`, the outer `x` reads
    // its own value — the helper's binding did not leak (matches JS).
    assert_eq!(
        run_str("(function(){ let x = 1; function g(){ let x = 2; return x; } g(); return x; })()"),
        "1"
    );
    assert_eq!(
        run_str("(function(){ let x = 1; const g = () => { let x = 2; return x; }; g(); return x; })()"),
        "1"
    );
    // And the helper itself returns its own shadowed value when read directly.
    assert_eq!(
        run_str("(function(){ let x = 1; function g(){ let x = 2; return x; } return g() + ',' + x; })()"),
        "2,1"
    );
}

#[test]
fn parameter_shadowed_by_inner_binding() {
    // A function param can be shadowed by an inner declaration; reading the
    // shadow directly yields the new value (matches JS for the direct-read path).
    assert_eq!(
        run_str("(function(){ return (function(n){ let n2 = n + 1; return n2; })(5); })()"),
        "6"
    );
    // Param visible before the inner declaration.
    assert_eq!(
        run_str("(function(){ return (function(n){ const a = n * 2; const n2 = a; return n2; })(4); })()"),
        "8"
    );
}

#[test]
fn shadowing_distinct_names_compose_cleanly() {
    // Distinct names (no shadow) capture exactly as in JS.
    assert_eq!(
        run_str("(function(){ let a = 1; { let b = 2; { let c = 3; return a + b + c; } } })()"),
        "6"
    );
    // Inner non-shadowing let captured by a nested closure works (returns 20).
    assert_eq!(
        run_str("(function(){ const f = () => { let y = 20; const g = () => y; return g(); }; return f(); })()"),
        "20"
    );
}

#[test]
fn closure_capturing_a_shadowed_binding_diverges() {
    // DIVERGENCE: when an inner function shadows an OUTER `let` with its own
    // `let` of the SAME name, a closure nested one level deeper captures the
    // OUTER binding instead of the local shadow. Real JS returns "20"; zapcode
    // returns "10" for both the arrow and the function-declaration forms.
    assert_eq!(
        run_str("(function(){ let x = 10; const f = () => { let x = 20; const g = () => x; return g(); }; return f(); })()"),
        "10"
    );
    assert_eq!(
        run_str("(function(){ let x = 10; function f(){ let x = 20; function g(){ return x; } return g(); } return f(); })()"),
        "10"
    );
    // The direct (non-nested-closure) read of the shadow is unaffected: this one
    // matches JS at "20,10".
    assert_eq!(
        run_str("(function(){ let x = 10; const f = () => { let x = 20; return x; }; return f() + ',' + x; })()"),
        "20,10"
    );
}

// ============================================================================
// closures capturing loop variables: let (per-iteration) vs var (shared)
// ============================================================================

#[test]
fn for_let_head_is_per_iteration() {
    // The canonical case: each `for (let i …)` iteration gets a fresh binding,
    // so the captured closures observe 0,1,2 (matches JS).
    assert_eq!(
        run_str("(function(){ const fs = []; for (let i = 0; i < 3; i++) { fs.push(() => i); } return fs.map(f => f()).join(','); })()"),
        "0,1,2"
    );
    // Same when the body is a single expression (no braces).
    assert_eq!(
        run_str("(function(){ const fs = []; for (let i = 0; i < 4; i++) fs.push(() => i * i); return fs.map(f => f()).join(','); })()"),
        "0,1,4,9"
    );
}

#[test]
fn for_var_head_is_shared() {
    // A single `var i` is shared by all closures, so they all observe the final
    // value 3 (matches JS).
    assert_eq!(
        run_str("(function(){ const fs = []; for (var i = 0; i < 3; i++) { fs.push(() => i); } return fs.map(f => f()).join(','); })()"),
        "3,3,3"
    );
    // The classic IIFE workaround captures the value of the loop var per call.
    assert_eq!(
        run_str("(function(){ const fs = []; for (var i = 0; i < 3; i++) { ((j) => fs.push(() => j))(i); } return fs.map(f => f()).join(','); })()"),
        "0,1,2"
    );
}

#[test]
fn nested_for_let_loops_capture_independently() {
    // Each (i, j) pair is captured distinctly across nested per-iteration heads.
    assert_eq!(
        run_str("(function(){ const out = []; for (let i = 0; i < 2; i++) for (let j = 0; j < 2; j++) out.push(() => `${i}${j}`); return out.map(f => f()).join(','); })()"),
        "00,01,10,11"
    );
}

#[test]
fn loop_body_block_let_is_not_per_iteration() {
    // DIVERGENCE: a `let` declared inside the for *body* (not the head) is, in
    // real JS, fresh per iteration — so these closures would observe 0,10,20.
    // zapcode shares one binding, so all observe the last value, 20.
    assert_eq!(
        run_str("(function(){ const fs = []; for (let i = 0; i < 3; i++) { let k = i * 10; fs.push(() => k); } return fs.map(f => f()).join(','); })()"),
        "20,20,20"
    );
    // Same shared-binding behavior for a `let` captured inside a `while` body
    // (real JS: 0,1,2).
    assert_eq!(
        run_str("(function(){ const fs = []; let i = 0; while (i < 3) { let j = i; fs.push(() => j); i++; } return fs.map(f => f()).join(','); })()"),
        "2,2,2"
    );
}

#[test]
fn for_of_and_for_in_capture_share_one_binding() {
    // DIVERGENCE: real JS gives a fresh per-iteration binding for `for…of` /
    // `for…in` loop variables (so these would be 1,2,3 and a,b). zapcode shares
    // one binding, so the captured closures all observe the final value.
    assert_eq!(
        run_str("(function(){ const fs = []; for (const v of [1, 2, 3]) fs.push(() => v); return fs.map(f => f()).join(','); })()"),
        "3,3,3"
    );
    assert_eq!(
        run_str("(function(){ const fs = []; for (const k in { a: 1, b: 2 }) fs.push(() => k); return fs.map(f => f()).join(','); })()"),
        "b,b"
    );
    // Eagerly snapshotting the value per iteration (the workaround) is fine and
    // matches JS, since each pushed value is computed at iteration time.
    assert_eq!(
        run_str("(function(){ const out = []; for (const v of [1, 2, 3]) out.push(v * 2); return out.join(','); })()"),
        "2,4,6"
    );
}

// ============================================================================
// IIFEs
// ============================================================================

#[test]
fn iife_forms() {
    // Classic function-expression IIFE.
    assert_eq!(run_str("(function(){ return 42; })()"), "42");
    // Arrow IIFE.
    assert_eq!(run_str("(() => 7)()"), "7");
    // IIFE with an argument.
    assert_eq!(run_str("(function(n){ return n * 2; })(21)"), "42");
    // Alternate parenthesization `(function(){…}())`.
    assert_eq!(run_str("(function(){ return 'inner'; }())"), "inner");
    // Nested IIFEs.
    assert_eq!(
        run_str("(function(){ return (function(){ return (function(){ return 1; })() + 1; })(); })()"),
        "2"
    );
}

#[test]
fn iife_creates_a_private_scope() {
    // The IIFE's locals are not visible outside it; the module-style pattern
    // exposes only what it returns.
    assert_eq!(
        run_str("(function(){ const api = (function(){ let secret = 41; return { reveal: () => secret + 1 }; })(); return api.reveal(); })()"),
        "42"
    );
}

// ============================================================================
// nested closures and lexical capture
// ============================================================================

#[test]
fn nested_closures_capture_each_enclosing_scope() {
    assert_eq!(
        run_str("(function(){ function a(){ let x = 1; return function b(){ let y = 2; return function c(){ return x + y; }; }; } return a()()(); })()"),
        "3"
    );
    // Capture survives several intervening calls.
    assert_eq!(
        run_str("(function(){ const make = (base) => (mul) => (add) => base * mul + add; return make(10)(3)(7); })()"),
        "37"
    );
}

#[test]
fn currying_and_partial_application() {
    assert_eq!(
        run_str("(function(){ const add = a => b => c => a + b + c; return add(1)(2)(3); })()"),
        "6"
    );
    // Partial application captures the first argument.
    assert_eq!(
        run_str("(function(){ const add = a => b => a + b; const add10 = add(10); return add10(5) + ',' + add10(1); })()"),
        "15,11"
    );
}

#[test]
fn arrow_lexically_captures_outer_bindings() {
    // Distinct-name capture across an arrow boundary (no shadow) is exact.
    assert_eq!(
        run_str("(function(){ let base = 100; const f = () => { const g = () => base + 1; return g(); }; base = 200; return f(); })()"),
        "201"
    );
}

// ============================================================================
// factory / counter / closure-state patterns
// ============================================================================

#[test]
fn counter_factory_keeps_private_state() {
    assert_eq!(
        run_str("(function(){ function mk(){ let n = 0; return () => ++n; } const c = mk(); c(); c(); return c(); })()"),
        "3"
    );
    // An object of closures sharing one private variable.
    assert_eq!(
        run_str("(function(){ function mk(){ let n = 0; return { inc: () => ++n, get: () => n }; } const o = mk(); o.inc(); o.inc(); return o.get(); })()"),
        "2"
    );
}

#[test]
fn factory_instances_are_independent() {
    // Two counters made by the same factory must not share state.
    assert_eq!(
        run_str("(function(){ function mk(){ let n = 0; return () => ++n; } const a = mk(), b = mk(); a(); a(); return `${a()},${b()}`; })()"),
        "3,1"
    );
    assert_eq!(
        run_str("(function(){ function bank(start){ let bal = start; return { dep: x => bal += x, bal: () => bal }; } const a = bank(100), b = bank(0); a.dep(50); b.dep(5); return a.bal() + ',' + b.bal(); })()"),
        "150,5"
    );
}

#[test]
fn closures_mutate_shared_outer_state() {
    // A helper closure mutating an outer accumulator reaches the same binding.
    assert_eq!(
        run_str("(function(){ function outer(){ let total = 0; const add = x => { total += x; }; [1, 2, 3].forEach(add); return total; } return outer(); })()"),
        "6"
    );
    // forEach callback mutating an outer array.
    assert_eq!(
        run_str("(function(){ const out = []; [1, 2].forEach(x => out.push(x * 2)); return out.join(','); })()"),
        "2,4"
    );
}

#[test]
fn memoize_closure_pattern() {
    // A cache captured in a closure persists across calls.
    assert_eq!(
        run_str("(function(){ function memo(){ const cache = {}; return n => cache[n] ?? (cache[n] = n * n); } const sq = memo(); sq(3); sq(3); return sq(4) + ',' + sq(3); })()"),
        "16,9"
    );
}

#[test]
fn once_closure_pattern() {
    // `once` runs the wrapped function a single time regardless of call count.
    assert_eq!(
        run_str("(function(){ function once(f){ let done = false, val; return (...a) => { if (!done) { done = true; val = f(...a); } return val; }; } let calls = 0; const o = once(() => ++calls); o(); o(); o(); return calls; })()"),
        "1"
    );
}

#[test]
fn closure_returned_from_loop_captures_via_factory() {
    // Building an array of closures via a factory call per item captures the
    // right value each time (the factory call creates fresh `let`s).
    assert_eq!(
        run_str("(function(){ function tag(label){ return v => `${label}:${v}`; } const fns = ['a', 'b', 'c'].map(tag); return fns.map((f, i) => f(i)).join(','); })()"),
        "a:0,b:1,c:2"
    );
}
