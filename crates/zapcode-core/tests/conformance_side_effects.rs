//! Conformance: side-effect *counts*, *order*, and *laziness* — the bug
//! classes that output-only (value-asserting) tests cannot see.
//!
//! Motivation: we shipped a class of bugs where `f().x += 1` evaluated `f()`
//! twice. The final VALUE was right, so value tests passed. The only way to
//! catch that family is to thread counters and ordered logs through the
//! result, so the assertion captures HOW MANY TIMES and IN WHAT ORDER
//! side-effectful expressions ran — not just what the program returned.
//!
//! Seven classes are covered, one test fn per class:
//!   1. Evaluation order (operands, args, assignment target-vs-value,
//!      literals, spread, template interpolations, comma operator)
//!   2. Short-circuit laziness (&& || ??, ternary, optional chaining,
//!      default params, destructuring defaults, switch case tests)
//!   3. Conversion counts (valueOf/toString once per coercion site, hint
//!      preference, toJSON once per stringify)
//!   4. Iterator pull counts (destructuring pulls exactly k nexts for k
//!      bindings, for-of break does not over-pull, spread drains to done
//!      once, Symbol.iterator called once per loop)
//!   5. Callback invocation counts (map/filter/forEach = len; find/some
//!      stop at first hit; every stops at first miss; sort comparator not
//!      called for 0/1-element arrays)
//!   6. Exception-path effects (finally exactly once on throw/return/break,
//!      effects before a throw persist, catch binding is not re-evaluated)
//!   7. Getters/setters (get once per read site, spread invokes each getter
//!      once, compound assignment = one get + one set)
//!
//! EVERY expected string below was produced by real Node v24.14.1 first
//! (the snippet's completion value under `eval`), then asserted here.

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
// 1. EVALUATION ORDER
// ============================================================================

#[test]
fn evaluation_order() {
    // Binary operands evaluate left-to-right. Node: "a,b|3"
    assert_eq!(
        run_str(
            "let L = []; const f = (t, v) => (L.push(t), v); \
             const r = f('a', 1) + f('b', 2); L.join(',') + '|' + r"
        ),
        "a,b|3"
    );

    // Arguments evaluate left-to-right. Node: "x,y|12"
    assert_eq!(
        run_str(
            "let L = []; const f = (t, v) => (L.push(t), v); \
             const h = (x, y) => x * 10 + y; \
             const r = h(f('x', 1), f('y', 2)); L.join(',') + '|' + r"
        ),
        "x,y|12"
    );

    // The callee expression evaluates before any argument. Node: "callee,a,b|3"
    assert_eq!(
        run_str(
            "let L = []; const f = (t, v) => (L.push(t), v); \
             const h = (x, y) => x + y; \
             const get = () => (L.push('callee'), h); \
             const r = get()(f('a', 1), f('b', 2)); L.join(',') + '|' + r"
        ),
        "callee,a,b|3"
    );

    // Assignment: computed KEY evaluates before the VALUE. Node: "key,val|7"
    assert_eq!(
        run_str(
            "let L = []; const f = (t, v) => (L.push(t), v); \
             const obj = {}; obj[f('key', 'k')] = f('val', 7); \
             L.join(',') + '|' + obj.k"
        ),
        "key,val|7"
    );

    // Assignment: the TARGET object expression evaluates before the RHS.
    // Node: "target,val|5"
    assert_eq!(
        run_str(
            "let L = []; const f = (t, v) => (L.push(t), v); \
             const arr = [[0]]; const pick = () => (L.push('target'), arr[0]); \
             pick()[0] = f('val', 5); L.join(',') + '|' + arr[0][0]"
        ),
        "target,val|5"
    );

    // Object literal: properties (and computed keys) evaluate in source
    // order; a computed key evaluates before its own value.
    // Node: "v1,k,v2,v3|{\"a\":1,\"b\":2,\"c\":3}"
    assert_eq!(
        run_str(
            "let L = []; const f = (t, v) => (L.push(t), v); \
             const o = { a: f('v1', 1), [f('k', 'b')]: f('v2', 2), c: f('v3', 3) }; \
             L.join(',') + '|' + JSON.stringify(o)"
        ),
        "v1,k,v2,v3|{\"a\":1,\"b\":2,\"c\":3}"
    );

    // Array literal with spread: elements evaluate in source order.
    // Node: "e1,sp,e2|1234"
    assert_eq!(
        run_str(
            "let L = []; const f = (t, v) => (L.push(t), v); \
             const a = [f('e1', 1), ...f('sp', [2, 3]), f('e2', 4)]; \
             L.join(',') + '|' + a.join('')"
        ),
        "e1,sp,e2|1234"
    );

    // Template literal interpolations evaluate left-to-right. Node: "i1,i2|x1y2z"
    assert_eq!(
        run_str(
            "let L = []; const f = (t, v) => (L.push(t), v); \
             const s = `x${f('i1', 1)}y${f('i2', 2)}z`; L.join(',') + '|' + s"
        ),
        "i1,i2|x1y2z"
    );

    // Comma operator: both sides evaluate, result is the right side.
    // Node: "c1,c2|2"
    assert_eq!(
        run_str(
            "let L = []; const f = (t, v) => (L.push(t), v); \
             const r = (f('c1', 1), f('c2', 2)); L.join(',') + '|' + r"
        ),
        "c1,c2|2"
    );
}

