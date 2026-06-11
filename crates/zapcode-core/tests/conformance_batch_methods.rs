//! Conformance: `.then`/`.catch`/`.finally` on a *batch* promise
//! (`Promise.{all,race,any,allSettled}` over deferred host calls).
//!
//! Previously these were pass-through no-ops — the method was silently
//! dropped. Now the method FORCES the batch, exactly like the single-call
//! path (N5): the VM suspends on all of the batch's calls with a recorded
//! `PromiseMethod` action; on resume the method runs against the assembled
//! result (or against a rejected promise when the host fails the
//! combinator via `resume_with_error`, so `.catch` recovers).
//!
//! Like the single-call `.then`-forcing path, the suspension is *eager* (at
//! the method call, not deferred to a tick) — that is the documented N5
//! contract for host calls, the durable-execution boundary.
//!
//! Batches over INTERNAL chains only (no host call to force) instead lower
//! to a real pending promise — see `conformance_batch_lowering.rs`. Batches
//! that MIX host calls with microtask-pending chains settle their chains by
//! borrowing drain ticks at the call site, then force the host calls.

use zapcode_core::compiler::instruction::BatchKind;
use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, Value, ZapcodeRun};

/// Drive `code`, answering every suspension deterministically (`r<n>` /
/// `m<n>_<i>`; for race/any combinators a single chosen value). `error_at`
/// fails that suspension instead.
fn drive(code: &str, error_at: Option<usize>) -> Result<Value, String> {
    let runner = ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        vec!["toolA".to_string(), "toolB".to_string()],
        ResourceLimits::default(),
    )
    .unwrap();
    let mut state = runner.start(Vec::new()).map_err(|e| e.to_string())?;
    let mut n = 0;
    loop {
        match state {
            VmState::Suspended { snapshot, .. } => {
                state = if error_at == Some(n) {
                    snapshot
                        .resume_with_error(Value::String(format!("e{n}").into()))
                        .map_err(|e| e.to_string())?
                        .state
                } else {
                    snapshot
                        .resume(Value::String(format!("r{n}").into()))
                        .map_err(|e| e.to_string())?
                        .state
                };
                n += 1;
            }
            VmState::SuspendedMany {
                calls,
                combinator,
                snapshot,
            } => {
                state = if error_at == Some(n) {
                    snapshot
                        .resume_with_error(Value::String(format!("e{n}").into()))
                        .map_err(|e| e.to_string())?
                        .state
                } else {
                    let count = match combinator {
                        BatchKind::Race | BatchKind::Any => 1,
                        _ => calls.len(),
                    };
                    let vals = (0..count)
                        .map(|i| Value::String(format!("m{n}_{i}").into()))
                        .collect();
                    snapshot.resume_many(vals).map_err(|e| e.to_string())?.state
                };
                n += 1;
            }
            VmState::Complete(v) => return Ok(v),
        }
    }
}

fn complete_str(r: Result<Value, String>) -> String {
    match r {
        Ok(Value::String(s)) => s.to_string(),
        Ok(other) => format!("{other:?}"),
        Err(e) => format!("ERR:{e}"),
    }
}

#[test]
fn then_on_all_batch_forces_and_chains() {
    assert_eq!(
        complete_str(drive(
            "await Promise.all([toolA('1'), toolB('2')]).then(arr => arr.join('&'))",
            None
        )),
        "m0_0&m0_1"
    );
    assert_eq!(
        complete_str(drive(
            "await Promise.all([toolA('1'), toolB('2')]).then(arr => arr.length).then(n => n * 10)",
            None
        )),
        "Int(20)"
    );
}

#[test]
fn catch_on_batch_recovers_a_host_rejection() {
    assert_eq!(
        complete_str(drive(
            "await Promise.all([toolA('1'), toolB('2')]).catch(e => 'rec:' + e)",
            Some(0)
        )),
        "rec:e0"
    );
}

#[test]
fn finally_on_batch_passes_result_through() {
    assert_eq!(
        complete_str(drive(
            "const log = []; \
             const r = await Promise.all([toolA('1')]).finally(() => log.push('fin')); \
             `${r.join('')}|${log.join('')}`",
            None
        )),
        "m0_0|fin"
    );
}

#[test]
fn then_on_race_batch_delivers_the_hosts_choice() {
    assert_eq!(
        complete_str(drive(
            "await Promise.race([toolA('1'), toolB('2')]).then(v => 'won:' + v)",
            None
        )),
        "won:m0_0"
    );
}

#[test]
fn then_on_all_settled_batch() {
    // (The host supplies the settled entries for allSettled; the assertion
    // is that the .then handler ran over the assembled array.)
    assert_eq!(
        complete_str(drive(
            "await Promise.allSettled([toolA('1')]).then(rs => 'got:' + rs.join(','))",
            None
        )),
        "got:m0_0"
    );
}
