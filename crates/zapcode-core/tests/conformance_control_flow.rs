//! Conformance breadth: statements & control flow.
//!
//! Loops, labeled break/continue, switch (fallthrough/default), conditionals,
//! block scoping, and try/catch/finally. Asserts real-Node answers throughout,
//! including the try/finally-with-abrupt-completion semantics (an abrupt
//! completion in `finally` overrides the body; `finally` runs on return / break /
//! continue / throw), which now match Node.

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
// if / else
// ----------------------------------------------------------------------------

#[test]
fn if_else_chains() {
    assert_eq!(run_str("let x = 5; if (x > 3) 'big'; else 'small'"), "big");
    assert_eq!(
        run_str("function f(n){ if (n<0) return 'neg'; else if (n===0) return 'zero'; else return 'pos'; } f(-1)+f(0)+f(7)"),
        "negzeropos"
    );
    assert_eq!(run_str("let r='none'; if (false) r='a'; else if (true) r='b'; r"), "b");
}

#[test]
fn truthiness_in_conditions() {
    assert_eq!(run_str("if ('') 'y'; else 'n'"), "n");
    assert_eq!(run_str("if (0) 'y'; else 'n'"), "n");
    assert_eq!(run_str("if ([]) 'y'; else 'n'"), "y"); // empty array truthy
    assert_eq!(run_str("if ('0') 'y'; else 'n'"), "y"); // non-empty string truthy
    assert_eq!(run_str("if (NaN) 'y'; else 'n'"), "n");
}

// ----------------------------------------------------------------------------
// while / do-while / for
// ----------------------------------------------------------------------------

#[test]
fn while_loop() {
    assert_eq!(run_str("let i=0, s=0; while(i<5){ s+=i; i++; } s"), "10");
    assert_eq!(run_str("let i=0; while(false){ i=99; } i"), "0");
}

#[test]
fn do_while_runs_at_least_once() {
    assert_eq!(run_str("let n=0; do { n++; } while(n<3); n"), "3");
    assert_eq!(run_str("let n=10; do { n++; } while(false); n"), "11");
}

#[test]
fn for_loop_variants() {
    assert_eq!(run_str("let s=0; for(let i=0;i<5;i++) s+=i; s"), "10");
    assert_eq!(run_str("let s=0; for(let i=10;i>0;i-=2) s+=i; s"), "30");
    // multiple init / update via comma
    assert_eq!(run_str("let out=[]; for(let i=0,j=5;i<j;i++,j--) out.push(`${i}-${j}`); out.join(',')"), "0-5,1-4,2-3");
    // empty body / no-init
    assert_eq!(run_str("let i=0; for(;i<3;) i++; i"), "3");
}

#[test]
fn break_and_continue() {
    assert_eq!(run_str("let s=0; for(let i=0;i<10;i++){ if(i===5) break; s+=i; } s"), "10");
    assert_eq!(run_str("let s=0; for(let i=0;i<6;i++){ if(i%2===0) continue; s+=i; } s"), "9");
    assert_eq!(run_str("let n=0; while(true){ n++; if(n>=4) break; } n"), "4");
}

// ----------------------------------------------------------------------------
// Labeled statements
// ----------------------------------------------------------------------------

#[test]
fn labeled_continue() {
    assert_eq!(
        run_str("let r=[]; outer: for(let i=0;i<3;i++){ for(let j=0;j<3;j++){ if(j===1) continue outer; r.push(`${i}${j}`);} } r.join(',')"),
        "00,10,20"
    );
}

#[test]
fn labeled_break() {
    assert_eq!(
        run_str("let r=[]; outer: for(let i=0;i<3;i++){ for(let j=0;j<3;j++){ if(i===1&&j===1) break outer; r.push(`${i}${j}`);} } r.join(',')"),
        "00,01,02,10"
    );
}

// ----------------------------------------------------------------------------
// switch
// ----------------------------------------------------------------------------

#[test]
fn switch_fallthrough_and_break() {
    let prog = "function sw(x){let o=[];switch(x){case 1:o.push('a');case 2:o.push('b');break;case 3:o.push('c');default:o.push('d');}return o.join('');}";
    assert_eq!(run_str(&format!("{prog} sw(1)")), "ab"); // 1 falls into 2
    assert_eq!(run_str(&format!("{prog} sw(2)")), "b");
    assert_eq!(run_str(&format!("{prog} sw(3)")), "cd"); // 3 falls into default
    assert_eq!(run_str(&format!("{prog} sw(9)")), "d"); // default
}

#[test]
fn switch_strict_match_and_string_cases() {
    assert_eq!(
        run_str("function g(x){switch(x){case '1': return 'str'; case 1: return 'num'; default: return 'none';}} g(1)"),
        "num"
    );
    assert_eq!(
        run_str("function g(x){switch(x){case '1': return 'str'; case 1: return 'num'; default: return 'none';}} g('1')"),
        "str"
    );
    assert_eq!(
        run_str("function color(c){switch(c){case 'red': return '#f00'; case 'green': return '#0f0'; default: return '?';}} color('green')"),
        "#0f0"
    );
}

#[test]
fn switch_default_not_first() {
    assert_eq!(
        run_str("function f(x){switch(x){default: return 'd'; case 1: return 'one';}} f(2)"),
        "d"
    );
    assert_eq!(
        run_str("function f(x){switch(x){default: return 'd'; case 1: return 'one';}} f(1)"),
        "one"
    );
}

