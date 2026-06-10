//! Conformance: microtask queue — `.then`/`.catch`/`.finally` defer to the
//! queue instead of running inline (microtask-design Stage 1).
//!
//! Reactions on a settled promise enqueue a microtask; reactions on a pending
//! chain link register on the promise and enqueue when it settles. The queue
//! drains FIFO after the synchronous run completes (and at each `await` of a
//! pending chain), which is what produces JS's `.then` ordering. A `throw`
//! escaping a handler rejects the chain (so `.catch` down the chain receives
//! it), and a rejection nobody handles fails the run deterministically at
//! end-of-drain.
//!
//! Every ordering assertion here is ground-truthed against real Node.
//!
//! Residual (Stage 3, documented in docs/microtask-design.md): `await` of an
//! already-*settled* promise continues inline rather than yielding a tick, so
//! observations made *between* an await and the end of the program can see
//! fewer ticks than Node. Tests below observe through awaited chains and
//! final values, where the two models agree.

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, Value, ZapcodeRun, ZapcodeSnapshot};

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
//  Ordering: handlers defer past synchronous code, FIFO across chains
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn then_order_defers_past_sync_code() {
    // Node: B,A — the .then handler runs after the synchronous code, not at
    // the point the method is called.
    assert_eq!(
        run_str(
            "const log = []; \
             const p = Promise.resolve('A').then(v => { log.push(v); }); \
             log.push('B'); \
             await p; \
             log.join(',')"
        ),
        "B,A"
    );
}

#[test]
fn fifo_across_independent_chains() {
    // Node: 1,2,3,4 — tick 1 runs the first link of each chain in creation
    // order, tick 2 the second links.
    assert_eq!(
        run_str(
            "const log = []; \
             Promise.resolve().then(() => log.push(1)).then(() => log.push(3)); \
             Promise.resolve().then(() => log.push(2)).then(() => log.push(4)); \
             await Promise.resolve().then(() => {}).then(() => {}).then(() => {}); \
             log.join(',')"
        ),
        "1,2,3,4"
    );
}

#[test]
fn nested_then_runs_before_later_links() {
    // Node: a,b,c — a `.then` queued inside a handler runs after that handler
    // finishes but before the next chain link queued by its settlement.
    assert_eq!(
        run_str(
            "const log = []; \
             const p1 = Promise.resolve(1).then(v => { \
                 log.push('a'); \
                 Promise.resolve(2).then(() => log.push('c')); \
                 log.push('b'); \
             }); \
             await p1.then(() => {}); \
             log.join(',')"
        ),
        "a,b,c"
    );
}

#[test]
fn chained_values_flow_tick_by_tick() {
    assert_eq!(
        run_str("await Promise.resolve(5).then(x => x + 1).then(x => x * 2)"),
        "12"
    );
}

#[test]
fn finally_runs_in_order_and_passes_value_through() {
    assert_eq!(
        run_str(
            "const log = []; \
             const p = Promise.resolve(7) \
                 .finally(() => log.push('fin')) \
                 .then(v => log.push('then:' + v)); \
             log.push('sync'); \
             await p; \
             log.join(',')"
        ),
        "sync,fin,then:7"
    );
}

#[test]
fn catch_is_skipped_on_fulfillment() {
    assert_eq!(
        run_str(
            "await Promise.resolve(3) \
                 .catch(() => 'nope') \
                 .then(x => x * 2)"
        ),
        "6"
    );
}

