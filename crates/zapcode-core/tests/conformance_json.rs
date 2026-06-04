//! Conformance breadth: JSON.stringify / JSON.parse.
//!
//! Serialization (nested values, undefined/function dropping, control-char &
//! quote escaping, NaN/Infinity -> null, indentation, array & date handling) and
//! parsing (objects/arrays/primitives, round-trips). A FUNCTION replacer
//! (stringify) and a reviver (parse) are invoked per entry, and a user-defined
//! `toJSON()` on a plain object is honored (as is built-in `Date#toJSON`). Array
//! replacers, string replacers, and indentation all work and match Node.

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
// stringify: scalars & containers
// ----------------------------------------------------------------------------

#[test]
fn stringify_scalars() {
    assert_eq!(run_str("JSON.stringify(42)"), "42");
    assert_eq!(run_str("JSON.stringify('hello')"), "\"hello\"");
    assert_eq!(run_str("JSON.stringify(true)"), "true");
    assert_eq!(run_str("JSON.stringify(null)"), "null");
    assert_eq!(run_str("JSON.stringify(3.14)"), "3.14");
    assert_eq!(run_str("String(JSON.stringify(undefined))"), "undefined"); // returns undefined
}

#[test]
fn stringify_objects_and_arrays() {
    assert_eq!(run_str("JSON.stringify({a:1, b:'two', c:true, d:null})"), "{\"a\":1,\"b\":\"two\",\"c\":true,\"d\":null}");
    assert_eq!(run_str("JSON.stringify([1, 'x', true, null])"), "[1,\"x\",true,null]");
    assert_eq!(run_str("JSON.stringify({a:{b:[1,2]}, c:[{d:3}]})"), "{\"a\":{\"b\":[1,2]},\"c\":[{\"d\":3}]}");
    assert_eq!(run_str("JSON.stringify({})"), "{}");
    assert_eq!(run_str("JSON.stringify([])"), "[]");
    assert_eq!(run_str("JSON.stringify([[1],[2,3]])"), "[[1],[2,3]]");
}

#[test]
fn stringify_preserves_key_insertion_order() {
    assert_eq!(run_str("JSON.stringify({z:1, a:2, m:3})"), "{\"z\":1,\"a\":2,\"m\":3}");
}

// ----------------------------------------------------------------------------
// stringify: dropping & escaping
// ----------------------------------------------------------------------------

#[test]
fn stringify_drops_undefined_and_functions_in_objects() {
    assert_eq!(run_str("JSON.stringify({a:1, b:undefined, c:2})"), "{\"a\":1,\"c\":2}");
    assert_eq!(run_str("JSON.stringify({a:1, f:function(){}, b:2})"), "{\"a\":1,\"b\":2}");
}

#[test]
fn stringify_array_holes_become_null() {
    assert_eq!(run_str("JSON.stringify([1, undefined, 2])"), "[1,null,2]");
    assert_eq!(run_str("JSON.stringify([1, function(){}, 2])"), "[1,null,2]");
}

#[test]
fn stringify_escapes_special_characters() {
    assert_eq!(run_str("JSON.stringify('line1\\nline2')"), "\"line1\\nline2\"");
    assert_eq!(run_str("JSON.stringify('tab\\there')"), "\"tab\\there\"");
    assert_eq!(run_str("JSON.stringify('say \"hi\"')"), "\"say \\\"hi\\\"\"");
    assert_eq!(run_str("JSON.stringify('back\\\\slash')"), "\"back\\\\slash\"");
    // control characters use \uXXXX escapes
    assert_eq!(run_str("JSON.stringify('\\u0001\\u0002')"), "\"\\u0001\\u0002\"");
}

#[test]
fn stringify_nan_and_infinity_become_null() {
    assert_eq!(run_str("JSON.stringify({a:NaN, b:Infinity, c:-Infinity})"), "{\"a\":null,\"b\":null,\"c\":null}");
    assert_eq!(run_str("JSON.stringify([NaN, Infinity])"), "[null,null]");
}

// ----------------------------------------------------------------------------
// stringify: indentation & array replacer
// ----------------------------------------------------------------------------

#[test]
fn stringify_with_indentation() {
    assert_eq!(run_str("JSON.stringify({a:1, b:2}, null, 2)"), "{\n  \"a\": 1,\n  \"b\": 2\n}");
    assert_eq!(run_str("JSON.stringify({a:1}, null, '\\t')"), "{\n\t\"a\": 1\n}");
    assert_eq!(run_str("JSON.stringify([1, 2], null, 2)"), "[\n  1,\n  2\n]");
}

#[test]
fn stringify_with_array_replacer_whitelist() {
    assert_eq!(run_str("JSON.stringify({a:1, b:2, c:3}, ['a', 'c'])"), "{\"a\":1,\"c\":3}");
    assert_eq!(run_str("JSON.stringify({x:1, y:2}, ['x'])"), "{\"x\":1}");
}

