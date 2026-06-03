//! Conformance breadth: Date and Error.
//!
//! Date construction (epoch ms, ISO string, `Date.UTC`, `Date.parse`), UTC
//! component getters, arithmetic/coercion, Invalid Date, `instanceof`, and
//! `toISOString`/`toJSON`. Error construction & subtypes (`TypeError`,
//! `RangeError`), `name`/`message`/`stack`, `instanceof Error`, throw/catch,
//! rethrow, and the built-in `AggregateError`. All UTC-based to be deterministic
//! and Node-matching. One documented gap is pinned to actual behavior: a USER
//! `class X extends Error` neither propagates `super(message)` nor establishes the
//! `instanceof Error` chain (built-in error subtypes do both correctly).

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
        other => panic!("expected completion for `{code}`, got {other:?}"),
    }
}

// ----------------------------------------------------------------------------
// Date construction
// ----------------------------------------------------------------------------

#[test]
fn date_construction_from_epoch_and_string() {
    assert_eq!(run_str("new Date(0).getTime()"), "0");
    assert_eq!(run_str("new Date(1000).getTime()"), "1000");
    assert_eq!(run_str("new Date('1970-01-01T00:00:00.000Z').getTime()"), "0");
    assert_eq!(run_str("new Date('2020-01-01T00:00:00.000Z').getTime()"), "1577836800000");
}

#[test]
fn date_static_utc_and_parse() {
    assert_eq!(run_str("Date.UTC(2020, 0, 1)"), "1577836800000"); // month is 0-based
    assert_eq!(run_str("Date.parse('1970-01-01T00:00:00.000Z')"), "0");
    assert_eq!(run_str("Date.parse('2020-01-01T00:00:00.000Z')"), "1577836800000");
    assert_eq!(run_str("new Date(Date.UTC(2000, 0, 1)).toISOString()"), "2000-01-01T00:00:00.000Z");
}

// ----------------------------------------------------------------------------
// Date components (UTC) & formatting
// ----------------------------------------------------------------------------

#[test]
fn date_utc_component_getters() {
    let d = "const d = new Date('2020-06-15T13:45:30.000Z');";
    assert_eq!(run_str(&format!("{d} d.getUTCFullYear()")), "2020");
    assert_eq!(run_str(&format!("{d} d.getUTCMonth()")), "5"); // June is month 5 (0-based)
    assert_eq!(run_str(&format!("{d} d.getUTCDate()")), "15");
    assert_eq!(run_str(&format!("{d} d.getUTCHours()")), "13");
    assert_eq!(run_str(&format!("{d} d.getUTCMinutes()")), "45");
    assert_eq!(run_str(&format!("{d} d.getUTCSeconds()")), "30");
}

#[test]
fn date_iso_and_json() {
    assert_eq!(run_str("new Date(0).toISOString()"), "1970-01-01T00:00:00.000Z");
    assert_eq!(run_str("new Date(0).toJSON()"), "1970-01-01T00:00:00.000Z");
    assert_eq!(run_str("new Date(1577836800000).toISOString()"), "2020-01-01T00:00:00.000Z");
    assert_eq!(run_str("JSON.stringify({when: new Date(0)})"), "{\"when\":\"1970-01-01T00:00:00.000Z\"}");
}

// ----------------------------------------------------------------------------
// Date arithmetic / coercion / instanceof / invalid
// ----------------------------------------------------------------------------

#[test]
fn date_arithmetic_and_coercion() {
    // Subtracting two dates coerces to ms.
    assert_eq!(run_str("const a = new Date(1000); const b = new Date(3000); b - a"), "2000");
    assert_eq!(run_str("Number(new Date(5000))"), "5000"); // numeric coercion -> epoch ms
    assert_eq!(run_str("typeof Number(new Date(0))"), "number");
}

#[test]
fn date_instanceof_and_invalid() {
    assert_eq!(run_str("new Date(0) instanceof Date"), "true");
    assert_eq!(run_str("new Date() instanceof Date"), "true");
    assert_eq!(run_str("String(new Date('not a date').getTime())"), "NaN");
    assert_eq!(run_str("String(new Date('garbage'))"), "Invalid Date");
}

