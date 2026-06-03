//! Conformance breadth: generators & iteration (`function*` / `yield` / `yield*`).
//!
//! A language-grade conformance pass for the generator feature, organized by
//! capability:
//!   * `function*` declaration + `yield` mechanics (single, multiple, undefined,
//!     conditional, expression-position, parameters, closures)
//!   * the explicit iterator protocol: `.next()`, `.next(value)`, `done`/`value`
//!     result-object shape, exhaustion semantics
//!   * `.return(v)` early termination (standalone + mid-`for...of`)
//!   * consuming via `for...of` (totals, accumulation, nesting, early `break`,
//!     `return`-stops-iteration, re-iterating an exhausted generator)
//!   * `yield*` delegation to an ARRAY (flattens), and the delegate's
//!     completion value via `const r = yield* arr`
//!   * lazy / on-demand evaluation (side effects only fire as far as consumed)
//!   * returning values from a generator body (surfaced on the final result)
//!   * independent, isolated generator instances
//!   * infinite generators with a bounded "take"
//!   * generators as object-literal methods and free function values
//!
//! Several behaviors are KNOWN, DOCUMENTED divergences from real JS (see
//! `STRESS-PASS-BUGS.md`). Those are pinned to zapcode's ACTUAL behavior, each
//! with a comment stating the real-JS answer, so the suite stays GREEN without
//! asserting something false:
//!   * `yield*` delegating to another GENERATOR does not flatten (yields the
//!     generator object → `[object Generator]`). JS: it flattens.
//!   * `yield*` over an EMPTY ARRAY yields one stray `undefined`. JS: nothing.
//!   * `yield*` over a STRING yields the whole string as one value (JS: chars).
//!   * `yield*` over a Set / Map does not iterate the collection's elements.
//!   * a generator's `.throw(...)` method does not exist.
//!   * `it[Symbol.iterator]()` (generator self-iterability via the well-known
//!     symbol) is not dispatched.
//!   * `[...gen()]` spread and array-destructuring of a generator are not
//!     supported.
//!   * a class method declared `*method(){}` does not compile its body as a
//!     generator (object-literal `*m(){}` does).
//!   * a plain object exposing a custom `[Symbol.iterator]` is not `for...of`-able.

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, ZapcodeRun};

/// Run `code` to completion and render the final value as a JS-ish string.
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

/// Run `code` capturing `console.log` output (trimmed).
fn run_stdout(code: &str) -> String {
    let result = ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        Vec::new(),
        ResourceLimits::default(),
    )
    .unwrap()
    .run(Vec::new())
    .unwrap();
    result.stdout.trim().to_string()
}

/// Run that tolerates a RuntimeError (returns the error text prefixed `ERR:`),
/// for pinning documented "this is not supported" behavior without panicking.
fn run_or_err(code: &str) -> String {
    match ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        Vec::new(),
        ResourceLimits::default(),
    )
    .unwrap()
    .run(Vec::new())
    {
        Ok(r) => match r.state {
            VmState::Complete(v) => v.to_js_string(&r.heap),
            other => format!("NONCOMPLETE:{other:?}"),
        },
        Err(e) => format!("ERR:{e}"),
    }
}

// ============================================================================
// 1. function* declaration + basic yield mechanics
// ============================================================================

#[test]
fn yields_in_order() {
    assert_eq!(
        run_str("function* g(){ yield 1; yield 2; yield 3; } const it=g(); `${it.next().value},${it.next().value},${it.next().value}`"),
        "1,2,3"
    );
    // single yield
    assert_eq!(
        run_str("function* g(){ yield 42; } g().next().value"),
        "42"
    );
    // zero yields: immediately done
    assert_eq!(
        run_str("function* g(){} const r=g().next(); `${String(r.value)},${r.done}`"),
        "undefined,true"
    );
}

#[test]
fn bare_yield_produces_undefined() {
    assert_eq!(
        run_str("function* g(){ yield; yield; } const it=g(); const a=it.next(); const b=it.next(); `${String(a.value)},${a.done},${String(b.value)},${b.done}`"),
        "undefined,false,undefined,false"
    );
}

