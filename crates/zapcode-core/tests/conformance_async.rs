//! Conformance suite (round 1): Async / Promises.
//!
//! Test262-style breadth across the async surface the interpreter exposes:
//! `Promise.{resolve,reject,all,race,any,allSettled}`, `then`/`catch`/`finally`
//! chaining with value- and promise-unwrap, `await` in array / object / ternary /
//! template / call-argument / logical positions, `for await (… of …)` over arrays
//! of promises (sequential consumption), nested awaits, async function / async
//! arrow return-value adoption, and sequential-vs-parallel evaluation order.
//!
//! Programs are run to completion and stringified with `to_js_string` (often via
//! an explicit `JSON.stringify`) so the result is byte-comparable to real Node.
//!
//! ── Harness / parser notes ──────────────────────────────────────────────────
//!   * Top-level `await EXPR` works, but `await (` at statement start is parsed
//!     as a *call* `await(...)`, so any `await`ed expression that must begin with
//!     `(` (e.g. `await (c ? p : q)`) is either assigned through a variable first
//!     or wrapped in a named `async function main() { … } main();`. The
//!     `main()`-as-last-statement pattern resolves the top-level promise and is
//!     used wherever `for await`, multi-statement bodies, or `try/catch` are
//!     needed (top-level `for await` is itself a ParseError — for-await is only
//!     legal inside an async function, exactly like real JS).
//!
//! ── Documented divergences from real JS (NOT asserted to the real answer) ────
//!   (verified against the live interpreter; see STRESS-PASS-BUGS.md for context)
//!   * (FIXED by microtask Stage 3 — kept for history) the await-rejection
//!     boundary used to wrap reasons in a fresh `Error`; the ORIGINAL reason
//!     (identity, type, message) now rethrows at `await`, matching Node, and
//!     IS asserted below.
//!   * `Promise.any` ALL-REJECT delivers the real AggregateError-shaped
//!     reason (`e instanceof AggregateError`, `e.name === "AggregateError"`,
//!     `e.errors` populated) since Stage 3. Residual divergence pinned WITH a
//!     comment: it is not an `Error` *instance* (`e instanceof Error` is
//!     `false`; Node: `true`).
//!   * TOP-LEVEL AWAIT IS STILL INLINE (Stage 3 covers async *function*
//!     bodies, which now park at `await` with spec tick order): a top-level
//!     `await` of an already-settled promise continues inline rather than
//!     yielding a tick. Tests wrap ordering-sensitive code in
//!     `async function main()` (the recommended pattern anyway).
//!   * `Promise.race([])` / `Promise.any([])` over an *empty* array stay pending
//!     forever in real JS; here the awaited value is a still-pending promise that
//!     string-coerces to `"[object Promise]"`. Pinned WITH a comment.

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

/// Wrap a multi-statement async body in `async function main(){…} main();` so a
/// top-level promise (and any `for await` / `try/catch` inside) is resolved to a
/// plain completion value. `body` should `return` its result.
fn run_main(body: &str) -> String {
    run_str(&format!("async function main() {{\n{body}\n}}\nmain();"))
}

// ════════════════════════════════════════════════════════════════════════════
//  Promise.resolve
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn resolve_scalar_values() {
    assert_eq!(run_str("await Promise.resolve(42)"), "42");
    assert_eq!(run_str("await Promise.resolve('hi')"), "hi");
    assert_eq!(run_str("await Promise.resolve(true)"), "true");
    assert_eq!(run_str("await Promise.resolve(null)"), "null");
    assert_eq!(run_str("String(await Promise.resolve(undefined))"), "undefined");
    assert_eq!(run_str("String(await Promise.resolve())"), "undefined");
    assert_eq!(run_str("await Promise.resolve(0)"), "0");
    assert_eq!(run_str("await Promise.resolve(-3.5)"), "-3.5");
}

#[test]
fn resolve_object_and_array_values() {
    assert_eq!(
        run_str("JSON.stringify(await Promise.resolve({ a: 1, b: 'x' }))"),
        r#"{"a":1,"b":"x"}"#
    );
    assert_eq!(
        run_str("JSON.stringify(await Promise.resolve([1, 2, 3]))"),
        "[1,2,3]"
    );
    assert_eq!(
        run_str("JSON.stringify(await Promise.resolve({ nested: { deep: [true, null] } }))"),
        r#"{"nested":{"deep":[true,null]}}"#
    );
}

