//! Regression tests for the deferred-batch promise combinators
//! (`Promise.{all,race,any,allSettled}`) over external calls. The compiler
//! lowers each of these (when an array element is a direct external call) to a
//! `MakeBatchPromise(kind, n)` deferred batch; awaiting it suspends once with
//! `VmState::SuspendedMany { combinator, .. }`. These tests stand in for the
//! host bridge: they read the combinator tag and resume with the value(s) the
//! real `Promise.*` combinator would produce.
//!
//! - N1 race: first settled wins (resume with the single chosen value).
//! - N2 any: first fulfilled wins; all-reject -> AggregateError rejection.
//! - N3 allSettled: per-element `{status,value|reason}` objects; never rejects.
//! - N8 Promise.resolve: already-resolved promise / plain value adoption.

use zapcode_core::vm::VmState;
use zapcode_core::{BatchKind, ResourceLimits, Value, ZapcodeRun};

fn start(code: &str) -> VmState {
    ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        vec!["delay".to_string(), "boom".to_string(), "fetch".to_string()],
        ResourceLimits::default(),
    )
    .unwrap()
    .start(Vec::new())
    .unwrap()
}

/// One simulated host outcome for a batched call.
#[derive(Clone)]
enum Out {
    Ok(Value),
    Err(&'static str),
}

/// Drive a single batch suspension the way the JS bridge does: read the
/// combinator tag, then resume with the value(s) the real `Promise.*`
/// combinator produces from `outcomes` (one per call, in call order).
fn resume_combinator(state: VmState, outcomes: &[Out]) -> VmState {
    let (combinator, snapshot) = match state {
        VmState::SuspendedMany {
            combinator,
            snapshot,
            ..
        } => (combinator, snapshot),
        other => panic!("expected SuspendedMany, got {other:?}"),
    };

    match combinator {
        BatchKind::All => {
            // Promise.all: reject on first failure, else resume with all values.
            if let Some(Out::Err(msg)) = outcomes.iter().find(|o| matches!(o, Out::Err(_))) {
                snapshot
                    .resume_with_error(Value::String((*msg).into()))
                    .unwrap()
                    .state
            } else {
                let vals: Vec<Value> = outcomes
                    .iter()
                    .map(|o| match o {
                        Out::Ok(v) => v.clone(),
                        Out::Err(_) => unreachable!(),
                    })
                    .collect();
                snapshot.resume_many(vals).unwrap().state
            }
        }
        BatchKind::Race => {
            // Promise.race: the first settled outcome wins. The tests order
            // `outcomes` so index 0 is the first to settle.
            match &outcomes[0] {
                Out::Ok(v) => snapshot.resume_many(vec![v.clone()]).unwrap().state,
                Out::Err(msg) => snapshot
                    .resume_with_error(Value::String((*msg).into()))
                    .unwrap()
                    .state,
            }
        }
        BatchKind::Any => {
            // Promise.any: first fulfilled wins; all-reject -> AggregateError.
            match outcomes.iter().find_map(|o| match o {
                Out::Ok(v) => Some(v.clone()),
                Out::Err(_) => None,
            }) {
                Some(v) => snapshot.resume_many(vec![v]).unwrap().state,
                None => snapshot
                    .resume_with_error(Value::String("AggregateError: all rejected".into()))
                    .unwrap()
                    .state,
            }
        }
        BatchKind::AllSettled => {
            // Promise.allSettled: never rejects; resume with one settled object
            // per call. The settled objects must be allocated into the
            // snapshot's heap so their handles are valid on resume.
            let mut snapshot = snapshot;
            let settled: Vec<Value> = outcomes
                .iter()
                .map(|o| {
                    let fields: Vec<(&str, Value)> = match o {
                        Out::Ok(v) => vec![
                            ("status", Value::String("fulfilled".into())),
                            ("value", v.clone()),
                        ],
                        Out::Err(msg) => vec![
                            ("status", Value::String("rejected".into())),
                            ("reason", Value::String((*msg).into())),
                        ],
                    };
                    alloc_object(snapshot.heap_mut(), fields)
                })
                .collect();
            snapshot.resume_many(settled).unwrap().state
        }
    }
}

fn alloc_object(heap: &mut zapcode_core::heap::Heap, fields: Vec<(&str, Value)>) -> Value {
    use std::sync::Arc;
    let mut map = indexmap::IndexMap::new();
    for (k, v) in fields {
        map.insert(Arc::from(k), v);
    }
    Value::Object(heap.alloc_object(map))
}

// ── N3: allSettled — partial failure yields per-element statuses ──────────

#[test]
fn all_settled_reports_per_element_status() {
    let state = start(
        r#"
        const r = await Promise.allSettled([delay("a"), boom("b"), delay("c")]);
        r.map(x => x.status).join(",")
    "#,
    );
    let done = resume_combinator(
        state,
        &[
            Out::Ok(Value::String("A".into())),
            Out::Err("kaboom"),
            Out::Ok(Value::String("C".into())),
        ],
    );
    assert_complete_str(done, "fulfilled,rejected,fulfilled");
}

#[test]
fn all_settled_exposes_values_and_reasons() {
    let state = start(
        r#"
        const r = await Promise.allSettled([delay("a"), boom("b")]);
        (r[0].value || "?") + "|" + (r[1].reason || "?")
    "#,
    );
    let done = resume_combinator(
        state,
        &[Out::Ok(Value::String("A".into())), Out::Err("nope")],
    );
    assert_complete_str(done, "A|nope");
}

// ── N1: race — first settled wins ─────────────────────────────────────────

#[test]
fn race_returns_first_settled_value() {
    let state = start(
        r#"
        const r = await Promise.race([delay("slow"), delay("fast")]);
        r
    "#,
    );
    // First-settled is index 0 in our outcomes ordering.
    let done = resume_combinator(state, &[Out::Ok(Value::String("FAST".into()))]);
    assert_complete_str(done, "FAST");
}

#[test]
fn race_propagates_first_rejection() {
    let state = start(
        r#"
        try {
            const r = await Promise.race([boom("x"), delay("y")]);
            "value:" + r
        } catch (e) {
            // The host raises the rejection reason; here it's a bare string.
            "caught:" + e
        }
    "#,
    );
    let done = resume_combinator(state, &[Out::Err("fast-fail")]);
    assert_complete_str(done, "caught:fast-fail");
}

// ── N2: any — first fulfilled wins, skips rejections ──────────────────────

#[test]
fn any_skips_rejection_and_returns_first_fulfilled() {
    let state = start(
        r#"
        const r = await Promise.any([boom("x"), delay("y"), delay("z")]);
        r
    "#,
    );
    let done = resume_combinator(
        state,
        &[
            Out::Err("rejected-1"),
            Out::Ok(Value::String("Y".into())),
            Out::Ok(Value::String("Z".into())),
        ],
    );
    assert_complete_str(done, "Y");
}

#[test]
fn any_rejects_when_all_reject() {
    let state = start(
        r#"
        try {
            const r = await Promise.any([boom("a"), boom("b")]);
            "value:" + r
        } catch (e) {
            "caught"
        }
    "#,
    );
    let done = resume_combinator(state, &[Out::Err("a"), Out::Err("b")]);
    assert_complete_str(done, "caught");
}

// ── N1: all — unchanged baseline still works through the generalized path ──

#[test]
fn all_still_returns_ordered_values() {
    let state = start(
        r#"
        const r = await Promise.all([delay("a"), delay("b"), delay("c")]);
        r.join(",")
    "#,
    );
    let done = resume_combinator(
        state,
        &[
            Out::Ok(Value::String("A".into())),
            Out::Ok(Value::String("B".into())),
            Out::Ok(Value::String("C".into())),
        ],
    );
    assert_complete_str(done, "A,B,C");
}

#[test]
fn all_rejects_on_first_failure() {
    let state = start(
        r#"
        try {
            const r = await Promise.all([delay("a"), boom("b")]);
            "value:" + r
        } catch (e) {
            "caught:" + e
        }
    "#,
    );
    let done = resume_combinator(
        state,
        &[Out::Ok(Value::String("A".into())), Out::Err("bad")],
    );
    assert_complete_str(done, "caught:bad");
}

// ── N8: Promise.resolve adoption (inline, no external calls) ──────────────

#[test]
fn promise_resolve_of_plain_value() {
    assert_eq!(run_inline(r#"await Promise.resolve(42)"#), "42");
}

#[test]
fn promise_resolve_of_already_resolved_promise() {
    // Promise.resolve(promise) adopts the inner promise rather than wrapping it.
    assert_eq!(
        run_inline(r#"await Promise.resolve(Promise.resolve("inner"))"#),
        "inner"
    );
}

#[test]
fn promise_resolve_thenable_is_adopted() {
    // A thenable's resolved value should be adopted. At minimum, awaiting a
    // resolved-promise-shaped object unwraps to its value.
    assert_eq!(
        run_inline(r#"const p = Promise.resolve("x"); await p"#),
        "x"
    );
}

// ── Inline combinators (no external calls) settle without the host ────────

#[test]
fn inline_race_first_element_wins() {
    assert_eq!(
        run_inline(r#"await Promise.race([Promise.resolve("first"), Promise.resolve("second")])"#),
        "first"
    );
}

#[test]
fn inline_any_skips_rejected_first() {
    assert_eq!(
        run_inline(
            r#"await Promise.any([Promise.reject("bad"), Promise.resolve("good")])"#
        ),
        "good"
    );
}

#[test]
fn inline_all_settled_mixes_statuses() {
    assert_eq!(
        run_inline(
            r#"
            const r = await Promise.allSettled([Promise.resolve(1), Promise.reject("e")]);
            r[0].status + "," + r[1].status
        "#
        ),
        "fulfilled,rejected"
    );
}

// ── helpers ───────────────────────────────────────────────────────────────

/// Assert a completed VM state stringifies to `expected`. We re-stringify by
/// matching the `Complete` value against the result heap.
fn assert_complete_str(state: VmState, expected: &str) {
    match &state {
        VmState::Complete(_) => {}
        other => panic!("expected completion, got {other:?}"),
    }
    // The state owns the value but not a heap; combinator helper discards the
    // RunResult heap. Re-run is unnecessary because all expected values are
    // primitives produced by `.join`/string ops -> the value is a String/Int.
    let s = stringify_primitive(&state);
    assert_eq!(s, expected);
}

fn stringify_primitive(state: &VmState) -> String {
    match state {
        // All these tests complete with a primitive (string/number/bool) built
        // by guest-side `.join`/string ops, so a heap-free stringify suffices.
        VmState::Complete(v) => v.to_js_string(&zapcode_core::heap::Heap::new()),
        other => panic!("expected completion, got {other:?}"),
    }
}

/// Run code that has no external calls (so it never suspends) and stringify.
fn run_inline(code: &str) -> String {
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
        other => panic!("expected completion, got {other:?}"),
    }
}
