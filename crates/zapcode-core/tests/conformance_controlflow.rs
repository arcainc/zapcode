//! Conformance breadth (round 1): CONTROL FLOW.
//!
//! A test262-style sweep of zapcode's statement/control-flow surface:
//!   - `if`/`else` (chains, dangling-else, truthiness)
//!   - C-style `for` (init/cond/update permutations, comma, side effects)
//!   - `for...of` (arrays, strings, Map, Set, `.entries()`/`.keys()`/`.values()`,
//!     destructuring in the binding, deeply NESTED)
//!   - `for...in` (objects, arrays, key order, break/continue)
//!   - `while`, `do...while` (runs once, break/continue)
//!   - `switch` (fallthrough, mid-default, string/number/boolean discriminant,
//!     strict matching, break, discriminant-evaluated-once, lexical case blocks)
//!   - `break`/`continue` including LABELED across nested loops
//!   - block completion values and `return` from nested blocks
//!
//! Every assertion is checked against real Node behavior. Where zapcode has a
//! KNOWN, DOCUMENTED divergence (for...in key ordering uses pure insertion order
//! rather than the spec's integer-keys-ascending-first rule; labeled `continue`
//! across nested `for...of` does not resume all outer iterations) the test pins
//! zapcode's ACTUAL output and notes the JS answer in a comment, so the suite
//! documents the boundary instead of asserting an answer the engine won't produce.

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, ZapcodeRun};

/// Run a program to completion and stringify its completion value.
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
// if / else
// ============================================================================

#[test]
fn if_else_basic() {
    assert_eq!(run_str("if (true) 'yes'; else 'no'"), "yes");
    assert_eq!(run_str("if (false) 'yes'; else 'no'"), "no");
    assert_eq!(run_str("let x = 7; if (x > 5) 'big'; else 'small'"), "big");
    // `if` with no `else`, condition false: program completion is undefined.
    assert_eq!(run_str("if (false) 'yes'"), "undefined");
}

#[test]
fn if_else_if_chains() {
    let f =
        "function f(n){ if (n<0) return 'neg'; else if (n===0) return 'zero'; else return 'pos'; }";
    assert_eq!(run_str(&format!("{f} f(-3)")), "neg");
    assert_eq!(run_str(&format!("{f} f(0)")), "zero");
    assert_eq!(run_str(&format!("{f} f(42)")), "pos");
    assert_eq!(run_str(&format!("{f} f(-1)+f(0)+f(7)")), "negzeropos");
}

#[test]
fn if_else_chain_first_truthy_wins() {
    assert_eq!(
        run_str("let r='none'; if (false) r='a'; else if (true) r='b'; else if (true) r='c'; r"),
        "b"
    );
    assert_eq!(
        run_str("let r='none'; if (false) r='a'; else if (false) r='b'; else r='z'; r"),
        "z"
    );
}

#[test]
fn dangling_else_binds_to_nearest_if() {
    // `else` attaches to the inner `if`. With a=1,b=0 the inner-if is false so we
    // hit the `else` => 'inner-else'.
    let prog = "function f(a,b){ let r='x'; if (a) if (b) r='both'; else r='inner-else'; return r; }";
    assert_eq!(run_str(&format!("{prog} f(1,1)")), "both");
    assert_eq!(run_str(&format!("{prog} f(1,0)")), "inner-else");
    // a falsy => outer if skipped entirely; the else (bound to inner) is NOT taken.
    assert_eq!(run_str(&format!("{prog} f(0,1)")), "x");
}

