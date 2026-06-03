//! Conformance breadth: destructuring.
//!
//! Covers array destructuring (nested, holes-skip, defaults, rest, swap), object
//! destructuring (rename, nested, defaults, rest, computed-key), destructuring in
//! function parameters and in `for…of`, and mixed/deep patterns.
//!
//! Where zapcode matches real Node, the real-JS answer is asserted. zapcode has a
//! cluster of DOCUMENTED, KNOWN destructuring divergences (see STRESS-PASS-BUGS.md,
//! cluster D / H3 and the `conformance_functions.rs` notes); those cases are pinned
//! to zapcode's *actual* behavior with an explicit `// DIVERGENCE` comment and the
//! real-JS answer noted, never asserted as correct. The known divergences are:
//!   * Nested ARRAY patterns inside a var-decl (`[[a],[b]]`, `[a,[b,c]]`) bind the
//!     inner names to `undefined` (object-nesting of objects works; an array nested
//!     inside an object var-decl pattern also fails to bind).
//!   * ARRAY-pattern defaults (`[a = 1] = []`) are not applied (object-pattern
//!     defaults DO work, including in var-decls and as nested object props).
//!   * Computed object keys built from a *variable* (`{[k]: v}`) don't bind (a
//!     *string-literal* computed key `{['a']: v}` works, since it's static).
//!   * Destructured-PARAMETER defaults (`function f({a = 1} = {})`) yield `NaN`
//!     (pattern-with-default *parameters* don't apply their defaults).
//!   * Destructuring ASSIGNMENT to pre-existing / member targets
//!     (`[a, b] = [b, a]`, `({x: o.p} = …)`) is a CompileError (only declaration-
//!     form destructuring is supported).
//! Object-pattern var-decl destructuring (rename / nested-object / defaults / rest /
//! default-expression laziness) and array rest / hole-skip are fully correct, as is
//! all destructuring in `for…of` bindings — including nested array patterns there.

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, ZapcodeRun};

/// Run a program and return the completion value rendered as a JS string.
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

/// True iff the program errors (rather than completing) — used to pin the
/// unsupported destructuring-assignment forms without asserting a brittle message.
/// The "unsupported assignment target" error is raised lazily at run time (the
/// `ZapcodeRun::new` constructor succeeds), so this checks the run result.
fn errors_out(code: &str) -> bool {
    match ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        Vec::new(),
        ResourceLimits::default(),
    ) {
        Err(_) => true,
        Ok(run) => run.run(Vec::new()).is_err(),
    }
}

// ============================================================================
// Array destructuring — basics
// ============================================================================

#[test]
fn array_basic_binding() {
    assert_eq!(run_str("const [a, b] = [1, 2]; a + b"), "3");
    assert_eq!(run_str("const [a, b, c] = [10, 20, 30]; `${a},${b},${c}`"), "10,20,30");
    assert_eq!(run_str("const [x] = ['only']; x"), "only");
    assert_eq!(run_str("let [a, b] = [4, 5]; a = a + 1; `${a},${b}`"), "5,5");
}

#[test]
fn array_more_targets_than_values_bind_undefined() {
    // Surplus pattern slots bind to undefined, exactly like JS.
    assert_eq!(run_str("const [a, b, c] = [1, 2]; String(c)"), "undefined");
    assert_eq!(run_str("const [a, b] = [1]; `${a},${String(b)}`"), "1,undefined");
    assert_eq!(run_str("const [a] = []; String(a)"), "undefined");
}

#[test]
fn array_fewer_targets_than_values_ignores_extra() {
    assert_eq!(run_str("const [a, b] = [1, 2, 3, 4]; a + b"), "3");
    assert_eq!(run_str("const [first] = [9, 8, 7]; first"), "9");
}

#[test]
fn array_destructure_from_function_return() {
    assert_eq!(run_str("function pair(){ return [1, 2]; } const [a, b] = pair(); a + b"), "3");
    assert_eq!(
        run_str("const make = () => ['k', 'v']; const [k, v] = make(); `${k}=${v}`"),
        "k=v"
    );
}

// ============================================================================
// Array destructuring — holes (elisions) skip elements
// ============================================================================

#[test]
fn array_holes_skip_elements() {
    assert_eq!(run_str("const [a, , c] = [1, 2, 3]; `${a},${c}`"), "1,3");
    assert_eq!(run_str("const [, b] = [1, 2]; b"), "2");
    assert_eq!(run_str("const [, , third] = [1, 2, 3]; third"), "3");
    assert_eq!(run_str("const [a, , , d] = [1, 2, 3, 4]; `${a},${d}`"), "1,4");
}

