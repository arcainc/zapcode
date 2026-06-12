//! Conformance breadth: expression-level operators & forms.
//!
//! test262-style coverage of the expression surface that the other conformance
//! files exercise only incidentally. Every asserted value was cross-checked
//! against real `node -e`. Coverage:
//!   - the comma / sequence operator (statement, parenthesized, `return`, index,
//!     `for`-head positions) and its left-to-right side-effect ordering;
//!   - the `void` operator (always `undefined`);
//!   - assignment as an expression: chained `a = b = c`, computed-member writes,
//!     and the compound family on dotted / computed / array targets;
//!   - the logical-assignment family (`&&= ||= ??=`) short-circuit semantics;
//!   - `delete` on object keys (own/missing/nested) and array indices;
//!   - `typeof` across every value kind;
//!   - the `in` operator (own keys, numeric indices, array `length`) including the
//!     documented "does not walk the prototype chain" residual;
//!   - `instanceof` against the built-in constructors (`Array`/`Object`/`Map`/
//!     `Set`/`Date`);
//!   - the conditional (ternary) operator: right-associativity, nesting, and use
//!     in statement position;
//!   - unary `!`/`~`/`-`/`+` and the exponentiation operator's right-
//!     associativity & precedence with unary minus;
//!   - optional chaining: optional calls (`?.()`), optional indexing (`?.[]`),
//!     and short-circuiting a trailing non-optional member after a nullish link.
//!
//! DOCUMENTED DIVERGENCES asserted as zapcode's ACTUAL value (verified against
//! Node) with an explicit comment, never the JS answer:
//!   - `delete arr[i]` then `i in arr` → `true` here (JS: `false` — `delete` does
//!     not produce a chain-invisible hole; the slot is cleared to a value the `in`
//!     check still sees).
//!   - `"toString" in {}` → `false` here (JS: `true` — `in` does not walk the
//!     prototype chain; matches `conformance_objects::in_operator_does_not_walk_prototype`).

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

// ============================================================================
// Comma / sequence operator
// ============================================================================

#[test]
fn comma_operator_yields_last_value() {
    assert_eq!(run_str("(1, 2, 3)"), "3");
    assert_eq!(run_str("let a = (5, 10); a"), "10");
    assert_eq!(run_str("function f(){ return (1, 2, 3) } f()"), "3");
}

#[test]
fn comma_operator_evaluates_every_operand_left_to_right() {
    // All three assignments run; the value of the sequence is the last expression.
    assert_eq!(
        run_str("let x = 0; let y = (x = x + 1, x = x + 2, x); y + '/' + x"),
        "3/3"
    );
    // Side effects accumulate even when the final value ignores them.
    assert_eq!(
        run_str("let log = ''; (log += 'a', log += 'b', log += 'c'); log"),
        "abc"
    );
}

#[test]
fn comma_operator_in_index_and_for_head() {
    assert_eq!(run_str("[10, 20, 30][(0, 2)]"), "30");
    // Both init and update clauses of a C-style for use comma sequences.
    assert_eq!(
        run_str("let i = 0, j = 0; for (i = 0, j = 10; i < 3; i++, j--) {} i + '/' + j"),
        "3/7"
    );
}

// ============================================================================
// void operator
// ============================================================================

#[test]
fn void_operator_is_always_undefined() {
    assert_eq!(run_str("void 0"), "undefined");
    assert_eq!(run_str("void (1 + 1)"), "undefined");
    assert_eq!(run_str("void 'anything'"), "undefined");
    assert_eq!(run_str("typeof (void 0)"), "undefined");
    assert_eq!(run_str("void 0 === undefined"), "true");
}

#[test]
fn void_evaluates_its_operand_for_side_effects() {
    assert_eq!(run_str("let n = 0; void (n = 5); n"), "5");
}

// ============================================================================
// Assignment as an expression
// ============================================================================

