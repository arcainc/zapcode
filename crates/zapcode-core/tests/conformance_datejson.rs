//! Conformance breadth (round 1): **Date** and **JSON** — a wide, test262-style
//! sweep of the two host facilities agents lean on most for I/O.
//!
//! The interpreter runs in a deterministic UTC sandbox with no wall clock, so:
//!   * `Date.now()` and `new Date()` (no args) are epoch `0` (deterministic replay);
//!   * `getTimezoneOffset()` is `0`;
//!   * the local `getX` getters alias the `getUTCX` getters (UTC = local here).
//! Every expectation below was cross-checked against real Node run as `TZ=UTC`.
//!
//! DOCUMENTED DIVERGENCES asserted to ACTUAL zapcode behavior (with comments),
//! never to the real-JS answer:
//!   * `Date#toString` / `Date#toDateString` return the ISO-8601 string, not Node's
//!     human-readable `"Thu Jan 01 1970 …"` form.
//!
//! Coverage map:
//!   Date construction         — epoch ms, ISO/space-separated/offset strings,
//!                                date-only, multi-arg (y,m,d,h,mi,s,ms), no-arg.
//!   Date statics              — `Date.UTC`, `Date.parse`, `Date.now`.
//!   Date getUTC* accessors     — full year/month/date/day/hours/min/sec/ms, dow.
//!   Date arithmetic/coercion  — subtraction, unary `+`, `Number()`, comparisons.
//!   Date Invalid Date         — NaN getters, "Invalid Date" formatting, instanceof.
//!   Date instanceof + format  — `instanceof Date`/`Object`, `toISOString`/`toJSON`.
//!   JSON.stringify            — scalars, containers, drop undefined/function,
//!                                array holes -> null, control-char/quote escaping,
//!                                NaN/Infinity -> null, indentation (num & string),
//!                                array replacer, Date -> ISO, Map/Set/Error -> {}.
//!   JSON.parse                — objects/arrays/primitives, escapes, whitespace,
//!                                deep nesting, round-trips, parse-then-mutate.
//!   Function replacer/reviver — invoked per entry; user `toJSON()` honored.

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

/// Assert `code` evaluates to `expected` (its `to_js_string` rendering).
fn check(code: &str, expected: &str) {
    assert_eq!(run_str(code), expected, "for code `{code}`");
}

// ============================================================================
// DATE — construction
// ============================================================================

#[test]
fn date_construct_from_epoch_millis() {
    check("new Date(0).getTime()", "0");
    check("new Date(1).getTime()", "1");
    check("new Date(1000).getTime()", "1000");
    check("new Date(1577836800000).getTime()", "1577836800000");
    check("new Date(-1).getTime()", "-1");
    check("new Date(-86400000).getTime()", "-86400000"); // one day before epoch
    check("new Date(1234567890123).getTime()", "1234567890123");
    // Float epoch is truncated to integer millis.
    check("new Date(1500.9).getTime()", "1500");
}

#[test]
fn date_construct_from_iso_string() {
    check("new Date('1970-01-01T00:00:00.000Z').getTime()", "0");
    check("new Date('2020-01-01T00:00:00.000Z').getTime()", "1577836800000");
    check(
        "new Date('2009-02-13T23:31:30.123Z').getTime()",
        "1234567890123",
    );
    // Fractional seconds shorter than 3 digits pad on the right (.5 -> 500ms).
    check("new Date('2020-06-15T13:45:30.5Z').getUTCMilliseconds()", "500");
    check("new Date('2020-06-15T13:45:30.05Z').getUTCMilliseconds()", "50");
    // No seconds component.
    check("new Date('2020-01-01T12:00Z').toISOString()", "2020-01-01T12:00:00.000Z");
}

#[test]
fn date_construct_date_only_string_is_utc() {
    // A date-only string is interpreted as UTC midnight (matches Node).
    check("new Date('2020-01-01').toISOString()", "2020-01-01T00:00:00.000Z");
    check("new Date('2020-02-29').toISOString()", "2020-02-29T00:00:00.000Z"); // leap day
    check("new Date('1999-12-31').getUTCFullYear()", "1999");
    check("new Date('2020-06-15').getUTCMonth()", "5"); // June -> 5
}

