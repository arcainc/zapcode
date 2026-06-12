//! Demonstrate in-run heap compaction making `memory_limit_bytes` a LIVE
//! ceiling instead of a cumulative-allocation one. The workload churns large
//! throwaway arrays: live set is one array (~16 KB), but the cumulative bytes
//! ever allocated far exceed the limit. With in-run GC it completes; under the
//! old cumulative accounting it would trip the memory limit.
//! `cargo run --release -p zapcode-core --example profile_inrun`.

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, ZapcodeRun};

fn main() {
    // 1500 iterations, each building a fresh 1000-element array (dead the next
    // iteration). One array ≈ 16 KB live; cumulative ≈ 1500 × 16 KB ≈ 24 MB.
    let code = "let last = 0; \
                for (let i = 0; i < 1500; i++) { \
                    const big = Array.from({ length: 1000 }, (_, j) => i + j); \
                    last = big[500]; \
                } \
                last";

    // A tight 8 MiB limit: well above the ~16 KB live set, well below the
    // ~24 MB cumulative total. The allocation-COUNT limit (cumulative DoS
    // guard) is left generous so memory is the binding constraint.
    let mut limits = ResourceLimits::default();
    limits.memory_limit_bytes = 8 * 1024 * 1024;
    limits.max_allocations = 10_000_000;

    let runner = ZapcodeRun::new(code.to_string(), Vec::new(), Vec::new(), limits).unwrap();
    match runner.run(Vec::new()) {
        Ok(r) => match r.state {
            VmState::Complete(v) => println!(
                "completed under an 8 MiB LIVE limit: last = {} \
                 (~24 MB total churned, ~16 KB live)",
                v.to_js_string(&r.heap)
            ),
            other => println!("unexpected: {other:?}"),
        },
        Err(e) => println!("FAILED (cumulative accounting would do this): {e}"),
    }
}
