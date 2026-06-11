//! Stage-by-stage timing probe for the first-execution pipeline:
//! parse → compile → VM construction → dispatch. Run with
//! `cargo run --release -p zapcode-core --example profile_pipeline`.

use std::collections::HashSet;
use std::time::Instant;

fn main() {
    let source = "1 + 2 * 3";
    let n = 20_000u32;

    // Parse
    let t = Instant::now();
    for _ in 0..n {
        let _ = zapcode_core::parser::parse(source).unwrap();
    }
    let parse = t.elapsed() / n;

    // Compile (parse re-done so we can reuse the IR per iteration)
    let program = zapcode_core::parser::parse(source).unwrap();
    let ext: HashSet<String> = HashSet::new();
    let t = Instant::now();
    for _ in 0..n {
        let _ = zapcode_core::compiler::compile_with_externals(&program, ext.clone()).unwrap();
    }
    let compile = t.elapsed() / n;

    // Full pipeline (what the <5µs target measures)
    let t = Instant::now();
    for _ in 0..n {
        let _ = zapcode_core::vm::eval_ts(source).unwrap();
    }
    let full = t.elapsed() / n;

    println!("iterations: {n}");
    println!("parse:        {parse:?}");
    println!("compile:      {compile:?}");
    println!("full eval_ts: {full:?}");
    println!(
        "residual (VM construct + dispatch + glue): {:?}",
        full.saturating_sub(parse + compile)
    );
}
