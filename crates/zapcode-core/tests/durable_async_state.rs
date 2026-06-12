//! Durable hardening for the microtask/async machinery (microtask-design
//! Stage 4).
//!
//! Everything Stages 1–3 added to the VM's async state — the microtask
//! queue, reaction records on pending promises, parked `AsyncTask`s,
//! unhandled-rejection marks — must be transparent to durable execution:
//!
//! * **Replay determinism**: a program driven with a dump→load round-trip at
//!   EVERY suspension (the Temporal activity-boundary pattern) must produce
//!   the same suspension sequence and the same final value as the same
//!   program driven in-memory.
//! * **Byte determinism**: capturing the same logical state twice — with
//!   async state in flight — must produce identical snapshot bytes
//!   (content-addressing / dedup), and dump→load→dump must be idempotent.
//! * **Runaway drains** (risk R4) must hit resource limits, never hang.

use zapcode_core::vm::VmState;
use zapcode_core::{
    ResourceLimits, Value, ZapcodeRun, ZapcodeSessionSnapshot, ZapcodeSessionState, ZapcodeSnapshot,
};

// ════════════════════════════════════════════════════════════════════════════
//  Harness
// ════════════════════════════════════════════════════════════════════════════

/// What the driver should do at suspension index `n` (0-based).
#[derive(Clone, Copy, PartialEq)]
enum Reply {
    /// Resolve with the deterministic scripted value `r<n>`.
    Value,
    /// Fail the call via `resume_with_error("e<n>")`.
    Error,
}

/// Drive `code` to completion, replying to every suspension with a scripted
/// deterministic value (`r0`, `r1`, …) — or an error where `script` says so.
/// With `hop`, every suspension serializes the snapshot to bytes and loads it
/// back before resuming. Returns the suspension trace (`name(arg,…)` /
/// `many:kind[name,…]`) and the final value or error, stringified.
fn drive(
    code: &str,
    externals: &[&str],
    hop: bool,
    script: &dyn Fn(usize) -> Reply,
) -> (Vec<String>, std::result::Result<String, String>) {
    let runner = ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        externals.iter().map(|s| s.to_string()).collect(),
        ResourceLimits::default(),
    )
    .unwrap();

    let mut trace = Vec::new();
    let mut state = match runner.start(Vec::new()) {
        Ok(s) => s,
        Err(e) => return (trace, Err(e.to_string())),
    };
    let mut n = 0usize;
    loop {
        assert!(n < 64, "runaway suspension loop (>{n} suspensions)");
        match state {
            VmState::Suspended {
                function_name,
                args,
                snapshot,
            } => {
                let arg_strs: Vec<String> = args.iter().map(|a| format!("{a:?}")).collect();
                trace.push(format!("{function_name}({})", arg_strs.join(",")));
                let snapshot = if hop {
                    ZapcodeSnapshot::load(&snapshot.dump().unwrap()).unwrap()
                } else {
                    snapshot
                };
                let outcome = match script(n) {
                    Reply::Value => snapshot.resume(Value::String(format!("r{n}").into())),
                    Reply::Error => {
                        snapshot.resume_with_error(Value::String(format!("e{n}").into()))
                    }
                };
                n += 1;
                match outcome {
                    Ok(run) => state = run.state,
                    Err(e) => return (trace, Err(e.to_string())),
                }
            }
            VmState::SuspendedMany {
                calls,
                combinator,
                snapshot,
            } => {
                let names: Vec<String> = calls.iter().map(|c| c.name.clone()).collect();
                trace.push(format!("many:{combinator:?}[{}]", names.join(",")));
                let snapshot = if hop {
                    ZapcodeSnapshot::load(&snapshot.dump().unwrap()).unwrap()
                } else {
                    snapshot
                };
                let replies: Vec<Value> = (0..calls.len())
                    .map(|i| Value::String(format!("m{n}_{i}").into()))
                    .collect();
                n += 1;
                match snapshot.resume_many(replies) {
                    Ok(run) => state = run.state,
                    Err(e) => return (trace, Err(e.to_string())),
                }
            }
            VmState::Complete(v) => {
                // Stringify through a fresh run's heap? No — Complete values
                // referencing the heap can't outlive it here; use Debug for
                // scalars and a marker for handles (the tests below always
                // complete with strings/numbers).
                return (trace, Ok(format!("{v:?}")));
            }
        }
    }
}