// ============================================================================
// 2. SHORT-CIRCUIT LAZINESS
// ============================================================================

#[test]
fn short_circuit_laziness() {
    // && / || / ?? must NOT evaluate the RHS when short-circuited.
    // Node: "0|false,true,x"
    assert_eq!(
        run_str(
            "let n = 0; const f = () => (n++, true); \
             const a = false && f(); const b = true || f(); const c = 'x' ?? f(); \
             n + '|' + a + ',' + b + ',' + c"
        ),
        "0|false,true,x"
    );

    // …and MUST evaluate it (exactly once each) when not short-circuited.
    // Node: "3|F,F,F"
    assert_eq!(
        run_str(
            "let n = 0; const f = () => (n++, 'F'); \
             const a = true && f(); const b = false || f(); const c = null ?? f(); \
             n + '|' + a + ',' + b + ',' + c"
        ),
        "3|F,F,F"
    );

    // Ternary evaluates ONLY the taken branch. Node: "t1,e2|t1,e2"
    assert_eq!(
        run_str(
            "let L = []; const f = (t) => (L.push(t), t); \
             const r1 = true ? f('t1') : f('e1'); \
             const r2 = false ? f('t2') : f('e2'); \
             L.join(',') + '|' + r1 + ',' + r2"
        ),
        "t1,e2|t1,e2"
    );

    // Optional call on a nullish receiver skips the ENTIRE call, including
    // argument evaluation. Node: "0|true"
    assert_eq!(
        run_str(
            "let n = 0; const f = () => (n++, 1); const obj = null; \
             const r = obj?.m(f()); n + '|' + (r === undefined)"
        ),
        "0|true"
    );

    // …but evaluates arguments exactly once when the receiver exists.
    // Node: "1|2"
    assert_eq!(
        run_str(
            "let n = 0; const f = () => (n++, 1); \
             const o2 = { m: (x) => x + 1 }; const r = o2?.m(f()); n + '|' + r"
        ),
        "1|2"
    );

    // A nullish link mid-chain short-circuits the REST of the chain,
    // including trailing call arguments. Node: "0|true"
    assert_eq!(
        run_str(
            "let n = 0; const f = () => (n++, 1); const o = { a: undefined }; \
             const r = o.a?.b.c(f()); n + '|' + (r === undefined)"
        ),
        "0|true"
    );

    // Default parameter expressions run ONLY for missing/undefined arguments
    // (not for null). Node: "2|5,9,9,null"
    assert_eq!(
        run_str(
            "let n = 0; const f = () => (n++, 9); const d = (x = f()) => x; \
             const r1 = d(5); const r2 = d(); const r3 = d(undefined); const r4 = d(null); \
             n + '|' + r1 + ',' + r2 + ',' + r3 + ',' + r4"
        ),
        "2|5,9,9,null"
    );

    // Destructuring defaults fire ONLY on undefined (not null, not present).
    // Node: "2|1,7,7,null"
    assert_eq!(
        run_str(
            "let n = 0; const f = () => (n++, 7); \
             const { a = f() } = { a: 1 }; const { b = f() } = {}; \
             const [c = f()] = [undefined]; const [d = f()] = [null]; \
             n + '|' + a + ',' + b + ',' + c + ',' + d"
        ),
        "2|1,7,7,null"
    );

    // Switch case-test expressions evaluate in order, stopping at the match;
    // tests after the match are never evaluated. Node: "c1,c2"
    assert_eq!(
        run_str(
            "let L = []; const f = (t, v) => (L.push(t), v); \
             switch (2) { case f('c1', 1): break; case f('c2', 2): break; case f('c3', 3): break; } \
             L.join(',')"
        ),
        "c1,c2"
    );
}