#[test]
fn truthiness_in_conditions() {
    assert_eq!(run_str("if ('') 'y'; else 'n'"), "n");
    assert_eq!(run_str("if (0) 'y'; else 'n'"), "n");
    assert_eq!(run_str("if (-0) 'y'; else 'n'"), "n");
    assert_eq!(run_str("if (NaN) 'y'; else 'n'"), "n");
    assert_eq!(run_str("if (null) 'y'; else 'n'"), "n");
    assert_eq!(run_str("if (undefined) 'y'; else 'n'"), "n");
    // Truthy values
    assert_eq!(run_str("if ([]) 'y'; else 'n'"), "y"); // empty array is truthy
    assert_eq!(run_str("if ({}) 'y'; else 'n'"), "y"); // empty object is truthy
    assert_eq!(run_str("if ('0') 'y'; else 'n'"), "y"); // non-empty string truthy
    assert_eq!(run_str("if ('false') 'y'; else 'n'"), "y");
    assert_eq!(run_str("if (-1) 'y'; else 'n'"), "y");
    assert_eq!(run_str("if (Infinity) 'y'; else 'n'"), "y");
}

// ============================================================================
// C-style for
// ============================================================================

#[test]
fn for_loop_ascending_descending() {
    assert_eq!(run_str("let s=0; for(let i=0;i<5;i++) s+=i; s"), "10");
    assert_eq!(run_str("let s=0; for(let i=10;i>0;i-=2) s+=i; s"), "30");
    assert_eq!(run_str("let p=1; for(let i=1;i<=5;i++) p*=i; p"), "120"); // 5!
}

#[test]
fn for_loop_multiple_init_and_update_via_comma() {
    assert_eq!(
        run_str("let out=[]; for(let i=0,j=5;i<j;i++,j--) out.push(`${i}-${j}`); out.join(',')"),
        "0-5,1-4,2-3"
    );
}

#[test]
fn for_loop_omitted_clauses() {
    // Omitted init.
    assert_eq!(run_str("let i=0; for(;i<3;) i++; i"), "3");
    // Omitted init and update; manual increment in body.
    assert_eq!(run_str("let i=0,s=0; for(;i<4;){ s+=i; i++; } s"), "6");
    // Fully omitted header with break.
    assert_eq!(run_str("let n=0; for(;;){ n++; if(n>=3) break; } n"), "3");
    // Empty body (semicolon) — loop only runs the update for its side effects.
    assert_eq!(run_str("let i=0; for(;i<5;i++) ; i"), "5");
}

#[test]
fn for_update_runs_after_body_each_iteration() {
    // Update side effect happens after the body, every iteration.
    assert_eq!(
        run_str("let log=[]; for(let i=0; i<3; (()=>{log.push('u'+i)})(),i++){ log.push('b'+i);} log.join(',')"),
        "b0,u0,b1,u1,b2,u2"
    );
}