#[test]
fn date_construct_string_with_offset_and_space() {
    // `+02:00` offset is subtracted to reach UTC.
    check(
        "new Date('2020-06-15T13:45:30+02:00').toISOString()",
        "2020-06-15T11:45:30.000Z",
    );
    check(
        "new Date('2020-06-15T13:45:30-05:00').toISOString()",
        "2020-06-15T18:45:30.000Z",
    );
    // Space separator between date and time (UTC sandbox).
    check("new Date('1970-01-01 00:00:01').getTime()", "1000");
}

#[test]
fn date_construct_multi_arg_utc() {
    // Multi-arg construction is UTC in this sandbox; month is 0-based.
    check("new Date(2020, 0, 1).toISOString()", "2020-01-01T00:00:00.000Z");
    check("new Date(2020, 5, 15).toISOString()", "2020-06-15T00:00:00.000Z");
    check(
        "new Date(1999, 11, 31, 23, 59, 59, 999).toISOString()",
        "1999-12-31T23:59:59.999Z",
    );
    // Omitted trailing components default to 0 (day defaults to 1).
    check("new Date(2021, 2).toISOString()", "2021-03-01T00:00:00.000Z");
    check("new Date(2021, 2, 10, 6).toISOString()", "2021-03-10T06:00:00.000Z");
}

#[test]
fn date_construct_two_digit_year_maps_to_1900s() {
    // MakeFullYear: a year in 0..=99 in the multi-arg form maps to 1900+yy.
    check("new Date(99, 0, 1).getUTCFullYear()", "1999");
    check("new Date(0, 0, 1).getUTCFullYear()", "1900");
    check("new Date(50, 0, 1).getUTCFullYear()", "1950");
    check("new Date(99, 5, 15, 10, 30, 0).toISOString()", "1999-06-15T10:30:00.000Z");
    // 100 and up are taken literally (NOT offset).
    check("new Date(100, 0, 1).getUTCFullYear()", "100");
    check("new Date(1999, 0, 1).getUTCFullYear()", "1999");
}

#[test]
fn date_time_magnitude_beyond_max_is_invalid() {
    // The valid time window is ±8.64e15 ms; one past the edge is Invalid Date.
    check("new Date(8640000000000000).getTime()", "8640000000000000");
    check("String(new Date(8640000000000001).getTime())", "NaN");
    check("new Date(-8640000000000000).getTime()", "-8640000000000000");
    check("String(new Date(-8640000000000001).getTime())", "NaN");
    check("String(new Date(1e16).getTime())", "NaN");
    // The same clip applies to multi-arg / Date.UTC construction.
    check("Date.UTC(275760, 0, 1)", "8639977881600000");
    check("String(Date.UTC(275761, 0, 1))", "NaN");
    check("Date.UTC(-271821, 3, 20)", "-8640000000000000");
    check("String(Date.UTC(-271821, 3, 19))", "NaN");
    // Out-of-range components must not panic — huge values clip to NaN.
    check("String(Date.UTC(2020, 1e15, 1))", "NaN");
    check("String(Date.UTC(2020, 0, 1, 1e15))", "NaN");
}

#[test]
fn date_construct_no_args_is_epoch_zero() {
    // Deterministic sandbox: no wall clock, so `new Date()` is epoch 0.
    check("new Date().getTime()", "0");
    check("new Date().toISOString()", "1970-01-01T00:00:00.000Z");
}

// ============================================================================
// DATE — statics (UTC / parse / now)
// ============================================================================

#[test]
fn date_static_utc() {
    check("Date.UTC(2020, 0, 1)", "1577836800000"); // 0-based month
    check("Date.UTC(1970, 0, 1)", "0");
    check("Date.UTC(2020, 1, 29)", "1582934400000"); // leap day
    check("Date.UTC(1999, 11, 31, 23, 59, 59, 999)", "946684799999");
    check("new Date(Date.UTC(2000, 0, 1)).toISOString()", "2000-01-01T00:00:00.000Z");
    // Defaults: missing day -> 1, missing time -> 0.
    check("Date.UTC(2021, 5)", "1622505600000");
}

#[test]
fn date_utc_nonfinite_component_is_nan() {
    // Any non-finite (ToNumber-coerced) component makes the whole result NaN —
    // it must NOT silently become 0 via an `as i64` cast.
    check("String(Date.UTC(2020, NaN, 1))", "NaN");
    check("String(Date.UTC(NaN, 0, 1))", "NaN");
    check("String(Date.UTC(2020, 0, Infinity))", "NaN");
    check("String(Date.UTC(2020, 0, -Infinity))", "NaN");
    check("String(Date.UTC(2020, 'x', 1))", "NaN"); // ToNumber('x') -> NaN
    // A finite call is unaffected.
    check("Date.UTC(2020, 0, 1)", "1577836800000");
    // Two-digit year is mapped to 1900+yy here too (MakeFullYear).
    check("new Date(Date.UTC(99, 0, 1)).getUTCFullYear()", "1999");
}