#[test]
fn yields_various_value_types() {
    // strings
    assert_eq!(
        run_str("function* g(){ yield 'a'; yield 'b'; } let s=''; for(const x of g()) s+=x; s"),
        "ab"
    );
    // booleans + null + numbers
    assert_eq!(
        run_str("function* g(){ yield true; yield false; yield 0; } const it=g(); `${it.next().value},${it.next().value},${it.next().value}`"),
        "true,false,0"
    );
    // objects (carried by reference through the iterator)
    assert_eq!(
        run_str("function* g(){ yield {a:1}; yield {a:2}; } let o=[]; for(const x of g()) o.push(x.a); o.join(',')"),
        "1,2"
    );
    // arrays as single yielded values (NOT delegated — that's yield*)
    assert_eq!(
        run_str("function* g(){ yield [1,2]; yield [3]; } let o=[]; for(const x of g()) o.push(x.length); o.join(',')"),
        "2,1"
    );
}

#[test]
fn conditional_and_loop_driven_yields() {
    assert_eq!(
        run_str("function* g(n){ if(n>0) yield 'pos'; else yield 'nonpos'; yield 'end'; } let o=[]; for(const x of g(5)) o.push(x); o.join(',')"),
        "pos,end"
    );
    assert_eq!(
        run_str("function* g(n){ if(n>0) yield 'pos'; else yield 'nonpos'; yield 'end'; } let o=[]; for(const x of g(-5)) o.push(x); o.join(',')"),
        "nonpos,end"
    );
    // for-loop body driving yields
    assert_eq!(
        run_str("function* squares(n){ for(let i=1;i<=n;i++) yield i*i; } let o=[]; for(const x of squares(4)) o.push(x); o.join(',')"),
        "1,4,9,16"
    );
    // while-loop body driving yields (countdown)
    assert_eq!(
        run_str("function* countdown(n){ while(n>0) yield n--; } let o=[]; for(const x of countdown(3)) o.push(x); o.join(',')"),
        "3,2,1"
    );
}

#[test]
fn yield_in_expression_position() {
    // a yield expression evaluates to the value passed to the resuming .next()
    assert_eq!(
        run_str("function* g(){ let x=(yield 1)+(yield 2); yield x; } const it=g(); it.next(); it.next(10); it.next(20).value"),
        "30"
    );
    // yield used directly in a template literal interpolation
    assert_eq!(
        run_str("function* g(){ const name = yield 'who?'; yield `hi ${name}`; } const it=g(); it.next(); it.next('Ada').value"),
        "hi Ada"
    );
}

#[test]
fn parameters_and_closure_capture() {
    assert_eq!(
        run_str("function* range(a,b){ for(let i=a;i<b;i++) yield i; } let o=[]; for(const x of range(2,5)) o.push(x); o.join(',')"),
        "2,3,4"
    );
    // closes over its start argument
    assert_eq!(
        run_str("function* counter(start){ let n=start; while(n<start+3) yield n++; } let o=[]; for(const x of counter(100)) o.push(x); o.join(',')"),
        "100,101,102"
    );
    // closes over an outer variable
    assert_eq!(
        run_str("let step=10; function* g(){ let n=0; while(n<30) { yield n; n+=step; } } let o=[]; for(const x of g()) o.push(x); o.join(',')"),
        "0,10,20"
    );
}

// ============================================================================
// 2. typeof + result-object shape
// ============================================================================

#[test]
fn typeof_generator_function_and_object() {
    // a generator function is a function
    assert_eq!(run_str("function* g(){ yield 1; } typeof g"), "function");
    // a called generator is an object (the iterator)
    assert_eq!(run_str("function* g(){ yield 1; } typeof g()"), "object");
}

#[test]
fn next_result_object_shape() {
    // both value and done are own keys of the result
    assert_eq!(
        run_str("function* g(){ yield 1; } const r=g().next(); `${'value' in r},${'done' in r}`"),
        "true,true"
    );
    // not-done step: value present, done false
    assert_eq!(
        run_str("function* g(){ yield 7; } const r=g().next(); `${r.value},${r.done}`"),
        "7,false"
    );
    // done step: value undefined, done true
    assert_eq!(
        run_str("function* g(){ yield 1; } const it=g(); it.next(); const r=it.next(); `${String(r.value)},${r.done}`"),
        "undefined,true"
    );
}