#[test]
fn array_holes_then_rest() {
    assert_eq!(
        run_str("const [, , ...rest] = [1, 2, 3, 4, 5]; JSON.stringify(rest)"),
        "[3,4,5]"
    );
    assert_eq!(run_str("const [, second, ...rest] = [1, 2, 3]; `${second}|${rest.join(',')}`"), "2|3");
}

// ============================================================================
// Array destructuring — rest element
// ============================================================================

#[test]
fn array_rest_basic() {
    assert_eq!(run_str("const [a, ...rest] = [1, 2, 3]; `${a}:${rest.join(',')}`"), "1:2,3");
    assert_eq!(run_str("const [...all] = [1, 2, 3]; all.join('-')"), "1-2-3");
    assert_eq!(run_str("const [first, ...others] = ['x', 'y', 'z']; others.join('-')"), "y-z");
}

#[test]
fn array_rest_is_a_real_array() {
    // The rest binding must be a genuine Array (length, methods, isArray).
    assert_eq!(run_str("const [a, ...rest] = [1, 2, 3, 4]; rest.length"), "3");
    assert_eq!(run_str("const [a, ...rest] = [1, 2, 3, 4]; Array.isArray(rest)"), "true");
    assert_eq!(run_str("const [a, ...rest] = [1, 2, 3, 4]; rest.map(x => x * 2).join(',')"), "4,6,8");
}

#[test]
fn array_rest_empty_when_exhausted() {
    assert_eq!(run_str("const [a, b, ...rest] = [1, 2]; rest.length"), "0");
    assert_eq!(run_str("const [a, b, ...rest] = [1, 2]; JSON.stringify(rest)"), "[]");
    assert_eq!(run_str("const [a, b, ...rest] = [1, 2]; Array.isArray(rest)"), "true");
}

#[test]
fn array_rest_collects_remaining_in_order() {
    assert_eq!(
        run_str("const arr = [1, 2, 3, 4, 5]; const [first, ...tail] = arr; JSON.stringify([first, tail])"),
        "[1,[2,3,4,5]]"
    );
    assert_eq!(
        run_str("const [a, b, ...rest] = ['p', 'q', 'r', 's']; JSON.stringify(rest)"),
        r#"["r","s"]"#
    );
}

// ============================================================================
// Array destructuring — swap idiom
// ============================================================================

#[test]
fn array_swap_via_temp_works() {
    // The textbook swap uses destructuring assignment; that form is a documented
    // CompileError here (see array_destructuring_assignment_unsupported). The
    // temp-variable swap — the equivalent semantics — works fine.
    assert_eq!(run_str("let a = 1, b = 2; const t = a; a = b; b = t; `${a},${b}`"), "2,1");
}

#[test]
fn array_swap_into_fresh_bindings_works() {
    // You CAN "swap" when both targets are freshly-declared (a new binding-form
    // destructuring, not assignment to existing names).
    assert_eq!(run_str("let a = 1, b = 2; const [x, y] = [b, a]; `${x},${y}`"), "2,1");
}

// ============================================================================
// Object destructuring — basics & shorthand
// ============================================================================

#[test]
fn object_basic_shorthand() {
    assert_eq!(run_str("const {a, b} = {a: 1, b: 2}; a + b"), "3");
    assert_eq!(run_str("const {x} = {x: 42}; x"), "42");
    assert_eq!(run_str("const {a, b, c} = {a: 1, b: 2, c: 3}; `${a},${b},${c}`"), "1,2,3");
}

#[test]
fn object_missing_key_binds_undefined() {
    assert_eq!(run_str("const {a, z} = {a: 1}; String(z)"), "undefined");
    assert_eq!(run_str("const {missing} = {}; typeof missing"), "undefined");
}

#[test]
fn object_extra_keys_ignored() {
    assert_eq!(run_str("const {a} = {a: 1, b: 2, c: 3}; a"), "1");
}

// ============================================================================
// Object destructuring — rename (alias)
// ============================================================================

#[test]
fn object_rename_basic() {
    assert_eq!(run_str("const {a: x} = {a: 5}; x"), "5");
    assert_eq!(run_str("const {a: x, b: y} = {a: 1, b: 2}; `${x},${y}`"), "1,2");
    assert_eq!(run_str("const {a: x, b: y} = {a: 3, b: 4}; x * y"), "12");
}

