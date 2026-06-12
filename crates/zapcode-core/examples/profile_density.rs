//! Many-VM density probe: what does one suspended agent VM cost when a host
//! holds thousands of them in memory (the "1000 durable agents per worker"
//! scenario)?
//!
//! Creates N suspended VMs (typical agent code paused at a tool call, same
//! shape as `profile_snapshot.rs`), holds all their `ZapcodeSnapshot`s, and
//! reports:
//!   - total / per-VM serialized bytes (`dump()`)
//!   - process RSS before / after, so we see the *live* in-memory cost per VM
//!     (which includes programs, stack, frames, heap — not just the wire size)
//!   - a garbage-slot probe: the heap is append-only (no free / no GC), so loop
//!     temporaries stay in the heap and get serialized. Comparing a baseline
//!     run against one that churns short-lived objects before suspending
//!     measures how much dead state an in-run compaction pass could reclaim.
//!
//! Run with `cargo run --release -p zapcode-core --example profile_density`.

use std::process::Command;
use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, ZapcodeRun, ZapcodeSnapshot};

/// Typical agent shape: some state built up, an async helper, suspension on
/// a tool call mid-loop (mirrors profile_snapshot.rs).
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

/// Same agent, but it churns short-lived objects before the first tool call.
/// Live state at suspension is identical to AGENT_CODE plus one small counter;
/// every `tmp` is unreachable garbage by then — yet the append-only heap keeps
/// (and the snapshot serializes) every slot it ever allocated.
const CHURNY_AGENT_CODE: &str = r#"
    const results = [];
    const config = { retries: 3, model: "small", tags: ["a", "b"] };
    let checksum = 0;
    for (let i = 0; i < 500; i++) {
        const tmp = { id: i, payload: [i, i + 1], meta: { source: "loop" } };
        checksum += tmp.payload.length;
    }
    async function step(id) {
        const r = await callTool(id);
        results.push({ id, r });
        return r;
    }
    async function main() {
        for (const id of ["alpha", "beta", "gamma"]) {
            await step(id);
        }
        return results.length + ":" + config.retries + ":" + checksum;
    }
    main();
"#;

/// Resident set size of this process in bytes. `ps -o rss=` reports KiB and
/// works on both macOS and Linux, keeping this example std-only (no /proc
/// parsing, no extra deps).
fn rss_bytes() -> u64 {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("failed to run ps");
    let kib: u64 = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .expect("unparseable rss from ps");
    kib * 1024
}

fn suspended(code: &str) -> ZapcodeSnapshot {
    let runner = ZapcodeRun::new(
        code.to_string(),
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

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn main() {
    let n: usize = 1000;

    // Warm up allocator + lazily-initialized statics (builtin-globals template,
    // oxc arenas) so they don't pollute the before/after RSS delta.
    drop(suspended(AGENT_CODE));

    // Baseline density: hold only the dumped bytes (what a host that parks
    // agents in RAM-but-serialized form would pay). Run before the live-VM
    // phase so its small freed buffers can't subsidize the big delta.
    println!("== density: {n} suspended agent VMs held as serialized bytes ==");
    let rss_before = rss_bytes();
    let mut blobs: Vec<Vec<u8>> = Vec::with_capacity(n);
    for _ in 0..n {
        blobs.push(suspended(AGENT_CODE).dump().unwrap());
    }
    let rss_after = rss_bytes();
    println!(
        "RSS before {:.1} MiB, after {:.1} MiB, delta {:.1} MiB => ~{} KiB per parked VM",
        mib(rss_before),
        mib(rss_after),
        mib(rss_after.saturating_sub(rss_before)),
        rss_after.saturating_sub(rss_before) / 1024 / n as u64
    );
    drop(blobs);

    println!("\n== density: {n} suspended agent VMs held live ==");
    let rss_before = rss_bytes();
    let mut snaps: Vec<ZapcodeSnapshot> = Vec::with_capacity(n);
    for _ in 0..n {
        snaps.push(suspended(AGENT_CODE));
    }
    let rss_after = rss_bytes();

    // Serialized cost, measured after the RSS sample so dump()'s temporary
    // buffers don't inflate the live-VM delta.
    let total_serialized: usize = snaps.iter().map(|s| s.dump().unwrap().len()).sum();
    println!(
        "serialized: total {} bytes, per-VM {} bytes",
        total_serialized,
        total_serialized / n
    );
    println!(
        "RSS before {:.1} MiB, after {:.1} MiB, delta {:.1} MiB => ~{} KiB live per suspended VM",
        mib(rss_before),
        mib(rss_after),
        mib(rss_after.saturating_sub(rss_before)),
        rss_after.saturating_sub(rss_before) / 1024 / n as u64
    );
    let heap_slots = snaps[0].heap().len();
    println!("heap slots per VM: {heap_slots}");
    drop(snaps);

    // Garbage probe: identical live state at suspension, but 1000 dead slots
    // (500 iterations x {object, nested array} per iteration, plus the nested
    // meta object) churned beforehand. Everything the churny variant carries
    // beyond the baseline is reclaimable by an in-run compaction pass.
    println!("\n== garbage probe: append-only heap retains loop temporaries ==");
    let mut base = suspended(AGENT_CODE);
    let mut churny = suspended(CHURNY_AGENT_CODE);
    let base_bytes = base.dump().unwrap().len();
    let churny_bytes = churny.dump().unwrap().len();
    println!(
        "baseline: {} heap slots, {} serialized bytes",
        base.heap().len(),
        base_bytes
    );
    println!(
        "churny:   {} heap slots, {} serialized bytes",
        churny.heap().len(),
        churny_bytes
    );
    println!(
        "garbage carried: {} slots, {} serialized bytes ({:.1}x snapshot growth from dead state)",
        churny.heap().len() - base.heap().len(),
        churny_bytes.saturating_sub(base_bytes),
        churny_bytes as f64 / base_bytes as f64
    );
}