/// Assert that an in-memory drive and a dump/load-at-every-suspension drive
/// of `code` agree on the suspension trace and the final outcome.
fn assert_replay_identical(code: &str, externals: &[&str]) {
    let all_values = |_: usize| Reply::Value;
    let (trace_mem, result_mem) = drive(code, externals, false, &all_values);
    let (trace_hop, result_hop) = drive(code, externals, true, &all_values);
    assert_eq!(
        trace_mem, trace_hop,
        "suspension traces diverged for:\n{code}"
    );
    assert_eq!(result_mem, result_hop, "results diverged for:\n{code}");
}

// ════════════════════════════════════════════════════════════════════════════
//  Replay determinism with async state in flight
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn replay_fanout_of_async_calls_with_sequential_tools() {
    // Parked tasks + ResumeAsync microtasks + a live task frame at every
    // suspension; the stage-3 session-workflow shape.
    assert_replay_identical(
        "async function load(id) { \
             const t = await fetchT(id); \
             const a = await enrich(t); \
             return id + ':' + a; \
         } \
         const r = await Promise.all([load('1'), load('2'), load('3')]); \
         r.join(',')",
        &["fetchT", "enrich"],
    );
}

#[test]
fn replay_tool_inside_then_with_queued_reactions() {
    // Suspension happens MID-DRAIN with another reaction queued behind it.
    assert_replay_identical(
        "const log = []; \
         const p = Promise.resolve('go').then(v => callTool(v)); \
         Promise.resolve(1).then(() => log.push('q')); \
         const r = await p.then(x => x + '!'); \
         r + '|' + log.join('')",
        &["callTool"],
    );
}

#[test]
fn replay_parked_task_awaiting_another_task() {
    // At the suspension, task `a` exists ONLY as a serialized AsyncTask
    // parked on `b`'s result promise.
    assert_replay_identical(
        "async function b() { return await callTool('b'); } \
         async function a(pb) { const v = await pb; return 'a:' + v; } \
         const pb = b(); \
         const pa = a(pb); \
         (await pa) + '/' + (await pb)",
        &["callTool"],
    );
}

#[test]
fn replay_try_catch_in_parked_body_with_tool_error() {
    // try-frames migrate into the AsyncTask; the scripted error resumes into
    // the body's own catch after a hop.
    let script = |n: usize| if n == 1 { Reply::Error } else { Reply::Value };
    let code = "async function f() { \
                    const first = await callTool('one'); \
                    try { \
                        const second = await callTool('two'); \
                        return first + '+' + second; \
                    } catch (e) { \
                        return first + '/recovered:' + e; \
                    } \
                } \
                await f()";
    let (trace_mem, result_mem) = drive(code, &["callTool"], false, &script);
    let (trace_hop, result_hop) = drive(code, &["callTool"], true, &script);
    assert_eq!(trace_mem, trace_hop);
    assert_eq!(result_mem, result_hop);
    assert_eq!(
        result_mem.unwrap(),
        "String(Valid(\"r0/recovered:e1\"))",
        "tool error must land in the parked body's catch"
    );
}

#[test]
fn replay_batch_suspension_inside_then_handler() {
    // A Promise.all of bare tool calls inside a .then handler: a
    // SuspendedMany raised mid-drain.
    assert_replay_identical(
        "const r = await Promise.resolve('x').then(async () => { \
             const pair = await Promise.all([toolA('1'), toolB('2')]); \
             return pair.join('&'); \
         }); \
         r",
        &["toolA", "toolB"],
    );
}

#[test]
fn replay_finally_forcing_tool_after_park() {
    // `.finally(() => tool())` forces the cleanup call; the original value
    // passes through after the forced call resumes.
    assert_replay_identical(
        "async function f() { await null; return 'kept'; } \
         await f().finally(() => cleanup('c'))",
        &["cleanup"],
    );
}

#[test]
fn replay_for_await_over_async_calls() {
    assert_replay_identical(
        "async function step(i) { const v = await callTool(i); return v; } \
         async function main() { \
             const out = []; \
             for await (const v of [step('a'), step('b')]) { out.push(v); } \
             return out.join(','); \
         } \
         main();",
        &["callTool"],
    );
}