// ============================================================================
// 3. CONVERSION COUNTS (valueOf / toString / toJSON)
// ============================================================================

#[test]
fn conversion_counts() {
    // `obj + 1` calls valueOf exactly once. Node: "1|8"
    assert_eq!(
        run_str(
            "let n = 0; const obj = { valueOf() { n++; return 7; } }; \
             const r = obj + 1; n + '|' + r"
        ),
        "1|8"
    );

    // Template interpolation calls toString exactly once. Node: "1|<S>"
    assert_eq!(
        run_str(
            "let n = 0; const obj = { toString() { n++; return 'S'; } }; \
             const r = `<${obj}>`; n + '|' + r"
        ),
        "1|<S>"
    );

    // String(obj) calls toString exactly once. Node: "1|S"
    assert_eq!(
        run_str(
            "let n = 0; const obj = { toString() { n++; return 'S'; } }; \
             const r = String(obj); n + '|' + r"
        ),
        "1|S"
    );

    // Relational comparison calls valueOf exactly once. Node: "1|true"
    assert_eq!(
        run_str(
            "let n = 0; const obj = { valueOf() { n++; return 3; } }; \
             const r = obj < 5; n + '|' + r"
        ),
        "1|true"
    );

    // Number-hinted coercion prefers valueOf and does NOT call toString.
    // Node: "1,0|2"
    assert_eq!(
        run_str(
            "let v = 0, s = 0; \
             const o = { valueOf() { v++; return 1; }, toString() { s++; return 'x'; } }; \
             const r = o + 1; v + ',' + s + '|' + r"
        ),
        "1,0|2"
    );

    // String-hinted coercion prefers toString and does NOT call valueOf.
    // Node: "0,1|x"
    assert_eq!(
        run_str(
            "let v = 0, s = 0; \
             const o = { valueOf() { v++; return 1; }, toString() { s++; return 'x'; } }; \
             const r = `${o}`; v + ',' + s + '|' + r"
        ),
        "0,1|x"
    );

    // JSON.stringify calls toJSON exactly once and serializes its result.
    // Node: "1|{\"b\":2}"
    assert_eq!(
        run_str(
            "let n = 0; const obj = { a: 1, toJSON() { n++; return { b: 2 }; } }; \
             const r = JSON.stringify(obj); n + '|' + r"
        ),
        "1|{\"b\":2}"
    );

    // Each nested toJSON-bearing value is converted exactly once. Node: "2|[1,2]"
    assert_eq!(
        run_str(
            "let n = 0; const mk = (id) => ({ toJSON() { n++; return id; } }); \
             const r = JSON.stringify([mk(1), mk(2)]); n + '|' + r"
        ),
        "2|[1,2]"
    );
}

// ============================================================================
// 4. ITERATOR PULL COUNTS
// ============================================================================

#[test]
fn iterator_pull_counts() {
    // for-of with break in the first iteration pulls exactly once. Node: "1,1|1"
    assert_eq!(
        run_str(
            "let nexts = 0, iters = 0; \
             const iter = { [Symbol.iterator]() { iters++; let i = 0; \
               return { next() { nexts++; i++; return { value: i, done: i > 5 }; } }; } }; \
             let got = 0; for (const v of iter) { got = v; break; } \
             iters + ',' + nexts + '|' + got"
        ),
        "1,1|1"
    );

    // …break in the second iteration pulls exactly twice. Node: "2|2"
    assert_eq!(
        run_str(
            "let nexts = 0; \
             const iter = { [Symbol.iterator]() { let i = 0; \
               return { next() { nexts++; i++; return { value: i, done: i > 5 }; } }; } }; \
             let got = 0; for (const v of iter) { got = v; if (v === 2) break; } \
             nexts + '|' + got"
        ),
        "2|2"
    );

    // Spread drains to done exactly once: 5 values = 6 next() calls
    // (the 6th observes done), one [Symbol.iterator]() call. Node: "1,6|12345"
    assert_eq!(
        run_str(
            "let nexts = 0, iters = 0; \
             const iter = { [Symbol.iterator]() { iters++; let i = 0; \
               return { next() { nexts++; i++; return { value: i, done: i > 5 }; } }; } }; \
             const a = [...iter]; iters + ',' + nexts + '|' + a.join('')"
        ),
        "1,6|12345"
    );

    // [Symbol.iterator]() itself is called exactly once PER loop. Node: "2|1212"
    assert_eq!(
        run_str(
            "let iters = 0; \
             const iter = { [Symbol.iterator]() { iters++; let i = 0; \
               return { next() { i++; return { value: i, done: i > 2 }; } }; } }; \
             let s = ''; for (const v of iter) s += v; for (const v of iter) s += v; \
             iters + '|' + s"
        ),
        "2|1212"
    );

    // A fully-drained for-of over 3 values makes exactly 4 next() calls. Node: "4|123"
    assert_eq!(
        run_str(
            "let nexts = 0; \
             const iter = { [Symbol.iterator]() { let i = 0; \
               return { next() { nexts++; i++; return { value: i, done: i > 3 }; } }; } }; \
             let s = ''; for (const v of iter) s += v; nexts + '|' + s"
        ),
        "4|123"
    );
}