#[test]
fn date_static_parse() {
    check("Date.parse('1970-01-01T00:00:00.000Z')", "0");
    check("Date.parse('2020-01-01T00:00:00.000Z')", "1577836800000");
    check("Date.parse('2020-06-15T13:45:30.123Z')", "1592228730123");
    check("Date.parse('2020-01-01')", "1577836800000");
    // Parse failure -> NaN.
    check("String(Date.parse('not a date'))", "NaN");
    check("String(Date.parse('2021-13-01'))", "NaN"); // month out of range
}

#[test]
fn date_parse_iso_time_components_are_range_checked() {
    // Node rejects out-of-range clock fields rather than rolling them over.
    check("String(Date.parse('2020-01-01T25:00:00Z'))", "NaN"); // hour > 24
    check("String(Date.parse('2020-01-01T23:60:00Z'))", "NaN"); // minute > 59
    check("String(Date.parse('2020-01-01T23:59:60Z'))", "NaN"); // second > 59
    // Hour 24 is the one over-23 value JS allows, but ONLY as 24:00:00.000.
    check("Date.parse('2020-01-01T24:00:00Z')", "1577923200000");
    check("String(Date.parse('2020-01-01T24:00:01Z'))", "NaN"); // 24 with non-zero sec -> NaN
    check("String(Date.parse('2020-01-01T24:01:00Z'))", "NaN"); // 24 with non-zero min -> NaN
    // In-range fields still parse.
    check("Date.parse('2020-01-01T23:59:59Z')", "1577923199000");
}

#[test]
fn date_parse_bare_far_future_integer_is_nan() {
    // A bare integer string is read as a calendar year; one that overflows the
    // representable Date window is Invalid Date, not a garbage far-future date.
    check("String(Date.parse('1234567890123'))", "NaN");
    check("String(new Date('1234567890123').getTime())", "NaN");
    check("String(Date.parse('1234567'))", "NaN"); // year 1,234,567 is out of range
    check("String(Date.parse('275761'))", "NaN"); // one past the max representable year
    // Years that DO fit still parse (verified against Node).
    check("Date.parse('2020')", "1577836800000");
    check("Date.parse('123456')", "3833727840000000");
    check("Date.parse('275760')", "8639977881600000"); // the boundary year
}

#[test]
fn date_static_now_is_zero() {
    check("Date.now()", "0");
    check("typeof Date.now()", "number");
}

#[test]
fn date_parse_round_trips_through_iso() {
    // parse(toISOString) is the identity on the epoch ms.
    check(
        "const d = new Date(1592228730123); Date.parse(d.toISOString()) === d.getTime()",
        "true",
    );
}

// ============================================================================
// DATE — getUTC* accessors
// ============================================================================

#[test]
fn date_get_utc_components_full() {
    let d = "const d = new Date('2020-06-15T13:45:30.123Z');";
    check(&format!("{d} d.getUTCFullYear()"), "2020");
    check(&format!("{d} d.getUTCMonth()"), "5"); // June, 0-based
    check(&format!("{d} d.getUTCDate()"), "15");
    check(&format!("{d} d.getUTCHours()"), "13");
    check(&format!("{d} d.getUTCMinutes()"), "45");
    check(&format!("{d} d.getUTCSeconds()"), "30");
    check(&format!("{d} d.getUTCMilliseconds()"), "123");
}

#[test]
fn date_get_utc_day_of_week() {
    // 0 = Sunday. 1970-01-01 was a Thursday (4).
    check("new Date(0).getUTCDay()", "4");
    check("new Date('2020-12-19T00:00:00Z').getUTCDay()", "6"); // Saturday
    check("new Date('2020-12-20T00:00:00Z').getUTCDay()", "0"); // Sunday
    check("new Date('2020-12-21T00:00:00Z').getUTCDay()", "1"); // Monday
}