#[test]
fn stringify_honors_builtin_date_to_json() {
    assert_eq!(run_str("JSON.stringify({d: new Date(0)})"), "{\"d\":\"1970-01-01T00:00:00.000Z\"}");
    assert_eq!(run_str("JSON.stringify(new Date(0))"), "\"1970-01-01T00:00:00.000Z\"");
}

// ----------------------------------------------------------------------------
// parse
// ----------------------------------------------------------------------------

#[test]
fn parse_objects_arrays_primitives() {
    assert_eq!(run_str("JSON.parse('{\"a\":1,\"b\":[2,3]}').a"), "1");
    assert_eq!(run_str("JSON.parse('{\"a\":1,\"b\":[2,3]}').b[1]"), "3");
    assert_eq!(run_str("JSON.parse('[1,2,3]').length"), "3");
    assert_eq!(run_str("JSON.parse('42')"), "42");
    assert_eq!(run_str("JSON.parse('true')"), "true");
    assert_eq!(run_str("JSON.parse('\"str\"')"), "str");
    assert_eq!(run_str("String(JSON.parse('null'))"), "null");
    assert_eq!(run_str("JSON.parse('{\"nested\":{\"deep\":{\"v\":7}}}').nested.deep.v"), "7");
}

#[test]
fn parse_then_mutate() {
    assert_eq!(run_str("const o = JSON.parse('{\"items\":[1,2]}'); o.items.push(3); o.items.join(',')"), "1,2,3");
}

#[test]
fn round_trip_stringify_parse() {
    assert_eq!(run_str("JSON.stringify(JSON.parse(JSON.stringify({x:[1,{y:2}]})))"), "{\"x\":[1,{\"y\":2}]}");
    assert_eq!(run_str("const orig = {n:1, list:[true, 'a', null]}; JSON.stringify(JSON.parse(JSON.stringify(orig)))"), "{\"n\":1,\"list\":[true,\"a\",null]}");
}

// ----------------------------------------------------------------------------
// Documented I4-family gaps (asserting actual behavior, not the JS answer)
// ----------------------------------------------------------------------------

#[test]
fn function_replacer_and_reviver() {
    // A FUNCTION replacer (stringify) transforms each entry; a reviver (parse) is
    // invoked per entry. (Array replacers also work; see
    // stringify_with_array_replacer_whitelist.)
    assert_eq!(
        run_str("JSON.stringify({a:1, b:2}, (k, v) => typeof v === 'number' ? v * 10 : v)"),
        "{\"a\":10,\"b\":20}"
    );
    assert_eq!(
        run_str("JSON.parse('{\"a\":1}', (k, v) => typeof v === 'number' ? v + 100 : v).a"),
        "101"
    );
    // A replacer returning undefined drops the property.
    assert_eq!(
        run_str("JSON.stringify({a:1, b:2}, (k, v) => k === 'b' ? undefined : v)"),
        "{\"a\":1}"
    );
    // A reviver returning undefined drops the property.
    assert_eq!(
        run_str("JSON.stringify(JSON.parse('{\"a\":1,\"b\":2}', (k, v) => k === 'b' ? undefined : v))"),
        "{\"a\":1}"
    );
}

#[test]
fn user_to_json_on_plain_object_honored() {
    // A user-defined `toJSON()` on a PLAIN object is called; its return value is
    // serialized in place of the object (matching built-in `Date#toJSON`, see
    // stringify_honors_builtin_date_to_json).
    assert_eq!(run_str("JSON.stringify({toJSON(){ return 'custom'; }})"), "\"custom\"");
    assert_eq!(run_str("JSON.stringify({x: {toJSON(){ return 5; }}})"), "{\"x\":5}");
}

#[test]
fn stringify_keeps_user_double_underscore_keys() {
    // JSON.stringify must NOT drop user keys that merely start with `__` — only
    // the interpreter's exact reserved internal markers are hidden. Node:
    // JSON.stringify({__v:1,a:2}) === '{"__v":1,"a":2}'.
    assert_eq!(run_str("JSON.stringify({__v:1,a:2})"), "{\"__v\":1,\"a\":2}");
    assert_eq!(
        run_str("JSON.stringify({a:{__id__:3},__meta__:1})"),
        "{\"a\":{\"__id__\":3},\"__meta__\":1}"
    );
    assert_eq!(
        run_str("JSON.stringify({__proto_like__:'kept',normal:1})"),
        "{\"__proto_like__\":\"kept\",\"normal\":1}"
    );
    // A class instance still hides its internal brand keys.
    assert_eq!(
        run_str("class C { a = 1; b = 2; } JSON.stringify(new C())"),
        "{\"a\":1,\"b\":2}"
    );
}
