//! Dispatch-loop profiling workload: heavy recursion + a tight loop, run
//! long enough for a sampling profiler. `cargo run --release --example
//! profile_dispatch` (or under `samply record`).

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, ZapcodeRun};

fn run(code: &str) -> String {
    let mut limits = ResourceLimits::default();
    limits.max_allocations = usize::MAX;
    limits.memory_limit_bytes = usize::MAX;
    let r = ZapcodeRun::new(code.to_string(), Vec::new(), Vec::new(), limits)
        .unwrap()
        .run(Vec::new())
        .unwrap();
    match r.state {
        VmState::Complete(v) => format!("{v:?}"),
        other => panic!("{other:?}"),
    }
}

fn main() {
    let fib = "function fib(n) { if (n <= 1) { return n; } return fib(n - 1) + fib(n - 2); } fib(22)";
    let lop = "let sum = 0; for (let i = 0; i < 200000; i++) { sum += i * 2 + 1; } sum";
    let t = std::time::Instant::now();
    let a = run(fib);
    let t_fib = t.elapsed();
    let t = std::time::Instant::now();
    let b = run(lop);
    let t_loop = t.elapsed();
    println!("fib(22)={a} in {t_fib:?}; loop200k={b} in {t_loop:?}");
}
