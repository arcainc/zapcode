//! Conformance breadth: Errors & exceptions.
//!
//! This suite exercises the error/exception surface of the zapcode interpreter
//! the way a language conformance suite would: construction of the built-in
//! `Error` family (`Error`, `TypeError`, `RangeError`, `ReferenceError`,
//! `SyntaxError`, `AggregateError`), their `name`/`message`/`stack` shape,
//! stringification, `instanceof` (including ancestor matching via a user
//! subclass chain), and the full `try`/`catch`/`finally` control flow:
//! `throw` of Error and non-Error values, rethrow / identity preservation,
//! nested `try`, optional catch binding, catch-parameter scoping, propagation
//! through nested function calls, and the fact that *runtime* errors caught in
//! a `catch` are real `Error` objects (`e.name`, `e.message`,
//! `e instanceof Error`).
//!
//! Every assertion below has been verified against the interpreter's actual
//! output and, where it matches, against real Node semantics. Several DOCUMENTED
//! divergences from real JS are pinned to zapcode's *actual* behavior and called
//! out inline with a `DIVERGENCE:` comment so the suite stays green and honest:
//!
//!   * Reading an unbound identifier yields `undefined` (no `ReferenceError`);
//!     calling an unbound identifier throws a `TypeError` ("undefined is not a
//!     function"), not a real-JS `ReferenceError` ("x is not defined").
//!   * A user `class X extends Error` does NOT establish `instanceof Error`
//!     (the built-in subtypes do). `super(message)` is not auto-propagated to
//!     `this.message`; set it explicitly.
//!   * `new Error(undefined).message` is the string `"undefined"` (real JS: `""`).
//!
//! try/catch/finally completion semantics now match Node: a `finally` runs on
//! every way of leaving its `try`/`catch` (normal, return, break, continue,
//! throw), a `finally` with no `catch` re-propagates a pending exception, and an
//! abrupt completion *inside* the finally (return/throw) supersedes the body's.

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
// Construction: name / message
// ============================================================================

#[test]
fn error_base_name_and_message() {
    assert_eq!(run_str("new Error('boom').name"), "Error");
    assert_eq!(run_str("new Error('boom').message"), "boom");
    assert_eq!(run_str("new Error('').message"), "");
    assert_eq!(run_str("new Error().name"), "Error");
    assert_eq!(run_str("new Error().message"), "");
}

#[test]
fn subtype_names() {
    assert_eq!(run_str("new TypeError('t').name"), "TypeError");
    assert_eq!(run_str("new RangeError('r').name"), "RangeError");
    assert_eq!(run_str("new ReferenceError('ref').name"), "ReferenceError");
    assert_eq!(run_str("new SyntaxError('syn').name"), "SyntaxError");
    assert_eq!(run_str("new AggregateError([], 'agg').name"), "AggregateError");
}

#[test]
fn subtype_names_in_one_array() {
    assert_eq!(
        run_str(
            "[new Error('').name, new TypeError('').name, new RangeError('').name, \
             new ReferenceError('').name, new SyntaxError('').name, \
             new AggregateError([],'').name].join('|')"
        ),
        "Error|TypeError|RangeError|ReferenceError|SyntaxError|AggregateError"
    );
}

#[test]
fn subtype_messages() {
    assert_eq!(run_str("new TypeError('t').message"), "t");
    assert_eq!(run_str("new RangeError('r').message"), "r");
    assert_eq!(run_str("new ReferenceError('ref').message"), "ref");
    assert_eq!(run_str("new SyntaxError('syn').message"), "syn");
}

#[test]
fn message_coerces_non_string_args() {
    // The message argument is coerced to a string at construction (matches Node
    // for these cases).
    assert_eq!(run_str("new Error(42).message"), "42");
    assert_eq!(run_str("new Error(true).message"), "true");
    assert_eq!(run_str("new Error({}).message"), "[object Object]");
    // null -> "null" (matches Node: String(null)).
    assert_eq!(run_str("new Error(null).message"), "null");
    // DIVERGENCE: real Node gives "" for an explicit `undefined` message; zapcode
    // stringifies it to "undefined".
    assert_eq!(run_str("new Error(undefined).message"), "undefined");
}