#[test]
fn replay_unhandled_mark_survives_a_hop() {
    // An orphaned rejection settles (and is marked) BEFORE a tool
    // suspension; the mark must survive the hop so the run still fails
    // deterministically at end-of-drain — in both modes.
    let code = "const orphan = Promise.resolve(1).then(() => { throw 'k' }); \
                const r = await Promise.resolve('x').then(() => callTool('t')); \
                r";
    let all_values = |_: usize| Reply::Value;
    let (trace_mem, result_mem) = drive(code, &["callTool"], false, &all_values);
    let (trace_hop, result_hop) = drive(code, &["callTool"], true, &all_values);
    assert_eq!(trace_mem, trace_hop);
    assert_eq!(result_mem, result_hop);
    let err = result_mem.expect_err("orphaned rejection must fail the run");
    assert!(
        err.contains("Unhandled promise rejection") && err.contains('k'),
        "unexpected error: {err}"
    );
}

#[test]
fn replay_rejection_cascade_after_resume_error() {
    // The failed tool rejects the chain; a .catch added downstream recovers.
    let script = |n: usize| if n == 0 { Reply::Error } else { Reply::Value };
    let code = "const r = await Promise.resolve('x') \
                    .then(() => callTool('t')) \
                    .then(v => 'ok:' + v) \
                    .catch(e => 'rec:' + e); \
                r";
    let (trace_mem, result_mem) = drive(code, &["callTool"], false, &script);
    let (trace_hop, result_hop) = drive(code, &["callTool"], true, &script);
    assert_eq!(trace_mem, trace_hop);
    assert_eq!(result_mem, result_hop);
    assert_eq!(result_mem.unwrap(), "String(Valid(\"rec:e0\"))");
}

// ════════════════════════════════════════════════════════════════════════════
//  Byte determinism with async state in flight
// ════════════════════════════════════════════════════════════════════════════

/// Suspend a program that has a parked AsyncTask, queued microtasks, AND an
/// unhandled-rejection mark in flight, and return the suspension snapshot.
fn suspended_with_async_state() -> ZapcodeSnapshot {
    let runner = ZapcodeRun::new(
        "async function parked() { await null; return await callTool('late'); } \
         const p = parked(); \
         Promise.resolve(1).then(() => { throw 'orphan' }); \
         const r = await Promise.resolve('x').then(() => callTool('first')); \
         (await p) + r"
            .to_string(),
        Vec::new(),
        vec!["callTool".to_string()],
        ResourceLimits::default(),
    )
    .unwrap();
    match runner.start(Vec::new()).unwrap() {
        VmState::Suspended { snapshot, .. } => snapshot,
        other => panic!("expected suspension, got {other:?}"),
    }
}

#[test]
fn snapshot_bytes_deterministic_with_async_state() {
    let snap = suspended_with_async_state();
    let a = snap.dump().unwrap();
    let b = snap.dump().unwrap();
    assert_eq!(a, b, "two dumps of the same state must be byte-identical");
}

#[test]
fn snapshot_dump_load_dump_is_idempotent_with_async_state() {
    // Regression: `CallFrame.boxed`/`env` were HashMaps, whose per-instance
    // randomized iteration order made a decoded frame re-serialize to
    // different bytes (breaking content-addressing). They are BTreeMaps now.
    let snap = suspended_with_async_state();
    let first = snap.dump().unwrap();
    let second = ZapcodeSnapshot::load(&first).unwrap().dump().unwrap();
    assert_eq!(first, second, "dump → load → dump must be byte-identical");
}

#[test]
fn snapshot_dump_load_dump_is_idempotent_with_captured_closure_frame() {
    // The latent pre-async case: a suspension inside a closure whose frame
    // carries several captured-cell bindings (`env`) and promoted locals
    // (`boxed`) — enough entries that an order-randomizing map would shuffle.
    let runner = ZapcodeRun::new(
        "function makeWorker(a, b, c) { \
             let total = 0; \
             return async () => { \
                 total += a + b + c; \
                 const r = await callTool(String(total)); \
                 return r + ':' + total + a + b + c; \
             }; \
         } \
         const w = makeWorker(1, 2, 3); \
         await w()"
            .to_string(),
        Vec::new(),
        vec!["callTool".to_string()],
        ResourceLimits::default(),
    )
    .unwrap();
    let snap = match runner.start(Vec::new()).unwrap() {
        VmState::Suspended { snapshot, .. } => snapshot,
        other => panic!("expected suspension, got {other:?}"),
    };
    let first = snap.dump().unwrap();
    let second = ZapcodeSnapshot::load(&first).unwrap().dump().unwrap();
    assert_eq!(first, second, "dump → load → dump must be byte-identical");
}

