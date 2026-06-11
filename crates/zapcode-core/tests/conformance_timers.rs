//! Conformance: deterministic timers (`setTimeout`/`clearTimeout`/
//! `queueMicrotask`).
//!
//! The VM has no clock — a timer's delay is an ORDERING key (smaller fires
//! first, ties by creation order). Timers fire as macrotasks at the
//! top-level drain: after the microtask queue empties (and the per-tick
//! unhandled-rejection check passes), the earliest timer's callback runs as
//! a job. This preserves every relative ordering real JS guarantees
//! (microtasks before timers, 0ms before 5ms, creation order on ties)
//! without wall time — so replay stays deterministic.
//!
//! Timers serialize in snapshots (wire v13): a suspension with timers in
//! flight fires them on resume.

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, Value, ZapcodeRun, ZapcodeSnapshot};

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

fn run_main(body: &str) -> String {
    run_str(&format!("async function main() {{ {body} }} main();"))
}

#[test]
fn settimeout_sleep_pattern_resolves() {
    // THE canonical agent sleep — must complete, not hang on a pending promise.
    assert_eq!(
        run_main("await new Promise(resolve => setTimeout(resolve, 10)); return 'slept';"),
        "slept"
    );
}

#[test]
fn microtasks_run_before_timers() {
    assert_eq!(
        run_main(
            "const order = []; \
             setTimeout(() => order.push('timeout'), 0); \
             await Promise.resolve().then(() => order.push('micro')); \
             await new Promise(r => setTimeout(r, 1)); \
             return order.join(',');"
        ),
        "micro,timeout"
    );
}

#[test]
fn timers_fire_by_delay_then_creation_order() {
    assert_eq!(
        run_main(
            "const order = []; \
             setTimeout(() => order.push('b5'), 5); \
             setTimeout(() => order.push('a0'), 0); \
             setTimeout(() => order.push('c5'), 5); \
             setTimeout(() => order.push('d1'), 1); \
             await new Promise(r => setTimeout(r, 9)); \
             return order.join(',');"
        ),
        "a0,d1,b5,c5"
    );
}

#[test]
fn clear_timeout_cancels() {
    assert_eq!(
        run_main(
            "const order = []; \
             const id = setTimeout(() => order.push('cancelled'), 0); \
             setTimeout(() => order.push('kept'), 1); \
             clearTimeout(id); \
             await new Promise(r => setTimeout(r, 2)); \
             return order.join(',');"
        ),
        "kept"
    );
}

#[test]
fn queue_microtask_runs_before_timers() {
    assert_eq!(
        run_main(
            "const order = []; \
             setTimeout(() => order.push('t'), 0); \
             queueMicrotask(() => order.push('q')); \
             await new Promise(r => setTimeout(r, 1)); \
             return order.join(',');"
        ),
        "q,t"
    );
}

#[test]
fn timer_callback_chain_keeps_draining() {
    // A timer scheduling another timer: each macrotask drains its microtasks
    // before the next fires.
    assert_eq!(
        run_main(
            "const order = []; \
             let done; \
             const gate = new Promise(r => { done = r; }); \
             setTimeout(() => { \
                 order.push('first'); \
                 Promise.resolve().then(() => order.push('first-micro')); \
                 setTimeout(() => { order.push('second'); done(); }, 0); \
             }, 0); \
             await gate; \
             return order.join(',');"
        ),
        "first,first-micro,second"
    );
}

#[test]
fn typeof_settimeout_is_function() {
    assert_eq!(run_str("typeof setTimeout"), "function");
}

// ════════════════════════════════════════════════════════════════════════════
//  Durability: timers in flight survive a snapshot hop
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn timers_survive_a_snapshot_hop() {
    // The tool call suspends while a timer is pending; the hop must replay
    // identically to the in-memory run (timer fires after resume).
    let code = "async function main() { \
                    const order = []; \
                    let release; \
                    const gate = new Promise(r => { release = r; }); \
                    setTimeout(() => { order.push('timer'); release(); }, 5); \
                    order.push(await callTool('x')); \
                    await gate; \
                    return order.join(','); \
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
                    state = snapshot.resume(Value::String("tool".into())).unwrap().state;
                }
                VmState::Complete(v) => return format!("{v:?}"),
                other => panic!("unexpected {other:?}"),
            }
        }
    };
    let in_memory = drive(false);
    assert_eq!(in_memory, drive(true));
    assert_eq!(in_memory, "String(Valid(\"tool,timer\"))");
}

#[test]
fn snapshot_bytes_deterministic_with_timers_in_flight() {
    let make = || {
        let runner = ZapcodeRun::new(
            "async function main() { \
                 setTimeout(() => {}, 3); \
                 return await callTool('x'); \
             } \
             main();"
                .to_string(),
            Vec::new(),
            vec!["callTool".to_string()],
            ResourceLimits::default(),
        )
        .unwrap();
        match runner.start(Vec::new()).unwrap() {
            VmState::Suspended { snapshot, .. } => snapshot.dump().unwrap(),
            other => panic!("unexpected {other:?}"),
        }
    };
    let a = make();
    let b = make();
    assert_eq!(a, b);
    let reloaded = ZapcodeSnapshot::load(&a).unwrap().dump().unwrap();
    assert_eq!(a, reloaded);
}