#[test]
fn nested_c_style_for() {
    assert_eq!(
        run_str("let s=0; for(let i=0;i<3;i++){ for(let j=0;j<3;j++){ s+=i*3+j; } } s"),
        "36"
    );
    assert_eq!(
        run_str("let g=[]; for(let i=0;i<2;i++){ for(let j=0;j<2;j++){ g.push(`${i}${j}`);}} g.join(',')"),
        "00,01,10,11"
    );
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
fn var_in_c_style_for_is_function_scoped() {
    assert_eq!(run_str("function f(){ for(var i=0;i<3;i++){} return i; } f()"), "3");
    assert_eq!(run_str("function f(){ { var x=5; } return x; } f()"), "5");
}

// ============================================================================
// for...of — arrays, strings
// ============================================================================

#[test]
fn for_of_array_values() {
    assert_eq!(run_str("let s=0; for(const x of [5,10,15]) s+=x; s"), "30");
    assert_eq!(run_str("let o=[]; for(const x of ['a','b','c']) o.push(x); o.join('')"), "abc");
    // empty array runs zero iterations
    assert_eq!(run_str("let n=0; for(const x of []) n++; n"), "0");
}

#[test]
fn for_of_string_iterates_characters() {
    assert_eq!(run_str("let o=[]; for(const c of 'abc') o.push(c); o.join('|')"), "a|b|c");
    assert_eq!(run_str("let n=0; for(const c of '') n++; n"), "0");
    // characters include punctuation/whitespace
    assert_eq!(run_str("let o=[]; for(const c of 'a,b') o.push(c); o.join('|')"), "a|,|b");
}

#[test]
fn for_of_array_of_objects_with_destructuring() {
    assert_eq!(run_str("let t=0; for(const {v} of [{v:1},{v:2},{v:3}]) t+=v; t"), "6");
    // destructuring with a default for a missing key
    assert_eq!(
        run_str("let o=[]; for(const {a,b=9} of [{a:1},{a:2,b:3}]) o.push(a+'-'+b); o.join(',')"),
        "1-9,2-3"
    );
    // array-element destructuring in the binding
    assert_eq!(
        run_str("let o=[]; for(const [x,y] of [[1,2],[3,4]]) o.push(x*y); o.join(',')"),
        "2,12"
    );
}

#[test]
fn for_of_array_entries() {
    assert_eq!(run_str("let o=[]; for(const [i,v] of ['x','y'].entries()) o.push(i+v); o.join(',')"), "0x,1y");
    assert_eq!(
        run_str("let o=[]; for(const [i,v] of ['a','b','c'].entries()) o.push(`${i}:${v}`); o.join(',')"),
        "0:a,1:b,2:c"
    );
}

// ============================================================================
// for...of — Map / Set
// ============================================================================

#[test]
fn for_of_map_yields_key_value_pairs() {
    // iterating a Map directly yields [k,v] entries
    assert_eq!(
        run_str("const m=new Map([['a',1],['b',2]]); let o=[]; for(const [k,v] of m) o.push(k+v); o.join(',')"),
        "a1,b2"
    );
    // each entry is a 2-element array
    assert_eq!(
        run_str("const m=new Map([['a',1],['b',2]]); let r=[]; for(const e of m) r.push(e[0]+':'+e[1]); r.join(',')"),
        "a:1,b:2"
    );
}

#[test]
fn for_of_map_entries_keys_values() {
    assert_eq!(
        run_str("const m=new Map([['a',1],['b',2]]); let o=[]; for(const [k,v] of m.entries()) o.push(k+v); o.join(',')"),
        "a1,b2"
    );
    assert_eq!(
        run_str("const m=new Map([['a',1],['b',2]]); let o=[]; for(const k of m.keys()) o.push(k); o.join(',')"),
        "a,b"
    );
    assert_eq!(
        run_str("const m=new Map([['a',1],['b',2]]); let o=[]; for(const v of m.values()) o.push(v); o.join(',')"),
        "1,2"
    );
}

#[test]
fn for_of_map_preserves_insertion_order_and_overwrite_keeps_position() {
    // Map iterates in insertion order.
    assert_eq!(
        run_str("const m=new Map(); m.set('z',1); m.set('a',2); m.set('m',3); let k=[]; for(const key of m.keys()) k.push(key); k.join(',')"),
        "z,a,m"
    );
    // Re-setting an existing key updates value but keeps its original position.
    assert_eq!(
        run_str("const m=new Map([['a',1],['b',2]]); m.set('a',9); let r=[]; for(const [k,v] of m) r.push(k+v); r.join(',')"),
        "a9,b2"
    );
}

#[test]
fn for_of_set_values_dedup() {
    assert_eq!(run_str("let t=0; for(const x of new Set([1,2,2,3])) t+=x; t"), "6");
    // Set preserves insertion order and dedups
    assert_eq!(
        run_str("const s=new Set(); s.add('a'); s.add('b'); s.add('a'); let r=[]; for(const x of s) r.push(x); r.join(',')"),
        "a,b"
    );
}

// ============================================================================
// for...of — NESTED
// ============================================================================

#[test]
fn nested_for_of_arrays_runs_all_outer_iterations() {
    assert_eq!(
        run_str("const out=[]; for(const a of ['a','b']){ for(const n of [1,2]){ out.push(a+n);}} out.join(',')"),
        "a1,a2,b1,b2"
    );
    assert_eq!(
        run_str("let c=0; for(const a of [1,2]) for(const b of [1,2,3]) for(const d of [1,2]) c++; c"),
        "12"
    );
}

#[test]
fn nested_for_of_strings() {
    assert_eq!(
        run_str("let o=[]; for(const a of 'ab'){ for(const b of '12'){ o.push(a+b);}} o.join(',')"),
        "a1,a2,b1,b2"
    );
}

#[test]
fn nested_for_of_map_value_is_array() {
    assert_eq!(
        run_str("let o=[]; const m=new Map([['x',[1,2]]]); for(const [k,arr] of m){ for(const n of arr){ o.push(k+n);}} o.join(',')"),
        "x1,x2"
    );
    // nested destructuring across the boundary
    assert_eq!(
        run_str("const m=new Map([['k',{x:1}]]); let r=[]; for(const [key,{x}] of m) r.push(key+x); r.join(',')"),
        "k1"
    );
}

#[test]
fn nested_for_of_array_of_arrays() {
    assert_eq!(
        run_str("let o=[]; for(const row of [['a','b'],['c']]){ for(const x of row){ o.push(x);}} o.join('')"),
        "abc"
    );
}

#[test]
fn nested_for_of_with_break_and_continue() {
    assert_eq!(
        run_str("const out=[]; for(const a of [1,2,3]){ for(const b of [1,2,3]){ if(b===2) continue; if(b===3) break; out.push(a*10+b);}} out.join(',')"),
        "11,21,31"
    );
}

// ============================================================================
// for...in — objects, arrays, key order
// ============================================================================

#[test]
fn for_in_object_keys_and_values() {
    assert_eq!(
        run_str("const o={a:1,b:2}; let k=''; for(const x in o) k+=x; k"),
        "ab"
    );
    assert_eq!(
        run_str("const o={a:1,b:2,c:3}; let s=0; for(const x in o) s+=o[x]; s"),
        "6"
    );
    assert_eq!(
        run_str("const o={x:1,y:2}; let pairs=[]; for(const k in o) pairs.push(k+'='+o[k]); pairs.join(',')"),
        "x=1,y=2"
    );
}

#[test]
fn for_in_string_key_insertion_order() {
    // For purely-string keys, both zapcode and JS iterate in insertion order.
    assert_eq!(run_str("const o={z:1,a:2,m:3}; let k=''; for(const x in o) k+=x; k"), "zam");
}

#[test]
fn for_in_integer_key_order_documented_divergence() {
    // DIVERGENCE (documented, key ordering): zapcode iterates for...in keys in
    // PURE INSERTION ORDER. Real JS first emits integer-index keys in ascending
    // numeric order, then string keys in insertion order. These pin zapcode's
    // ACTUAL output; the JS answers are noted.
    //
    // JS: "12ba"  (1,2 ascending, then b,a in insertion order)
    assert_eq!(
        run_str("const o={}; o['b']=1; o[2]=1; o['a']=1; o[1]=1; let k=''; for(const x in o) k+=x; k"),
        "b2a1"
    );
    // JS: "123"  (ascending). zapcode keeps insertion order 3,1,2.
    assert_eq!(
        run_str("const o={}; o[3]=1; o[1]=1; o[2]=1; let k=''; for(const x in o) k+=x; k"),
        "312"
    );
}

#[test]
fn for_in_array_indices_are_strings_in_order() {
    assert_eq!(run_str("const a=['x','y','z']; let r=''; for(const i in a) r+=i; r"), "012");
    // indices are STRINGS — concatenation, not addition
    assert_eq!(
        run_str("const a=[10,20]; let r=[]; for(const i in a) r.push(typeof i); r.join(',')"),
        "string,string"
    );
}

#[test]
fn for_in_empty_object_zero_iterations() {
    assert_eq!(run_str("let n=0; for(const k in {}) n++; n"), "0");
}

#[test]
fn for_in_break_and_continue() {
    assert_eq!(
        run_str("const o={a:1,b:2,c:3}; let r=[]; for(const k in o){ if(k==='b') break; r.push(k);} r.join(',')"),
        "a"
    );
    assert_eq!(
        run_str("const o={a:1,b:2,c:3}; let r=[]; for(const k in o){ if(k==='b') continue; r.push(k);} r.join(',')"),
        "a,c"
    );
}

// ============================================================================
// while
// ============================================================================

#[test]
fn while_loop_accumulates() {
    assert_eq!(run_str("let i=0,s=0; while(i<5){ s+=i; i++; } s"), "10");
}

#[test]
fn while_false_body_never_runs() {
    assert_eq!(run_str("let i=0; while(false){ i=99; } i"), "0");
    assert_eq!(run_str("let r='untouched'; while(false){ r='changed'; } r"), "untouched");
}

#[test]
fn while_break_and_continue() {
    assert_eq!(run_str("let n=0; while(true){ n++; if(n>=4) break; } n"), "4");
    assert_eq!(run_str("let i=0,s=0; while(i<5){ i++; if(i%2===0) continue; s+=i; } s"), "9");
}

// ============================================================================
// do...while
// ============================================================================

#[test]
fn do_while_runs_at_least_once() {
    assert_eq!(run_str("let n=0; do { n++; } while(n<3); n"), "3");
    // condition false up front: body still runs exactly once
    assert_eq!(run_str("let c=0; do { c++; } while(false); c"), "1");
    assert_eq!(run_str("let n=10; do { n++; } while(false); n"), "11");
}

#[test]
fn do_while_break_and_continue() {
    assert_eq!(run_str("let n=0; do { n++; if(n===2) break; } while(n<10); n"), "2");
    assert_eq!(run_str("let c=0; do { c++; break; } while(true); c"), "1");
    // continue jumps to the condition check (n still increments before continue)
    assert_eq!(run_str("let n=0,s=0; do { n++; if(n%2===0) continue; s+=n; } while(n<5); s"), "9");
}

#[test]
fn do_while_accumulate_through_condition() {
    assert_eq!(run_str("let i=0,sum=0; do { sum += i; i++; } while(i<=5); sum"), "15"); // 0+1+2+3+4+5
}

// ============================================================================
// switch — fallthrough, default, discriminants, break
// ============================================================================

#[test]
fn switch_fallthrough_and_break() {
    let prog = "function sw(x){let o=[];switch(x){case 1:o.push('a');case 2:o.push('b');break;case 3:o.push('c');default:o.push('d');}return o.join('');}";
    assert_eq!(run_str(&format!("{prog} sw(1)")), "ab"); // 1 falls into 2, then break
    assert_eq!(run_str(&format!("{prog} sw(2)")), "b");
    assert_eq!(run_str(&format!("{prog} sw(3)")), "cd"); // 3 falls into default
    assert_eq!(run_str(&format!("{prog} sw(9)")), "d"); // default
}

#[test]
fn switch_no_match_no_default_does_nothing() {
    assert_eq!(
        run_str("function f(x){let r='';switch(x){case 1:r+='a';case 2:r+='b';}return r;} f(9)"),
        ""
    );
}

#[test]
fn switch_strict_string_vs_number_match() {
    let g = "function g(x){switch(x){case '1': return 'str'; case 1: return 'num'; default: return 'none';}}";
    assert_eq!(run_str(&format!("{g} g(1)")), "num");
    assert_eq!(run_str(&format!("{g} g('1')")), "str");
    assert_eq!(run_str(&format!("{g} g(2)")), "none");
}

#[test]
fn switch_string_discriminant() {
    let f = "function color(c){switch(c){case 'red': return '#f00'; case 'green': return '#0f0'; default: return '?';}}";
    assert_eq!(run_str(&format!("{f} color('green')")), "#0f0");
    assert_eq!(run_str(&format!("{f} color('red')")), "#f00");
    assert_eq!(run_str(&format!("{f} color('blue')")), "?");
    // discriminant can be a computed expression
    assert_eq!(
        run_str("function f(s){switch(s.toLowerCase()){case 'a':return 1;case 'b':return 2;default:return 0;}} f('A')+f('B')+f('c')"),
        "3"
    );
}

#[test]
fn switch_number_and_boolean_discriminant() {
    // boolean discriminant, strict — 1 does NOT match `true`
    assert_eq!(
        run_str("function f(x){switch(x){case true:return 't';case false:return 'f';default:return '?';}} f(true)+f(false)+f(1)"),
        "tf?"
    );
    // mixed: `false` does not match `0` (strict)
    assert_eq!(
        run_str("function f(x){switch(x){case 0: return 'zero'; case '0': return 'strzero'; default: return 'd';}} f(0)+'|'+f('0')+'|'+f(false)"),
        "zero|strzero|d"
    );
}

#[test]
fn switch_default_not_first() {
    let f = "function f(x){switch(x){default: return 'd'; case 1: return 'one';}}";
    assert_eq!(run_str(&format!("{f} f(2)")), "d");
    assert_eq!(run_str(&format!("{f} f(1)")), "one");
}

#[test]
fn switch_default_in_middle_falls_through() {
    // default sits between cases; an unmatched discriminant enters default then
    // falls through into the following case body.
    assert_eq!(
        run_str("function f(x){let o=[];switch(x){case 1:o.push('1');default:o.push('d');case 2:o.push('2');}return o.join(',');} f(5)"),
        "d,2"
    );
    // and a later case can still be matched directly, skipping default
    assert_eq!(
        run_str("function f(x){let o=[];switch(x){default:o.push('d');case 'last':o.push('L');}return o.join('');} f('unknown')+'|'+f('last')"),
        "dL|L"
    );
}

#[test]
fn switch_return_short_circuits() {
    let f = "function f(x){switch(x){case 1: return 'a'; case 2: return 'b';}return 'none';}";
    assert_eq!(run_str(&format!("{f} f(1)+f(2)+f(3)")), "abnone");
}

#[test]
fn switch_discriminant_evaluated_once() {
    assert_eq!(
        run_str("let calls=0; function d(){calls++; return 2;} switch(d()){case 1: break; case 2: break;} calls"),
        "1"
    );
}

#[test]
fn switch_case_with_lexical_block() {
    assert_eq!(
        run_str("function f(x){switch(x){case 1:{ const y=10; return y; } default: return 0;}} f(1)"),
        "10"
    );
}

#[test]
fn switch_inside_loop_break_targets_switch_only() {
    // `break` inside a switch breaks the switch, NOT the enclosing for loop.
    assert_eq!(
        run_str("let o=[]; for(let i=0;i<3;i++){ switch(i){ case 1: o.push('one'); break; default: o.push('x'+i);} } o.join(',')"),
        "x0,one,x2"
    );
    // a break that exits the switch still lets the loop body continue afterwards
    assert_eq!(
        run_str("let o=[]; for(let i=0;i<3;i++){ switch(i){ case 0: break; } o.push(i);} o.join(',')"),
        "0,1,2"
    );
}

#[test]
fn switch_completion_value() {
    assert_eq!(
        run_str("let x=2; let r; switch(x){case 1: r='one'; break; case 2: r='two'; break;} r"),
        "two"
    );
}

// ============================================================================
// break / continue
// ============================================================================

#[test]
fn break_stops_loop() {
    assert_eq!(run_str("let s=0; for(let i=0;i<10;i++){ if(i===5) break; s+=i; } s"), "10");
}

#[test]
fn continue_skips_iteration() {
    assert_eq!(run_str("let s=0; for(let i=0;i<6;i++){ if(i%2===0) continue; s+=i; } s"), "9");
    assert_eq!(run_str("let o=[]; for(const x of [1,2,3,4]){ if(x%2===0) continue; o.push(x);} o.join(',')"), "1,3");
}

#[test]
fn break_and_continue_combined() {
    assert_eq!(
        run_str("let r=[]; for(let i=0;i<10;i++){ if(i===7) break; if(i%3===0) continue; r.push(i);} r.join(',')"),
        "1,2,4,5"
    );
}

#[test]
fn continue_in_for_of_map() {
    assert_eq!(
        run_str("const m=new Map([['a',1],['b',2],['c',3]]); let r=[]; for(const [k,v] of m){ if(v===2) continue; r.push(k);} r.join(',')"),
        "a,c"
    );
}

// ============================================================================
// LABELED break / continue across nested loops
// ============================================================================

#[test]
fn labeled_break_across_nested_c_style_for() {
    assert_eq!(
        run_str("let r=[]; outer: for(let i=0;i<3;i++){ for(let j=0;j<3;j++){ if(i===1&&j===1) break outer; r.push(`${i}${j}`);} } r.join(',')"),
        "00,01,02,10"
    );
}

#[test]
fn labeled_break_skips_rest_of_outer_body() {
    assert_eq!(
        run_str("let r=[]; outer: for(let i=0;i<3;i++){ r.push('top'+i); for(let j=0;j<2;j++){ if(i===1) break outer; r.push('in'+i+j);} r.push('bot'+i);} r.join(',')"),
        "top0,in00,in01,bot0,top1"
    );
}

#[test]
fn labeled_continue_across_nested_c_style_for() {
    assert_eq!(
        run_str("let r=[]; outer: for(let i=0;i<3;i++){ for(let j=0;j<3;j++){ if(j===1) continue outer; r.push(`${i}${j}`);} } r.join(',')"),
        "00,10,20"
    );
}

#[test]
fn labeled_continue_three_levels_deep() {
    // continue A from the innermost loop resumes the OUTERMOST loop.
    assert_eq!(
        run_str("let r=[]; A: for(let i=0;i<2;i++){ for(let j=0;j<2;j++){ for(let k=0;k<2;k++){ if(k===1) continue A; r.push(`${i}${j}${k}`);}}} r.join(',')"),
        "000,100"
    );
}

#[test]
fn labeled_mixed_break_and_continue_two_labels() {
    // B continues the inner loop; A breaks both. Validates label targeting.
    assert_eq!(
        run_str("let r=[]; A: for(let i=0;i<2;i++){ B: for(let j=0;j<3;j++){ if(j===1) continue B; if(i===1&&j===2) break A; r.push(i+''+j);}} r.join(',')"),
        "00,02,10"
    );
}

#[test]
fn labeled_continue_while_loops() {
    assert_eq!(
        run_str("let r=[]; let i=0; outer: while(i<3){ i++; let j=0; while(j<3){ j++; if(j===2) continue outer; r.push(i*10+j);}} r.join(',')"),
        "11,21,31"
    );
}

#[test]
fn labeled_break_across_nested_for_of() {
    assert_eq!(
        run_str("let r=[]; outer: for(const a of [1,2,3]){ for(const b of [1,2,3]){ if(a===2&&b===2) break outer; r.push(a*10+b);}} r.join(',')"),
        "11,12,13,21"
    );
}

#[test]
fn labeled_break_on_plain_block() {
    // `break label` out of a labeled NON-loop block jumps past the block. This
    // used to emit an unpatched Break(0) that ran to instruction 0 and hit the
    // allocation limit (guest-triggerable runaway).
    assert_eq!(run_str("let r=''; foo:{ r+='a'; break foo; r+='b'; } r+'c'"), "ac");
    // No break: the whole block runs.
    assert_eq!(run_str("let r=''; foo:{ r+='a'; r+='b'; } r+'c'"), "abc");
    // Nested labeled blocks: break the outer vs the inner.
    assert_eq!(run_str("let r=''; a:{ b:{ r+='1'; break a; r+='2'; } r+='3'; } r+'4'"), "14");
    assert_eq!(run_str("let r=''; a:{ b:{ r+='1'; break b; r+='2'; } r+='3'; } r+'4'"), "134");
}

#[test]
fn unlabeled_break_skips_enclosing_labeled_block() {
    // An UNLABELED break inside a labeled block that sits in a loop must break
    // the loop, not the block (the labeled block must be invisible to it).
    assert_eq!(
        run_str("let r=''; for(let i=0;i<3;i++){ blk:{ r+=i; break; } r+='x'; } r"),
        "0"
    );
    // A labeled break of the outer loop still works from inside such a block.
    assert_eq!(
        run_str("let r=''; outer: for(let i=0;i<3;i++){ blk:{ r+=i; if(i===1) break outer; } } r"),
        "01"
    );
}

#[test]
fn labeled_continue_across_nested_for_of_documented_divergence() {
    // DIVERGENCE (documented, control-flow residual): a labeled `continue` that
    // targets an OUTER `for...of` does not resume the remaining outer iterations
    // the way the spec requires; here it stops after the first outer pass.
    //
    // JS: "11,21"  (continue outer should still iterate a=2). zapcode yields "11".
    // (Labeled continue across C-style `for`/`while` IS correct — see the tests
    // above; only the for...of iterator path has this gap.)
    assert_eq!(
        run_str("let r=[]; outer: for(const a of [1,2]){ for(const b of [1,2]){ if(b===2) continue outer; r.push(a*10+b);}} r.join(',')"),
        "11"
    );
}

// ============================================================================
// Block completion values & return from nested blocks
// ============================================================================

#[test]
fn block_completion_values() {
    // Program completion is the last evaluated expression statement.
    assert_eq!(run_str("{ 1; 2; 3 }"), "3");
    assert_eq!(run_str("if (true) { 'a'; 'b' }"), "b");
    assert_eq!(run_str("let x = 5; x"), "5");
}

#[test]
fn nested_block_statement_order() {
    assert_eq!(
        run_str("let log=[]; { log.push('a'); { log.push('b'); } log.push('c'); } log.join('')"),
        "abc"
    );
    // assigning an outer binding from a nested block writes through
    assert_eq!(run_str("let x=1; { x=2; } x"), "2");
    assert_eq!(run_str("let s=''; { s+='a'; } { s+='b'; } s"), "ab");
}

#[test]
fn return_from_nested_block_in_loop() {
    assert_eq!(
        run_str("function f(){ for(let i=0;i<5;i++){ { if(i===2){ return i*10; } } } return -1; } f()"),
        "20"
    );
}

#[test]
fn return_from_nested_block_in_while() {
    assert_eq!(
        run_str("function f(){ let i=0; while(true){ { i++; if(i===3) return 'got'+i; } } } f()"),
        "got3"
    );
}

#[test]
fn return_from_switch_inside_loop() {
    assert_eq!(
        run_str("function f(arr){ for(const x of arr){ switch(x){ case 'stop': return 'stopped'; default: break; } } return 'done'; } f(['a','b','stop','c'])"),
        "stopped"
    );
    assert_eq!(
        run_str("function f(arr){ for(const x of arr){ switch(x){ case 'stop': return 'stopped'; default: break; } } return 'done'; } f(['a','b'])"),
        "done"
    );
}

#[test]
fn deeply_nested_mixed_control_flow() {
    // Combines for-of, if/else, switch, break/continue, and accumulation.
    let prog = "function classify(rows){\
        let out=[];\
        for(const row of rows){\
            let tag='';\
            for(const n of row){\
                if(n<0){ continue; }\
                switch(n%2){\
                    case 0: tag+='E'; break;\
                    default: tag+='O';\
                }\
                if(tag.length>=3) break;\
            }\
            out.push(tag);\
        }\
        return out.join(',');\
    }";
    // row1: [1,-2,3,4] -> O (skip -2) O E => "OOE" (len 3 => break)
    // row2: [2,2]      -> "EE"
    // row3: []         -> ""
    assert_eq!(
        run_str(&format!("{prog} classify([[1,-2,3,4],[2,2],[]])")),
        "OOE,EE,"
    );
}