// ============================================================================
// 3. .next(value) — passing values back into a suspended yield
// ============================================================================

#[test]
fn next_passes_value_into_yield() {
    assert_eq!(
        run_str("function* g(){ const x=yield 1; const y=yield x+1; yield y+1; } const it=g(); it.next(); `${it.next(10).value},${it.next(20).value}`"),
        "11,21"
    );
    assert_eq!(
        run_str("function* g(){ const x=yield 1; yield x*2; } const it=g(); it.next(); it.next(10).value"),
        "20"
    );
}

#[test]
fn first_next_argument_is_ignored() {
    // there is no suspended yield to receive the first .next()'s argument
    assert_eq!(
        run_str("function* g(){ const x=yield 1; yield x; } const it=g(); it.next(42); it.next(7).value"),
        "7"
    );
}

#[test]
fn running_accumulator_via_next() {
    assert_eq!(
        run_str("function* adder(){ let sum=0; while(true){ const x=yield sum; sum+=x; } } const it=adder(); const a=it.next().value; const b=it.next(10).value; const c=it.next(20).value; `${a},${b},${c}`"),
        "0,10,30"
    );
}

#[test]
fn command_driven_generator_returns_on_signal() {
    // a generator that loops until a sentinel value is .next()-ed in
    assert_eq!(
        run_str("function* g(){ while(true){ const cmd=yield 'tick'; if(cmd==='stop') return 'stopped'; } } const it=g(); it.next(); it.next('go'); const r=it.next('stop'); `${r.value},${r.done}`"),
        "stopped,true"
    );
}

// ============================================================================
// 4. returning values from a generator body
// ============================================================================

#[test]
fn return_statement_value_surfaces_on_done_step() {
    // `return v` provides the value of the final (done) step
    assert_eq!(
        run_str("function* g(){ yield 1; return 'fin'; } const it=g(); it.next(); const r=it.next(); `${r.value},${r.done}`"),
        "fin,true"
    );
    // `return v` value is NOT visited by for...of (which stops at done)
    assert_eq!(
        run_str("function* g(){ yield 1; return 99; yield 2; } let o=[]; for(const x of g()) o.push(x); o.join(',')"),
        "1"
    );
    // bare return stops iteration too
    assert_eq!(
        run_str("function* g(){ yield 1; return; yield 2; } let o=[]; for(const x of g()) o.push(x); o.join(',')"),
        "1"
    );
}

#[test]
fn return_value_inside_conditional() {
    assert_eq!(
        run_str("function* g(n){ yield 1; if(n) return 'early'; yield 2; } const it=g(true); it.next(); const r=it.next(); `${r.value},${r.done}`"),
        "early,true"
    );
    assert_eq!(
        run_str("function* g(n){ yield 1; if(n) return 'early'; yield 2; } let o=[]; for(const x of g(false)) o.push(x); o.join(',')"),
        "1,2"
    );
}

// ============================================================================
// 5. exhaustion semantics
// ============================================================================

#[test]
fn next_after_exhaustion_stays_done() {
    assert_eq!(
        run_str("function* g(){ yield 1; } const it=g(); it.next(); it.next(); const r=it.next(); `${String(r.value)},${r.done}`"),
        "undefined,true"
    );
    // many extra .next() calls remain done with undefined value
    assert_eq!(
        run_str("function* g(){ yield 1; } const it=g(); it.next(); it.next(); it.next(); it.next(); const r=it.next(); `${String(r.value)},${r.done}`"),
        "undefined,true"
    );
}

#[test]
fn reiterating_exhausted_generator_yields_nothing() {
    assert_eq!(
        run_str("function* g(){ yield 1; yield 2; } const it=g(); for(const x of it){} let o=[]; for(const x of it) o.push(x); o.length"),
        "0"
    );
}

// ============================================================================
// 6. .return(v) — early termination
// ============================================================================

