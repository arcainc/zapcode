//! Snapshot-target probe: size of a typical agent-code snapshot (<2 KB
//! target) and the dump→load→resume round-trip latency (<2 ms target).
//! Run with `cargo run --release -p zapcode-core --example profile_snapshot`.

use std::time::Instant;
use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, Value, ZapcodeRun, ZapcodeSnapshot};

/// Typical agent shape: some state built up, an async helper, suspension on
/// a tool call mid-loop.
const AGENT_CODE: &str = r#"
    const results = [];
    const config = { retries: 3, model: "small", tags: ["a", "b"] };
    async function step(id) {
        const r = await callTool(id);
        results.push({ id, r });
        return r;
    }
    async function main() {
        for (const id of ["alpha", "beta", "gamma"]) {
            await step(id);
        }
        return results.length + ":" + config.retries;
    }
    main();
"#;

fn suspended() -> zapcode_core::ZapcodeSnapshot {
    let runner = ZapcodeRun::new(
        AGENT_CODE.to_string(),
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

fn main() {
    let n = 2_000u32;

    let snap = suspended();
    let bytes = snap.dump().unwrap();
    println!("snapshot size: {} bytes (target < 2048)", bytes.len());

    // dump → load → resume round-trip
    let t = Instant::now();
    for _ in 0..n {
        let bytes = suspended().dump().unwrap();
        let restored = ZapcodeSnapshot::load(&bytes).unwrap();
        let _ = restored
            .resume(Value::String("tool-result".into()))
            .unwrap();
    }
    let with_start = t.elapsed() / n;

    // isolate: dump+load+resume only (suspension pre-built per iteration is
    // included above; measure dump/load/resume on a fresh snapshot each time
    // but subtract the start cost)
    let t = Instant::now();
    for _ in 0..n {
        let _ = suspended();
    }
    let start_only = t.elapsed() / n;

    println!("start (parse+compile+run to suspension): {start_only:?}");
    println!(
        "dump + load + resume round-trip: {:?} (target < 2 ms)",
        with_start.saturating_sub(start_only)
    );
}