/// DOCUMENTED STRUCTURAL DIVERGENCE — array destructuring over-pulls.
///
/// GROUND-TRUTHED against Node v24: `const [a, b] = iter` calls next()
/// exactly TWICE — k pulls for k bindings; JS does NOT pull an extra next()
/// to check done (it calls iterator.return() instead). Zapcode's destructure
/// lowering goes through `IterableToArray`, which eagerly drains the custom
/// iterator to done (6 pulls here) before indexing — the bound VALUES are
/// right, but the pull COUNT is not.
///
/// Fixing this needs either a new bounded-drain instruction (a wire-format
/// change: bytecode layout → FORMAT_VERSION bump) or re-lowering destructuring
/// onto GetIterator/IteratorNext, which would also change the documented
/// LENIENT behavior for non-iterables (`const [a] = 5` currently yields
/// undefined instead of Node's TypeError). Structural; not fixed here.
#[test]
#[ignore = "destructuring drains the iterable eagerly: 6 next() pulls instead of Node's 2"]
fn iterator_destructure_pulls_exactly_k() {
    assert_eq!(
        run_str(
            "let nexts = 0, iters = 0; \
             const iter = { [Symbol.iterator]() { iters++; let i = 0; \
               return { next() { nexts++; i++; return { value: i * 10, done: i > 5 }; } }; } }; \
             const [a, b] = iter; iters + ',' + nexts + '|' + a + ',' + b"
        ),
        "1,2|10,20"
    );
}

// ============================================================================
// 5. CALLBACK INVOCATION COUNTS
// ============================================================================

#[test]
fn callback_invocation_counts() {
    // map / filter / forEach call the callback exactly len times. Node: "3,3,3|246"
    assert_eq!(
        run_str(
            "let m = 0, f = 0, e = 0; const a = [1, 2, 3]; \
             const r = a.map(x => { m++; return x * 2; }); \
             a.filter(x => { f++; return x > 1; }); \
             a.forEach(x => { e++; }); \
             m + ',' + f + ',' + e + '|' + r.join('')"
        ),
        "3,3,3|246"
    );

    // find stops at the first hit. Node: "2|5"
    assert_eq!(
        run_str(
            "let n = 0; const r = [1, 5, 2, 7].find(x => { n++; return x > 4; }); n + '|' + r"
        ),
        "2|5"
    );

    // findIndex stops at the first hit. Node: "2|1"
    assert_eq!(
        run_str(
            "let n = 0; const r = [1, 5, 2, 7].findIndex(x => { n++; return x > 4; }); n + '|' + r"
        ),
        "2|1"
    );

    // some stops at the first true. Node: "2|true"
    assert_eq!(
        run_str(
            "let n = 0; const r = [1, 5, 2].some(x => { n++; return x > 4; }); n + '|' + r"
        ),
        "2|true"
    );

    // every stops at the first false. Node: "2|false"
    assert_eq!(
        run_str(
            "let n = 0; const r = [5, 1, 9].every(x => { n++; return x > 4; }); n + '|' + r"
        ),
        "2|false"
    );

    // The sort comparator is NEVER called for 0- or 1-element arrays
    // (and IS called for a 2-element one). Node: "0|true"
    assert_eq!(
        run_str(
            "let n = 0; const cmp = (a, b) => { n++; return a - b; }; \
             [].sort(cmp); [9].sort(cmp); const after0 = n; \
             [2, 1].sort(cmp); after0 + '|' + (n > 0)"
        ),
        "0|true"
    );

    // reduce: len calls with an initial value, len-1 without. Node: "3,2|6,6"
    assert_eq!(
        run_str(
            "let n = 0; const r = [1, 2, 3].reduce((a, x) => { n++; return a + x; }, 0); \
             let m = 0; const r2 = [1, 2, 3].reduce((a, x) => { m++; return a + x; }); \
             n + ',' + m + '|' + r + ',' + r2"
        ),
        "3,2|6,6"
    );
}