// ============================================================================
// Construction: stack
// ============================================================================

#[test]
fn stack_is_a_string() {
    assert_eq!(run_str("typeof new Error('x').stack"), "string");
    assert_eq!(run_str("typeof new TypeError('x').stack"), "string");
    assert_eq!(run_str("typeof new RangeError('x').stack"), "string");
}

#[test]
fn stack_first_line_is_name_colon_message() {
    // zapcode's stack begins with the "Name: message" header (as Node's does).
    assert_eq!(run_str("new Error('boom').stack"), "Error: boom");
    assert_eq!(run_str("new TypeError('boom').stack"), "TypeError: boom");
    assert_eq!(run_str("new RangeError('r').stack"), "RangeError: r");
}

// ============================================================================
// Stringification: String() / template / concat
// ============================================================================

#[test]
fn string_of_error_is_name_colon_message() {
    assert_eq!(run_str("String(new Error('boom'))"), "Error: boom");
    assert_eq!(run_str("String(new TypeError('msg'))"), "TypeError: msg");
    assert_eq!(run_str("String(new RangeError('r'))"), "RangeError: r");
    assert_eq!(run_str("String(new ReferenceError('x'))"), "ReferenceError: x");
    assert_eq!(run_str("String(new SyntaxError('y'))"), "SyntaxError: y");
}

#[test]
fn string_of_error_without_message_is_just_name() {
    assert_eq!(run_str("String(new Error())"), "Error");
    assert_eq!(run_str("String(new Error(''))"), "Error");
    assert_eq!(run_str("String(new TypeError(''))"), "TypeError");
    assert_eq!(run_str("String(new RangeError())"), "RangeError");
}

#[test]
fn error_in_template_and_concat() {
    assert_eq!(run_str("'' + new Error('cat')"), "Error: cat");
    assert_eq!(run_str("`${new TypeError('boom')}`"), "TypeError: boom");
    assert_eq!(run_str("'<' + new RangeError('r') + '>'"), "<RangeError: r>");
}

#[test]
fn string_of_aggregate_error() {
    assert_eq!(run_str("String(new AggregateError([], 'oops'))"), "AggregateError: oops");
    assert_eq!(run_str("String(new AggregateError([]))"), "AggregateError");
}

// ============================================================================
// typeof / Object brand
// ============================================================================

#[test]
fn typeof_error_is_object() {
    assert_eq!(run_str("typeof new Error('x')"), "object");
    assert_eq!(run_str("typeof new TypeError('x')"), "object");
    assert_eq!(run_str("typeof new AggregateError([], 'x')"), "object");
}

#[test]
fn error_constructors_are_functions() {
    assert_eq!(run_str("typeof Error"), "function");
    assert_eq!(run_str("typeof TypeError"), "function");
    assert_eq!(run_str("typeof RangeError"), "function");
    assert_eq!(run_str("typeof ReferenceError"), "function");
    assert_eq!(run_str("typeof SyntaxError"), "function");
    assert_eq!(run_str("typeof AggregateError"), "function");
}

// ============================================================================
// instanceof — base, subtype, Object
// ============================================================================

#[test]
fn error_instanceof_self_and_object() {
    assert_eq!(run_str("new Error('x') instanceof Error"), "true");
    assert_eq!(run_str("new Error('x') instanceof Object"), "true");
}

#[test]
fn subtype_instanceof_error_base() {
    assert_eq!(
        run_str(
            "[new TypeError('') instanceof Error, new RangeError('') instanceof Error, \
             new ReferenceError('') instanceof Error, new SyntaxError('') instanceof Error].join(',')"
        ),
        "true,true,true,true"
    );
}

#[test]
fn subtype_instanceof_self() {
    assert_eq!(run_str("new TypeError('t') instanceof TypeError"), "true");
    assert_eq!(run_str("new RangeError('r') instanceof RangeError"), "true");
    assert_eq!(run_str("new ReferenceError('x') instanceof ReferenceError"), "true");
    assert_eq!(run_str("new SyntaxError('y') instanceof SyntaxError"), "true");
}