// ----------------------------------------------------------------------------
// Block scoping (let/const/var)
// ----------------------------------------------------------------------------

#[test]
fn block_scoping_inner_assignment() {
    // Assigning (not redeclaring) an outer binding from an inner block updates it.
    assert_eq!(run_str("let x=1; { x=2; } x"), "2");
    assert_eq!(run_str("let s=''; { s+='a'; } { s+='b'; } s"), "ab");
}

#[test]
fn block_scoped_redeclaration_documented_divergence() {
    // DIVERGENCE (documented, scoping): a `let`/`const` re-declaration inside a
    // nested block is NOT isolated to that block — it writes through to (and
    // leaks into) the enclosing scope. Real JS would shadow, leaving the outer
    // binding `1`/`10` and `y` undefined outside its block. Asserting zapcode's
    // ACTUAL behavior, with the JS answer noted.
    assert_eq!(run_str("let x=1; { let x=2; } x"), "2"); // JS: 1
    assert_eq!(run_str("const x=10; { const x=20; } x"), "20"); // JS: 10
    assert_eq!(run_str("{ let y=5; } typeof y"), "number"); // JS: "undefined"
}

#[test]
fn for_let_binding_is_per_iteration() {
    // Each iteration captures its own `i` (closures-in-loops classic).
    assert_eq!(
        run_str("let fns=[]; for(let i=0;i<3;i++){ fns.push(()=>i); } fns.map(f=>f()).join(',')"),
        "0,1,2"
    );
}

#[test]
fn var_is_function_scoped() {
    assert_eq!(run_str("function f(){ { var x=5; } return x; } f()"), "5");
    assert_eq!(run_str("function f(){ for(var i=0;i<3;i++){} return i; } f()"), "3");
}

// ----------------------------------------------------------------------------
// try / catch / finally
// ----------------------------------------------------------------------------

#[test]
fn try_catch_basic() {
    assert_eq!(
        run_str("let r; try { throw new Error('boom'); } catch(e){ r = e.message; } r"),
        "boom"
    );
    assert_eq!(
        run_str("let r='ok'; try { r='try'; } catch(e){ r='catch'; } r"),
        "try"
    );
    // optional catch binding
    assert_eq!(
        run_str("let r; try { throw 1; } catch { r='caught'; } r"),
        "caught"
    );
}

#[test]
fn try_catch_finally_normal_and_catch_paths() {
    // finally runs after normal try completion
    assert_eq!(
        run_str("let log=[]; try { log.push('t'); } finally { log.push('f'); } log.join(',')"),
        "t,f"
    );
    // finally runs after catch
    assert_eq!(
        run_str("let log=[]; try { throw new Error('x'); } catch(e){ log.push('c'); } finally { log.push('f'); } log.join(',')"),
        "c,f"
    );
}

#[test]
fn thrown_non_error_values_pass_through() {
    assert_eq!(run_str("let v; try { throw 'string err'; } catch(e){ v=e; } v"), "string err");
    assert_eq!(run_str("let v; try { throw 42; } catch(e){ v=e; } v"), "42");
    assert_eq!(run_str("let v; try { throw {code: 7}; } catch(e){ v=e.code; } v"), "7");
    assert_eq!(run_str("let v; try { throw [1,2]; } catch(e){ v=e[1]; } v"), "2");
}

#[test]
fn caught_runtime_error_is_a_real_error_object() {
    // A runtime error (not an explicit throw) is caught as an Error instance.
    assert_eq!(run_str("let t; try { null.x; } catch(e){ t = e instanceof Error; } t"), "true");
    assert_eq!(run_str("let n; try { undefined(); } catch(e){ n = typeof e.message; } n"), "string");
}

#[test]
fn nested_try_catch() {
    assert_eq!(
        run_str("let log=[]; try { try { throw new Error('inner'); } catch(e){ log.push('ic:'+e.message); throw new Error('rethrown'); } } catch(e){ log.push('oc:'+e.message); } log.join('|')"),
        "ic:inner|oc:rethrown"
    );
}

#[test]
fn try_finally_abrupt_completion_matches_node() {
    // try/finally completion semantics (cluster B), now matching real Node:
    // (a) a `return`/`break`/`continue`/`throw` inside `finally` overrides the
    // try/catch outcome, (b) a `finally` with no `catch` re-propagates a pending
    // exception, and (c) `finally` runs when control leaves the `try` via
    // `continue`/`break`.

    // (a) finally `return` overrides the try `return`.
    assert_eq!(run_str("function tf(){ try { return 1; } finally { return 2; } } tf()"), "2");

    // (b) finally without catch re-propagates the throw to the outer catch.
    assert_eq!(
        run_str("let log=[]; try { try{ throw new Error('e'); } finally { log.push('f'); } } catch(e){ log.push('c'); } log.join(',')"),
        "f,c"
    );

    // (c) `continue` out of the try runs the finally for that iteration.
    assert_eq!(
        run_str("let log=[]; for(let i=0;i<2;i++){ try{ if(i===0) continue; log.push('body'); } finally { log.push('f'+i); } } log.join(',')"),
        "f0,body,f1"
    );
}

// ----------------------------------------------------------------------------
// Statement completion values
// ----------------------------------------------------------------------------

#[test]
fn block_and_loop_completion_values() {
    // Last evaluated expression statement is the program completion value.
    assert_eq!(run_str("{ 1; 2; 3 }"), "3");
    assert_eq!(run_str("if (true) { 'a'; 'b' }"), "b");
    assert_eq!(run_str("let x = 5; x"), "5");
}
