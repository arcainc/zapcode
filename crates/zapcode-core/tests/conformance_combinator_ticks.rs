//! Conformance: `Promise.race`/`any` settle in tick order over pending
//! chains, and `.then`/`.catch`/`.finally` always return a NEW dependent
//! promise.
//!
//! * **race/any tick order**: with microtask-pending elements, the winner is
//!   the first element to settle (race) / fulfill (any) as the queue drains
//!   — a shallower chain in a later slot beats a deeper chain in an earlier
//!   one, as in Node. Losing elements' later rejections are absorbed by the
//!   combinator (sink reactions), not reported as unhandled.
//! * **method identity**: `p.then()` / `p.catch(h)` / `p.finally(x)` return
//!   a fresh promise (`!== p`) that forwards the outcome — including with
//!   non-callable handlers.
//!
//! All assertions ground-truthed against real Node.
//!
//! Pinned (pre-existing): promise methods chained directly on a *batch*
//! promise (`Promise.race(...).catch(h)` where the batch holds deferred host
//! calls) are pass-through no-ops — guard the `await` with try/catch
//! instead.

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

// ════════════════════════════════════════════════════════════════════════════
//  race / any in tick order
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn race_picks_the_shallower_chain_regardless_of_slot() {
    assert_eq!(
        run_str(
            "await Promise.race([ \
                 Promise.resolve(1).then(x => x).then(x => 'slow'), \
                 Promise.resolve(2).then(x => 'fast'), \
             ])"
        ),
        "fast"
    );
}

#[test]
fn race_rejection_that_settles_first_wins() {
    assert_eq!(
        run_str(
            "let r = 'none'; \
             try { \
                 await Promise.race([ \
                     Promise.resolve(1).then(x => x).then(() => 'slow'), \
                     Promise.resolve(2).then(() => { throw 'boom' }), \
                 ]); \
             } catch (e) { r = 'caught:' + e; } \
             r"
        ),
        "caught:boom"
    );
}

#[test]
fn race_losers_rejecting_later_are_not_unhandled() {
    // The combinator absorbed the losing element; its later rejection must
    // not fail the run at end-of-drain.
    assert_eq!(
        run_str(
            "const r = await Promise.race([ \
                 Promise.resolve('quick'), \
                 Promise.resolve(1).then(() => { throw 'late-loser' }), \
             ]); \
             r"
        ),
        "quick"
    );
}

#[test]
fn any_skips_an_earlier_rejection_for_a_later_fulfillment() {
    assert_eq!(
        run_str(
            "await Promise.any([ \
                 Promise.resolve(1).then(() => { throw 'r1' }), \
                 Promise.resolve(2).then(x => 'ok2').then(x => x), \
             ])"
        ),
        "ok2"
    );
}

#[test]
fn any_picks_the_shallower_fulfillment() {
    assert_eq!(
        run_str(
            "await Promise.any([ \
                 Promise.resolve(1).then(x => x).then(() => 'deep'), \
                 Promise.resolve(2).then(() => 'shallow'), \
             ])"
        ),
        "shallow"
    );
}

#[test]
fn race_with_plain_value_still_wins_immediately() {
    assert_eq!(
        run_str("await Promise.race([7, Promise.resolve(99).then(x => x)])"),
        "7"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  AggregateError is a real Error
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn any_all_reject_aggregate_error_is_an_error_instance() {
    // Node: true|true|AggregateError|a,b
    assert_eq!(
        run_str(
            "async function main() { \
                 try { await Promise.any([Promise.reject('a'), Promise.reject('b')]); return 'no'; } \
                 catch (e) { \
                     return (e instanceof AggregateError) + '|' + (e instanceof Error) \
                         + '|' + e.name + '|' + e.errors.join(','); \
                 } \
             } \
             main();"
        ),
        "true|true|AggregateError|a,b"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  then/catch/finally identity and pass-through reactions
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn then_returns_a_new_promise() {
    assert_eq!(
        run_str("const p = Promise.resolve(1); `${p.then() === p}|${await p.then()}`"),
        "false|1"
    );
    assert_eq!(
        run_str("const q = Promise.resolve(2); `${q.catch(() => {}) === q}`"),
        "false"
    );
}

#[test]
fn handlerless_then_forwards_a_rejection() {
    assert_eq!(
        run_str(
            "await Promise.reject('x') \
                 .then(undefined, undefined) \
                 .catch(e => 'c:' + e)"
        ),
        "c:x"
    );
}

#[test]
fn non_callable_finally_passes_the_value_through() {
    assert_eq!(run_str("await Promise.resolve(3).finally(7)"), "3");
}
