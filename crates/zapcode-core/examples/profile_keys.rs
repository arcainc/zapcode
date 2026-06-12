//! Object-key duplication probe: would interning object keys (`Arc<str>`) save
//! memory?
//!
//! Runs a representative object-heavy agent program to a tool-call suspension,
//! then walks the snapshot heap and counts, for every object key:
//!   - total key occurrences vs unique key *strings* (the upper bound an
//!     interner could deduplicate down to), and
//!   - unique `Arc<str>` *allocations* per string (via pointer identity), which
//!     shows how much sharing `Arc` cloning already gives us within one VM.
//!
//! Heap-key duplication is the measurable proxy: each extra allocation of the
//! same string costs `len + ~16B Arc header (+ allocator slack)`; each extra
//! *occurrence* still costs a 16B fat pointer in the IndexMap entry whether or
//! not keys are interned — interning only collapses the backing allocations.
//!
//! Run with `cargo run --release -p zapcode-core --example profile_keys`.

use std::collections::HashMap;
use std::sync::Arc;
use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, ZapcodeRun, ZapcodeSnapshot};

/// Object-heavy agent shape: a loop building uniform records (the classic
/// agent pattern — accumulate structured tool results), then a tool call.
const OBJECT_HEAVY_CODE: &str = r#"
    const records = [];
    for (let i = 0; i < 200; i++) {
        const user = {
            id: i,
            name: "user" + i,
            email: "u" + i + "@example.com",
            active: i % 2 === 0,
            profile: { age: 20 + (i % 40), city: "Paris", tags: ["agent", "test"] },
        };
        records.push({
            id: i,
            user,
            status: "ok",
            retries: 0,
            meta: { source: "api", ts: 1700000000 + i },
        });
    }
    async function main() {
        const summary = await callTool("summarize");
        return records.length + ":" + summary;
    }
    main();
"#;

fn suspended() -> ZapcodeSnapshot {
    let runner = ZapcodeRun::new(
        OBJECT_HEAVY_CODE.to_string(),
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

#[derive(Default)]
struct KeyStat {
    /// How many object entries use this key string.
    occurrences: usize,
    /// Distinct `Arc<str>` allocations backing those occurrences. Same-pointer
    /// clones share one allocation; >1 here means duplicate backing buffers an
    /// interner would collapse.
    allocations: HashMap<usize, usize>,
}

fn main() {
    let mut snap = suspended();
    let heap = snap.heap();

    let mut object_slots = 0usize;
    let mut array_slots = 0usize;
    let mut stats: HashMap<String, KeyStat> = HashMap::new();
    let mut total_occurrences = 0usize;

    // The heap is a flat arena of every array/object slot in the VM (globals,
    // locals, builtin template objects all point into it), so walking handles
    // 0..len covers every reachable — and unreachable — object key.
    for h in 0..heap.len() as u32 {
        if heap.is_array(h) {
            array_slots += 1;
            continue;
        }
        let Some(fields) = heap.object(h) else {
            continue;
        };
        object_slots += 1;
        for key in fields.keys() {
            total_occurrences += 1;
            let stat = stats.entry(key.to_string()).or_default();
            stat.occurrences += 1;
            // Thin out the *const str fat pointer to its data address: pointer
            // identity == same backing allocation.
            let addr = Arc::as_ptr(key) as *const u8 as usize;
            *stat.allocations.entry(addr).or_insert(0) += 1;
        }
    }

    let unique_strings = stats.len();
    let total_allocations: usize = stats.values().map(|s| s.allocations.len()).sum();
    let total_key_bytes: usize = stats
        .iter()
        .map(|(k, s)| k.len() * s.occurrences)
        .sum();
    let allocated_key_bytes: usize = stats
        .iter()
        // Each distinct allocation pays the string bytes + ~16B Arc refcount header.
        .map(|(k, s)| (k.len() + 16) * s.allocations.len())
        .sum();
    let interned_key_bytes: usize = stats.keys().map(|k| k.len() + 16).sum();

    println!("heap slots: {} ({object_slots} objects, {array_slots} arrays)", heap.len());
    println!("object-key occurrences: {total_occurrences}");
    println!("unique key strings:     {unique_strings}");
    println!("distinct Arc<str> allocations backing them: {total_allocations}");
    println!(
        "key bytes if every occurrence owned its string: {total_key_bytes}"
    );
    println!(
        "key bytes actually allocated (string + 16B Arc header per allocation): {allocated_key_bytes}"
    );
    println!(
        "key bytes if fully interned (one allocation per unique string): {interned_key_bytes}"
    );
    println!(
        "=> interning would reclaim ~{} bytes in this VM ({} duplicate allocations)",
        allocated_key_bytes.saturating_sub(interned_key_bytes),
        total_allocations - unique_strings
    );

    let mut top: Vec<(&String, &KeyStat)> = stats.iter().collect();
    top.sort_by(|a, b| b.1.occurrences.cmp(&a.1.occurrences).then(a.0.cmp(b.0)));
    println!("\ntop 20 hottest key strings (occurrences / distinct allocations):");
    println!("{:<24} {:>11} {:>11}", "key", "occurrences", "allocations");
    for (key, stat) in top.iter().take(20) {
        println!(
            "{:<24} {:>11} {:>11}",
            key,
            stat.occurrences,
            stat.allocations.len()
        );
    }
}