#[test]
fn return_method_terminates_and_reports_value() {
    // .return(v) yields {value: v, done: true} and finishes the generator
    assert_eq!(
        run_str("function* g(){ yield 1; yield 2; yield 3; } const it=g(); it.next(); const r=it.return(99); `${r.value},${r.done},${it.next().done}`"),
        "99,true,true"
    );
    // .return() on a fresh generator (never started)
    assert_eq!(
        run_str("function* g(){ yield 1; } const it=g(); const r=it.return(7); `${r.value},${r.done}`"),
        "7,true"
    );
    // a subsequent .next() after return is done with undefined value
    assert_eq!(
        run_str("function* g(){ yield 1; yield 2; } const it=g(); it.return(5); const r=it.next(); `${String(r.value)},${r.done}`"),
        "undefined,true"
    );
}

#[test]
fn return_inside_for_of_stops_the_loop() {
    // calling it.return() inside the loop body stops further iteration
    assert_eq!(
        run_str("function* g(){ yield 1; yield 2; yield 3; } let o=[]; const it=g(); for(const x of it){ o.push(x); if(x===2) it.return(); } o.join(',')"),
        "1,2"
    );
}

// ============================================================================
// 7. consuming with for...of
// ============================================================================

#[test]
fn for_of_basic_and_console() {
    assert_eq!(
        run_str("function* g(){ yield 1; yield 2; yield 3; } let o=[]; for(const x of g()) o.push(x); o.join(',')"),
        "1,2,3"
    );
    assert_eq!(
        run_stdout("function* nums(){ yield 10; yield 20; yield 30; } for(const n of nums()) console.log(n);"),
        "10\n20\n30"
    );
}

#[test]
fn for_of_accumulation() {
    assert_eq!(
        run_str("function* g(){ yield 10; yield 20; } let s=0; for(const x of g()) s+=x; s"),
        "30"
    );
    assert_eq!(
        run_str("function* squares(n){ for(let i=1;i<=n;i++) yield i*i; } let s=0; for(const x of squares(4)) s+=x; s"),
        "30"
    );
}

#[test]
fn for_of_early_break() {
    assert_eq!(
        run_str("function* g(){ yield 1; yield 2; yield 3; yield 4; } let o=[]; for(const x of g()){ if(x>2) break; o.push(x); } o.join(',')"),
        "1,2"
    );
    // continue inside the loop
    assert_eq!(
        run_str("function* g(){ yield 1; yield 2; yield 3; yield 4; } let o=[]; for(const x of g()){ if(x%2===0) continue; o.push(x); } o.join(',')"),
        "1,3"
    );
}

#[test]
fn nested_for_of_over_generators() {
    assert_eq!(
        run_str("function* g(){ yield 1; yield 2; } let o=[]; for(const a of g()) for(const b of g()) o.push(a+''+b); o.join(',')"),
        "11,12,21,22"
    );
    // outer generator, inner array
    assert_eq!(
        run_str("function* g(){ yield 'a'; yield 'b'; } let o=[]; for(const a of g()) for(const n of [1,2]) o.push(a+n); o.join(',')"),
        "a1,a2,b1,b2"
    );
}

#[test]
fn generator_consuming_another_iterable_internally() {
    // a generator can itself for...of over an array and transform it
    assert_eq!(
        run_str("function* g(arr){ for(const x of arr) yield x*2; } let o=[]; for(const v of g([1,2,3])) o.push(v); o.join(',')"),
        "2,4,6"
    );
}

// ============================================================================
// 8. yield* delegation (supported forms: array, string, nested)
// ============================================================================

#[test]
fn yield_star_over_array_flattens() {
    assert_eq!(
        run_str("function* g(){ yield* [1,2,3]; } let o=[]; for(const x of g()) o.push(x); o.join(',')"),
        "1,2,3"
    );
    assert_eq!(
        run_str("function* g(){ yield 0; yield* [1,2]; yield 3; } let o=[]; for(const x of g()) o.push(x); o.join(',')"),
        "0,1,2,3"
    );
    // single-element delegate
    assert_eq!(
        run_str("function* g(){ yield* [42]; } g().next().value"),
        "42"
    );
}