#[test]
fn object_rename_original_name_not_bound() {
    // After renaming `a` to `x`, the name `a` is NOT introduced.
    assert_eq!(run_str("const {a: x} = {a: 5}; typeof a"), "undefined");
}

// ============================================================================
// Object destructuring — defaults
// ============================================================================

#[test]
fn object_default_applied_when_missing() {
    assert_eq!(run_str("const {z = 42} = {}; z"), "42");
    assert_eq!(run_str("const {a = 1, b = 2} = {a: 10}; `${a},${b}`"), "10,2");
}

#[test]
fn object_default_skipped_when_present() {
    assert_eq!(run_str("const {z = 42} = {z: 7}; z"), "7");
    assert_eq!(run_str("const {a = 1} = {a: 0}; a"), "0"); // present falsy value still wins
}

#[test]
fn object_rename_with_default() {
    assert_eq!(run_str("const {a: x = 10} = {}; x"), "10");
    assert_eq!(run_str("const {a: x = 10} = {a: 3}; x"), "3");
    assert_eq!(run_str("const {a: x = 1, b: y = 2} = {b: 20}; `${x},${y}`"), "1,20");
}

#[test]
fn object_default_referencing_earlier_binding() {
    // A later default may reference an earlier-bound name in the same pattern.
    assert_eq!(run_str("const {a = 5, b = a * 2} = {a: 3}; `${a},${b}`"), "3,6");
    assert_eq!(run_str("const {a = 5, b = a * 2} = {}; `${a},${b}`"), "5,10");
}

#[test]
fn object_default_expression_is_lazy() {
    // The default expression must only evaluate when the property is absent.
    assert_eq!(
        run_str("let called = 0; function d(){ called++; return 9; } const {x = d()} = {x: 5}; `${x},${called}`"),
        "5,0"
    );
    assert_eq!(
        run_str("let called = 0; function d(){ called++; return 9; } const {x = d()} = {}; `${x},${called}`"),
        "9,1"
    );
}

// ============================================================================
// Object destructuring — rest
// ============================================================================