#[test]
fn subtype_instanceof_object() {
    assert_eq!(run_str("new TypeError('x') instanceof Object"), "true");
    assert_eq!(run_str("new RangeError('x') instanceof Object"), "true");
}

#[test]
fn subtype_instanceof_cross_negatives() {
    // A subtype is not an instance of a *sibling* subtype.
    assert_eq!(
        run_str(
            "[new RangeError('') instanceof TypeError, new TypeError('') instanceof RangeError, \
             new SyntaxError('') instanceof ReferenceError].join(',')"
        ),
        "false,false,false"
    );
}

#[test]
fn base_error_is_not_a_subtype() {
    // A plain Error is not a TypeError/RangeError (the chain is one-directional).
    assert_eq!(run_str("new Error('') instanceof TypeError"), "false");
    assert_eq!(run_str("new Error('') instanceof RangeError"), "false");
}

// ============================================================================
// AggregateError specifics
// ============================================================================

#[test]
fn aggregate_error_construct() {
    assert_eq!(run_str("new AggregateError([1,2], 'all failed').name"), "AggregateError");
    assert_eq!(run_str("new AggregateError([1,2], 'all failed').message"), "all failed");
    assert_eq!(run_str("new AggregateError([1,2,3], 'x').errors.length"), "3");
}

#[test]
fn aggregate_error_errors_array_contents() {
    assert_eq!(run_str("new AggregateError([10,20,30], 'x').errors[1]"), "20");
    assert_eq!(
        run_str("new AggregateError([new Error('a'), new Error('b')], 'x').errors[0].message"),
        "a"
    );
}

#[test]
fn aggregate_error_default_empty_errors() {
    assert_eq!(run_str("new AggregateError([]).errors.length"), "0");
    assert_eq!(run_str("new AggregateError([], 'm').errors.length"), "0");
}

#[test]
fn aggregate_error_instanceof_chain() {
    // instanceof AggregateError and Error true; sibling subtype false.
    assert_eq!(
        run_str(
            "let e = new AggregateError([], 'm'); \
             (e instanceof AggregateError)+','+(e instanceof Error)+','+(e instanceof TypeError)"
        ),
        "true,true,false"
    );
}

// ============================================================================
// try / catch — Error objects
// ============================================================================

#[test]
fn throw_and_catch_error_reads_name_and_message() {
    assert_eq!(
        run_str("let m; try { throw new RangeError('out'); } catch(e){ m = e.name + ':' + e.message; } m"),
        "RangeError:out"
    );
    assert_eq!(
        run_str("let m; try { throw new TypeError('bad type'); } catch(e){ m = e.name + ':' + e.message; } m"),
        "TypeError:bad type"
    );
}

#[test]
fn caught_error_instanceof_in_catch() {
    assert_eq!(
        run_str(
            "let t; try { throw new TypeError('x'); } catch(e){ \
             t = (e instanceof TypeError) + ',' + (e instanceof Error); } t"
        ),
        "true,true"
    );
}

#[test]
fn catch_classifies_error_types() {
    assert_eq!(
        run_str(
            "function classify(e){ if (e instanceof TypeError) return 'T'; \
             if (e instanceof RangeError) return 'R'; return '?'; } \
             let out=[]; \
             try { throw new TypeError(''); } catch(e){ out.push(classify(e)); } \
             try { throw new RangeError(''); } catch(e){ out.push(classify(e)); } \
             out.join(',')"
        ),
        "T,R"
    );
}

#[test]
fn catch_optional_binding() {
    // `catch {` with no binding parameter.
    assert_eq!(run_str("let r='no'; try { throw 1; } catch { r='yes'; } r"), "yes");
    assert_eq!(run_str("let r='no'; try { throw new Error('x'); } catch { r='handled'; } r"), "handled");
}