// ----------------------------------------------------------------------------
// Error construction & subtypes
// ----------------------------------------------------------------------------

#[test]
fn error_basic_fields() {
    assert_eq!(run_str("new Error('boom').message"), "boom");
    assert_eq!(run_str("new Error('boom').name"), "Error");
    // NOTE: `String(err)` produces the spec "Name: message" form; the `.toString()`
    // METHOD is not exposed on Error objects here (use String() / concatenation).
    assert_eq!(run_str("String(new Error('boom'))"), "Error: boom");
    assert_eq!(run_str("String(new Error('msg'))"), "Error: msg");
    assert_eq!(run_str("'' + new Error('cat')"), "Error: cat");
    assert_eq!(run_str("typeof new Error('x').stack"), "string");
}

#[test]
fn error_subtypes() {
    assert_eq!(run_str("new TypeError('t').name"), "TypeError");
    assert_eq!(run_str("new RangeError('r').name"), "RangeError");
    assert_eq!(run_str("new TypeError('t').message"), "t");
    assert_eq!(run_str("new TypeError('x') instanceof Error"), "true");
    assert_eq!(run_str("new RangeError('x') instanceof RangeError"), "true");
    assert_eq!(run_str("String(new TypeError('msg'))"), "TypeError: msg");
    assert_eq!(run_str("typeof AggregateError"), "function");
}

// ----------------------------------------------------------------------------
// throw / catch / rethrow with Error objects
// ----------------------------------------------------------------------------

#[test]
fn throw_catch_error() {
    assert_eq!(
        run_str("let m; try { throw new RangeError('out'); } catch(e){ m = e.name + ':' + e.message; } m"),
        "RangeError:out"
    );
    assert_eq!(
        run_str("let t; try { throw new TypeError('x'); } catch(e){ t = (e instanceof TypeError) + ',' + (e instanceof Error); } t"),
        "true,true"
    );
}

#[test]
fn rethrow_chains_to_outer_catch() {
    assert_eq!(
        run_str("let log = []; try { try { throw new Error('a'); } catch(e){ log.push('inner:' + e.message); throw new Error('b'); } } catch(e){ log.push('outer:' + e.message); } log.join('|')"),
        "inner:a|outer:b"
    );
}

#[test]
fn caught_runtime_error_is_real_error() {
    // A native runtime error (not an explicit throw) is a real Error with a message.
    assert_eq!(run_str("let t; try { null.x; } catch(e){ t = e instanceof Error; } t"), "true");
    assert_eq!(run_str("let t; try { undefinedFn(); } catch(e){ t = typeof e.message; } t"), "string");
}

// ----------------------------------------------------------------------------
// User subclass of Error
// ----------------------------------------------------------------------------

#[test]
fn user_error_subclass() {
    // A USER `class X extends Error` now (a) propagates `super(message)` to
    // `this.message`, and (b) establishes the `instanceof Error` chain — matching
    // built-in error subtypes (TypeError/RangeError) and real Node.
    assert_eq!(
        run_str("class MyErr extends Error { constructor(m){ super(m); this.name = 'MyErr'; } } let r; try { throw new MyErr('custom'); } catch(e){ r = e.name + ':' + String(e.message) + ':' + (e instanceof Error); } r"),
        "MyErr:custom:true"
    );
    // Explicitly assigning the message in the constructor still works.
    assert_eq!(
        run_str("class MyErr extends Error { constructor(m){ super(m); this.message = m; this.name = 'MyErr'; } } let r; try { throw new MyErr('custom'); } catch(e){ r = e.name + ':' + e.message; } r"),
        "MyErr:custom"
    );
    // A subclass with NO own constructor still forwards the message via the
    // implicit `constructor(...a){ super(...a) }`; `name` defaults to "Error".
    assert_eq!(
        run_str("class MyErr extends Error {} let e = new MyErr('boom'); e.name + ':' + e.message + ':' + (e instanceof Error)"),
        "Error:boom:true"
    );
    // No-arg subclass construction yields an empty message (not undefined), like JS.
    assert_eq!(
        run_str("class MyErr extends Error {} String(new MyErr().message)"),
        ""
    );
}
