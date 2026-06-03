//! Conformance breadth: generators & iteration.
//!
//! Generator functions consumed via `for...of` and the explicit iterator protocol
//! (`.next()`, `.next(value)`), including infinite generators with bounded "take",
//! generator closures over parameters, early `break`, `return`-stops-iteration,
//! and `yield*` delegation to an ARRAY. Two documented gaps are pinned to actual
//! behavior: `yield*` delegating to another GENERATOR does not flatten, and a
//! plain object with a custom `[Symbol.iterator]` is not iterable by `for...of`.
//! (Spread `[...gen()]` and array-destructuring of a generator are also not
//! supported — covered in `spread_and_destructure_of_generator_*`.)

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

/// Run that tolerates a RuntimeError (returns the error text prefixed `ERR:`),
/// for pinning documented "this is not supported" behavior without panicking.
fn run_or_err(code: &str) -> String {
    match ZapcodeRun::new(code.to_string(), Vec::new(), Vec::new(), ResourceLimits::default())
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

// ----------------------------------------------------------------------------
// for...of consumption
// ----------------------------------------------------------------------------

#[test]
fn generator_for_of() {
    assert_eq!(
        run_str("function* g(){ yield 1; yield 2; yield 3; } let o = []; for (const x of g()) o.push(x); o.join(',')"),
        "1,2,3"
    );
    assert_eq!(
        run_str("function* g(){ yield 10; yield 20; } let s = 0; for (const x of g()) s += x; s"),
        "30"
    );
}

#[test]
fn generator_with_parameters_and_closure() {
    assert_eq!(
        run_str("function* range(a, b){ for (let i = a; i < b; i++) yield i; } let o = []; for (const x of range(2, 5)) o.push(x); o.join(',')"),
        "2,3,4"
    );
    assert_eq!(
        run_str("function* counter(start){ let n = start; while (n < start + 3) yield n++; } let o = []; for (const x of counter(100)) o.push(x); o.join(',')"),
        "100,101,102"
    );
}

#[test]
fn generator_early_break_and_return() {
    // for-of can break out early.
    assert_eq!(
        run_str("function* g(){ yield 1; yield 2; yield 3; yield 4; } let o = []; for (const x of g()){ if (x > 2) break; o.push(x); } o.join(',')"),
        "1,2"
    );
    // a bare `return` stops the generator.
    assert_eq!(
        run_str("function* g(){ yield 1; return; yield 2; } let o = []; for (const x of g()) o.push(x); o.join(',')"),
        "1"
    );
    // `return value` also stops iteration (the value is not yielded).
    assert_eq!(
        run_str("function* g(){ yield 1; return 99; yield 2; } let o = []; for (const x of g()) o.push(x); o.join(',')"),
        "1"
    );
}

// ----------------------------------------------------------------------------
// Explicit iterator protocol
// ----------------------------------------------------------------------------

#[test]
fn generator_next_value_and_done() {
    assert_eq!(
        run_str("function* g(){ yield 'a'; yield 'b'; } const it = g(); `${it.next().value},${it.next().value},${it.next().done}`"),
        "a,b,true"
    );
    assert_eq!(
        run_str("function* g(){ yield 1; } const it = g(); const a = it.next(); const b = it.next(); `${a.value},${a.done},${String(b.value)},${b.done}`"),
        "1,false,undefined,true"
    );
}

#[test]
fn generator_next_passes_value_into_yield() {
    // The value passed to `.next(v)` becomes the result of the paused `yield`.
    assert_eq!(
        run_str("function* g(){ const x = yield 1; const y = yield x + 1; yield y + 1; } const it = g(); it.next(); `${it.next(10).value},${it.next(20).value}`"),
        "11,21"
    );
    assert_eq!(
        run_str("function* g(){ const x = yield 1; yield x * 2; } const it = g(); it.next(); it.next(10).value"),
        "20"
    );
}

#[test]
fn infinite_generator_with_bounded_take() {
    assert_eq!(
        run_str("function* nat(){ let n = 0; while (true) yield n++; } const it = nat(); `${it.next().value},${it.next().value},${it.next().value}`"),
        "0,1,2"
    );
    // take(k) over an infinite generator
    assert_eq!(
        run_str("function* nat(){ let n = 1; while (true) yield n++; } const it = nat(); let out = []; for (let i = 0; i < 5; i++) out.push(it.next().value); out.join(',')"),
        "1,2,3,4,5"
    );
}

// ----------------------------------------------------------------------------
// yield* delegation
// ----------------------------------------------------------------------------

#[test]
fn yield_star_over_array_flattens() {
    assert_eq!(
        run_str("function* g(){ yield* [1, 2, 3]; } let o = []; for (const x of g()) o.push(x); o.join(',')"),
        "1,2,3"
    );
    assert_eq!(
        run_str("function* g(){ yield 0; yield* [1, 2]; yield 3; } let o = []; for (const x of g()) o.push(x); o.join(',')"),
        "0,1,2,3"
    );
}

#[test]
fn yield_star_over_generator_documented_divergence() {
    // DIVERGENCE (documented): `yield*` delegating to another GENERATOR does not
    // flatten — the delegate generator object is yielded as a single value
    // (rendering "[object Generator]") instead of its elements. `yield*` over an
    // ARRAY works (see yield_star_over_array_flattens). Asserting actual behavior.
    assert_eq!(
        run_str("function* inner(){ yield 1; yield 2; } function* outer(){ yield 0; yield* inner(); yield 3; } let o = []; for (const x of outer()) o.push(String(x)); o.join(',')"),
        "0,[object Generator],3" // JS: "0,1,2,3"
    );
}

// ----------------------------------------------------------------------------
// Documented unsupported consumption forms
// ----------------------------------------------------------------------------

#[test]
fn spread_and_destructure_of_generator_documented_divergence() {
    // DIVERGENCE (documented): spread `[...gen()]` and array-destructuring of a
    // generator are not supported. Use `for...of` or `.next()` to consume a
    // generator. Asserting actual behavior.
    assert!(
        run_or_err("function* g(){ yield 1; yield 2; } [...g()].join(',')").starts_with("ERR:"),
        "expected spread of a generator to error"
    );
    // array-destructuring binds undefined rather than pulling from the iterator
    assert_eq!(
        run_str("function* g(){ yield 1; yield 2; } const [a, b] = g(); `${String(a)},${String(b)}`"),
        "undefined,undefined" // JS: "1,2"
    );
}

#[test]
fn custom_symbol_iterator_object_documented_divergence() {
    // DIVERGENCE (documented): a plain object exposing a custom `[Symbol.iterator]`
    // is not recognized as iterable by `for...of` (the well-known-symbol iterator
    // protocol is not dispatched). Arrays/strings/Map/Set/generators iterate fine.
    let out = run_or_err(
        "const obj = { [Symbol.iterator](){ let i = 0; return { next(){ return i < 2 ? {value: i++, done: false} : {value: undefined, done: true}; } }; } }; let o = []; for (const x of obj) o.push(x); o.join(',')",
    );
    assert!(out.starts_with("ERR:"), "expected custom Symbol.iterator object to be non-iterable, got {out}");
}
