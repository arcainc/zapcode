//! Regression tests for the "edges round 1" divergence batch:
//!   O5  — `"key" in obj` parsing when the left operand is a string literal, and
//!         `in` membership for array `length` / numeric-string index keys.
//!   H6  — Map.set / Set.add chaining and return-the-collection under reference
//!         semantics (a shared heap handle).
//!   G3  — a global regex used in `while ((m = re.exec(s)) !== null)` terminates:
//!         /g exec advances `lastIndex` and eventually returns null.
//!   O8  — a minimal `Symbol` global (`typeof Symbol === "function"`, `Symbol()`
//!         returns a unique `typeof === "symbol"` value).
//!   B-extra — stale `last_global_name` no longer leaks a builtin method into an
//!         unrelated member access (e.g. `String(o.missing)`), discovered while
//!         probing for cheap wins.

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, ZapcodeRun};

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
        VmState::Complete(v) => v.to_js_string(&result.heap),
        other => panic!("expected completion, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// O5: `in` with a string-literal left operand + array membership
// ---------------------------------------------------------------------------

#[test]
fn in_operator_string_literal_left() {
    // Previously a PARSE error: the trailing object literal was mangled by the
    // statement-start brace heuristic when the line started with a string literal.
    assert_eq!(run_str(r#""a" in {a:1}"#), "true");
    assert_eq!(run_str(r#""b" in {a:1}"#), "false");
    // In expression / ternary position.
    assert_eq!(run_str(r#"const o={x:5}; ("x" in o) ? "Y" : "N""#), "Y");
    assert_eq!(run_str(r#"const r = "x" in {x:5} ? "Y":"N"; r"#), "Y");
    // Other expression-context keywords/operators with a trailing object literal
    // must not be mangled either.
    assert_eq!(run_str(r#"const o = {a:1}; typeof o === "object""#), "true");
}

#[test]
fn in_operator_array_membership() {
    // Numeric index membership.
    assert_eq!(run_str(r#"0 in [1,2]"#), "true");
    assert_eq!(run_str(r#"5 in [1,2]"#), "false");
    assert_eq!(run_str(r#"const a=[10,20,30]; (1 in a)+","+(3 in a)"#), "true,false");
    // `length` is an own property of every array.
    assert_eq!(run_str(r#""length" in [1,2]"#), "true");
    // Numeric-string keys behave like indices.
    assert_eq!(run_str(r#""0" in [1,2]"#), "true");
    assert_eq!(run_str(r#""2" in [1,2]"#), "false");
    // Own-key membership only: inherited prototype keys stay absent.
    assert_eq!(run_str(r#""toString" in {a:1}"#), "false");
    assert_eq!(run_str(r#""push" in [1,2]"#), "false");
}

// ---------------------------------------------------------------------------
// H6: Map.set / Set.add chaining returns the collection (reference semantics)
// ---------------------------------------------------------------------------

#[test]
fn map_set_chaining_returns_self() {
    assert_eq!(run_str(r#"const m=new Map(); m.set('a',1).set('b',2); m.size"#), "2");
    assert_eq!(
        run_str(r#"const m=new Map(); m.set('a',1).set('b',2).set('c',3); m.size"#),
        "3"
    );
    // The return value of set is the map itself (handle identity).
    assert_eq!(run_str(r#"const m=new Map(); const r=m.set('a',1); r===m"#), "true");
    // Mutations are visible through every alias of the shared handle.
    assert_eq!(
        run_str(r#"const m=new Map(); const n=m; m.set('x',1).set('y',2); n.size"#),
        "2"
    );
}

#[test]
fn set_add_chaining_returns_self() {
    assert_eq!(run_str(r#"const s=new Set(); s.add(1).add(2); s.size"#), "2");
    // Duplicate adds are no-ops (SameValueZero), chaining still returns the set.
    assert_eq!(run_str(r#"const s=new Set(); s.add(1).add(2).add(2).add(3); s.size"#), "3");
    assert_eq!(run_str(r#"const s=new Set(); const r=s.add(1); r===s"#), "true");
    assert_eq!(
        run_str(r#"const s=new Set(); const t=s; s.add(1).add(2); t.size"#),
        "2"
    );
}

// ---------------------------------------------------------------------------
// G3: global regex exec maintains lastIndex and terminates
// ---------------------------------------------------------------------------

#[test]
fn global_regex_exec_terminates() {
    // The canonical `while ((m = re.exec(s)) !== null)` loop must terminate.
    assert_eq!(
        run_str(
            r#"const re=/\d/g; const s="a1b2c3"; let n=0; let m;
               while((m=re.exec(s))!==null){ n++; if(n>1000) break; } n"#
        ),
        "3"
    );
    // The matched substrings are produced in order.
    assert_eq!(
        run_str(
            r#"const re=/\d+/g; const s="12 34 5"; let out=[]; let m;
               while((m=re.exec(s))!==null){ out.push(m[0]); } out.join(',')"#
        ),
        "12,34,5"
    );
    // Capture groups are still returned for /g exec.
    assert_eq!(
        run_str(r#"const re=/(\d)(\w)/g; const m=re.exec("a1b2c"); m[1]+":"+m[2]"#),
        "1:b"
    );
}

#[test]
fn global_regex_test_advances_and_resets() {
    // /g test advances lastIndex, then resets to 0 when exhausted.
    assert_eq!(
        run_str(r#"const re=/x/g; [re.test("axbx"), re.test("axbx"), re.test("axbx")].join(',')"#),
        "true,true,false"
    );
    // After exhaustion + reset, a fresh scan succeeds again.
    assert_eq!(
        run_str(
            r#"const re=/x/g; re.test("axbx"); re.test("axbx"); re.test("axbx");
               re.test("axbx")"#
        ),
        "true"
    );
}

#[test]
fn nonglobal_regex_exec_unchanged() {
    // A non-global regex's exec always matches from the start (no lastIndex
    // advance) — unchanged behavior. The loop guard proves we did not silently
    // start advancing a /-without-g regex.
    assert_eq!(
        run_str(
            r#"const re=/\d/; const s="a1b2"; let n=0; let m;
               while((m=re.exec(s))!==null){ n++; if(n>=5) break; } n"#
        ),
        "5"
    );
}

// ---------------------------------------------------------------------------
// O8: minimal Symbol global
// ---------------------------------------------------------------------------

#[test]
fn symbol_feature_detection() {
    assert_eq!(run_str(r#"typeof Symbol"#), "function");
    assert_eq!(run_str(r#"typeof Symbol === "function""#), "true");
}

#[test]
fn symbol_value_is_unique_and_typed() {
    assert_eq!(run_str(r#"const x=Symbol(); typeof x"#), "symbol");
    // Each Symbol() is a distinct value (handle identity under strict_eq).
    assert_eq!(run_str(r#"Symbol() === Symbol()"#), "false");
    // A symbol equals itself.
    assert_eq!(run_str(r#"const s=Symbol("d"); s===s"#), "true");
    // Optional description, coerced to string; absent description is undefined.
    assert_eq!(run_str(r#"const s=Symbol("hi"); s.description"#), "hi");
    assert_eq!(run_str(r#"const s=Symbol(); typeof s.description"#), "undefined");
    // Simple use as a computed object key does not throw.
    assert_eq!(run_str(r#"const s=Symbol("k"); const o={}; o[s]=5; o[s]"#), "5");
}

#[test]
fn builtin_constructors_typeof_function() {
    // Bonus correctness from the typeof marker change: callable builtins report
    // "function"; pure namespaces stay "object".
    assert_eq!(run_str(r#"typeof String"#), "function");
    assert_eq!(run_str(r#"typeof Number"#), "function");
    assert_eq!(run_str(r#"typeof Object"#), "function");
    assert_eq!(run_str(r#"typeof Array"#), "function");
    assert_eq!(run_str(r#"typeof Map"#), "function");
    assert_eq!(run_str(r#"typeof parseInt"#), "function");
    assert_eq!(run_str(r#"typeof Math"#), "object");
    assert_eq!(run_str(r#"typeof JSON"#), "object");
    assert_eq!(run_str(r#"typeof console"#), "object");
}

// ---------------------------------------------------------------------------
// B-extra: stale last_global_name no longer leaks a builtin method
// ---------------------------------------------------------------------------

#[test]
fn missing_property_in_call_arg_is_undefined() {
    // Regression: `String(o.zzz)` used to resolve the missing property `zzz` to a
    // `String` builtin method (stale `last_global_name`), yielding "function".
    assert_eq!(run_str(r#"const o={a:1}; String(o.zzz)"#), "undefined");
    assert_eq!(run_str(r#"const o={}; typeof String(o.z)"#), "string");
    assert_eq!(run_str(r#"const o={a:1}; String(Number(o.zzz))"#), "NaN");
    // The legitimate `Math.floor`-style shortcut still works (immediate read
    // after LoadGlobal), including nested.
    assert_eq!(run_str(r#"Math.floor(Math.max(1.2, 2.8))"#), "2");
    assert_eq!(run_str(r#"const s='abc'; String(s.toUpperCase())"#), "ABC");
}