#[test]
fn yield_star_multiple_delegations() {
    assert_eq!(
        run_str("function* g(){ yield* [1,2]; yield* [3,4]; } let o=[]; for(const x of g()) o.push(x); o.join(',')"),
        "1,2,3,4"
    );
    assert_eq!(
        run_str("function* g(){ yield 'a'; yield* ['b','c']; yield 'd'; } let o=[]; for(const x of g()) o.push(x); o.join(',')"),
        "a,b,c,d"
    );
}

#[test]
fn yield_star_over_string_yields_whole_string_divergence() {
    // DIVERGENCE (documented-style): `yield* 'abc'` yields the WHOLE string as a
    // single value rather than iterating it character-by-character. Asserting
    // actual behavior; JS would yield 'a','b','c' (three values).
    assert_eq!(
        run_str("function* g(){ yield* 'abc'; } const it=g(); let c=0; let r=it.next(); while(!r.done){ c++; r=it.next(); } c"),
        "1" // JS: 3
    );
    assert_eq!(
        run_str("function* g(){ yield* 'abc'; } g().next().value"),
        "abc" // JS: "a"
    );
}

#[test]
fn yield_star_array_delegate_completion_value_is_undefined() {
    // `const r = yield* array` binds the iterator's completion value, which for
    // an array iterator is undefined (matches JS). Drive the iterator fully and
    // collect each yielded value: the three array elements, then the bound `r`.
    assert_eq!(
        run_str("function* g(){ const r=yield* [1,2,3]; yield ('r='+String(r)); } const it=g(); let out=[]; let s=it.next(); while(!s.done){ out.push(String(s.value)); s=it.next(); } out.join(',')"),
        "1,2,3,r=undefined"
    );
}

// ============================================================================
// 9. lazy / on-demand evaluation
// ============================================================================

#[test]
fn unstarted_generator_runs_no_body() {
    assert_eq!(
        run_str("let log=[]; function* g(){ log.push('a'); yield 1; } const it=g(); log.length"),
        "0"
    );
}

#[test]
fn body_runs_only_up_to_each_yield() {
    // one .next() runs only up to the first yield
    assert_eq!(
        run_str("let log=[]; function* g(){ log.push('a'); yield 1; log.push('b'); yield 2; log.push('c'); } const it=g(); it.next(); log.join(',')"),
        "a"
    );
    // two .next() runs up to the second yield
    assert_eq!(
        run_str("let log=[]; function* g(){ log.push('a'); yield 1; log.push('b'); yield 2; log.push('c'); } const it=g(); it.next(); it.next(); log.join(',')"),
        "a,b"
    );
    // exhausting runs the trailing code after the last yield
    assert_eq!(
        run_str("let log=[]; function* g(){ log.push('a'); yield 1; log.push('b'); yield 2; log.push('c'); } const it=g(); it.next(); it.next(); it.next(); log.join(',')"),
        "a,b,c"
    );
}

#[test]
fn lazy_take_over_infinite_generator() {
    // an infinite generator is fine when only a prefix is consumed
    assert_eq!(
        run_str("function* nat(){ let n=0; while(true) yield n++; } const it=nat(); `${it.next().value},${it.next().value},${it.next().value}`"),
        "0,1,2"
    );
    // explicit take(k)
    assert_eq!(
        run_str("function* nat(){ let n=1; while(true) yield n++; } const it=nat(); let o=[]; for(let i=0;i<5;i++) o.push(it.next().value); o.join(',')"),
        "1,2,3,4,5"
    );
    // sum of a take
    assert_eq!(
        run_str("function* nat(){ let n=0; while(true) yield n++; } const it=nat(); let s=0; for(let i=0;i<10;i++) s+=it.next().value; s"),
        "45"
    );
}

#[test]
fn infinite_fibonacci_take() {
    assert_eq!(
        run_str("function* fib(){ let a=0,b=1; while(true){ yield a; const t=a; a=b; b=t+b; } } const it=fib(); let o=[]; for(let i=0;i<7;i++) o.push(it.next().value); o.join(',')"),
        "0,1,1,2,3,5,8"
    );
}

