//! Regression tests for the upvalue-cell closure model: callbacks mutating
//! outer scope, independent closure instances, per-iteration `let` bindings,
//! and (critically) that shared cells survive snapshot serialization.

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
        VmState::Complete(v) => v.to_js_string(),
        other => panic!("expected completion, got {other:?}"),
    }
}

#[test]
fn callback_mutates_outer_scope() {
    assert_eq!(run_str("let s = 0; [1,2,3].forEach(x => s += x); s"), "6");
    assert_eq!(
        run_str("const out = []; [1,2].forEach(x => out.push(x * 2)); out.join(',')"),
        "2,4"
    );
    // Mutation through a named helper closure still reaches the outer binding.
    assert_eq!(
        run_str(
            "function outer(){ let total = 0; const add = (x) => { total += x; }; [1,2,3].forEach(add); return total; } outer()"
        ),
        "6"
    );
}

#[test]
fn map_and_set_foreach_side_effects() {
    assert_eq!(
        run_str(
            "const out=[]; new Map([['a',1],['b',2]]).forEach((v,k)=>out.push(k+v)); out.join(',')"
        ),
        "a1,b2"
    );
    assert_eq!(
        run_str("let s=0; new Set([1,2,3]).forEach(v=>s+=v); s"),
        "6"
    );
}

#[test]
fn closure_instances_are_independent() {
    // Each call to the factory must get its own captured binding.
    assert_eq!(
        run_str("function mk(){ let n = 0; return () => ++n; } const a = mk(), b = mk(); a(); a(); b(); a()"),
        "3"
    );
    assert_eq!(
        run_str(
            "function mk(){ let n = 0; return () => ++n; } const a = mk(), b = mk(); a(); a(); b()"
        ),
        "1"
    );
}

#[test]
fn let_loop_per_iteration_binding() {
    assert_eq!(
        run_str("const fs = []; for (let i = 0; i < 3; i++) { fs.push(() => i); } fs.map(f => f()).join(',')"),
        "0,1,2"
    );
    // `var` keeps function-scope semantics: all closures share the final value.
    assert_eq!(
        run_str("const fs = []; for (var i = 0; i < 3; i++) { fs.push(() => i); } fs.map(f => f()).join(',')"),
        "3,3,3"
    );
}

// ── Durability: shared cells must survive dump/load ──────────────────────

fn start(code: &str) -> VmState {
    ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        vec!["callTool".to_string()],
        ResourceLimits::default(),
    )
    .unwrap()
    .start(Vec::new())
    .unwrap()
}

fn take_snapshot(state: VmState) -> ZapcodeSnapshot {
    match state {
        VmState::Suspended { snapshot, .. } => snapshot,
        other => panic!("expected suspension, got {other:?}"),
    }
}

#[test]
fn captured_mutable_cell_survives_snapshot_roundtrip() {
    // `inc` captures `n` by reference. We mutate it before a suspend, serialize
    // the whole VM to bytes, reload, resume, mutate again — the count must carry
    // across the dump/load, proving the shared cell is serialized with identity.
    let state = start(
        r#"
        let n = 0;
        const inc = () => ++n;
        inc();                          // n = 1
        const x = await callTool("go");
        inc();                          // n = 2 after resume
        n + x
    "#,
    );

    let bytes = take_snapshot(state).dump().unwrap();
    let reloaded = ZapcodeSnapshot::load(&bytes).unwrap();
    let resumed = reloaded.resume(Value::Int(10)).unwrap();

    match resumed {
        VmState::Complete(v) => assert_eq!(v, Value::Int(12)),
        other => panic!("expected completion, got {other:?}"),
    }
}