#[test]
fn rejection_passes_through_handlerless_then() {
    // `.then(onFulfilled)` with no onRejected lets a rejection cascade to the
    // next `.catch` in the chain.
    assert_eq!(
        run_str(
            "await Promise.reject('boom') \
                 .then(x => 'fulfilled:' + x) \
                 .catch(e => 'caught:' + e)"
        ),
        "caught:boom"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  Throws inside handlers reject the chain
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn throw_in_then_routes_to_catch_with_identity() {
    // The original thrown value (not a stringified copy) reaches the handler.
    assert_eq!(
        run_str(
            "const r = await Promise.resolve(1) \
                 .then(() => { throw new Error('boom') }) \
                 .catch(e => `${e instanceof Error}:${e.message}`); \
             r"
        ),
        "true:boom"
    );
}

#[test]
fn throw_in_then_skips_intermediate_links() {
    // Node: the rejection skips fulfilled handlers until a reject handler.
    assert_eq!(
        run_str(
            "const log = []; \
             const r = await Promise.resolve(1) \
                 .then(() => { throw 'x' }) \
                 .then(() => log.push('skipped')) \
                 .catch(e => 'caught:' + e); \
             `${r}|${log.length}`"
        ),
        "caught:x|0"
    );
}

#[test]
fn throw_inside_handler_does_not_reach_callers_try() {
    // A microtask is its own turn: a try/catch around the *registration* of
    // the chain must not catch a throw from inside the handler. The rejection
    // travels down the chain instead.
    assert_eq!(
        run_str(
            "let caught = 'no'; \
             let chain; \
             try { \
                 chain = Promise.resolve(1).then(() => { throw 'inner' }); \
             } catch (e) { caught = 'outer:' + e; } \
             const r = await chain.catch(e => 'chain:' + e); \
             `${r}|${caught}`"
        ),
        "chain:inner|no"
    );
}

#[test]
fn await_of_rejected_chain_is_catchable() {
    // Documented divergence (see conformance_async.rs header): the value
    // caught at the await-rejection boundary is a wrapped Error, so only
    // catchability is asserted — not the reason's identity.
    assert_eq!(
        run_str(
            "let r = 'none'; \
             try { \
                 await Promise.resolve(1).then(() => { throw new Error('x') }); \
             } catch (e) { r = 'caught:' + (e instanceof Error); } \
             r"
        ),
        "caught:true"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  Unhandled rejections surface at end-of-drain
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn unhandled_rejection_in_orphaned_chain_fails_the_run() {
    // The chain is never awaited and has no catch — Node exits nonzero with
    // an unhandled rejection; the run fails deterministically at end-of-drain.
    let err = run_err(
        "Promise.resolve(1).then(() => { throw new Error('kaboom') }); \
         'done'",
    );
    assert!(
        err.contains("Unhandled promise rejection") && err.contains("kaboom"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejection_handled_by_late_catch_is_not_unhandled() {
    // The catch attaches one statement later (same synchronous run) — that
    // still counts as handled, the run completes.
    assert_eq!(
        run_str(
            "const log = []; \
             const p = Promise.resolve(1).then(() => { throw 'x' }); \
             p.catch(e => log.push('caught:' + e)); \
             await p.catch(() => {}); \
             log.join(',')"
        ),
        "caught:x"
    );
}

#[test]
fn rejection_consumed_by_await_is_not_unhandled() {
    // `await` rethrowing into a guest catch consumes the rejection — no
    // end-of-drain report.
    assert_eq!(
        run_str(
            "let r = 'none'; \
             const p = Promise.resolve(1).then(() => { throw 'x' }); \
             try { await p; } catch (e) { r = 'caught'; } \
             r"
        ),
        "caught"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  Thenable adoption
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn handler_returning_promise_is_adopted() {
    assert_eq!(
        run_str("await Promise.resolve(1).then(() => Promise.resolve(9))"),
        "9"
    );
    // A returned pending chain is adopted link by link.
    assert_eq!(
        run_str(
            "await Promise.resolve(1) \
                 .then(() => Promise.resolve(1).then(x => x + 1)) \
                 .then(x => x * 10)"
        ),
        "20"
    );
}

#[test]
fn async_handler_result_is_adopted() {
    // An async handler returns a Promise (Stage 2); the chain adopts it.
    assert_eq!(
        run_str("await Promise.resolve(4).then(async x => x + 1).then(x => x * 2)"),
        "10"
    );
}

#[test]
fn handler_returning_rejected_promise_rejects_the_chain() {
    assert_eq!(
        run_str(
            "await Promise.resolve(1) \
                 .then(() => Promise.reject('bad')) \
                 .catch(e => 'caught:' + e)"
        ),
        "caught:bad"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  Combinators over pending chains
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn promise_all_over_then_chains() {
    assert_eq!(
        run_str(
            "JSON.stringify(await Promise.all([ \
                 Promise.resolve(1).then(x => x * 10), \
                 Promise.resolve(2).then(x => x * 10), \
                 3, \
             ]))"
        ),
        "[10,20,3]"
    );
}

#[test]
fn promise_all_settled_over_then_chains() {
    assert_eq!(
        run_str(
            "const r = await Promise.allSettled([ \
                 Promise.resolve(1).then(x => x + 1), \
                 Promise.resolve(1).then(() => { throw 'bad' }), \
             ]); \
             `${r[0].status}:${r[0].value},${r[1].status}:${r[1].reason}`"
        ),
        "fulfilled:2,rejected:bad"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  Durable execution: suspension mid-drain
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn tool_call_inside_then_suspends_mid_drain_and_resumes_in_order() {
    // The first reaction's handler calls a tool — the VM suspends *mid-drain*
    // with another reaction still queued. After a dump/load round-trip, the
    // resume settles the chain and the drain continues FIFO.
    let runner = ZapcodeRun::new(
        "const log = []; \
         const p = Promise.resolve('go').then(v => callTool('first')); \
         Promise.resolve(1).then(() => log.push('second')); \
         const r = await p.then(x => x + '!'); \
         `${r}|${log.join('')}`"
            .to_string(),
        Vec::new(),
        vec!["callTool".to_string()],
        ResourceLimits::default(),
    )
    .unwrap();

    let state = runner.start(Vec::new()).unwrap();
    let snapshot = match state {
        VmState::Suspended {
            function_name,
            args,
            snapshot,
        } => {
            assert_eq!(function_name, "callTool");
            assert_eq!(args.first(), Some(&Value::String("first".into())));
            snapshot
        }
        other => panic!("expected suspension on callTool, got {other:?}"),
    };

    // Serialize → deserialize: the remaining microtask queue must survive.
    let bytes = snapshot.dump().unwrap();
    let restored = ZapcodeSnapshot::load(&bytes).unwrap();
    let final_state = restored
        .resume(Value::String("FIRST".into()))
        .unwrap()
        .state;
    match final_state {
        VmState::Complete(Value::String(s)) => assert_eq!(s.to_string(), "FIRST!|second"),
        other => panic!("expected completion, got {other:?}"),
    }
}

#[test]
fn tool_error_inside_then_rejects_the_chain_for_catch() {
    // The forced tool call fails: the chain rejects, and a .catch in the
    // chain receives it after resume_with_error.
    let runner = ZapcodeRun::new(
        "const r = await Promise.resolve('go') \
             .then(() => callTool('x')) \
             .catch(e => 'recovered'); \
         r"
        .to_string(),
        Vec::new(),
        vec!["callTool".to_string()],
        ResourceLimits::default(),
    )
    .unwrap();

    let state = runner.start(Vec::new()).unwrap();
    let snapshot = match state {
        VmState::Suspended { snapshot, .. } => snapshot,
        other => panic!("expected suspension, got {other:?}"),
    };
    let final_state = snapshot
        .resume_with_error(Value::String("tool failed".into()))
        .unwrap()
        .state;
    match final_state {
        VmState::Complete(Value::String(s)) => assert_eq!(s.to_string(), "recovered"),
        other => panic!("expected completion, got {other:?}"),
    }
}