#[test]
fn date_local_getters_alias_utc() {
    // No timezone in the sandbox: local getters equal the UTC getters.
    let d = "const d = new Date('2020-06-15T13:45:30.123Z');";
    check(&format!("{d} d.getFullYear() === d.getUTCFullYear()"), "true");
    check(&format!("{d} d.getMonth() === d.getUTCMonth()"), "true");
    check(&format!("{d} d.getDate() === d.getUTCDate()"), "true");
    check(&format!("{d} d.getHours() === d.getUTCHours()"), "true");
    check(&format!("{d} d.getMinutes() === d.getUTCMinutes()"), "true");
    check(&format!("{d} d.getSeconds() === d.getUTCSeconds()"), "true");
    check(&format!("{d} d.getDay() === d.getUTCDay()"), "true");
    check(&format!("{d} d.getTimezoneOffset()"), "0");
}

#[test]
fn date_components_before_epoch() {
    // Negative epoch ms: components borrow correctly across the boundary.
    check("new Date(-1).getUTCFullYear()", "1969");
    check("new Date(-1).getUTCMonth()", "11"); // December
    check("new Date(-1).getUTCDate()", "31");
    check("new Date(-1).getUTCHours()", "23");
    check("new Date(-1).getUTCMinutes()", "59");
    check("new Date(-1).getUTCSeconds()", "59");
    check("new Date(-1).getUTCMilliseconds()", "999");
    check("new Date(-1).toISOString()", "1969-12-31T23:59:59.999Z");
}

#[test]
fn date_components_far_future() {
    check("new Date('2099-12-31T23:59:59.999Z').getUTCFullYear()", "2099");
    check("new Date('2099-12-31T23:59:59.999Z').toISOString()", "2099-12-31T23:59:59.999Z");
    check("new Date('2099-12-31T23:59:59.999Z').getUTCMonth()", "11");
}

// ============================================================================
// DATE — arithmetic / coercion / comparison
// ============================================================================

#[test]
fn date_subtraction_yields_millis() {
    check("const a = new Date(1000); const b = new Date(3000); b - a", "2000");
    check("const a = new Date(1000); const b = new Date(3000); a - b", "-2000");
    check(
        "new Date('2020-01-02T00:00:00Z') - new Date('2020-01-01T00:00:00Z')",
        "86400000", // one day in ms
    );
    check("new Date(5000) - new Date(5000)", "0");
}

#[test]
fn date_numeric_coercion() {
    check("Number(new Date(5000))", "5000");
    check("+new Date(123)", "123");
    check("typeof Number(new Date(0))", "number");
    check("typeof +new Date(7)", "number");
    // The multiplicative operators force numeric coercion (epoch ms).
    check("new Date(1000) * 2", "2000");
    check("new Date(2000) / 2", "1000");
    check("new Date(3000) - 1000", "2000");
    // NOTE: binary `+` uses STRING coercion for a Date in real JS too (Date's
    // default ToPrimitive hint is "string"); zapcode concatenates the ISO string —
    // exercised under the documented `toString`-returns-ISO behavior, not asserted
    // here to avoid pinning the human-readable-vs-ISO divergence.
}

#[test]
fn date_relational_comparison() {
    check("new Date(1000) < new Date(2000)", "true");
    check("new Date(2000) > new Date(1000)", "true");
    check("new Date(1000) <= new Date(1000)", "true");
    check("new Date(3000) >= new Date(3000)", "true");
    check("new Date(1000) < new Date(1000)", "false");
}

// ============================================================================
// DATE — Invalid Date
// ============================================================================

#[test]
fn date_invalid_time_is_nan() {
    check("String(new Date('garbage').getTime())", "NaN");
    check("String(new Date('not a date').valueOf())", "NaN");
    check("String(new Date('2021-13-01').getTime())", "NaN"); // month out of 1..12 range
    // NOTE: day 30 in February is NOT rejected — both Node and zapcode accept the
    // raw day-of-month and roll it into a valid epoch (no per-month day validation).
    check("new Date('2020-02-30').getTime()", "1583020800000");
}

#[test]
fn date_invalid_getters_are_nan() {
    let d = "const d = new Date('garbage');";
    check(&format!("{d} String(d.getUTCFullYear())"), "NaN");
    check(&format!("{d} String(d.getUTCMonth())"), "NaN");
    check(&format!("{d} String(d.getUTCDate())"), "NaN");
    check(&format!("{d} String(d.getUTCHours())"), "NaN");
    check(&format!("{d} String(d.getTimezoneOffset())"), "NaN");
}