#[test]
fn divergence_catch_parameter_not_block_scoped() {
    // DIVERGENCE: in real JS the catch parameter is its own block-scoped binding
    // that shadows an outer `let e` only inside the catch block (so `e` is still
    // "outer" afterward). zapcode binds the catch parameter to the SAME slot as
    // the outer `e`, so the caught value leaks out. Pinned to actual behavior.
    assert_eq!(run_str("let e = 'outer'; try { throw 'inner'; } catch(e){ } e"), "inner");
    assert_eq!(
        run_str("let e = 'outer'; let seen; try { throw 'inner'; } catch(e){ seen = e; } seen + '|' + e"),
        "inner|inner"
    );
}

#[test]
fn catch_with_distinct_name_leaves_outer_untouched() {
    // A catch parameter with a name that doesn't collide with an outer binding
    // does not affect that outer binding (matches Node).
    assert_eq!(run_str("let x = 'outer'; try { throw 'inner'; } catch(err){ } x"), "outer");
    assert_eq!(run_str("let x = 'outer'; try { throw 'inner'; } catch(err){ x = err; } x"), "inner");
}

// ============================================================================
// throw of non-Error values
// ============================================================================

#[test]
fn throw_string_value() {
    assert_eq!(run_str("let e2; try { throw 'boom'; } catch (e) { e2 = e; } e2"), "boom");
    assert_eq!(run_str("let t; try { throw 'hi'; } catch(e){ t = typeof e; } t"), "string");
}

#[test]
fn throw_number_value() {
    assert_eq!(run_str("let e2; try { throw 42; } catch (e) { e2 = e; } e2"), "42");
    assert_eq!(run_str("let t; try { throw 3.5; } catch(e){ t = typeof e; } t"), "number");
}

#[test]
fn throw_boolean_value() {
    assert_eq!(run_str("let r; try { throw true; } catch(e){ r = e; } r"), "true");
    assert_eq!(run_str("let r; try { throw false; } catch(e){ r = (e === false); } r"), "true");
}

#[test]
fn throw_null_and_undefined() {
    assert_eq!(run_str("let r; try { throw null; } catch(e){ r = (e === null); } r"), "true");
    assert_eq!(run_str("let r; try { throw undefined; } catch(e){ r = (e === undefined); } r"), "true");
}

#[test]
fn throw_object_literal() {
    assert_eq!(run_str("let r; try { throw {code:42}; } catch(e){ r = e.code; } r"), "42");
    assert_eq!(
        run_str("let r; try { throw {code:'E1', detail:'x'}; } catch(e){ r = e.code + '/' + e.detail; } r"),
        "E1/x"
    );
}

#[test]
fn throw_array_value() {
    assert_eq!(run_str("let r; try { throw [1,2,3]; } catch(e){ r = e.length + ':' + e[2]; } r"), "3:3");
}

// ============================================================================
// rethrow / identity
// ============================================================================

#[test]
fn rethrow_chains_to_outer_catch() {
    assert_eq!(
        run_str(
            "let log = []; \
             try { try { throw new Error('a'); } catch(e){ log.push('inner:' + e.message); throw new Error('b'); } } \
             catch(e){ log.push('outer:' + e.message); } \
             log.join('|')"
        ),
        "inner:a|outer:b"
    );
}

#[test]
fn rethrow_same_object_preserves_message() {
    assert_eq!(
        run_str(
            "let r; try { try { throw new Error('orig'); } catch(e){ throw e; } } \
             catch(e2){ r = e2.message; } r"
        ),
        "orig"
    );
}

#[test]
fn rethrow_preserves_value_identity() {
    assert_eq!(
        run_str(
            "let same; \
             try { let orig = new Error('id'); \
                   try { throw orig; } catch(e){ same = (e.message === 'id'); throw e; } } \
             catch(e2){ same = same && (e2.message === 'id'); } same"
        ),
        "true"
    );
}

#[test]
fn rethrow_as_different_error_type() {
    assert_eq!(
        run_str(
            "let m; try { try { throw 1; } catch(e){ throw new TypeError('rethrown'); } } \
             catch(e2){ m = e2.name + ':' + e2.message; } m"
        ),
        "TypeError:rethrown"
    );
}

#[test]
fn caught_error_custom_name_preserved() {
    assert_eq!(
        run_str("let n; try { let e = new RangeError('z'); e.name = 'CustomR'; throw e; } catch(x){ n = x.name; } n"),
        "CustomR"
    );
}

