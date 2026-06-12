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

    // Pre-compiled run: compile once via ZapcodeProgram, then only pay
    // VM construction + dispatch per run.
    let program = zapcode_core::ZapcodeProgram::compile(source, Vec::new()).unwrap();
    let t = Instant::now();
    for _ in 0..n {
        let _ = program
            .run(Vec::new(), zapcode_core::ResourceLimits::default())
            .unwrap();
    }
    let precompiled = t.elapsed() / n;

    println!("iterations: {n}");
    println!("parse:        {parse:?}");
    println!("compile:      {compile:?}");
    println!("full eval_ts: {full:?}");
    println!("precompiled run (ZapcodeProgram, compile amortized): {precompiled:?}");
    println!(
        "residual (VM construct + dispatch + glue): {:?}",
        full.saturating_sub(parse + compile)
    );

    // ── Realistic agent program: compile-per-run vs compile-once ──────
    // Parse + compile scales with source size while dispatch stays cheap, so
    // program caching matters much more here than for a one-line expression.
    let agent_source = r#"
        function summarize(items: { name: string; qty: number; price: number }[]) {
            let total = 0;
            const lines: string[] = [];
            for (const item of items) {
                const cost = item.qty * item.price;
                total += cost;
                lines.push(`${item.name} x${item.qty} = ${cost}`);
            }
            return { lines, total };
        }
        const items = [
            { name: "widget", qty: 3, price: 7 },
            { name: "gadget", qty: 2, price: 19 },
            { name: "gizmo", qty: 5, price: 3 },
        ];
        const report = summarize(items);
        const sorted = items.map((i) => i.name).sort().join(",");
        JSON.stringify({ total: report.total, sorted, count: report.lines.length })
    "#;
    let n2 = 5_000u32;

    let runner = zapcode_core::ZapcodeRun::new(
        agent_source.to_string(),
        Vec::new(),
        Vec::new(),
        zapcode_core::ResourceLimits::default(),
    )
    .unwrap();
    let t = Instant::now();
    for _ in 0..n2 {
        let _ = runner.run(Vec::new()).unwrap();
    }
    let agent_full = t.elapsed() / n2;

    let program = zapcode_core::ZapcodeProgram::compile(agent_source, Vec::new()).unwrap();
    let t = Instant::now();
    for _ in 0..n2 {
        let _ = program
            .run(Vec::new(), zapcode_core::ResourceLimits::default())
            .unwrap();
    }
    let agent_precompiled = t.elapsed() / n2;

    println!();
    println!("realistic agent program ({n2} iterations):");
    println!("  ZapcodeRun (parse+compile every run): {agent_full:?}");
    println!("  ZapcodeProgram (compile once):        {agent_precompiled:?}");
    println!(
        "  parse+compile amortized away:          {:?}",
        agent_full.saturating_sub(agent_precompiled)
    );
}
