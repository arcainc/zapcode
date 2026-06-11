//! Conformance: `.then`/`.catch`/`.finally` on a combinator over INTERNAL
//! promises (microtask-pending chains — no host calls to force).
//!
//! Such a batch lowers to a REAL pending promise (`lower_internal_batch`):
//! each pending element gets a "combine" reaction, already-settled elements
//! fold in immediately, and per-kind progress lives on the promise object
//! itself — so it snapshots like any other heap data. The method then runs
//! through the ordinary pending-promise path and returns a true dependent
//! promise.
//!
//! Before the lowering, `.then` on such a batch either borrowed a tick from
//! the microtask queue (worked only while the queue was non-empty) or fell
//! through to a legacy pass-through that returned the batch itself and
//! DROPPED the handler — `Promise.all([pending]).then(cb)` with an empty
//! queue lost `cb` entirely.
//!
//! Every expected value here is real-Node ground truth.

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, Value, ZapcodeRun, ZapcodeSnapshot};

/// Run a host-call-free program; `main()`'s value is the completion value.
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

/// The gate pattern: an internal promise resolved AFTER `.then` registration,
/// with nothing else in the microtask queue at registration time.
fn gated(body: &str) -> String {
    run_str(&format!(
        "async function main() {{ {body} }} main();"
    ))
}

// ════════════════════════════════════════════════════════════════════════════
//  The reported hole: empty microtask queue at `.then` time
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn all_then_with_empty_queue_keeps_its_handler() {
    // Node: 11. The legacy pass-through returned `[1]` (handler dropped).
    assert_eq!(
        gated(
            "let resolve; \
             const gate = new Promise(r => { resolve = r; }); \
             async function worker() { await gate; return 1; } \
             const chained = Promise.all([worker()]).then(v => v[0] + 10); \
             resolve(5); \
             return await chained;"
        ),
        "11"
    );
}