// ============================================================================
// nested try / propagation through functions
// ============================================================================

#[test]
fn nested_try_inner_handles() {
    assert_eq!(
        run_str(
            "let log=[]; \
             try { try { throw 1; } catch(e){ log.push('i'+e); } } catch(e){ log.push('o'); } \
             log.join(',')"
        ),
        "i1"
    );
}

#[test]
fn throw_propagates_through_nested_functions() {
    assert_eq!(
        run_str(
            "function a(){ b(); } function b(){ throw new RangeError('deep'); } \
             let m; try { a(); } catch(e){ m = e.name + ':' + e.message; } m"
        ),
        "RangeError:deep"
    );
}

#[test]
fn throw_from_called_function() {
    assert_eq!(
        run_str(
            "function boom(){ throw new TypeError('fromFn'); } \
             let m; try { boom(); } catch(e){ m = e.name + ':' + e.message; } m"
        ),
        "TypeError:fromFn"
    );
}

#[test]
fn return_short_circuits_try_on_error() {
    assert_eq!(run_str("function f(){ try { return 1; } catch(e){ return 2; } } f()"), "1");
    assert_eq!(run_str("function f(){ try { throw 1; return 1; } catch(e){ return 2; } } f()"), "2");
}

// ============================================================================
// try / catch / finally — semantics that match Node
// ============================================================================

#[test]
fn finally_runs_after_normal_completion() {
    assert_eq!(
        run_str("let log=[]; try { log.push('t'); } finally { log.push('f'); } log.join(',')"),
        "t,f"
    );
}

#[test]
fn finally_runs_after_caught_throw() {
    assert_eq!(
        run_str("let log=[]; try { log.push('t'); throw new Error('x'); } catch(e){ log.push('c'); } finally { log.push('f'); } log.join(',')"),
        "t,c,f"
    );
}

#[test]
fn catch_and_finally_both_run() {
    assert_eq!(
        run_str("let s=[]; try { throw new Error('x'); } catch(e){ s.push('c'); } finally { s.push('f'); } s.join(',')"),
        "c,f"
    );
}

#[test]
fn finally_runs_when_inner_try_throws_and_outer_catches() {
    // try/finally (no catch) inside an outer try/catch: the finally body runs and
    // then the pending throw re-propagates to the outer catch.
    assert_eq!(
        run_str(
            "let s=[]; try { try { s.push('t'); throw 1; } finally { s.push('f'); } } \
             catch(e){ s.push('c'); } s.join(',')"
        ),
        "t,f,c"
    );
}

#[test]
fn finally_with_loop_break() {
    // The finally runs for the breaking iteration before control leaves the loop.
    assert_eq!(
        run_str(
            "let s=[]; for(let i=0;i<3;i++){ try { if(i===1) break; s.push('t'+i); } finally { s.push('f'+i); } } s.join(',')"
        ),
        "t0,f0,f1"
    );
}

#[test]
fn finally_with_loop_continue() {
    // The finally runs on each iteration, including the one that `continue`s.
    assert_eq!(
        run_str(
            "let s=[]; for(let i=0;i<3;i++){ try { if(i===1) continue; s.push('t'+i); } finally { s.push('f'+i); } } s.join(',')"
        ),
        "t0,f0,f1,t2,f2"
    );
}

#[test]
fn nested_finally_ordering_inner_caught() {
    // Inner try/catch handles, both finallys run in order.
    assert_eq!(
        run_str(
            "let s=[]; try { try { s.push('a'); throw 1; } catch(e){ s.push('c'); } } finally { s.push('d'); } s.join(',')"
        ),
        "a,c,d"
    );
}

// ============================================================================
// try / catch / finally — abrupt-completion semantics (matching Node)
// ============================================================================

#[test]
fn try_finally_no_catch_repropagates_throw() {
    // A `try { throw } finally { }` with NO local catch runs the finally body and
    // then re-propagates the in-flight exception to the enclosing catch, which
    // binds `v` and runs its body.
    assert_eq!(
        run_str("let v='none'; let s=[]; try { try { throw 'XX'; } finally { s.push('f'); } } catch(e){ v = e; } v + '|' + s.join(',')"),
        // finally ran ('f') and the outer catch then bound `v` to the throw.
        "XX|f"
    );
}