#[test]
fn resolve_is_object_typed() {
    assert_eq!(run_str("typeof Promise.resolve(1)"), "object");
}

#[test]
fn resolve_of_a_promise_is_idempotent() {
    // Promise.resolve(p) on an already-resolved promise returns it as-is.
    assert_eq!(
        run_str("const p = Promise.resolve(1); (Promise.resolve(p) === p)"),
        "true"
    );
    // Awaiting Promise.resolve(p) unwraps to the inner value (no double-wrap).
    assert_eq!(
        run_str("const p = Promise.resolve(5); await Promise.resolve(p)"),
        "5"
    );
    // Awaiting a resolved promise twice via reassignment yields the scalar.
    assert_eq!(
        run_str("const p1 = Promise.resolve(42); const p2 = Promise.resolve(p1); await p2"),
        "42"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  Promise.reject
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn reject_caught_by_handler_passes_original_reason() {
    // `.catch(fn)` receives the ORIGINAL reason (spec-correct path).
    assert_eq!(
        run_str("await Promise.reject('oops').catch(e => 'caught:' + e)"),
        "caught:oops"
    );
    assert_eq!(
        run_str("await Promise.reject(404).catch(code => 'code=' + code)"),
        "code=404"
    );
    // `.then(_, onRejected)` second-arg also receives the original reason.
    assert_eq!(
        run_str("await Promise.reject('e').then(v => 'ok', e => 'handled:' + e)"),
        "handled:e"
    );
}

#[test]
fn reject_propagating_to_await_is_catchable_error() {
    // The await-rejection rethrows the ORIGINAL reason (identity preserved
    // since microtask Stage 3) — a caught string is that string, as in Node.
    assert_eq!(
        run_main(
            r#"
            try { await Promise.reject('boom'); return 'no-throw'; }
            catch (e) { return 'caught:' + e + '|' + (typeof e === 'string'); }
            "#
        ),
        "caught:boom|true"
    );
    // Rejecting with an Error instance: the SAME Error object (message intact)
    // rethrows at the await boundary.
    assert_eq!(
        run_main(
            r#"
            try { await Promise.reject(new Error('m')); return 'no-throw'; }
            catch (e) { return (e instanceof Error) + ':' + e.message; }
            "#
        ),
        "true:m"
    );
}

#[test]
fn reject_recovered_then_continues_fulfilled() {
    // catch recovers, downstream then sees the recovered value.
    assert_eq!(
        run_str("await Promise.reject('e').catch(() => 'recovered').then(x => x + '!')"),
        "recovered!"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  Promise.all — order preservation + value/promise unwrap
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn all_resolves_in_input_order_not_settle_order() {
    assert_eq!(
        run_str("JSON.stringify(await Promise.all([Promise.resolve(1), Promise.resolve(2), Promise.resolve(3)]))"),
        "[1,2,3]"
    );
    // Result order follows the INPUT array, independent of which would settle
    // first in a real event loop.
    assert_eq!(
        run_str("JSON.stringify(await Promise.all([Promise.resolve(3), Promise.resolve(1), Promise.resolve(2)]))"),
        "[3,1,2]"
    );
}

#[test]
fn all_mixes_plain_values_and_promises() {
    assert_eq!(
        run_str("JSON.stringify(await Promise.all([1, 2, 3]))"),
        "[1,2,3]"
    );
    assert_eq!(
        run_str("JSON.stringify(await Promise.all([1, Promise.resolve(2), 'three', Promise.resolve(4)]))"),
        r#"[1,2,"three",4]"#
    );
}

#[test]
fn all_empty_array_resolves_to_empty_array() {
    assert_eq!(run_str("JSON.stringify(await Promise.all([]))"), "[]");
}

#[test]
fn all_with_spread_and_nesting() {
    assert_eq!(
        run_str(
            "const ps = [Promise.resolve(1), Promise.resolve(2)]; \
             JSON.stringify(await Promise.all([...ps, Promise.resolve(3)]))"
        ),
        "[1,2,3]"
    );
    assert_eq!(
        run_str("JSON.stringify(await Promise.all([Promise.all([1, 2]), Promise.all([3, 4])]))"),
        "[[1,2],[3,4]]"
    );
}

#[test]
fn all_preserves_object_element_shapes() {
    assert_eq!(
        run_str("JSON.stringify(await Promise.all([Promise.resolve({ a: 1 }), Promise.resolve([2, 3])]))"),
        r#"[{"a":1},[2,3]]"#
    );
}

#[test]
fn all_rejects_when_any_element_rejects() {
    // First rejection short-circuits; await rethrows the original reason.
    assert_eq!(
        run_main(
            r#"
            try { await Promise.all([Promise.resolve(1), Promise.reject('bad'), Promise.resolve(3)]); return 'no-throw'; }
            catch (e) { return 'caught:' + e; }
            "#
        ),
        "caught:bad"
    );
}

#[test]
fn all_results_feed_subsequent_computation() {
    assert_eq!(
        run_main(
            r#"
            const [a, b, c] = await Promise.all([Promise.resolve(10), Promise.resolve(20), Promise.resolve(30)]);
            return a + b + c;
            "#
        ),
        "60"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  Promise.race — first settled wins
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn race_single_element() {
    assert_eq!(run_str("await Promise.race([Promise.resolve('a')])"), "a");
}

#[test]
fn race_returns_first_settled_value() {
    // With synchronously-resolved inputs the first array element settles first.
    assert_eq!(
        run_str("await Promise.race([Promise.resolve('first'), Promise.resolve('second')])"),
        "first"
    );
    assert_eq!(
        run_str("await Promise.race([Promise.resolve('x'), Promise.resolve('y'), Promise.resolve('z')])"),
        "x"
    );
}

#[test]
fn race_with_plain_value_first() {
    assert_eq!(run_str("await Promise.race([7, Promise.resolve(99)])"), "7");
}

#[test]
fn race_rejection_first_propagates_as_error() {
    assert_eq!(
        run_main(
            r#"
            try { await Promise.race([Promise.reject('R'), Promise.resolve('ok')]); return 'no-throw'; }
            catch (e) { return 'caught:' + e; }
            "#
        ),
        "caught:R"
    );
}

#[test]
fn race_over_empty_array_stays_pending() {
    // Documented divergence: real JS would hang forever; here the awaited value
    // is a still-pending promise that string-coerces to "[object Promise]".
    assert_eq!(run_str("String(await Promise.race([]))"), "[object Promise]");
}

// ════════════════════════════════════════════════════════════════════════════
//  Promise.any — first fulfilled wins, skipping rejections
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn any_returns_first_fulfilled() {
    assert_eq!(
        run_str("await Promise.any([Promise.resolve('first'), Promise.resolve('second')])"),
        "first"
    );
}

#[test]
fn any_skips_leading_rejections() {
    assert_eq!(
        run_str("await Promise.any([Promise.reject('r1'), Promise.resolve('win')])"),
        "win"
    );
    assert_eq!(
        run_str("await Promise.any([Promise.reject('r1'), Promise.reject('r2'), Promise.resolve('win')])"),
        "win"
    );
}

#[test]
fn any_mixes_plain_values() {
    assert_eq!(
        run_str("await Promise.any([Promise.reject('r'), 'plain'])"),
        "plain"
    );
}

#[test]
fn any_all_reject_throws_catchable_error() {
    // The original AggregateError-shaped reason now reaches the catch (Node:
    // "true|true|AggregateError"). Residual divergence pinned WITH a comment:
    // the constructed AggregateError is not an `Error` instance here.
    assert_eq!(
        run_main(
            r#"
            try { await Promise.any([Promise.reject('a'), Promise.reject('b')]); return 'no-throw'; }
            catch (e) {
                return (e instanceof AggregateError) + '|' + (e instanceof Error) + '|' + e.name;
            }
            "#
        ),
        "true|false|AggregateError"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  AggregateError (direct construction)
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn aggregate_error_direct_construction() {
    assert_eq!(
        run_str("const e = new AggregateError([1, 2], 'all failed'); e.name + '|' + e.message"),
        "AggregateError|all failed"
    );
    assert_eq!(
        run_str("const e = new AggregateError(['x', 'y'], 'm'); JSON.stringify(e.errors)"),
        r#"["x","y"]"#
    );
    assert_eq!(
        run_str("new AggregateError([], 'm') instanceof Error"),
        "true"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  Promise.allSettled — per-element status, never rejects
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn all_settled_per_element_status() {
    assert_eq!(
        run_str(
            "JSON.stringify(await Promise.allSettled([Promise.resolve(1), Promise.reject('e'), Promise.resolve('x')]))"
        ),
        r#"[{"status":"fulfilled","value":1},{"status":"rejected","reason":"e"},{"status":"fulfilled","value":"x"}]"#
    );
}

#[test]
fn all_settled_status_sequence() {
    assert_eq!(
        run_str(
            "(await Promise.allSettled([Promise.resolve(1), Promise.reject('e')])).map(r => r.status).join(',')"
        ),
        "fulfilled,rejected"
    );
}

#[test]
fn all_settled_mixes_plain_values() {
    // Non-promise elements are treated as already-fulfilled.
    assert_eq!(
        run_str(
            "JSON.stringify(await Promise.allSettled([1, Promise.resolve(2), Promise.reject('e3')]))"
        ),
        r#"[{"status":"fulfilled","value":1},{"status":"fulfilled","value":2},{"status":"rejected","reason":"e3"}]"#
    );
}

#[test]
fn all_settled_never_rejects_even_when_all_reject() {
    // Crucial allSettled invariant: it resolves (never throws) even when every
    // input rejects.
    assert_eq!(
        run_main(
            r#"
            const r = await Promise.allSettled([Promise.reject('a'), Promise.reject('b')]);
            return r.map(x => x.status + ':' + x.reason).join(',');
            "#
        ),
        "rejected:a,rejected:b"
    );
}

#[test]
fn all_settled_empty_array() {
    assert_eq!(run_str("JSON.stringify(await Promise.allSettled([]))"), "[]");
}

#[test]
fn all_settled_carries_object_values_and_reasons() {
    assert_eq!(
        run_str(
            "JSON.stringify(await Promise.allSettled([Promise.resolve({ ok: true }), Promise.reject({ code: 5 })]))"
        ),
        r#"[{"status":"fulfilled","value":{"ok":true}},{"status":"rejected","reason":{"code":5}}]"#
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  then — value & promise unwrap, chaining
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn then_transforms_fulfilled_value() {
    assert_eq!(run_str("await Promise.resolve(42).then(x => x + 8)"), "50");
    assert_eq!(
        run_str("await Promise.resolve('a').then(s => s.toUpperCase())"),
        "A"
    );
}

#[test]
fn then_chains_multiple_stages() {
    assert_eq!(
        run_str("await Promise.resolve(1).then(x => x + 1).then(x => x * 10).then(x => x - 5)"),
        "15"
    );
}

#[test]
fn then_unwraps_returned_promise() {
    // Returning a promise from a then callback adopts its resolution.
    assert_eq!(
        run_str("await Promise.resolve(1).then(x => Promise.resolve(x + 10))"),
        "11"
    );
    // Nested thenable returned from then.
    assert_eq!(
        run_str("await Promise.resolve(1).then(x => Promise.resolve(x).then(y => y + 100))"),
        "101"
    );
}

#[test]
fn then_returning_rejected_promise_is_caught_downstream() {
    assert_eq!(
        run_str("await Promise.resolve(1).then(() => Promise.reject('inner')).catch(e => 'c:' + e)"),
        "c:inner"
    );
}

#[test]
fn then_with_two_arguments_takes_fulfill_path() {
    assert_eq!(
        run_str("await Promise.resolve(5).then(v => v + 1, e => 'err')"),
        "6"
    );
}

#[test]
fn then_produces_object_result() {
    assert_eq!(
        run_str("JSON.stringify(await Promise.resolve({ a: 1 }).then(o => ({ ...o, b: 2 })))"),
        r#"{"a":1,"b":2}"#
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  catch — rejection handling, skip-on-fulfill
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn catch_handles_rejection() {
    assert_eq!(
        run_str("await Promise.reject('bad').catch(e => 'recovered:' + e)"),
        "recovered:bad"
    );
}

#[test]
fn catch_is_skipped_when_fulfilled() {
    assert_eq!(
        run_str("await Promise.resolve('ok').catch(() => 'never')"),
        "ok"
    );
}

#[test]
fn catch_then_continues_with_recovered_value() {
    assert_eq!(
        run_str("await Promise.reject('e').catch(() => 'recovered').then(x => x + '!')"),
        "recovered!"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  finally — side-effect, value passthrough
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn finally_passes_through_fulfilled_value() {
    // finally's return value is ignored; the original resolution flows through.
    assert_eq!(run_str("await Promise.resolve('v').finally(() => 'x')"), "v");
    assert_eq!(
        run_str("await Promise.resolve(42).finally(() => 999)"),
        "42"
    );
    assert_eq!(
        run_str("await Promise.resolve(7).then(x => x).finally(() => 999)"),
        "7"
    );
}

#[test]
fn finally_ignores_its_own_return_for_string_value() {
    assert_eq!(
        run_str("await Promise.resolve('original').finally(() => 'ignored')"),
        "original"
    );
}

#[test]
fn finally_runs_side_effect_and_preserves_value() {
    assert_eq!(
        run_main(
            r#"
            let ran = false;
            const v = await Promise.resolve('keep').finally(() => { ran = true; });
            return v + '|' + ran;
            "#
        ),
        "keep|true"
    );
}

#[test]
fn finally_on_rejection_still_throws() {
    // finally does not swallow a rejection; the original reason rethrows.
    assert_eq!(
        run_main(
            r#"
            try { await Promise.reject('boom').finally(() => 'ignored'); return 'no-throw'; }
            catch (e) { return 'caught:' + e; }
            "#
        ),
        "caught:boom"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  await in expression positions
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn await_in_array_literal_positions() {
    assert_eq!(
        run_str("const a = [await Promise.resolve(1), 2, await Promise.resolve(3)]; JSON.stringify(a)"),
        "[1,2,3]"
    );
    // Multiple awaits then a transform.
    assert_eq!(
        run_str("[await Promise.resolve(1), await Promise.resolve(2)].map(x => x * 2).join(',')"),
        "2,4"
    );
}

#[test]
fn await_in_object_literal_positions() {
    assert_eq!(
        run_str("const o = { a: await Promise.resolve(1), b: await Promise.resolve(2) }; JSON.stringify(o)"),
        r#"{"a":1,"b":2}"#
    );
    assert_eq!(
        run_str(
            "const o = { x: await Promise.resolve('X'), y: { z: await Promise.resolve(9) } }; JSON.stringify(o)"
        ),
        r#"{"x":"X","y":{"z":9}}"#
    );
}

#[test]
fn await_in_ternary_branches() {
    // Each branch may itself await (assigned through a var to avoid the
    // top-level `await (` call-parse quirk).
    assert_eq!(
        run_str("const c = true; const v = c ? await Promise.resolve('yes') : await Promise.resolve('no'); v"),
        "yes"
    );
    assert_eq!(
        run_str("const c = false; const v = c ? await Promise.resolve('yes') : await Promise.resolve('no'); v"),
        "no"
    );
}

#[test]
fn await_a_selected_promise_from_ternary() {
    // Select a promise via ternary, then await it.
    assert_eq!(
        run_str("const c = true; const p = c ? Promise.resolve('t') : Promise.resolve('f'); await p"),
        "t"
    );
    assert_eq!(
        run_main(
            "return await (false ? Promise.resolve('t') : Promise.resolve('f'));"
        ),
        "f"
    );
}

#[test]
fn await_in_template_literal() {
    assert_eq!(run_str("`v=${await Promise.resolve(9)}`"), "v=9");
    assert_eq!(
        run_str("`${await Promise.resolve('a')}-${await Promise.resolve('b')}`"),
        "a-b"
    );
}

#[test]
fn await_in_call_arguments() {
    assert_eq!(
        run_str("[await Promise.resolve(3), await Promise.resolve(4)].join('+')"),
        "3+4"
    );
    assert_eq!(
        run_str("Math.max(await Promise.resolve(5), await Promise.resolve(9), 7)"),
        "9"
    );
}

#[test]
fn await_in_logical_and_arithmetic_expressions() {
    assert_eq!(
        run_str("(await Promise.resolve(0)) || (await Promise.resolve('fallback'))"),
        "fallback"
    );
    assert_eq!(
        run_str("(await Promise.resolve(10)) + (await Promise.resolve(20))"),
        "30"
    );
    assert_eq!(
        run_str("(await Promise.resolve(3)) * (await Promise.resolve(4))"),
        "12"
    );
}

#[test]
fn await_of_non_promise_values() {
    assert_eq!(run_str("await 42"), "42");
    assert_eq!(run_str("await 'plain'"), "plain");
    assert_eq!(run_str("String(await null) + ',' + String(await undefined)"), "null,undefined");
    assert_eq!(run_str("const o = { x: 1 }; JSON.stringify(await o)"), r#"{"x":1}"#);
}

#[test]
fn nested_await() {
    assert_eq!(run_str("await Promise.resolve(await Promise.resolve(5))"), "5");
    assert_eq!(
        run_main("return await Promise.resolve(await Promise.resolve(await Promise.resolve('deep')));"),
        "deep"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  async functions & arrows — return-value adoption
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn async_function_return_is_awaitable() {
    assert_eq!(
        run_str("async function f() { return 7; } await f()"),
        "7"
    );
    assert_eq!(
        run_str("async function f(x) { return x * 2; } await f(21)"),
        "42"
    );
}

#[test]
fn async_arrow_return_is_awaitable() {
    assert_eq!(run_str("const f = async (x) => x * 2; await f(21)"), "42");
    assert_eq!(
        run_str("const f = async () => ({ k: 'v' }); JSON.stringify(await f())"),
        r#"{"k":"v"}"#
    );
}

#[test]
fn async_function_awaiting_inner_promise() {
    assert_eq!(
        run_str(
            "async function f() { const a = await Promise.resolve(2); const b = await Promise.resolve(3); return a + b; } await f()"
        ),
        "5"
    );
}

#[test]
fn async_function_returning_a_promise_unwraps_once() {
    // Returning a promise from an async function does not double-wrap.
    assert_eq!(
        run_str("async function f() { return Promise.resolve(99); } await f()"),
        "99"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  await in loops + accumulation
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn await_inside_for_loop_accumulates() {
    assert_eq!(
        run_str("let t = 0; for (let i = 0; i < 3; i++) { t += await Promise.resolve(i); } t"),
        "3"
    );
}

#[test]
fn await_inside_while_loop() {
    assert_eq!(
        run_main(
            r#"
            let i = 0, acc = '';
            while (i < 3) { acc += await Promise.resolve(String(i)); i++; }
            return acc;
            "#
        ),
        "012"
    );
}

#[test]
fn sequential_awaits_run_in_program_order() {
    // Data-dependency order is identical under any microtask model: the log is
    // built in the order the awaits are reached.
    assert_eq!(
        run_main(
            r#"
            const log = [];
            async function step(n) { log.push('s' + n); return n; }
            await step(1); await step(2); await step(3);
            return log.join(',');
            "#
        ),
        "s1,s2,s3"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  for await ... of  (over arrays of promises / values / mixed)
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn for_await_sums_array_of_promises() {
    assert_eq!(
        run_main(
            r#"
            let s = 0;
            for await (const x of [Promise.resolve(1), Promise.resolve(2), Promise.resolve(3)]) { s += x; }
            return s;
            "#
        ),
        "6"
    );
}

#[test]
fn for_await_over_plain_values() {
    assert_eq!(
        run_main(
            r#"
            let s = 0;
            for await (const x of [10, 20, 30]) { s += x; }
            return s;
            "#
        ),
        "60"
    );
}

#[test]
fn for_await_over_mixed_values_and_promises() {
    assert_eq!(
        run_main(
            r#"
            const out = [];
            for await (const x of [1, Promise.resolve(2), 3, Promise.resolve(4)]) { out.push(x); }
            return JSON.stringify(out);
            "#
        ),
        "[1,2,3,4]"
    );
}

#[test]
fn for_await_with_destructuring_binding() {
    assert_eq!(
        run_main(
            r#"
            const out = [];
            for await (const [a, b] of [Promise.resolve([1, 2]), Promise.resolve([3, 4])]) { out.push(a + b); }
            return JSON.stringify(out);
            "#
        ),
        "[3,7]"
    );
}

#[test]
fn for_await_honors_break() {
    assert_eq!(
        run_main(
            r#"
            let s = 0;
            for await (const x of [Promise.resolve(1), Promise.resolve(2), Promise.resolve(3)]) {
                if (x === 2) break;
                s += x;
            }
            return s;
            "#
        ),
        "1"
    );
}

#[test]
fn for_await_honors_continue() {
    assert_eq!(
        run_main(
            r#"
            let s = 0;
            for await (const x of [1, 2, 3, 4]) { if (x % 2 === 0) continue; s += x; }
            return s;
            "#
        ),
        "4"
    );
}

#[test]
fn for_await_applies_method_to_each_resolved_value() {
    assert_eq!(
        run_main(
            r#"
            const out = [];
            for await (const x of [Promise.resolve('a'), Promise.resolve('b')]) { out.push(x.toUpperCase()); }
            return out.join('');
            "#
        ),
        "AB"
    );
}

#[test]
fn for_await_nested_loops() {
    assert_eq!(
        run_main(
            r#"
            let total = 0;
            for await (const a of [Promise.resolve(1), Promise.resolve(2)]) {
                for await (const b of [Promise.resolve(10), Promise.resolve(20)]) {
                    total += a * b;
                }
            }
            return total;
            "#
        ),
        // (1*10 + 1*20) + (2*10 + 2*20) = 30 + 60 = 90
        "90"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  sequential vs parallel — observable data-dependency order
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn parallel_all_collects_in_input_order() {
    // Promise.all over a list of async-fn calls: results follow input order.
    assert_eq!(
        run_main(
            r#"
            async function step(n) { return n * 10; }
            const r = await Promise.all([step(1), step(2), step(3)]);
            return JSON.stringify(r);
            "#
        ),
        "[10,20,30]"
    );
}

#[test]
fn sequential_chain_threads_value_through_stages() {
    assert_eq!(
        run_main(
            r#"
            async function inc(x) { return x + 1; }
            let v = 0;
            v = await inc(v);
            v = await inc(v);
            v = await inc(v);
            return v;
            "#
        ),
        "3"
    );
}

#[test]
fn parallel_map_then_await_all() {
    // The common "map to promises, await all" pattern.
    assert_eq!(
        run_main(
            r#"
            const ids = [1, 2, 3, 4];
            const results = await Promise.all(ids.map(async (id) => id * id));
            return JSON.stringify(results);
            "#
        ),
        "[1,4,9,16]"
    );
}

#[test]
fn parallel_all_with_reduce_of_results() {
    assert_eq!(
        run_main(
            r#"
            const sums = await Promise.all([Promise.resolve(5), Promise.resolve(10), Promise.resolve(15)]);
            return sums.reduce((a, b) => a + b, 0);
            "#
        ),
        "30"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  combinator interplay / realistic agent patterns
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn all_settled_then_filter_fulfilled() {
    assert_eq!(
        run_main(
            r#"
            const settled = await Promise.allSettled([
                Promise.resolve(1),
                Promise.reject('skip'),
                Promise.resolve(3),
            ]);
            const ok = settled.filter(s => s.status === 'fulfilled').map(s => s.value);
            return JSON.stringify(ok);
            "#
        ),
        "[1,3]"
    );
}

#[test]
fn any_with_fallback_via_catch() {
    // any resolves to the first fulfilled; here it's the only non-rejected one.
    assert_eq!(
        run_str("await Promise.any([Promise.reject('x'), Promise.resolve('y')])"),
        "y"
    );
}

#[test]
fn race_then_chain_continues() {
    assert_eq!(
        run_str("await Promise.race([Promise.resolve(2), Promise.resolve(3)]).then(x => x * 100)"),
        "200"
    );
}

#[test]
fn all_results_chained_through_then() {
    assert_eq!(
        run_str(
            "await Promise.all([Promise.resolve(1), Promise.resolve(2)]).then(arr => arr.join('-'))"
        ),
        "1-2"
    );
}

#[test]
fn allsettled_reasons_remain_intact() {
    assert_eq!(
        run_str(
            "(await Promise.allSettled([Promise.reject('a'), Promise.resolve(1), Promise.reject('b')])) \
             .map(r => r.status === 'rejected' ? 'R(' + r.reason + ')' : 'F(' + r.value + ')').join(',')"
        ),
        "R(a),F(1),R(b)"
    );
}