#[test]
fn object_rest_basic() {
    assert_eq!(run_str("const {a, ...rest} = {a: 1, b: 2, c: 3}; JSON.stringify([a, rest])"), r#"[1,{"b":2,"c":3}]"#);
    assert_eq!(run_str("const {x, ...y} = {x: 1, y: 2}; JSON.stringify(y)"), r#"{"y":2}"#);
}

#[test]
fn object_rest_multiple_extracted_first() {
    assert_eq!(
        run_str("const {a, b, ...rest} = {a: 1, b: 2, c: 3, d: 4}; JSON.stringify(rest)"),
        r#"{"c":3,"d":4}"#
    );
}

#[test]
fn object_rest_empty_when_all_extracted() {
    assert_eq!(run_str("const {a, b, ...rest} = {a: 1, b: 2}; JSON.stringify(rest)"), "{}");
}

#[test]
fn object_rest_with_rename() {
    assert_eq!(
        run_str("const {a: x, ...rest} = {a: 1, b: 2, c: 3}; JSON.stringify([x, rest])"),
        r#"[1,{"b":2,"c":3}]"#
    );
}

// ============================================================================
// Object destructuring — nested (object-in-object)
// ============================================================================

#[test]
fn object_nested_one_level() {
    assert_eq!(run_str("const {a: {b}} = {a: {b: 99}}; b"), "99");
    assert_eq!(run_str("const {p: {q}} = {p: {q: 7}}; q"), "7");
}

#[test]
fn object_nested_deep() {
    assert_eq!(run_str("const {a: {b: {c}}} = {a: {b: {c: 5}}}; c"), "5");
    assert_eq!(
        run_str("const {a: {b: {c: {d}}}} = {a: {b: {c: {d: 'deep'}}}}; d"),
        "deep"
    );
}

#[test]
fn object_nested_with_rename_and_siblings() {
    assert_eq!(
        run_str("const {user: {name: n, age: a}} = {user: {name: 'Ann', age: 30}}; `${n}:${a}`"),
        "Ann:30"
    );
    assert_eq!(
        run_str("const {meta: {id}, value} = {meta: {id: 1}, value: 2}; `${id},${value}`"),
        "1,2"
    );
}

#[test]
fn object_extract_array_value_whole() {
    // Binding a property whose VALUE is an array (without destructuring into it)
    // works: you get the array itself.
    assert_eq!(run_str("const {list} = {list: [1, 2, 3]}; list.join('-')"), "1-2-3");
    assert_eq!(run_str("const {items: xs} = {items: [9, 8]}; xs.length + ':' + xs[0]"), "2:9");
}

// ============================================================================
// Object destructuring — computed keys
// ============================================================================

#[test]
fn object_computed_key_string_literal() {
    // A computed key that is a string LITERAL is resolved statically and works.
    assert_eq!(run_str("const {['a']: v} = {a: 99}; v"), "99");
    assert_eq!(run_str("const {['x']: a, ['y']: b} = {x: 1, y: 2}; `${a},${b}`"), "1,2");
}

#[test]
fn object_computed_key_from_variable_documented_divergence() {
    // DIVERGENCE: a computed key built from a *variable* does not bind — the value
    // comes out `undefined`. (JS: 5.) A string-LITERAL computed key works
    // (object_computed_key_string_literal). Pinned to zapcode's actual behavior.
    assert_eq!(run_str("const k = 'b'; const {[k]: v} = {b: 5}; String(v)"), "undefined"); // JS: 5
    assert_eq!(run_str("const key = 'x'; const {[key]: v} = {x: 42}; String(v)"), "undefined"); // JS: 42
}

// ============================================================================
// Destructuring in function parameters — object patterns
// ============================================================================

#[test]
fn param_object_shorthand() {
    assert_eq!(run_str("function f({a, b}){ return a + b; } f({a: 10, b: 20})"), "30");
    assert_eq!(run_str("const f = ({a}) => a; f({a: 42})"), "42");
    assert_eq!(run_str("const f = ({x, y}) => x + y; f({x: 1, y: 2})"), "3");
}

#[test]
fn param_object_rename() {
    assert_eq!(run_str("const f = ({a: x}) => x; f({a: 5})"), "5");
    assert_eq!(run_str("function f({a: x, b: y}){ return x * y; } f({a: 3, b: 4})"), "12");
}

#[test]
fn param_object_nested() {
    assert_eq!(run_str("const f = ({a: {b}}) => b; f({a: {b: 99}})"), "99");
    assert_eq!(run_str("function f({a: {b}}){ return b; } f({a: {b: 7}})"), "7");
    assert_eq!(
        run_str("function f({user: {name}}){ return name; } f({user: {name: 'Z'}})"),
        "Z"
    );
}

#[test]
fn param_object_rest() {
    assert_eq!(
        run_str("function f({a, ...rest}){ return JSON.stringify([a, rest]); } f({a: 1, b: 2, c: 3})"),
        r#"[1,{"b":2,"c":3}]"#
    );
}

#[test]
fn param_object_alongside_positional() {
    assert_eq!(
        run_str("function f(prefix, {a, b}){ return prefix + (a + b); } f('=', {a: 2, b: 3})"),
        "=5"
    );
}

// ============================================================================
// Destructuring in function parameters — array patterns
// ============================================================================

#[test]
fn param_array_basic() {
    assert_eq!(run_str("const f = ([x, y]) => x + y; f([3, 4])"), "7");
    assert_eq!(run_str("function f([a, b]){ return a * b; } f([5, 6])"), "30");
}

#[test]
fn param_array_rest() {
    assert_eq!(
        run_str("function f([a, ...rest]){ return a + ':' + rest.join(','); } f([1, 2, 3])"),
        "1:2,3"
    );
    assert_eq!(
        run_str("const f = ([head, ...tail]) => tail.length; f([1, 2, 3, 4])"),
        "3"
    );
}

#[test]
fn param_array_with_holes() {
    assert_eq!(run_str("function f([a, , c]){ return `${a},${c}`; } f([1, 2, 3])"), "1,3");
}

// ============================================================================
// Destructuring in function parameters — the .map([k, v]) idiom
// ============================================================================

#[test]
fn param_array_in_map_callback() {
    assert_eq!(
        run_str("Object.entries({x: 1, y: 2}).map(([k, v]) => k + v).join(',')"),
        "x1,y2"
    );
    assert_eq!(
        run_str("JSON.stringify(Object.fromEntries(Object.entries({x: 1, y: 2}).map(([k, v]) => [k, v * 10])))"),
        r#"{"x":10,"y":20}"#
    );
}

#[test]
fn param_object_in_callbacks() {
    assert_eq!(
        run_str("[{n: 1}, {n: 2}, {n: 3}].map(({n}) => n * 10).join(',')"),
        "10,20,30"
    );
    assert_eq!(
        run_str("[{v: 5}, {v: 1}, {v: 9}].filter(({v}) => v > 2).map(({v}) => v).join(',')"),
        "5,9"
    );
    assert_eq!(
        run_str("[{x: 1}, {x: 2}, {x: 3}].reduce((s, {x}) => s + x, 0)"),
        "6"
    );
}

// ============================================================================
// Destructuring in for…of bindings — array patterns
// ============================================================================

#[test]
fn for_of_array_pair() {
    assert_eq!(
        run_str("const out = []; for (const [k, v] of [['a', 1], ['b', 2]]) out.push(k + v); out.join(',')"),
        "a1,b2"
    );
    assert_eq!(
        run_str("const out = []; for (const [k, v] of Object.entries({x: 1, y: 2})) out.push(k + '=' + v); out.join(',')"),
        "x=1,y=2"
    );
}

#[test]
fn for_of_array_rest() {
    assert_eq!(
        run_str("const out = []; for (const [h, ...t] of [[1, 2, 3], [4, 5]]) out.push(h + ':' + t.join('-')); out.join(',')"),
        "1:2-3,4:5"
    );
}

#[test]
fn for_of_array_nested() {
    // Nested array patterns DO bind correctly in a for…of head (unlike a var-decl).
    assert_eq!(
        run_str("const out = []; for (const [i, [a, b]] of [[0, [1, 2]], [1, [3, 4]]]) out.push(i + ':' + a + '-' + b); out.join(',')"),
        "0:1-2,1:3-4"
    );
}

#[test]
fn for_of_array_holes() {
    assert_eq!(
        run_str("const out = []; for (const [, second] of [['a', 1], ['b', 2]]) out.push(second); out.join(',')"),
        "1,2"
    );
}

// ============================================================================
// Destructuring in for…of bindings — object patterns
// ============================================================================

#[test]
fn for_of_object_shorthand() {
    assert_eq!(
        run_str("const out = []; for (const {id} of [{id: 7}, {id: 8}]) out.push(id); out.join(',')"),
        "7,8"
    );
    assert_eq!(
        run_str("const out = []; for (const {id, name} of [{id: 1, name: 'a'}, {id: 2, name: 'b'}]) out.push(id + name); out.join(',')"),
        "1a,2b"
    );
}

#[test]
fn for_of_object_rename() {
    assert_eq!(
        run_str("const out = []; for (const {id: i, name: n} of [{id: 1, name: 'a'}]) out.push(i + n); out.join(',')"),
        "1a"
    );
}

#[test]
fn for_of_object_default() {
    assert_eq!(
        run_str("const out = []; for (const {id = 0} of [{id: 5}, {}]) out.push(id); out.join(',')"),
        "5,0"
    );
    assert_eq!(
        run_str("const out = []; for (const {id: i = -1} of [{id: 5}, {}]) out.push(i); out.join(',')"),
        "5,-1"
    );
}

#[test]
fn for_of_object_nested() {
    assert_eq!(
        run_str("const out = []; for (const {p: {q}} of [{p: {q: 1}}, {p: {q: 2}}]) out.push(q); out.join(',')"),
        "1,2"
    );
}

// ============================================================================
// Mixed / deep patterns (object containing array, array of objects, etc.)
// ============================================================================

#[test]
fn mixed_object_then_object_rename_chain() {
    assert_eq!(
        run_str("const {data: {result: {value: v}}} = {data: {result: {value: 100}}}; v"),
        "100"
    );
}

#[test]
fn mixed_deep_object_with_rest_and_default() {
    assert_eq!(
        run_str("const {a: {b = 5, ...rest}} = {a: {b: 1, c: 2, d: 3}}; JSON.stringify([b, rest])"),
        r#"[1,{"c":2,"d":3}]"#
    );
    assert_eq!(
        run_str("const {a: {b = 5, ...rest}} = {a: {c: 2}}; JSON.stringify([b, rest])"),
        r#"[5,{"c":2}]"#
    );
}

#[test]
fn mixed_realistic_record_shape() {
    // A realistic "unpack a record" shape: rename + nested object + rest siblings.
    let code = "
        const record = { id: 42, profile: { firstName: 'Ada', lastName: 'L' }, role: 'admin', tags: ['x'] };
        const { id, profile: { firstName: fn }, ...rest } = record;
        JSON.stringify([id, fn, rest]);
    ";
    assert_eq!(run_str(code), r#"[42,"Ada",{"role":"admin","tags":["x"]}]"#);
}

// ============================================================================
// DOCUMENTED DIVERGENCES — pinned to zapcode's actual behavior, never asserted
// as the real-JS answer. (See STRESS-PASS-BUGS.md cluster D / H3.)
// ============================================================================

#[test]
fn nested_array_var_decl_documented_divergence() {
    // DIVERGENCE: nested ARRAY patterns in a var-decl bind inner names to
    // `undefined` (the one-level array destructure works; nesting an array INTO an
    // array pattern does not). The for…of and parameter forms DO bind nested arrays
    // (see for_of_array_nested / mixed_param_array_of_objects), so this is specific
    // to the var-decl lowering.
    assert_eq!(run_str("const [[a], [b]] = [[1], [2]]; String(a) + ',' + String(b)"), "undefined,undefined"); // JS: 1,2
    assert_eq!(run_str("const [a, [b, c]] = [1, [2, 3]]; `${a},${String(b)},${String(c)}`"), "1,undefined,undefined"); // JS: 1,2,3
}

#[test]
fn array_inside_object_var_decl_documented_divergence() {
    // DIVERGENCE: an array pattern nested inside an OBJECT var-decl pattern does not
    // bind (the inner names come out `undefined`). (Object-in-object nesting works;
    // see object_nested_deep. And array-in-object works in for…of / params.)
    assert_eq!(run_str("const {arr: [x, y]} = {arr: [1, 2]}; String(x) + ',' + String(y)"), "undefined,undefined"); // JS: 1,2
    assert_eq!(run_str("const {p: {q: [r]}} = {p: {q: [42]}}; String(r)"), "undefined"); // JS: 42
}

#[test]
fn array_defaults_var_decl_documented_divergence() {
    // DIVERGENCE: ARRAY-pattern defaults are not applied — a missing slot stays
    // `undefined` instead of taking the default. (Object-pattern defaults DO work;
    // see object_default_applied_when_missing.)
    assert_eq!(run_str("const [a = 10, b = 20] = [1]; `${a},${String(b)}`"), "1,undefined"); // JS: 1,20
    assert_eq!(run_str("const [a = 10] = []; String(a)"), "undefined"); // JS: 10
    assert_eq!(run_str("const [a, b = a + 1] = [5]; `${a},${String(b)}`"), "5,undefined"); // JS: 5,6
}

#[test]
fn param_pattern_defaults_documented_divergence() {
    // DIVERGENCE: destructured-PARAMETER defaults (a default ON the pattern itself,
    // or defaults INSIDE a parameter pattern with the whole-arg default) yield NaN —
    // the pattern's own default and inner defaults aren't applied for the param form.
    // (Var-decl object defaults work; this is the param-default path specifically.)
    assert_eq!(run_str("function f({a = 1, b = 2} = {}){ return a + b; } f({a: 10})"), "NaN"); // JS: 12
    assert_eq!(run_str("function f({a = 1, b = 2} = {}){ return a + b; } f()"), "NaN"); // JS: 3
    assert_eq!(run_str("function f([a, b] = [7, 8]){ return a + b; } f()"), "NaN"); // JS: 15
}

#[test]
fn destructuring_assignment_to_existing_targets_unsupported() {
    // DIVERGENCE: destructuring ASSIGNMENT (to already-declared names or member
    // expressions, i.e. without a `const`/`let`/`var`) is a CompileError. Only the
    // declaration form is supported. JS accepts all of these. Pinned by compile
    // failure rather than a brittle message.
    assert!(is_compile_error("let a = 1, b = 2; [a, b] = [b, a]; a + ',' + b")); // JS swap: 2,1
    assert!(is_compile_error("let a, b; ({a, b} = {a: 1, b: 2}); a + b")); // JS: 3
    assert!(is_compile_error("const o = {}; ({x: o.p} = {x: 7}); o.p")); // JS: 7
    assert!(is_compile_error("let arr = []; [arr[0], arr[1]] = [1, 2]; JSON.stringify(arr)")); // JS: [1,2]
}
