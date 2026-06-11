//! Generator-mainloop Stage 4: durable hardening for generator state.
//!
//! Suspended generators (detached body frames + stashed try-frames in
//! `generator_try_frames`) and LIVE generator frames mid-pull must be
//! transparent to durable execution: replay with a dump→load at every
//! suspension is trace- and result-identical, and snapshot bytes are
//! deterministic with generator state in flight.

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, Value, ZapcodeRun, ZapcodeSnapshot};

/// Drive `code`, replying `r<n>` to every suspension; with `hop`, dump→load
/// at each one. Returns (suspension trace, final value or error).
fn drive(code: &str, hop: bool) -> (Vec<String>, Result<String, String>) {
    let runner = ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        vec!["callTool".to_string()],
        ResourceLimits::default(),
    )
    .unwrap();
    let mut trace = Vec::new();
    let mut state = match runner.start(Vec::new()) {
        Ok(s) => s,
        Err(e) => return (trace, Err(e.to_string())),
    };
    let mut n = 0;
    loop {
        assert!(n < 64, "runaway suspension loop");
        match state {
            VmState::Suspended {
                function_name,
                args,
                snapshot,
            } => {
                trace.push(format!(
                    "{function_name}({})",
                    args.iter()
                        .map(|a| format!("{a:?}"))
                        .collect::<Vec<_>>()
                        .join(",")
                ));
                let snapshot = if hop {
                    ZapcodeSnapshot::load(&snapshot.dump().unwrap()).unwrap()
                } else {
                    snapshot
                };
                n += 1;
                match snapshot.resume(Value::String(format!("r{}", n - 1).into())) {
                    Ok(run) => state = run.state,
                    Err(e) => return (trace, Err(e.to_string())),
                }
            }
            VmState::Complete(v) => return (trace, Ok(format!("{v:?}"))),
            other => panic!("unexpected state {other:?}"),
        }
    }
}

fn assert_replay_identical(code: &str) {
    let (t_mem, r_mem) = drive(code, false);
    let (t_hop, r_hop) = drive(code, true);
    assert_eq!(t_mem, t_hop, "suspension traces diverged for:\n{code}");
    assert_eq!(r_mem, r_hop, "results diverged for:\n{code}");
}

// ════════════════════════════════════════════════════════════════════════════
//  Replay determinism with generator state in flight
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn replay_tool_calls_across_pulls_and_suspended_generators() {
    // Suspension happens INSIDE a pull while ANOTHER generator sits
    // suspended (detached frame + stashed try-frames in the snapshot).
    assert_replay_identical(
        "function* idle() { try { yield 'parked'; } finally { } } \
         const parked = idle(); \
         parked.next(); \
         async function* worker() { \
             for (const id of ['a', 'b']) { \
                 const v = await callTool(id); \
                 yield id + ':' + v; \
             } \
         } \
         async function main() { \
             const out = []; \
             for await (const x of worker()) out.push(x); \
             return out.join(','); \
         } \
         main();",
    );
}

#[test]
fn replay_try_catch_in_generator_with_tool_error_path() {
    // The body's try-frame survives the hop; a failing tool resumed with a
    // VALUE here keeps the success path; the trace must be identical.
    assert_replay_identical(
        "async function* g() { \
             try { \
                 const v = await callTool('one'); \
                 yield 'ok:' + v; \
             } catch (e) { \
                 yield 'err:' + e; \
             } \
         } \
         async function main() { \
             const out = []; \
             for await (const x of g()) out.push(x); \
             return out.join(','); \
         } \
         main();",
    );
}

#[test]
fn replay_yield_star_with_tools_in_both_layers() {
    assert_replay_identical(
        "async function* inner() { yield await callTool('inner'); } \
         async function* outer() { \
             yield await callTool('outer-pre'); \
             yield* inner(); \
         } \
         async function main() { \
             const out = []; \
             for await (const v of outer()) out.push(v); \
             return out.join('|'); \
         } \
         main();",
    );
}

#[test]
fn replay_manual_next_with_promise_answers() {
    assert_replay_identical(
        "async function* g() { \
             const a = yield 'first'; \
             yield 'second:' + (await callTool(a)); \
         } \
         async function main() { \
             const it = g(); \
             const r1 = await it.next(); \
             const r2 = await it.next('sent'); \
             return r1.value + '/' + r2.value; \
         } \
         main();",
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  Byte determinism with generator state in flight
// ════════════════════════════════════════════════════════════════════════════

fn suspended_with_generator_state() -> ZapcodeSnapshot {
    // At the suspension: one generator suspended at a yield inside try
    // (stashed try-frames), one generator's frame LIVE mid-pull.
    let runner = ZapcodeRun::new(
        "function* parked() { try { yield 1; } catch (e) { yield 2; } } \
         const p = parked(); \
         p.next(); \
         async function* live() { yield await callTool('t'); } \
         async function main() { \
             const out = []; \
             for await (const v of live()) out.push(v); \
             return out.join(','); \
         } \
         main();"
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
fn snapshot_bytes_deterministic_with_generator_state() {
    let snap = suspended_with_generator_state();
    let a = snap.dump().unwrap();
    let b = snap.dump().unwrap();
    assert_eq!(a, b);
}

#[test]
fn snapshot_dump_load_dump_idempotent_with_generator_state() {
    let snap = suspended_with_generator_state();
    let first = snap.dump().unwrap();
    let second = ZapcodeSnapshot::load(&first).unwrap().dump().unwrap();
    assert_eq!(first, second);
}