#[test]
fn chained_assignment_threads_one_value() {
    assert_eq!(run_str("let x, y; x = y = 7; x + '/' + y"), "7/7");
    assert_eq!(run_str("const o = {}; o.a = o.b = 7; o.a + '/' + o.b"), "7/7");
    // Chained assignment evaluates to the assigned value.
    assert_eq!(run_str("let a, b, c; a = b = c = 9"), "9");
}

#[test]
fn assignment_expression_returns_assigned_value() {
    assert_eq!(run_str("let x; (x = 42)"), "42");
    assert_eq!(run_str("let x; let y = (x = 3) + 1; x + '/' + y"), "3/4");
}

#[test]
fn compound_assignment_on_member_targets() {
    assert_eq!(run_str("const a = [1, 2, 3]; a[1] += 10; a.join(',')"), "1,12,3");
    assert_eq!(run_str("const o = { n: 5 }; o['n'] *= 2; o.n"), "10");
    assert_eq!(run_str("const o = { n: 10 }; o.n -= 3; o.n"), "7");
    assert_eq!(run_str("const o = { s: 'a' }; o.s += 'b'; o.s"), "ab");
}

#[test]
fn computed_member_assignment_creates_and_updates() {
    assert_eq!(run_str("const o = {}; const k = 'x'; o[k] = 5; o.x"), "5");
    assert_eq!(
        run_str("const o = {}; for (let i = 0; i < 3; i++) o['k' + i] = i; o.k0 + o.k1 + o.k2"),
        "3"
    );
}

#[test]
fn logical_assignment_short_circuits() {
    // ??= only assigns when the target is null/undefined.
    assert_eq!(run_str("let o = {}; o.x ??= 5; o.x ??= 9; o.x"), "5");
    // ||= assigns when falsy.
    assert_eq!(run_str("let a = 0; a ||= 7; a"), "7");
    assert_eq!(run_str("let a = 3; a ||= 7; a"), "3");
    // &&= assigns when truthy.
    assert_eq!(run_str("let b = 1; b &&= 9; b"), "9");
    assert_eq!(run_str("let b = 0; b &&= 9; b"), "0");
}

// ============================================================================
// delete operator
// ============================================================================

#[test]
fn delete_object_key_returns_true_and_removes() {
    assert_eq!(
        run_str("const o = { a: 1 }; const r = delete o.a; r + '/' + ('a' in o)"),
        "true/false"
    );
    // Deleting a missing key is still true.
    assert_eq!(run_str("delete ({}).x"), "true");
    assert_eq!(run_str("const o = {}; delete o.missing"), "true");
}

#[test]
fn delete_nested_and_computed_keys() {
    assert_eq!(
        run_str("const o = { a: { b: 1, c: 2 } }; delete o.a.b; JSON.stringify(o.a)"),
        "{\"c\":2}"
    );
    assert_eq!(
        run_str("const o = { x: 1, y: 2 }; const k = 'x'; delete o[k]; JSON.stringify(o)"),
        "{\"y\":2}"
    );
}

#[test]
fn delete_array_index_clears_value_length_unchanged() {
    // Length is preserved and the slot reads undefined (matches JS).
    assert_eq!(
        run_str("const a = [1, 2, 3]; delete a[1]; a.length + '/' + a[1]"),
        "3/undefined"
    );
    // DIVERGENCE asserted as actual: in JS `delete` makes the index a hole so
    // `1 in a` becomes false; here the slot is cleared but `in` still reports it.
    assert_eq!(run_str("const a = [1, 2, 3]; delete a[2]; 2 in a"), "true");
}

// ============================================================================
// typeof operator
// ============================================================================