#[test]
fn finally_throw_wins_over_catch_throw() {
    // When the catch block throws and the same statement's finally ALSO throws,
    // the finally's exception wins (the catch's is discarded).
    assert_eq!(
        run_str(
            "let m='X'; \
             try { try { throw new Error('orig'); } catch(e){ throw new Error('fromCatch'); } finally { throw new Error('fromFinally'); } } \
             catch(e){ m = e.message; } m"
        ),
        "fromFinally"
    );
}

#[test]
fn try_throw_no_catch_then_finally_throw_propagates_finally() {
    // When the try throws (no catch) and the finally also throws, the finally's
    // exception propagates — this case DOES match Node.
    assert_eq!(
        run_str(
            "let m='X'; \
             try { try { throw new Error('orig'); } finally { throw new Error('fromFinally'); } } \
             catch(e){ m = e.message; } m"
        ),
        "fromFinally"
    );
}

#[test]
fn try_ok_then_finally_throw_propagates_finally() {
    // try completes normally, finally throws -> finally's exception is observed.
    assert_eq!(
        run_str(
            "let m='X'; try { try { } finally { throw new Error('fromFinally'); } } catch(e){ m = e.message; } m"
        ),
        "fromFinally"
    );
}

#[test]
fn try_return_finally_throw_replaces_return() {
    // `try { return 'R' } finally { throw … }` — the finally throw replaces the
    // pending return, so the outer catch fires with the finally's message.
    assert_eq!(
        run_str(
            "let m='X'; function f(){ try { return 'R'; } finally { throw new Error('fromFinally'); } } \
             try { f(); } catch(e){ m = e.message; } m"
        ),
        "fromFinally"
    );
}

#[test]
fn finally_side_effects_run_on_return_path() {
    // A finally body's side effects run when the try returns; the function still
    // returns its value once the finally completes normally.
    assert_eq!(
        run_str(
            "let s=[]; function f(){ try { return 'R'; } finally { s.push('F'); } } let v=f(); JSON.stringify([s, v])"
        ),
        "[[\"F\"],\"R\"]"
    );
}

// ============================================================================
// Runtime (non-throw) errors are real Error objects
// ============================================================================

#[test]
fn member_access_on_null_throws_typeerror() {
    assert_eq!(run_str("let n; try { null.x; } catch(e){ n = e.name; } n"), "TypeError");
    assert_eq!(run_str("let b; try { null.x; } catch(e){ b = e instanceof TypeError; } b"), "true");
    assert_eq!(run_str("let b; try { null.x; } catch(e){ b = e instanceof Error; } b"), "true");
    assert_eq!(
        run_str("let b; try { null.x; } catch(e){ b = typeof e.message === 'string' && e.message.length > 0; } b"),
        "true"
    );
}

#[test]
fn member_access_on_undefined_throws_typeerror() {
    assert_eq!(run_str("let n; try { let u; u.x; } catch(e){ n = e.name; } n"), "TypeError");
    assert_eq!(run_str("let b; try { let u; u.x; } catch(e){ b = e instanceof Error; } b"), "true");
}

#[test]
fn calling_non_function_throws_typeerror() {
    assert_eq!(run_str("let n; try { const x = 5; x(); } catch (e) { n = e.name; } n"), "TypeError");
    assert_eq!(run_str("let n; try { (5)(); } catch(e){ n = e.name; } n"), "TypeError");
    assert_eq!(run_str("let b; try { const x = 5; x(); } catch (e) { b = e instanceof TypeError; } b"), "true");
}

#[test]
fn runtime_typeerror_has_string_stack() {
    assert_eq!(run_str("let s; try { null.x; } catch(e){ s = typeof e.stack; } s"), "string");
}

#[test]
fn runtime_error_message_branch_pattern() {
    // The ubiquitous `e instanceof Error ? e.message : String(e)` hits the right branch.
    assert_eq!(
        run_str("let m; try { null.x; } catch (e) { m = e instanceof Error ? 'msg:'+(e.message.length>0) : 'str'; } m"),
        "msg:true"
    );
}