// ============================================================================
// 10. independent / isolated instances
// ============================================================================

#[test]
fn two_instances_advance_independently() {
    assert_eq!(
        run_str("function* counter(){ let n=0; while(true) yield n++; } const a=counter(); const b=counter(); `${a.next().value},${a.next().value},${b.next().value},${a.next().value}`"),
        "0,1,0,2"
    );
    // interleaved
    assert_eq!(
        run_str("function* c(){ let n=0; while(true) yield n++; } const a=c(),b=c(); `${a.next().value}${a.next().value}${b.next().value}${a.next().value}`"),
        "0102"
    );
}

#[test]
fn instances_have_isolated_parameters() {
    assert_eq!(
        run_str("function* range(a,b){ for(let i=a;i<b;i++) yield i; } const x=range(0,2); const y=range(10,12); `${x.next().value},${y.next().value},${x.next().value},${y.next().value}`"),
        "0,10,1,11"
    );
}

// ============================================================================
// 11. generators as methods / values
// ============================================================================

#[test]
fn object_literal_generator_method() {
    assert_eq!(
        run_str("const o={ *gen(){ yield 1; yield 2; } }; let r=[]; for(const x of o.gen()) r.push(x); r.join(',')"),
        "1,2"
    );
    // method closing over object state via a captured local
    assert_eq!(
        run_str("function make(){ let items=[10,20,30]; return { *all(){ for(const x of items) yield x; } }; } const o=make(); let r=[]; for(const x of o.all()) r.push(x); r.join(',')"),
        "10,20,30"
    );
}

#[test]
fn generator_function_assigned_to_variable() {
    assert_eq!(
        run_str("function* g(){ yield 1; yield 2; } const f=g; let o=[]; for(const x of f()) o.push(x); o.join(',')"),
        "1,2"
    );
    // passed as an argument and invoked
    assert_eq!(
        run_str("function* g(){ yield 5; yield 6; } function consume(mk){ let s=0; for(const x of mk()) s+=x; return s; } consume(g)"),
        "11"
    );
}

// ============================================================================
// 12. error flow through generators
// ============================================================================

#[test]
fn uncaught_throw_inside_generator_propagates_to_consumer() {
    // a throw inside the generator surfaces at the driving .next()
    assert_eq!(
        run_str("function* g(){ yield 1; throw new Error('boom'); } const it=g(); it.next(); let msg='?'; try { it.next(); } catch(e){ msg=e.message; } msg"),
        "boom"
    );
}

#[test]
fn try_catch_inside_generator_continues_iteration() {
    // an internal try/catch lets the generator keep yielding
    assert_eq!(
        run_str("function* g(){ try { yield 1; yield 2; } catch(e){} yield 3; } let o=[]; for(const x of g()) o.push(x); o.join(',')"),
        "1,2,3"
    );
}

// ============================================================================
// 13. DOCUMENTED DIVERGENCES — pinned to zapcode's ACTUAL behavior
// ============================================================================

#[test]
fn yield_star_over_generator_documented_divergence() {
    // DIVERGENCE (documented): `yield*` delegating to another GENERATOR does not
    // flatten — the delegate generator object is yielded as a single value
    // (rendering "[object Generator]"). `yield*` over an ARRAY/STRING works.
    // Asserting actual behavior. JS would give "0,1,2,3".
    assert_eq!(
        run_str("function* inner(){ yield 1; yield 2; } function* outer(){ yield 0; yield* inner(); yield 3; } let o=[]; for(const x of outer()) o.push(String(x)); o.join(',')"),
        "0,[object Generator],3"
    );
}

#[test]
fn manual_drive_of_inner_generator_is_the_workaround() {
    // The supported way to flatten one generator into another: drive its
    // iterator by hand. This works exactly like JS.
    assert_eq!(
        run_str("function* inner(){ yield 1; yield 2; } function* outer(){ const it=inner(); let r=it.next(); while(!r.done){ yield r.value; r=it.next(); } } let o=[]; for(const x of outer()) o.push(x); o.join(',')"),
        "1,2"
    );
}