#[test]
fn all_then_with_busy_queue_still_works() {
    // The pre-lowering drain-borrowing path's territory must keep working.
    assert_eq!(
        gated(
            "const log = []; \
             const p = Promise.resolve(1).then(x => { log.push('a' + x); return x * 2; }); \
             const out = await Promise.all([p, Promise.resolve(9)]).then(v => v.join('-')); \
             return out + '|' + log.join(',');"
        ),
        "2-9|a1"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  Rejection routing
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn all_rejection_reaches_the_then_reject_handler() {
    assert_eq!(
        gated(
            "let reject; \
             const gate = new Promise((r, j) => { reject = j; }); \
             async function w() { await gate; } \
             const c = Promise.all([w(), Promise.resolve(1)]) \
                 .then(() => 'ok', e => 'caught:' + e); \
             reject('boom'); \
             return await c;"
        ),
        "caught:boom"
    );
}

#[test]
fn all_rejection_reaches_catch() {
    assert_eq!(
        gated(
            "let reject; \
             const gate = new Promise((r, j) => { reject = j; }); \
             async function w() { await gate; } \
             const c = Promise.all([w()]).catch(e => 'caught:' + e); \
             reject('bad'); \
             return await c;"
        ),
        "caught:bad"
    );
}

#[test]
fn finally_on_lowered_batch_observes_then_passes_through() {
    assert_eq!(
        gated(
            "let resolve; \
             const gate = new Promise(r => { resolve = r; }); \
             async function w() { await gate; return 7; } \
             const log = []; \
             const c = Promise.all([w()]) \
                 .finally(() => log.push('fin')) \
                 .then(v => v[0] + ':' + log.join(',')); \
             resolve(0); \
             return await c;"
        ),
        "7:fin"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  The other combinator kinds
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn race_first_settled_internal_element_wins() {
    assert_eq!(
        gated(
            "let r1, r2; \
             const g1 = new Promise(r => r1 = r), g2 = new Promise(r => r2 = r); \
             async function a() { return 'A:' + await g1; } \
             async function b() { return 'B:' + await g2; } \
             const c = Promise.race([a(), b()]).then(v => 'won:' + v); \
             r2('two'); r1('one'); \
             return await c;"
        ),
        "won:B:two"
    );
}

#[test]
fn any_all_rejected_builds_aggregate_error() {
    assert_eq!(
        gated(
            "let j1, j2; \
             const g1 = new Promise((r, j) => j1 = j), g2 = new Promise((r, j) => j2 = j); \
             async function a() { await g1; } \
             async function b() { await g2; } \
             const c = Promise.any([a(), b()]).catch(e => e.name + ':' + e.errors.join(',')); \
             j1('e1'); j2('e2'); \
             return await c;"
        ),
        "AggregateError:e1,e2"
    );
}

#[test]
fn any_first_fulfillment_wins_over_rejections() {
    assert_eq!(
        gated(
            "let r1, j2; \
             const g1 = new Promise(r => r1 = r), g2 = new Promise((r, j) => j2 = j); \
             async function a() { return 'A' + await g1; } \
             async function b() { await g2; } \
             const c = Promise.any([a(), b()]).then(v => 'got:' + v); \
             j2('nope'); r1('!'); \
             return await c;"
        ),
        "got:A!"
    );
}

#[test]
fn all_settled_reports_both_outcomes() {
    assert_eq!(
        gated(
            "let r1, j2; \
             const g1 = new Promise(r => r1 = r), g2 = new Promise((r, j) => j2 = j); \
             async function a() { return 'va' + await g1; } \
             async function b() { await g2; return 'never'; } \
             const c = Promise.allSettled([a(), b()]) \
                 .then(rs => rs.map(r => r.status + ':' + (r.value ?? r.reason)).join('|')); \
             r1('1'); j2('e'); \
             return await c;"
        ),
        "fulfilled:va1|rejected:e"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  Mixed shapes
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn settled_elements_fold_in_alongside_pending_chains() {
    assert_eq!(
        gated(
            "let resolve; \
             const gate = new Promise(r => { resolve = r; }); \
             async function w() { return 'w' + await gate; } \
             const c = Promise.all([Promise.resolve('x'), w(), 'plain']) \
                 .then(v => v.join('+')); \
             resolve('1'); \
             return await c;"
        ),
        "x+w1+plain"
    );
}

#[test]
fn then_and_direct_await_agree_on_the_same_batch() {
    // `.then` lowers the batch to a real pending promise; a later direct
    // `await` of it must read the same settled value.
    assert_eq!(
        gated(
            "let resolve; \
             const gate = new Promise(r => { resolve = r; }); \
             async function w() { await gate; return 3; } \
             const batch = Promise.all([w()]); \
             const c = batch.then(v => v[0] * 2); \
             resolve(0); \
             const direct = await batch; \
             return (await c) + ':' + direct.join(',');"
        ),
        "6:3"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  Durable: lowered-batch state (combine reactions + per-kind progress)
//  must survive a snapshot hop
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn lowered_batch_survives_a_snapshot_hop() {
    // The gate resolves with a TOOL result, so the VM suspends while the
    // lowered batch (combine reactions on the worker promises, progress
    // fields on the batch object) is live in the heap. Replay with a
    // dump→load at the suspension must agree with the in-memory run.
    let code = "let resolve; \
                const gate = new Promise(r => { resolve = r; }); \
                async function worker(tag) { return tag + (await gate); } \
                async function main() { \
                    const chained = Promise.all([worker('a'), worker('b')]) \
                        .then(v => v.join('|')); \
                    resolve(await callTool('gate')); \
                    return await chained; \
                } \
                main();";
    let drive = |hop: bool| -> String {
        let runner = ZapcodeRun::new(
            code.to_string(),
            Vec::new(),
            vec!["callTool".to_string()],
            ResourceLimits::default(),
        )
        .unwrap();
        let mut state = runner.start(Vec::new()).unwrap();
        loop {
            match state {
                VmState::Suspended { snapshot, .. } => {
                    let snapshot = if hop {
                        ZapcodeSnapshot::load(&snapshot.dump().unwrap()).unwrap()
                    } else {
                        snapshot
                    };
                    state = snapshot.resume(Value::String("X".into())).unwrap().state;
                }
                VmState::Complete(v) => return format!("{v:?}"),
                other => panic!("unexpected state {other:?}"),
            }
        }
    };
    let in_memory = drive(false);
    let hopped = drive(true);
    assert_eq!(in_memory, hopped);
    assert_eq!(in_memory, "String(Valid(\"aX|bX\"))");
}