#[test]
fn date_invalid_formats_to_invalid_date() {
    check("String(new Date('garbage'))", "Invalid Date");
    check("String(new Date('2021-13-01'))", "Invalid Date");
    // Still a Date instance.
    check("new Date('garbage') instanceof Date", "true");
    check("new Date(NaN) instanceof Date", "true");
}

// ============================================================================
// DATE — instanceof / formatting
// ============================================================================

#[test]
fn date_instanceof() {
    check("new Date(0) instanceof Date", "true");
    check("new Date() instanceof Date", "true");
    check("new Date('2020-01-01') instanceof Date", "true");
    check("new Date(0) instanceof Object", "true");
    check("typeof new Date(0)", "object");
    check("typeof new Date(0).getTime", "function");
}

#[test]
fn date_to_iso_string() {
    check("new Date(0).toISOString()", "1970-01-01T00:00:00.000Z");
    check("new Date(1577836800000).toISOString()", "2020-01-01T00:00:00.000Z");
    check("new Date(1234567890123).toISOString()", "2009-02-13T23:31:30.123Z");
    // toISOString always renders fixed-width fields with millisecond precision.
    // (Constructed from raw epoch ms, not `Date.UTC(5,…)`, since a two-digit year
    // in the multi-arg / Date.UTC form is mapped to 1900+yy by MakeFullYear.)
    check("new Date(-62009366400000).toISOString()", "0005-01-01T00:00:00.000Z");
}

#[test]
fn date_to_json_equals_to_iso_string() {
    check("new Date(0).toJSON()", "1970-01-01T00:00:00.000Z");
    check("new Date(12345).toJSON() === new Date(12345).toISOString()", "true");
    check("new Date(1577836800000).toJSON()", "2020-01-01T00:00:00.000Z");
}

#[test]
fn date_to_string_returns_iso_documented_divergence() {
    // DIVERGENCE (documented): `Date#toString` and `Date#toDateString` return the
    // ISO-8601 string here, NOT Node's human-readable form
    // (`"Thu Jan 01 1970 00:00:00 GMT+0000 (Coordinated Universal Time)"` /
    // `"Thu Jan 01 1970"`). Asserting zapcode's actual ISO behavior.
    check("new Date(0).toString()", "1970-01-01T00:00:00.000Z");
    check("new Date(0).toDateString()", "1970-01-01T00:00:00.000Z");
}

// ============================================================================
// JSON.stringify — scalars & containers
// ============================================================================

#[test]
fn json_stringify_scalars() {
    check("JSON.stringify(42)", "42");
    check("JSON.stringify(-7)", "-7");
    check("JSON.stringify(0)", "0");
    check("JSON.stringify(3.14)", "3.14");
    check("JSON.stringify(1.5)", "1.5");
    check("JSON.stringify(-0)", "0"); // negative zero serializes as 0
    check("JSON.stringify('hello')", "\"hello\"");
    check("JSON.stringify('')", "\"\"");
    check("JSON.stringify(true)", "true");
    check("JSON.stringify(false)", "false");
    check("JSON.stringify(null)", "null");
    check("String(JSON.stringify(undefined))", "undefined"); // returns the value undefined
}

#[test]
fn json_stringify_objects() {
    check(
        "JSON.stringify({a:1, b:'two', c:true, d:null})",
        "{\"a\":1,\"b\":\"two\",\"c\":true,\"d\":null}",
    );
    check("JSON.stringify({})", "{}");
    check("JSON.stringify({single: 1})", "{\"single\":1}");
    // Key insertion order is preserved.
    check("JSON.stringify({z:1, a:2, m:3})", "{\"z\":1,\"a\":2,\"m\":3}");
}

#[test]
fn json_stringify_arrays() {
    check("JSON.stringify([1, 'x', true, null])", "[1,\"x\",true,null]");
    check("JSON.stringify([])", "[]");
    check("JSON.stringify([1])", "[1]");
    check("JSON.stringify([[1],[2,3]])", "[[1],[2,3]]");
    check("JSON.stringify([1, [2, [3, [4]]]])", "[1,[2,[3,[4]]]]");
}

#[test]
fn json_stringify_nested_mixed() {
    check(
        "JSON.stringify({a:{b:[1,2]}, c:[{d:3}]})",
        "{\"a\":{\"b\":[1,2]},\"c\":[{\"d\":3}]}",
    );
    check(
        "JSON.stringify({items:[{id:1,tags:['a','b']},{id:2,tags:[]}]})",
        "{\"items\":[{\"id\":1,\"tags\":[\"a\",\"b\"]},{\"id\":2,\"tags\":[]}]}",
    );
}