#[test]
fn yield_star_over_empty_array_yields_stray_undefined_divergence() {
    // DIVERGENCE (documented-style): `yield* []` yields a single `undefined`
    // instead of yielding nothing. Asserting actual behavior; JS would give
    // exactly "0,1" (two values).
    assert_eq!(
        run_str("function* g(){ yield 0; yield* []; yield 1; } const it=g(); let c=0; let r=it.next(); while(!r.done){ c++; r=it.next(); } c"),
        "3" // JS: 2
    );
}

#[test]
fn yield_star_over_set_does_not_iterate_elements_divergence() {
    // DIVERGENCE (documented): `yield*` over a Set does not iterate the set's
    // elements (the Set is not recognized as an iterable by the delegation
    // path). Asserting actual behavior; JS would yield 1,2,3.
    assert_eq!(
        run_str("function* g(){ yield* new Set([1,2,3]); } g().next().value"),
        "[object Object]"
    );
}

#[test]
fn generator_throw_method_is_unsupported_divergence() {
    // DIVERGENCE (documented): a generator object has no `.throw(...)` method
    // (so the "inject an exception at the suspended yield" pattern is not
    // available). Asserting actual behavior; in JS `it.throw('boom')` would
    // raise inside the generator.
    let out = run_or_err("function* g(){ try { yield 1; } catch(e){ yield 'caught'; } } const it=g(); it.next(); it.throw('boom')");
    assert!(out.starts_with("ERR:"), "expected it.throw(...) to be unsupported, got {out}");
}

#[test]
fn generator_symbol_iterator_self_reference_unsupported_divergence() {
    // DIVERGENCE (documented): `it[Symbol.iterator]()` (a generator returning
    // itself via the well-known iterator symbol) is not dispatched. `for...of`
    // over a generator works directly; this explicit form does not.
    let out = run_or_err("function* g(){ yield 1; } const it=g(); it[Symbol.iterator]()");
    assert!(out.starts_with("ERR:"), "expected it[Symbol.iterator]() to be unsupported, got {out}");
}

#[test]
fn spread_and_destructure_of_generator_documented_divergence() {
    // DIVERGENCE (documented): spread `[...gen()]` and array-destructuring of a
    // generator are not supported. Use `for...of` or `.next()`.
    assert!(
        run_or_err("function* g(){ yield 1; yield 2; } [...g()].join(',')").starts_with("ERR:"),
        "expected spread of a generator to error"
    );
    // array-destructuring binds undefined rather than pulling from the iterator
    assert_eq!(
        run_str("function* g(){ yield 1; yield 2; } const [a,b]=g(); `${String(a)},${String(b)}`"),
        "undefined,undefined" // JS: "1,2"
    );
}

#[test]
fn class_generator_method_does_not_compile_as_generator_divergence() {
    // DIVERGENCE (documented): a class method written `*method(){}` does not
    // compile its body as a generator (object-literal `*m(){}` does — see
    // object_literal_generator_method). Asserting actual behavior; the body
    // errors that `yield` is outside a generator function.
    let out = run_or_err("class C { *items(){ yield 'x'; yield 'y'; } } const c=new C(); let o=[]; for(const v of c.items()) o.push(v); o.join(',')");
    assert!(
        out.starts_with("ERR:"),
        "expected class *method() generator body to be unsupported, got {out}"
    );
}

#[test]
fn custom_symbol_iterator_object_documented_divergence() {
    // DIVERGENCE (documented): a plain object exposing a custom `[Symbol.iterator]`
    // is not recognized as iterable by `for...of` (the well-known-symbol iterator
    // protocol is not dispatched). Arrays/strings/generators iterate fine.
    let out = run_or_err(
        "const obj={ [Symbol.iterator](){ let i=0; return { next(){ return i<2 ? {value:i++,done:false} : {value:undefined,done:true}; } }; } }; let o=[]; for(const x of obj) o.push(x); o.join(',')",
    );
    assert!(out.starts_with("ERR:"), "expected custom Symbol.iterator object to be non-iterable, got {out}");
}