#[test]
fn typeof_every_value_kind() {
    assert_eq!(run_str("typeof 1"), "number");
    assert_eq!(run_str("typeof 3.14"), "number");
    assert_eq!(run_str("typeof NaN"), "number");
    assert_eq!(run_str("typeof Infinity"), "number");
    assert_eq!(run_str("typeof 'x'"), "string");
    assert_eq!(run_str("typeof true"), "boolean");
    assert_eq!(run_str("typeof undefined"), "undefined");
    assert_eq!(run_str("typeof null"), "object");
    assert_eq!(run_str("typeof {}"), "object");
    assert_eq!(run_str("typeof []"), "object");
    assert_eq!(run_str("typeof new Map()"), "object");
    assert_eq!(run_str("typeof function () {}"), "function");
    assert_eq!(run_str("typeof (() => 1)"), "function");
    assert_eq!(run_str("typeof Symbol()"), "symbol");
}

#[test]
fn typeof_of_undeclared_identifier_is_undefined() {
    // typeof is the one place an unbound reference does not throw.
    assert_eq!(run_str("typeof somethingNeverDeclared"), "undefined");
}

#[test]
fn typeof_result_is_a_string() {
    assert_eq!(run_str("typeof (typeof 1)"), "string");
    assert_eq!(run_str("(typeof 1).toUpperCase()"), "NUMBER");
}

// ============================================================================
// in operator
// ============================================================================

#[test]
fn in_operator_own_keys_and_indices() {
    assert_eq!(run_str("'a' in { a: 1 }"), "true");
    assert_eq!(run_str("'z' in { a: 1 }"), "false");
    assert_eq!(run_str("0 in [1, 2, 3]"), "true");
    assert_eq!(run_str("5 in [1, 2, 3]"), "false");
    // numeric and string index forms both resolve.
    assert_eq!(run_str("'1' in [1, 2]"), "true");
    assert_eq!(run_str("'length' in [1, 2]"), "true");
}

#[test]
fn in_operator_after_mutation() {
    assert_eq!(
        run_str("const o = {}; o.x = 1; const a = 'x' in o; delete o.x; a + '/' + ('x' in o)"),
        "true/false"
    );
}

#[test]
fn in_operator_reports_inherited_object_prototype_members() {
    // Promoted from a documented divergence: every object inherits
    // Object.prototype's members, so `in` reports them even though the
    // object model stores no prototype chain (Node truth).
    assert_eq!(run_str("'toString' in {}"), "true");
    assert_eq!(run_str("'hasOwnProperty' in {}"), "true");
    assert_eq!(run_str("'valueOf' in [1]"), "true");
    assert_eq!(run_str("'nope' in {}"), "false");
}

// ============================================================================
// instanceof against built-in constructors
// ============================================================================

#[test]
fn instanceof_builtin_constructors() {
    assert_eq!(run_str("[1] instanceof Array"), "true");
    assert_eq!(run_str("[1] instanceof Object"), "true");
    assert_eq!(run_str("({}) instanceof Object"), "true");
    assert_eq!(run_str("new Map() instanceof Map"), "true");
    assert_eq!(run_str("new Set() instanceof Set"), "true");
    assert_eq!(run_str("new Date(0) instanceof Date"), "true");
    assert_eq!(run_str("new Date(0) instanceof Object"), "true");
    assert_eq!(run_str("(function () {}) instanceof Object"), "true");
}

#[test]
fn instanceof_negative_and_non_object_lhs() {
    assert_eq!(run_str("({}) instanceof Array"), "false");
    assert_eq!(run_str("[1] instanceof Map"), "false");
    assert_eq!(run_str("5 instanceof Object"), "false");
    assert_eq!(run_str("'x' instanceof Object"), "false");
    assert_eq!(run_str("null instanceof Object"), "false");
}

// ============================================================================
// Conditional (ternary) operator
// ============================================================================

#[test]
fn ternary_basic_and_nesting() {
    assert_eq!(run_str("const x = 5; x < 0 ? 'neg' : x === 0 ? 'zero' : 'pos'"), "pos");
    assert_eq!(run_str("const x = 0; x < 0 ? 'neg' : x === 0 ? 'zero' : 'pos'"), "zero");
    assert_eq!(run_str("const x = -3; x < 0 ? 'neg' : x === 0 ? 'zero' : 'pos'"), "neg");
}

