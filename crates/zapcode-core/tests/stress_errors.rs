//! Regression tests for runtime errors caught as real Error objects (C4).
//! (Uses variable assignment rather than trailing try/catch, which is the
//! separate B1 completion-value fix.)

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, ZapcodeRun};

fn run_str(code: &str) -> String {
    let result = ZapcodeRun::new(code.to_string(), Vec::new(), Vec::new(), ResourceLimits::default())
        .unwrap().run(Vec::new()).unwrap();
    match result.state {
        VmState::Complete(v) => v.to_js_string(&result.heap),
        other => panic!("expected completion, got {other:?}"),
    }
}

#[test]
fn caught_type_error_has_name_message_instanceof() {
    assert_eq!(run_str("let n; try { null.x; } catch (e) { n = e.name; } n"), "TypeError");
    assert_eq!(run_str("let b; try { null.x; } catch (e) { b = e instanceof TypeError; } b"), "true");
    assert_eq!(run_str("let b; try { null.x; } catch (e) { b = e instanceof Error; } b"), "true");
    assert_eq!(run_str("let b; try { null.x; } catch (e) { b = typeof e.message === 'string' && e.message.length > 0; } b"), "true");
}

#[test]
fn caught_error_from_calling_non_function() {
    assert_eq!(run_str("let n; try { const x = 5; x(); } catch (e) { n = e.name; } n"), "TypeError");
}

#[test]
fn error_instanceof_branch_pattern() {
    // The ubiquitous `e instanceof Error ? e.message : String(e)` now hits the right branch.
    assert_eq!(
        run_str("let m; try { null.x; } catch (e) { m = e instanceof Error ? 'msg:'+(e.message.length>0) : 'str'; } m"),
        "msg:true"
    );
}

#[test]
fn guest_thrown_values_still_passthrough() {
    assert_eq!(run_str("let e2; try { throw 'boom'; } catch (e) { e2 = e; } e2"), "boom");
    assert_eq!(run_str("let e2; try { throw 42; } catch (e) { e2 = e; } e2"), "42");
    assert_eq!(run_str("let m; try { throw new Error('x'); } catch (e) { m = e.message; } m"), "x");
}

#[test]
fn aggregate_error() {
    assert_eq!(run_str("const e = new AggregateError([1,2], 'all failed'); e.name"), "AggregateError");
    assert_eq!(run_str("const e = new AggregateError([1,2], 'all failed'); e.message"), "all failed");
    assert_eq!(run_str("const e = new AggregateError([1,2], 'x'); e.errors.length"), "2");
    assert_eq!(run_str("const e = new AggregateError([], 'x'); e instanceof Error"), "true");
    assert_eq!(run_str("const e = new AggregateError([], 'x'); e instanceof AggregateError"), "true");
}