#[test]
fn divergence_calling_unbound_identifier_is_typeerror() {
    // DIVERGENCE: real Node throws a ReferenceError ("undefinedFn is not
    // defined") when *calling* an unbound name; zapcode reads the name as
    // `undefined` and then throws a TypeError ("undefined is not a function").
    assert_eq!(run_str("let n='none'; try { undefinedFn(); } catch(e){ n = e.name; } n"), "TypeError");
    assert_eq!(run_str("let m='none'; try { undefinedFn(); } catch(e){ m = e.message; } m"), "undefined is not a function");
    assert_eq!(run_str("let b='none'; try { undefinedFn(); } catch(e){ b = e instanceof Error; } b"), "true");
}

#[test]
fn divergence_reading_unbound_identifier_is_undefined() {
    // DIVERGENCE: real Node throws a ReferenceError on reading an unbound name;
    // zapcode yields `undefined` (and `typeof` of it is "undefined", which Node
    // also gives because typeof is special-cased — that part matches).
    assert_eq!(run_str("typeof someUndefinedVar"), "undefined");
    assert_eq!(run_str("let x = someUndefinedVar; typeof x"), "undefined");
}

// ============================================================================
// instanceof — ancestor matching via a user subclass chain
// ============================================================================

#[test]
fn user_subclass_chain_instanceof_all_ancestors() {
    // A non-error user class hierarchy: an instance is `instanceof` every
    // ancestor up the chain.
    assert_eq!(
        run_str(
            "class A{} class B extends A{} class C extends B{} let c = new C(); \
             (c instanceof C)+','+(c instanceof B)+','+(c instanceof A)"
        ),
        "true,true,true"
    );
}

#[test]
fn user_subclass_instance_not_instanceof_unrelated() {
    assert_eq!(
        run_str("class A{} class B extends A{} class Other{} let b = new B(); (b instanceof Other)"),
        "false"
    );
}

#[test]
fn user_error_subclass_instanceof_self_and_name_message() {
    // A user `class X extends Error`: `instanceof X` works and explicitly-set
    // name/message are visible.
    assert_eq!(
        run_str("class MyErr extends Error {} let e = new MyErr('oops'); e instanceof MyErr"),
        "true"
    );
    assert_eq!(
        run_str(
            "class MyErr extends Error { constructor(m){ super(m); this.name='MyErr'; this.message=m; } } \
             let e = new MyErr('oops'); e.name + ':' + e.message"
        ),
        "MyErr:oops"
    );
}

#[test]
fn user_error_subclass_is_instanceof_error() {
    // A user `class X extends Error` establishes the `instanceof Error` chain.
    assert_eq!(
        run_str("class MyErr extends Error {} let e = new MyErr('oops'); e instanceof Error"),
        "true"
    );
    // It's also an instance of its own class, and not an unrelated error type.
    assert_eq!(
        run_str("class MyErr extends Error {} let e = new MyErr('oops'); e instanceof MyErr"),
        "true"
    );
    assert_eq!(
        run_str("class MyErr extends Error {} let e = new MyErr('oops'); e instanceof TypeError"),
        "false"
    );
}

#[test]
fn user_error_subclass_throw_catch_distinguishes_by_self_type() {
    assert_eq!(
        run_str(
            "class MyErr extends Error { constructor(m){ super(m); this.name='MyErr'; this.message=m; } } \
             let r; try { throw new MyErr('boom'); } \
             catch(e){ r = (e instanceof MyErr) + ':' + e.name + ':' + e.message; } r"
        ),
        "true:MyErr:boom"
    );
}

// ============================================================================
// Error objects: custom properties, containers, serialization
// ============================================================================

#[test]
fn custom_property_on_error_is_readable() {
    assert_eq!(run_str("let e = new Error('x'); e.code = 'E1'; e.code"), "E1");
    assert_eq!(run_str("let e = new TypeError('x'); e.detail = {n:7}; e.detail.n"), "7");
}