// ============================================================================
// JSON.stringify — dropping & holes
// ============================================================================

#[test]
fn json_stringify_drops_undefined_and_functions_in_objects() {
    check("JSON.stringify({a:1, b:undefined, c:2})", "{\"a\":1,\"c\":2}");
    check("JSON.stringify({a:1, f:function(){}, b:2})", "{\"a\":1,\"b\":2}");
    check("JSON.stringify({a:undefined})", "{}");
    check("JSON.stringify({f(){}})", "{}");
}

#[test]
fn json_stringify_array_holes_become_null() {
    check("JSON.stringify([1, undefined, 2])", "[1,null,2]");
    check("JSON.stringify([1, function(){}, 2])", "[1,null,2]");
    check("JSON.stringify([undefined])", "[null]");
    check("JSON.stringify([undefined, undefined])", "[null,null]");
}

// ============================================================================
// JSON.stringify — escaping
// ============================================================================

#[test]
fn json_stringify_escapes_quotes_and_backslash() {
    check("JSON.stringify('say \"hi\"')", "\"say \\\"hi\\\"\"");
    check("JSON.stringify('back\\\\slash')", "\"back\\\\slash\"");
    check("JSON.stringify('a\"b\\\\c')", "\"a\\\"b\\\\c\"");
}

#[test]
fn json_stringify_escapes_whitespace_control() {
    check("JSON.stringify('line1\\nline2')", "\"line1\\nline2\"");
    check("JSON.stringify('tab\\there')", "\"tab\\there\"");
    check("JSON.stringify('\\r')", "\"\\r\"");
    check("JSON.stringify('\\b\\f')", "\"\\b\\f\""); // backspace + form feed
}

#[test]
fn json_stringify_escapes_other_control_chars_as_unicode() {
    // Control chars < 0x20 without a short escape use \u00XX (lowercase hex here).
    check("JSON.stringify('\\u0001\\u0002')", "\"\\u0001\\u0002\"");
    check("JSON.stringify('\\u001f')", "\"\\u001f\"");
    check("JSON.stringify('\\u0000')", "\"\\u0000\"");
}

#[test]
fn json_stringify_passes_through_non_ascii() {
    // Non-ASCII printable characters are emitted verbatim (not escaped).
    check("JSON.stringify('café')", "\"café\"");
    check("JSON.stringify('日本')", "\"日本\"");
}

// ============================================================================
// JSON.stringify — NaN / Infinity
// ============================================================================

#[test]
fn json_stringify_nan_and_infinity_become_null() {
    check("JSON.stringify(NaN)", "null");
    check("JSON.stringify(Infinity)", "null");
    check("JSON.stringify(-Infinity)", "null");
    check("JSON.stringify({a:NaN, b:Infinity, c:-Infinity})", "{\"a\":null,\"b\":null,\"c\":null}");
    check("JSON.stringify([NaN, Infinity, -Infinity])", "[null,null,null]");
}

// ============================================================================
// JSON.stringify — indentation
// ============================================================================

#[test]
fn json_stringify_numeric_indent() {
    check("JSON.stringify({a:1, b:2}, null, 2)", "{\n  \"a\": 1,\n  \"b\": 2\n}");
    check("JSON.stringify({a:1}, null, 1)", "{\n \"a\": 1\n}");
    check("JSON.stringify([1, 2], null, 2)", "[\n  1,\n  2\n]");
}

#[test]
fn json_stringify_string_indent() {
    check("JSON.stringify({a:1}, null, '\\t')", "{\n\t\"a\": 1\n}");
    check("JSON.stringify([1, 2, 3], null, '--')", "[\n--1,\n--2,\n--3\n]");
}

#[test]
fn json_stringify_nested_indent() {
    check(
        "JSON.stringify({a:{b:1}}, null, 2)",
        "{\n  \"a\": {\n    \"b\": 1\n  }\n}",
    );
    check(
        "JSON.stringify([{a:1}], null, 2)",
        "[\n  {\n    \"a\": 1\n  }\n]",
    );
    check(
        "JSON.stringify([1, [2]], null, 2)",
        "[\n  1,\n  [\n    2\n  ]\n]",
    );
    check(
        "JSON.stringify({a:[1,2], b:{c:3}}, null, '\\t')",
        "{\n\t\"a\": [\n\t\t1,\n\t\t2\n\t],\n\t\"b\": {\n\t\t\"c\": 3\n\t}\n}",
    );
}