// ════════════════════════════════════════════════════════════════════════════
//  Sessions: async state across chunk boundaries
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn session_chunks_hop_with_async_state_in_flight() {
    // Chunk 1 defines async helpers; chunk 2 fans out with tool calls. The
    // session dump/loads BETWEEN chunks and at EVERY suspension.
    let session =
        ZapcodeSessionSnapshot::new(vec!["fetchT".to_string()], ResourceLimits::default()).unwrap();

    let state = session
        .run_chunk(
            "async function double(id) { const v = await fetchT(id); return v + v; } 'ready'"
                .to_string(),
            Vec::new(),
        )
        .unwrap();
    let session = match state {
        ZapcodeSessionState::Complete {
            mut session,
            output,
            ..
        } => {
            assert_eq!(output.to_js_string(session.heap()), "ready");
            ZapcodeSessionSnapshot::load(&session.dump().unwrap()).unwrap()
        }
        other => panic!("expected chunk-1 completion, got {other:?}"),
    };

    let mut state = session
        .run_chunk(
            "const pair = await Promise.all([double('a'), double('b')]); pair.join('|')"
                .to_string(),
            Vec::new(),
        )
        .unwrap();
    let mut n = 0;
    loop {
        match state {
            ZapcodeSessionState::Suspended {
                function_name,
                session,
                ..
            } => {
                assert_eq!(function_name, "fetchT");
                let hopped = ZapcodeSessionSnapshot::load(&session.dump().unwrap()).unwrap();
                state = hopped
                    .resume(Value::String(format!("v{n}").into()))
                    .unwrap();
                n += 1;
            }
            ZapcodeSessionState::Complete {
                mut session,
                output,
                ..
            } => {
                assert_eq!(output.to_js_string(session.heap()), "v0v0|v1v1");
                break;
            }
            other => panic!("unexpected state {other:?}"),
        }
    }
    assert_eq!(n, 2, "expected one suspension per fan-out element");
}

// ════════════════════════════════════════════════════════════════════════════
//  Runaway drains hit resource limits (risk R4)
// ════════════════════════════════════════════════════════════════════════════

fn tight_limits() -> ResourceLimits {
    ResourceLimits {
        max_allocations: 50_000,
        time_limit_ms: 2_000,
        ..ResourceLimits::default()
    }
}

#[test]
fn infinite_then_loop_hits_a_limit() {
    // Each iteration enqueues a fresh reaction from inside the drain.
    let err = ZapcodeRun::new(
        "function loop() { Promise.resolve().then(loop); } loop(); 'unreachable-drain'".to_string(),
        Vec::new(),
        Vec::new(),
        tight_limits(),
    )
    .unwrap()
    .run(Vec::new())
    .expect_err("a runaway then-loop must hit a resource limit");
    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("alloc") || msg.contains("time") || msg.contains("limit"),
        "expected a resource-limit error, got: {msg}"
    );
}

#[test]
fn infinite_async_recursion_hits_a_limit() {
    // Each cycle parks a task, resumes it, and parks a new one.
    let err = ZapcodeRun::new(
        "async function loop() { await null; return loop(); } \
         await loop()"
            .to_string(),
        Vec::new(),
        Vec::new(),
        tight_limits(),
    )
    .unwrap()
    .run(Vec::new())
    .expect_err("runaway async recursion must hit a resource limit");
    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("alloc") || msg.contains("time") || msg.contains("limit"),
        "expected a resource-limit error, got: {msg}"
    );
}

#[test]
fn mutual_async_microtask_ping_pong_hits_a_limit() {
    let err = ZapcodeRun::new(
        "async function ping() { await null; return pong(); } \
         async function pong() { await null; return ping(); } \
         await ping()"
            .to_string(),
        Vec::new(),
        Vec::new(),
        tight_limits(),
    )
    .unwrap()
    .run(Vec::new())
    .expect_err("async ping-pong must hit a resource limit");
    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("alloc") || msg.contains("time") || msg.contains("limit"),
        "expected a resource-limit error, got: {msg}"
    );
}
