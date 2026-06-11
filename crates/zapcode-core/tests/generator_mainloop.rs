//! Generator-mainloop Stage 0: `gen.next()` pulls run in the MAIN loop
//! (`Continuation::GeneratorNext`, yield-as-detach), with try-frames
//! migrating across yields.
//!
//! New capability: a tool call inside a generator body pulled via `.next()`
//! suspends the whole VM durably (previously: "cannot suspend inside a
//! generator"). `for…of`/spread still use the legacy nested driver until
//! Stage 1.

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

// ════════════════════════════════════════════════════════════════════════════
//  Behavior parity through the main loop
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn manual_next_protocol_matches_node() {
    assert_eq!(
        run_str(
            "function* g(a) { const b = yield a + 1; yield b * 2; return 'end'; } \
             const it = g(10); \
             const r1 = it.next(); \
             const r2 = it.next(5); \
             const r3 = it.next(); \
             const r4 = it.next(); \
             `${r1.value},${r1.done}|${r2.value},${r2.done}|${r3.value},${r3.done}|${r4.value},${r4.done}`"
        ),
        // Node: 11,false|10,false|end,true|undefined,true
        "11,false|10,false|end,true|undefined,true"
    );
}

#[test]
fn try_catch_survives_a_yield() {
    // The try-frame migrates with the suspended body: a throw AFTER resuming
    // lands in the generator's own catch. (Previously the entry pointed at a
    // popped frame.)
    assert_eq!(
        run_str(
            "function* g() { \
                 try { \
                     yield 1; \
                     null.x; \
                 } catch (e) { \
                     yield 'caught'; \
                 } \
             } \
             const it = g(); \
             const a = it.next().value; \
             const b = it.next().value; \
             `${a},${b}`"
        ),
        "1,caught"
    );
}

#[test]
fn suspended_generators_try_does_not_leak_into_the_caller() {
    // While the generator is parked at a yield inside its `try`, a throw in
    // the CALLER must route to the caller's own catch — not be swallowed by
    // the generator's stashed try-frame.
    assert_eq!(
        run_str(
            "function* g() { try { yield 1; } catch (e) { return 'gen-caught'; } } \
             const it = g(); \
             it.next(); \
             let out = 'none'; \
             try { throw 'caller-error'; } catch (e) { out = 'caller-caught:' + e; } \
             out"
        ),
        "caller-caught:caller-error"
    );
}

#[test]
fn throw_escaping_the_body_marks_the_generator_done() {
    // Node: the exception propagates to the .next() caller and the generator
    // is finished (subsequent pulls answer done).
    assert_eq!(
        run_str(
            "function* g() { yield 1; null.x; } \
             const it = g(); \
             it.next(); \
             let caught = 'no'; \
             try { it.next(); } catch (e) { caught = 'yes'; } \
             const after = it.next(); \
             `${caught}|${after.done}`"
        ),
        "yes|true"
    );
}

#[test]
fn reentrant_next_is_a_type_error() {
    // Node: TypeError "Generator is already running".
    assert_eq!(
        run_str(
            "let it; \
             function* g() { yield it.next(); } \
             it = g(); \
             let out = 'no'; \
             try { it.next(); } catch (e) { out = 'caught:' + (e instanceof TypeError); } \
             out"
        ),
        "caught:true"
    );
}

#[test]
fn finally_runs_when_body_completes_across_yields() {
    assert_eq!(
        run_str(
            "const log = []; \
             function* g() { try { yield 1; yield 2; } finally { log.push('fin'); } } \
             const it = g(); \
             it.next(); it.next(); it.next(); \
             log.join(',')"
        ),
        "fin"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  New capability: tool calls inside generator bodies (via .next())
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn tool_call_inside_generator_body_suspends_durably() {
    // The generator body makes a host call mid-pull: the VM suspends with
    // the body's live frame in the snapshot, round-trips through bytes, and
    // the pull answers after resume.
    let runner = ZapcodeRun::new(
        "async function* steps() { \
             const a = yield 'start'; \
             const enriched = await callTool(a); \
             yield 'got:' + enriched; \
         } \
         const it = steps(); \
         const first = it.next().value; \
         const second = it.next('arg').value; \
         `${first}|${second}`"
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
            assert_eq!(args.first(), Some(&Value::String("arg".into())));
            snapshot
        }
        other => panic!("expected suspension inside the generator body, got {other:?}"),
    };
    let bytes = snapshot.dump().unwrap();
    let restored = ZapcodeSnapshot::load(&bytes).unwrap();
    match restored.resume(Value::String("X".into())).unwrap().state {
        VmState::Complete(Value::String(s)) => assert_eq!(s.to_string(), "start|got:X"),
        other => panic!("expected completion, got {other:?}"),
    }
}

#[test]
fn await_of_then_chain_inside_pulled_body() {
    // A pending chain awaited inside a generator body pulled by .next():
    // the main loop drains it (and the local survives the next yield).
    assert_eq!(
        run_str(
            "async function* g() { \
                 const v = await Promise.resolve(1).then(x => x + 1); \
                 yield v; \
                 yield v + 10; \
             } \
             const it = g(); \
             const a = it.next().value; \
             const b = it.next().value; \
             `${a},${b}`"
        ),
        "2,12"
    );
}