#[test]
fn json_stringify_indent_edge_cases() {
    // 0 / empty-string indent -> compact (no pretty printing).
    check("JSON.stringify({a:1}, null, 0)", "{\"a\":1}");
    check("JSON.stringify({a:1}, null, '')", "{\"a\":1}");
    // Numeric indent clamps to a max of 10 spaces (matches Node).
    check("JSON.stringify({a:1}, null, 20)", "{\n          \"a\": 1\n}");
    // Empty container with indent stays on one line.
    check("JSON.stringify({}, null, 2)", "{}");
    check("JSON.stringify([], null, 2)", "[]");
}

// ============================================================================
// JSON.stringify — array replacer (whitelist)
// ============================================================================

#[test]
fn json_stringify_array_replacer_whitelist() {
    check("JSON.stringify({a:1, b:2, c:3}, ['a', 'c'])", "{\"a\":1,\"c\":3}");
    check("JSON.stringify({x:1, y:2}, ['x'])", "{\"x\":1}");
    // A key not present in the object is simply skipped.
    check("JSON.stringify({a:1}, ['a', 'missing'])", "{\"a\":1}");
    // Empty whitelist drops all keys.
    check("JSON.stringify({a:1, b:2}, [])", "{}");
    // No key matches -> empty object.
    check("JSON.stringify({a:1, b:2, c:3}, [2])", "{}");
}

#[test]
fn json_stringify_array_replacer_numeric_keys() {
    // Numeric replacer entries are coerced to string keys.
    check("JSON.stringify({1:'x', 2:'y'}, [1])", "{\"1\":\"x\"}");
    check("JSON.stringify({1:'x', 2:'y'}, [1, 2])", "{\"1\":\"x\",\"2\":\"y\"}");
}

// ============================================================================
// JSON.stringify — Date / Map / Set / Error
// ============================================================================

#[test]
fn json_stringify_date_to_iso() {
    check("JSON.stringify(new Date(0))", "\"1970-01-01T00:00:00.000Z\"");
    check("JSON.stringify({when: new Date(0)})", "{\"when\":\"1970-01-01T00:00:00.000Z\"}");
    check("JSON.stringify([new Date(0)])", "[\"1970-01-01T00:00:00.000Z\"]");
    check(
        "JSON.stringify({d: new Date(1577836800000)})",
        "{\"d\":\"2020-01-01T00:00:00.000Z\"}",
    );
}

#[test]
fn json_stringify_map_set_error_become_empty_object() {
    // Map/Set/Error have no enumerable own data properties, so serialize as {}.
    check("JSON.stringify(new Map([['a', 1]]))", "{}");
    check("JSON.stringify(new Set([1, 2, 3]))", "{}");
    check("JSON.stringify(new Error('boom'))", "{}");
    check(
        "JSON.stringify({m: new Map(), s: new Set(), e: new Error('y')})",
        "{\"m\":{},\"s\":{},\"e\":{}}",
    );
}

// ============================================================================
// JSON.parse — primitives, containers, escapes, whitespace
// ============================================================================

#[test]
fn json_parse_primitives() {
    check("JSON.parse('42')", "42");
    check("JSON.parse('-42')", "-42");
    check("JSON.parse('3.14159')", "3.14159");
    check("JSON.parse('1.5e3')", "1500");
    check("JSON.parse('true')", "true");
    check("JSON.parse('false')", "false");
    check("JSON.parse('\"str\"')", "str");
    check("String(JSON.parse('null'))", "null");
}

#[test]
fn json_parse_objects_and_arrays() {
    check("JSON.parse('{\"a\":1,\"b\":[2,3]}').a", "1");
    check("JSON.parse('{\"a\":1,\"b\":[2,3]}').b[1]", "3");
    check("JSON.parse('[1,2,3]').length", "3");
    check("JSON.parse('[1,2,3]')[0]", "1");
    check("JSON.parse('[true,false,null]')[0]", "true");
    check("JSON.parse('{\"a\":null}').a === null", "true");
}