// ============================================================================
// 6. EXCEPTION-PATH EFFECTS
// ============================================================================

#[test]
fn exception_path_effects() {
    // finally runs exactly once on the throw path. Node: "t,f,c"
    assert_eq!(
        run_str(
            "let L = []; \
             try { try { L.push('t'); throw new Error('x'); } finally { L.push('f'); } } \
             catch (e) { L.push('c'); } L.join(',')"
        ),
        "t,f,c"
    );

    // finally runs exactly once on the return path. Node: "t,f|r"
    assert_eq!(
        run_str(
            "let L = []; \
             const g = () => { try { L.push('t'); return 'r'; } finally { L.push('f'); } }; \
             const r = g(); L.join(',') + '|' + r"
        ),
        "t,f|r"
    );

    // finally runs exactly once on the break path (and once per normal
    // iteration before it). Node: "i1,f1,i2,f2"
    assert_eq!(
        run_str(
            "let L = []; \
             for (const i of [1, 2, 3]) { \
               try { L.push('i' + i); if (i === 2) break; } finally { L.push('f' + i); } } \
             L.join(',')"
        ),
        "i1,f1,i2,f2"
    );

    // Side effects performed before a throw persist. Node: "1,2"
    assert_eq!(
        run_str(
            "let arr = []; \
             try { arr.push(1); arr.push(2); null.x; arr.push(3); } catch (e) {} \
             arr.join(',')"
        ),
        "1,2"
    );

    // The catch binding does not re-evaluate the thrown expression: the
    // pre-throw increment ran exactly once, and reading `e` twice yields the
    // same value with no extra effects. Node: "boom:boom:1"
    assert_eq!(
        run_str(
            "let n = 0; let out = ''; \
             try { n++; throw 'boom'; } catch (e) { out = e + ':' + e + ':' + n; } out"
        ),
        "boom:boom:1"
    );

    // finally runs exactly once even when catch rethrows. Node: "c1,f1,c2"
    assert_eq!(
        run_str(
            "let L = []; \
             try { try { throw new Error('a'); } catch (e) { L.push('c1'); throw e; } \
                   finally { L.push('f1'); } } \
             catch (e) { L.push('c2'); } L.join(',')"
        ),
        "c1,f1,c2"
    );
}

// ============================================================================
// 7. GETTERS / SETTERS
// ============================================================================

#[test]
fn getter_setter_invocation_counts() {
    // A get accessor is invoked exactly once per read site:
    // `obj.x + obj.x` = two gets. Node: "2|10"
    assert_eq!(
        run_str(
            "let g = 0; const obj = { get x() { g++; return 5; } }; \
             const r = obj.x + obj.x; g + '|' + r"
        ),
        "2|10"
    );

    // Spreading an object invokes each getter exactly once and snapshots the
    // values. Node: "1,1|{\"a\":1,\"b\":2}"
    assert_eq!(
        run_str(
            "let g1 = 0, g2 = 0; \
             const obj = { get a() { g1++; return 1; }, get b() { g2++; return 2; } }; \
             const c = { ...obj }; g1 + ',' + g2 + '|' + JSON.stringify(c)"
        ),
        "1,1|{\"a\":1,\"b\":2}"
    );

    // Compound assignment on an accessor pair: exactly one get + one set.
    // Node: "1,1|15"
    assert_eq!(
        run_str(
            "let g = 0, s = 0, backing = 10; \
             const obj = { get x() { g++; return backing; }, set x(v) { s++; backing = v; } }; \
             obj.x += 5; g + ',' + s + '|' + backing"
        ),
        "1,1|15"
    );

    // Postfix increment on an accessor pair: one get + one set, expression
    // yields the OLD value. Node: "1,1|3,4"
    assert_eq!(
        run_str(
            "let g = 0, s = 0, backing = 3; \
             const obj = { get x() { g++; return backing; }, set x(v) { s++; backing = v; } }; \
             const r = obj.x++; g + ',' + s + '|' + r + ',' + backing"
        ),
        "1,1|3,4"
    );

    // Plain assignment invokes ONLY the setter — never the getter. Node: "0,1|42"
    assert_eq!(
        run_str(
            "let g = 0, s = 0, backing = 0; \
             const obj = { get x() { g++; return backing; }, set x(v) { s++; backing = v; } }; \
             obj.x = 42; g + ',' + s + '|' + backing"
        ),
        "0,1|42"
    );
}