#[test]
fn error_stored_in_array_and_object() {
    assert_eq!(run_str("let a = [new Error('x')]; a[0].message"), "x");
    assert_eq!(run_str("let o = { err: new RangeError('r') }; o.err.name + ':' + o.err.message"), "RangeError:r");
}

#[test]
fn json_stringify_of_error_is_empty_object() {
    // Matches Node: Error's name/message/stack are non-enumerable for JSON.
    assert_eq!(run_str("JSON.stringify(new Error('x'))"), "{}");
    assert_eq!(run_str("JSON.stringify(new TypeError('x'))"), "{}");
}

#[test]
fn divergence_json_stringify_error_drops_custom_props() {
    // DIVERGENCE: in real JS a custom own data property added to an error IS
    // enumerable and serialized (`{"code":"E1"}`). zapcode's error objects
    // serialize to `{}` regardless of custom props (the error brand suppresses
    // all keys for JSON). The property is still readable via member access and
    // shows up in Object.keys. Pinned to actual behavior.
    assert_eq!(run_str("let e = new Error('x'); e.code = 'E1'; JSON.stringify(e)"), "{}");
    assert_eq!(run_str("let e = new Error('x'); e.code = 'E1'; e.code"), "E1");
}

// ============================================================================
// throw of computed / call-result Error values
// ============================================================================

#[test]
fn throw_error_built_by_helper() {
    assert_eq!(
        run_str(
            "function makeErr(msg){ return new RangeError('R:' + msg); } \
             let m; try { throw makeErr('hi'); } catch(e){ m = e.name + ':' + e.message; } m"
        ),
        "RangeError:R:hi"
    );
}

#[test]
fn throw_inside_conditional_expression_branch() {
    assert_eq!(
        run_str(
            "function check(n){ if (n < 0) throw new RangeError('neg'); return n; } \
             let out=[]; \
             try { out.push(check(5)); } catch(e){ out.push('E'); } \
             try { out.push(check(-1)); } catch(e){ out.push(e.message); } \
             out.join(',')"
        ),
        "5,neg"
    );
}

#[test]
fn finally_then_value_completion() {
    // After a caught throw + finally, subsequent code runs and the program's
    // completion value is the trailing expression.
    assert_eq!(
        run_str("let acc = 0; try { throw 1; } catch(e){ acc += e; } finally { acc += 10; } acc + 100"),
        "111"
    );
}

#[test]
fn error_cause_option() {
    // ES2022: `new Error(msg, { cause })` exposes an own `cause` property.
    assert_eq!(run_str("new Error('e', { cause: 'c' }).cause"), "c");
    assert_eq!(run_str("new TypeError('t', { cause: 42 }).cause"), "42");
    assert_eq!(
        run_str("JSON.stringify(new Error('e', { cause: { code: 5 } }).cause)"),
        "{\"code\":5}"
    );
    // No options / no `cause` key -> cause is undefined.
    assert_eq!(run_str("new Error('e').cause === undefined"), "true");
    assert_eq!(run_str("new Error('e', {}).cause === undefined"), "true");
    // The message/toString/JSON behavior is unchanged.
    assert_eq!(run_str("new Error('boom', { cause: 'x' }).message"), "boom");
    assert_eq!(run_str("JSON.stringify(new Error('x', { cause: 'y' }))"), "{}");
}

#[test]
fn constructed_instance_has_no_phantom_global_methods() {
    // A chained read on `new Builtin(...)` must resolve against the instance
    // (own prop / undefined), not be mistaken for a method on the global
    // constructor. Regression: `new Error('e').zzz` used to return a function
    // because the builtin-global shortcut leaked from the `new` into the read.
    assert_eq!(run_str("typeof new Error('e').zzz"), "undefined");
    assert_eq!(run_str("typeof new Map().zzz"), "undefined");
    assert_eq!(run_str("typeof new Date().zzz"), "undefined");
    assert_eq!(run_str("typeof new RegExp('a').zzz"), "undefined");
    // Real members still resolve.
    assert_eq!(run_str("typeof new Map().set"), "function");
    assert_eq!(run_str("new Error('e').message"), "e");
}