#[test]
fn json_parse_string_escapes() {
    // Escapes inside parsed strings are decoded by a single left-to-right scan.
    // The guest source must carry a JSON escape (backslash-n), so the Rust
    // literal needs `\\n` — `\n` would be a real newline, which JSON rejects as
    // a control character (matching Node's "Bad control character" SyntaxError).
    check("JSON.parse('\"a\\\\nb\"')", "a\nb"); // backslash-n -> real newline
    check("JSON.parse('\"tab\\\\tend\"')", "tab\tend");
    // A doubled backslash followed by `n` is a literal backslash then `n`, NOT a
    // newline (the old order-dependent .replace chain got this wrong).
    check("JSON.parse('\"a\\\\\\\\nb\"')", "a\\nb");
    check("JSON.parse('\"\\\\u0041\\\\u0042\"')", "AB"); // \uXXXX escapes -> "AB"
    check("JSON.parse('\"\\\\u0041\"').length", "1"); // single A is one char
    check("JSON.parse('\"\\\\/\\\\b\\\\f\\\\r\"')", "/\u{0008}\u{000C}\r"); // \/ \b \f \r
    check("JSON.parse('\"quote\\\\\"in\"')", "quote\"in"); // escaped quote
}

#[test]
fn json_parse_ignores_surrounding_whitespace() {
    check("JSON.parse('  {\"a\":1}  ').a", "1");
    check("JSON.parse('[ 1 , 2 , 3 ]')[2]", "3");
    check("JSON.parse('\\n\\t[1]\\n')[0]", "1");
}

#[test]
fn json_parse_deeply_nested() {
    check("JSON.parse('{\"x\":{\"y\":{\"z\":[1,2,3]}}}').x.y.z[2]", "3");
    check("JSON.parse('{\"nested\":{\"deep\":{\"v\":7}}}').nested.deep.v", "7");
    check("JSON.parse('[[[[42]]]]')[0][0][0][0]", "42");
}

// ============================================================================
// JSON — round-trips & integration
// ============================================================================

#[test]
fn json_round_trip_stringify_parse() {
    check("JSON.stringify(JSON.parse(JSON.stringify({x:[1,{y:2}]})))", "{\"x\":[1,{\"y\":2}]}");
    check(
        "const orig = {n:1, list:[true, 'a', null]}; JSON.stringify(JSON.parse(JSON.stringify(orig)))",
        "{\"n\":1,\"list\":[true,\"a\",null]}",
    );
    check(
        "JSON.stringify(JSON.parse('{\"a\":{\"b\":[1,2,3]},\"c\":\"text\"}'))",
        "{\"a\":{\"b\":[1,2,3]},\"c\":\"text\"}",
    );
}

#[test]
fn json_parse_then_mutate() {
    check(
        "const o = JSON.parse('{\"items\":[1,2]}'); o.items.push(3); o.items.join(',')",
        "1,2,3",
    );
    check(
        "const o = JSON.parse('{\"a\":1}'); o.b = 2; JSON.stringify(o)",
        "{\"a\":1,\"b\":2}",
    );
}

#[test]
fn json_date_round_trips_through_string() {
    // A Date serializes to ISO and parses back to the same epoch ms.
    check(
        "const s = JSON.stringify(new Date(1577836800000)); new Date(JSON.parse(s)).getTime()",
        "1577836800000",
    );
}

#[test]
fn json_globals_shape() {
    check("typeof JSON", "object");
    check("typeof JSON.stringify", "function");
    check("typeof JSON.parse", "function");
}

// ============================================================================
// Documented divergences (asserting ACTUAL behavior, not the JS answer)
// ============================================================================

#[test]
fn json_function_replacer_invoked() {
    // A FUNCTION replacer transforms each entry. (Array replacers also work; see
    // above.)
    check(
        "JSON.stringify({a:1, b:2}, (k, v) => typeof v === 'number' ? v * 10 : v)",
        "{\"a\":10,\"b\":20}",
    );
}

#[test]
fn json_reviver_invoked() {
    // A reviver (parse 2nd arg) is invoked per entry.
    check(
        "JSON.parse('{\"a\":1}', (k, v) => typeof v === 'number' ? v + 100 : v).a",
        "101",
    );
}

#[test]
fn json_user_to_json_on_plain_object_honored() {
    // A user-defined `toJSON()` on a PLAIN object is called (as is Date#toJSON —
    // see json_stringify_date_to_iso).
    check("JSON.stringify({toJSON(){ return 'custom'; }})", "\"custom\"");
    check("JSON.stringify({x: {toJSON(){ return 5; }}})", "{\"x\":5}");
}
