//! Conformance: the await-tick residuals left after microtask Stage 3.
//!
//! 1. **Top-level `await` yields a tick.** The top-level frame cannot detach
//!    into an AsyncTask, so an `await` of a settled/non-promise operand with
//!    reactions queued delivers its outcome through a sentinel pending
//!    promise whose settling microtask sits at the END of the current queue
//!    — Node's module-TLA "the resumption is enqueued after the current
//!    queue" order, including for rejections. (With an empty queue the value
//!    is delivered inline: nothing exists to interleave with, so no tick is
//!    observable.)
//! 2. **A cached host-call re-await yields a tick** (`await p` where `p` is
//!    a deferred host-call promise that already settled).
//! 3. **`await` inside async generator bodies** drains pending `.then`
//!    chains correctly (the generator drive loop now fires
//!    `process_continuation` for handler frames), and generator locals
//!    promoted to upvalue cells survive yields (`SuspendedFrame.boxed`,
//!    wire v8) — previously a boxed local written between yields silently
//!    reverted on resume, in *sync* generators too.
//!
//! Ordering assertions are ground-truthed against real Node (module TLA).
//!
//! Remaining pinned divergence: awaits inside async generator bodies run
//! inline (no tick) — full task semantics for generators would mean running
//! generator frames in the main loop, out of scope here.

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, Value, ZapcodeRun};

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
//  Top-level await yields a tick
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn top_level_await_interleaves_with_queued_reactions() {
    // Node module TLA: m1,after,m2,after2 — each await resumes after the
    // jobs that were queued at that moment, and jobs queued DURING those run
    // after the continuation (until the next await).
    assert_eq!(
        run_str(
            "const log = []; \
             Promise.resolve().then(() => log.push('m1')).then(() => log.push('m2')); \
             await 1; \
             log.push('after'); \
             await Promise.resolve(); \
             log.push('after2'); \
             log.join(',')"
        ),
        "m1,after,m2,after2"
    );
}

#[test]
fn top_level_await_rejection_yields_before_rethrow() {
    // Node: m,c:r,done — the queued reaction runs before the rejection
    // rethrows into the catch, and the ORIGINAL reason arrives.
    assert_eq!(
        run_str(
            "const log = []; \
             Promise.resolve().then(() => log.push('m')); \
             try { await Promise.reject('r'); } catch (e) { log.push('c:' + e); } \
             log.push('done'); \
             log.join(',')"
        ),
        "m,c:r,done"
    );
}

#[test]
fn top_level_await_of_async_call_keeps_value_semantics() {
    // The tick must not change delivered values.
    assert_eq!(
        run_str(
            "async function f() { return 5 } \
             Promise.resolve().then(() => {}); \
             const a = await f(); \
             const b = await 'x'; \
             `${a}${b}`"
        ),
        "5x"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  Cached host-call re-await yields a tick
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn cached_tool_promise_reawait_yields_at_top_level() {
    // Node: m,b:T — the second await of the (settled) host promise lets the
    // queued reaction run first. One host suspension only.
    let runner = ZapcodeRun::new(
        "const log = []; \
         const p = callTool('t'); \
         const a = await p; \
         Promise.resolve().then(() => log.push('m')); \
         const b = await p; \
         log.push('b:' + b); \
         log.join(',')"
            .to_string(),
        Vec::new(),
        vec!["callTool".to_string()],
        ResourceLimits::default(),
    )
    .unwrap();
    let state = runner.start(Vec::new()).unwrap();
    let snapshot = match state {
        VmState::Suspended { snapshot, .. } => snapshot,
        other => panic!("expected one suspension, got {other:?}"),
    };
    match snapshot.resume(Value::String("T".into())).unwrap().state {
        VmState::Complete(Value::String(s)) => assert_eq!(s.to_string(), "m,b:T"),
        other => panic!("expected completion, got {other:?}"),
    }
}

#[test]
fn cached_tool_promise_reawait_parks_in_async_body() {
    // Inside an async body the cached re-await parks the task (a full tick),
    // so a sibling body's queued resumption interleaves.
    let runner = ZapcodeRun::new(
        "const log = []; \
         async function user(p, tag) { \
             const v = await p; \
             log.push(tag + ':' + v); \
         } \
         const p = callTool('t'); \
         const first = await p; \
         const u1 = user(p, 'u1'); \
         const u2 = user(p, 'u2'); \
         await Promise.all([u1, u2]); \
         log.join(',')"
            .to_string(),
        Vec::new(),
        vec!["callTool".to_string()],
        ResourceLimits::default(),
    )
    .unwrap();
    let state = runner.start(Vec::new()).unwrap();
    let snapshot = match state {
        VmState::Suspended { snapshot, .. } => snapshot,
        other => panic!("expected one suspension, got {other:?}"),
    };
    match snapshot.resume(Value::String("T".into())).unwrap().state {
        VmState::Complete(Value::String(s)) => assert_eq!(s.to_string(), "u1:T,u2:T"),
        other => panic!("expected completion, got {other:?}"),
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  Async (and sync) generators: chain awaits + boxed locals across yields
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn async_generator_awaits_a_then_chain() {
    // Node: 2,12 — the chain settles inside the generator body and the local
    // holding it survives the yield. (Was 2,NaN: the drive loop never fired
    // process_continuation, leaking the handler's return onto the stack.)
    assert_eq!(
        run_str(
            "async function* gen() { \
                 const v = await Promise.resolve(1).then(x => x + 1); \
                 yield v; \
                 yield v + 10; \
             } \
             async function main() { \
                 const out = []; \
                 for await (const x of gen()) { out.push(x); } \
                 return out.join(','); \
             } \
             main();"
        ),
        "2,12"
    );
}

#[test]
fn generator_boxed_local_survives_yields() {
    // Regression (pre-existing, sync generators too): a generator local
    // promoted to an upvalue cell silently reverted to its pre-callback
    // value after a yield, because SuspendedFrame did not carry the
    // promoted-cell map.
    assert_eq!(
        run_str(
            "function* gen() { \
                 const w = [1].map(x => x + 1)[0]; \
                 yield w; \
                 yield w + 10; \
             } \
             async function main() { \
                 const out = []; \
                 for (const x of gen()) { out.push(x); } \
                 return out.join(','); \
             } \
             main();"
        ),
        "2,12"
    );
    assert_eq!(
        run_str(
            "async function* gen() { \
                 let w = 0; \
                 w = await Promise.resolve(1).then(x => x + 1); \
                 yield w; \
                 yield w; \
             } \
             async function main() { \
                 const out = []; \
                 for await (const x of gen()) { out.push(x); } \
                 return out.join(','); \
             } \
             main();"
        ),
        "2,2"
    );
}