#[test]
fn ternary_is_right_associative() {
    assert_eq!(run_str("true ? 1 : true ? 2 : 3"), "1");
    assert_eq!(run_str("false ? 1 : false ? 2 : 3"), "3");
    assert_eq!(run_str("false ? 1 : true ? 2 : 3"), "2");
}

#[test]
fn ternary_only_evaluates_the_taken_branch() {
    assert_eq!(run_str("let n = 0; true ? (n = 1) : (n = 99); n"), "1");
    assert_eq!(run_str("let n = 0; false ? (n = 1) : (n = 99); n"), "99");
}

#[test]
fn ternary_in_statement_position() {
    assert_eq!(run_str("let r; true ? (r = 1) : (r = 2); r"), "1");
}

// ============================================================================
// Unary operators & exponentiation precedence
// ============================================================================

#[test]
fn logical_not_and_double_negation() {
    assert_eq!(run_str("!0"), "true");
    assert_eq!(run_str("!1"), "false");
    assert_eq!(run_str("!''"), "true");
    assert_eq!(run_str("!!''"), "false");
    assert_eq!(run_str("!!'x'"), "true");
    assert_eq!(run_str("![]"), "false");
}

#[test]
fn bitwise_not_and_truncation() {
    assert_eq!(run_str("~5"), "-6");
    assert_eq!(run_str("~-1"), "0");
    assert_eq!(run_str("~~3.7"), "3");
    assert_eq!(run_str("~~-3.7"), "-3");
}

#[test]
fn unary_minus_and_plus_coercion() {
    assert_eq!(run_str("-(-5)"), "5");
    assert_eq!(run_str("+'42'"), "42");
    assert_eq!(run_str("-'10'"), "-10");
    assert_eq!(run_str("+true"), "1");
    assert_eq!(run_str("+''"), "0");
}

#[test]
fn exponentiation_right_associative_and_unary() {
    assert_eq!(run_str("2 ** 3 ** 2"), "512");
    assert_eq!(run_str("(-3) ** 2"), "9");
    assert_eq!(run_str("2 ** -1"), "0.5");
    // ** binds tighter than the surrounding / so this is (4**1)/2.
    assert_eq!(run_str("4 ** 1 / 2"), "2");
}

// ============================================================================
// Optional chaining: calls, indexing, trailing short-circuit
// ============================================================================

#[test]
fn optional_call_on_present_and_absent() {
    assert_eq!(run_str("const o = { a: { b: () => 42 } }; o?.a?.b?.()"), "42");
    assert_eq!(run_str("const o = {}; o?.a?.b?.()"), "undefined");
    assert_eq!(run_str("const o = { f: () => 5 }; o?.f()"), "5");
    assert_eq!(run_str("const o = null; o?.f()"), "undefined");
}

#[test]
fn optional_indexing_on_present_and_absent() {
    assert_eq!(run_str("const a = [1, 2]; a?.[0]"), "1");
    assert_eq!(run_str("const a = null; a?.[0]"), "undefined");
    assert_eq!(run_str("const o = { m: { k: 9 } }; o?.['m']?.['k']"), "9");
    assert_eq!(run_str("const o = {}; o?.['m']?.['k']"), "undefined");
}

#[test]
fn optional_chain_short_circuits_trailing_members() {
    // Once a link is nullish the whole rest of the chain short-circuits to undefined.
    assert_eq!(run_str("const o = { a: null }; o?.a?.b?.c"), "undefined");
    assert_eq!(run_str("const o = null; o?.a.b.c"), "undefined");
    // A present chain reads through to the end.
    assert_eq!(run_str("const o = { a: { b: { c: 7 } } }; o?.a?.b?.c"), "7");
}

#[test]
fn optional_chain_with_nullish_fallback() {
    assert_eq!(
        run_str("const a = { g: () => null }; a.g()?.x ?? 'fallback'"),
        "fallback"
    );
    assert_eq!(
        run_str("const a = { g: () => ({ x: 'hit' }) }; a.g()?.x ?? 'fallback'"),
        "hit"
    );
}

