//! In-run heap compaction under STRESS: with `enable_gc_stress_for_tests()`,
//! the VM mark-compacts the heap at EVERY top-level instruction boundary.
//! Maximally aggressive in-run handle rewriting that still produces correct
//! results is the proof that the live-root walk (`Vm::for_each_root_handle_mut`)
//! is complete — a missed root would dangle a handle and corrupt a result,
//! not merely leak. Every program here keeps live state (closures, generators,
//! accumulation, async, nested structures) across many allocation sites, so a
//! mis-rewritten handle shows up as a wrong answer.
//!
//! See `docs/in-run-memory-design.md`.

use zapcode_core::vm::{enable_gc_stress_for_tests, VmState};
use zapcode_core::{ResourceLimits, Value, ZapcodeRun, ZapcodeSnapshot};

fn run_str(code: &str) -> String {
    enable_gc_stress_for_tests();
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

#[test]
fn churn_loop_with_live_accumulator() {
    // Thousands of dead intermediate arrays/objects; the accumulator must
    // survive every compaction intact.
    assert_eq!(
        run_str(
            "let sum = 0; \
             for (let i = 0; i < 2000; i++) { const tmp = [i, i * 2, i * 3]; sum += tmp[1]; } \
             String(sum)"
        ),
        "3998000"
    );
}

#[test]
fn live_array_grows_across_compactions() {
    assert_eq!(
        run_str(
            "const acc = []; \
             for (let i = 0; i < 500; i++) { const junk = { a: i, b: [i] }; acc.push(i * i); } \
             String(acc.length) + ':' + String(acc[499])"
        ),
        "500:249001"
    );
}

#[test]
fn closures_capture_state_across_compactions() {
    // Captured cells must be rewritten correctly.
    assert_eq!(
        run_str(
            "function mk() { let n = 0; return { inc: () => ++n, get: () => n }; } \
             const cs = []; \
             for (let i = 0; i < 200; i++) { const c = mk(); c.inc(); c.inc(); cs.push(c); } \
             cs.reduce((s, c) => s + c.get(), 0) + '' "
        ),
        "400"
    );
}

#[test]
fn nested_objects_and_map_filter_reduce() {
    assert_eq!(
        run_str(
            "const data = []; \
             for (let i = 0; i < 300; i++) data.push({ id: i, tags: ['a' + i, 'b' + i] }); \
             const r = data.filter(d => d.id % 2 === 0).map(d => d.tags[0]).slice(0, 3).join(','); \
             r"
        ),
        "a0,a2,a4"
    );
}

#[test]
fn generator_state_survives_compaction() {
    assert_eq!(
        run_str(
            "function* nats() { let i = 0; while (true) yield i++; } \
             const it = nats(); let total = 0; \
             for (let k = 0; k < 1000; k++) { const tmp = [k]; total += it.next().value; } \
             String(total)"
        ),
        "499500"
    );
}

#[test]
fn async_accumulation_survives_compaction() {
    assert_eq!(
        run_str(
            "async function main() { \
                 let acc = ''; \
                 for (let i = 0; i < 100; i++) { const v = await Promise.resolve(i); if (i < 3) acc += v; } \
                 return acc; \
             } main();"
        ),
        "012"
    );
}

#[test]
fn map_set_state_survives_compaction() {
    assert_eq!(
        run_str(
            "const counts = new Map(); \
             for (let i = 0; i < 600; i++) { const k = 'k' + (i % 4); counts.set(k, (counts.get(k) ?? 0) + 1); } \
             [...counts.entries()].map(([k, v]) => k + '=' + v).join(',')"
        ),
        "k0=150,k1=150,k2=150,k3=150"
    );
}

#[test]
fn try_catch_across_compaction_keeps_caught_value() {
    assert_eq!(
        run_str(
            "let caught = ''; \
             for (let i = 0; i < 300; i++) { \
                 try { const tmp = [i]; if (i === 150) throw new Error('at ' + tmp[0]); } \
                 catch (e) { caught = e.message; } \
             } caught"
        ),
        "at 150"
    );
}

// ── GC + snapshot interplay: compaction during a durable run ───────────────

#[test]
fn durable_run_with_in_run_compaction_replays_identically() {
    enable_gc_stress_for_tests();
    // Churn before AND after a tool suspension; the hop must agree with the
    // in-memory run even though the heap is rewritten on nearly every step.
    let code = "async function main() { \
                    let pre = 0; \
                    for (let i = 0; i < 400; i++) { const t = [i, i]; pre += t[0]; } \
                    const r = await callTool('x'); \
                    let post = 0; \
                    for (let i = 0; i < 400; i++) { const t = { v: i }; post += t.v; } \
                    return pre + ':' + r + ':' + post; \
                } main();";
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
                    state = snapshot.resume(Value::String("R".into())).unwrap().state;
                }
                VmState::Complete(v) => return format!("{v:?}"),
                other => panic!("unexpected {other:?}"),
            }
        }
    };
    let in_memory = drive(false);
    assert_eq!(in_memory, drive(true));
    assert_eq!(in_memory, "String(Valid(\"79800:R:79800\"))");
}
