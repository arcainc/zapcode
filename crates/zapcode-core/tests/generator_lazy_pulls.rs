//! Generator-mainloop Stage 1: `for…of` / `for await` / `yield*` pulls run
//! in the MAIN loop (`GeneratorNext { for_of: true }`), so a tool call
//! inside a loop-driven generator body suspends the whole VM durably.
//!
//! Spread / `Array.from` / array destructuring still materialize eagerly via
//! the legacy nested driver — they consume the whole sequence by definition;
//! tool calls there remain unsupported until a later stage.

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

/// Drive with scripted host replies (`r0`, `r1`, …), dump/load at every
/// suspension.
fn drive_hopping(code: &str) -> String {
    let runner = ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        vec!["callTool".to_string()],
        ResourceLimits::default(),
    )
    .unwrap();
    let mut state = runner.start(Vec::new()).unwrap();
    let mut n = 0;
    loop {
        match state {
            VmState::Suspended { snapshot, .. } => {
                let restored = ZapcodeSnapshot::load(&snapshot.dump().unwrap()).unwrap();
                state = restored
                    .resume(Value::String(format!("r{n}").into()))
                    .unwrap()
                    .state;
                n += 1;
            }
            VmState::Complete(v) => match v {
                Value::String(s) => return s.to_string(),
                other => return format!("{other:?}"),
            },
            other => panic!("unexpected state {other:?}"),
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  Laziness and protocol parity
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn for_of_pulls_lazily_with_break() {
    assert_eq!(
        run_str(
            "function* nat() { let i = 0; while (true) yield i++; } \
             const out = []; \
             for (const x of nat()) { if (x >= 3) break; out.push(x); } \
             out.join(',')"
        ),
        "0,1,2"
    );
}

#[test]
fn yield_star_delegates_through_main_loop_pulls() {
    assert_eq!(
        run_str(
            "function* inner() { yield 1; yield 2; } \
             function* outer() { yield 0; yield* inner(); yield 3; } \
             const out = []; \
             for (const x of outer()) out.push(x); \
             out.join(',')"
        ),
        "0,1,2,3"
    );
}

#[test]
fn next_inside_a_for_of_body_advances_the_same_iterator() {
    // Node: the generator is NOT mid-pull during the loop body, so .next()
    // there legally consumes an element (the loop skips it).
    assert_eq!(
        run_str(
            "function* g() { yield 1; yield 2; yield 3; yield 4; } \
             const it = g(); \
             const out = []; \
             for (const x of it) { out.push(x); it.next(); } \
             out.join(',')"
        ),
        "1,3"
    );
}

#[test]
fn try_catch_across_yields_under_for_of() {
    // The try-frame stash/restore applies to loop-driven pulls too.
    assert_eq!(
        run_str(
            "function* g() { \
                 try { yield 1; null.x; } catch (e) { yield 'caught'; } \
             } \
             const out = []; \
             for (const x of g()) out.push(x); \
             out.join(',')"
        ),
        "1,caught"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  New capability: tool calls inside loop-driven generator bodies
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn tool_call_inside_for_await_driven_generator() {
    // Each pull suspends the VM at the tool call (with a dump/load hop) and
    // the loop keeps pulling.
    assert_eq!(
        drive_hopping(
            "async function* fetchAll() { \
                 for (const id of ['a', 'b']) { \
                     const v = await callTool(id); \
                     yield id + '=' + v; \
                 } \
             } \
             async function main() { \
                 const out = []; \
                 for await (const x of fetchAll()) out.push(x); \
                 return out.join(','); \
             } \
             main();"
        ),
        "a=r0,b=r1"
    );
}

#[test]
fn yield_star_over_a_tool_calling_generator() {
    assert_eq!(
        drive_hopping(
            "async function* inner() { yield await callTool('x'); } \
             async function* outer() { yield 'pre'; yield* inner(); yield 'post'; } \
             async function main() { \
                 const out = []; \
                 for await (const v of outer()) out.push(v); \
                 return out.join(','); \
             } \
             main();"
        ),
        "pre,r0,post"
    );
}