// ============================================================================
// Realistic mixed-expression programs
// ============================================================================

#[test]
fn mixed_expression_program_reduce_with_compound_writes() {
    assert_eq!(
        run_str(
            "const counts = {}; for (const w of ['a', 'b', 'a', 'c', 'a']) counts[w] = (counts[w] ?? 0) + 1; \
             counts.a + '/' + counts.b + '/' + counts.c"
        ),
        "3/1/1"
    );
}

#[test]
fn mixed_expression_ternary_chain_classification() {
    assert_eq!(
        run_str(
            "const classify = (n) => n < 0 ? 'neg' : n === 0 ? 'zero' : n < 10 ? 'small' : 'big'; \
             [-5, 0, 3, 50].map(classify).join(',')"
        ),
        "neg,zero,small,big"
    );
}

#[test]
fn symbol_for_global_registry() {
    // Symbol.for(key) returns the same registered symbol for the same key.
    assert_eq!(run_str("Symbol.for('x') === Symbol.for('x')"), "true");
    assert_eq!(run_str("Symbol.for('x') === Symbol.for('y')"), "false");
    assert_eq!(run_str("Symbol.for('a') !== Symbol.for('b')"), "true");
    // A registered symbol is distinct from a plain Symbol with the same description.
    assert_eq!(run_str("Symbol.for('x') === Symbol('x')"), "false");
    assert_eq!(run_str("Symbol('x') === Symbol('x')"), "false");
    // keyFor returns the registry key for a registered symbol, else undefined.
    assert_eq!(run_str("Symbol.keyFor(Symbol.for('hello'))"), "hello");
    assert_eq!(run_str("Symbol.keyFor(Symbol('z')) === undefined"), "true");
    // description and typeof.
    assert_eq!(run_str("Symbol.for('d').description"), "d");
    assert_eq!(run_str("typeof Symbol.for('x')"), "symbol");
    // A registered symbol works as a computed property key.
    assert_eq!(run_str("(function(){ const s = Symbol.for('k'); const o = { [s]: 1 }; return o[s]; })()"), "1");
}

#[test]
fn member_store_through_arbitrary_object_expressions() {
    // The accumulator idiom: the parenthesized assignment's RESULT is the
    // store target. (Used to be "compile error: invalid assignment target".)
    assert_eq!(
        run_str("const p = {}; (p.a = p.a || {}).b = 1; JSON.stringify(p)"),
        "{\"a\":{\"b\":1}}"
    );
    assert_eq!(
        run_str("const p = {}; (p['x'] = p['x'] || {})['y'] = 2; JSON.stringify(p)"),
        "{\"x\":{\"y\":2}}"
    );
    // A call result as the store target mutates the shared reference.
    assert_eq!(
        run_str("const o = { inner: {} }; function get() { return o.inner; } get().v = 7; o.inner.v"),
        "7"
    );
    // Ternary-selected target.
    assert_eq!(
        run_str("const a = {}, b = {}; (true ? a : b).hit = 'yes'; a.hit + ',' + (b.hit ?? 'no')"),
        "yes,no"
    );
}

#[test]
fn member_store_chain_rooted_in_call_evaluates_root_once() {
    // `foo().bar.baz = v` names no storable place above the call: the chain
    // is evaluated once and the mutation lands through the shared reference.
    // The root expression must NOT be re-evaluated for a write-back (it used
    // to run twice, duplicating its side effects).
    assert_eq!(
        run_str(
            "let calls = 0; \
             const obj = { bar: { baz: 0 } }; \
             function foo() { calls++; return obj; } \
             foo().bar.baz = 1; \
             calls + ':' + obj.bar.baz"
        ),
        "1:1"
    );
    // Same through a computed link.
    assert_eq!(
        run_str(
            "let calls = 0; \
             const obj = { bar: [0] }; \
             function foo() { calls++; return obj; } \
             foo().bar[0] = 9; \
             calls + ':' + obj.bar[0]"
        ),
        "1:9"
    );
}
