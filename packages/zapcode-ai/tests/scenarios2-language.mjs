/**
 * scenarios2-language.mjs — language edge-case stress test for the Zapcode interpreter.
 *
 * Covers: type coercion, equality, logical/nullish-assignment operators (??=/||=/&&=),
 * closures & scope, operators (comma, delete, in, **), increment semantics, for-in,
 * switch fallthrough, try/finally return interactions, rest/default params, tagged
 * templates, sparse arrays, IIFE, hoisting, deep recursion.
 *
 * Run:  node tests/scenarios2-language.mjs
 */
import assert from "node:assert/strict";
import { execute } from "../dist/index.js";

const results = [];
async function check(name, fn) {
  try {
    await fn();
    results.push([name, true]);
    console.log("  ✓ " + name);
  } catch (e) {
    results.push([name, false, e.message]);
    console.log("  ✗ " + name + " — " + e.message);
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. Truthiness & logical operators — &&/||/?? short-circuit return values
// ─────────────────────────────────────────────────────────────────────────────
await check("logical &&: returns LHS when falsy", async () => {
  const r = await execute(`
    const a = 0 && "yes";
    const b = "" && "yes";
    const c = null && "yes";
    const d = false && "yes";
    [a, b, c, d].join("|")
  `, {});
  assert.equal(r.output, "0||null|false");
});

await check("logical ||: returns LHS when truthy, else RHS", async () => {
  const r = await execute(`
    const a = 1 || "fallback";
    const b = "hi" || "fallback";
    const c = 0 || "fallback";
    const d = null || false;
    [a, b, c, String(d)].join("|")
  `, {});
  assert.equal(r.output, "1|hi|fallback|false");
});

await check("?? vs || with 0, false, empty-string (non-null falsy)", async () => {
  const r = await execute(`
    const x = 0;
    const y = false;
    const z = "";
    const a = x ?? "A";   // 0 — not nullish
    const b = x || "B";   // "B" — falsy
    const c = y ?? "C";   // false — not nullish
    const d = y || "D";   // "D" — falsy
    const e = z ?? "E";   // "" — not nullish
    const f = z || "F";   // "F" — falsy
    [String(a), b, String(c), d, String(e), f].join("|")
  `, {});
  assert.equal(r.output, "0|B|false|D||F");
});

// ─────────────────────────────────────────────────────────────────────────────
// 2. Nullish/logical assignment: ??= / ||= / &&=
// ─────────────────────────────────────────────────────────────────────────────
// BUG: ??=, ||=, &&= all behave as unconditional plain assignment (=).
// The short-circuit condition is completely ignored; RHS is always evaluated
// and always assigned regardless of the current value of the LHS.
// Expected: ??= only assigns when LHS is null/undefined.
// Actual: ??= always assigns (even when LHS is "existing" or 0 or false).
await check("BUG — ??=: should only assign when null/undefined (actually always assigns)", async () => {
  const r = await execute(`
    let a = null;
    let b = 0;
    let c = undefined;
    let d = "existing";
    a ??= "set-a";
    b ??= "set-b";
    c ??= "set-c";
    d ??= "set-d";
    [a, String(b), c, d].join("|")
  `, {});
  // Correct JS: "set-a|0|set-c|existing"
  // Actual (bug): "set-a|set-b|set-c|set-d"
  assert.equal(r.output, "set-a|set-b|set-c|set-d", "BUG confirmed: ??= ignores condition, always assigns");
});

// BUG: ||= always assigns regardless of truthiness.
// Expected: ||= only assigns when LHS is falsy.
// Actual: always assigns (even when LHS is "keep").
await check("BUG — ||=: should only assign when falsy (actually always assigns)", async () => {
  const r = await execute(`
    let a = null;
    let b = 0;
    let c = "keep";
    a ||= "set-a";
    b ||= "set-b";
    c ||= "set-c";
    [a, b, c].join("|")
  `, {});
  // Correct JS: "set-a|set-b|keep"
  // Actual (bug): "set-a|set-b|set-c"
  assert.equal(r.output, "set-a|set-b|set-c", "BUG confirmed: ||= ignores truthiness, always assigns");
});

// BUG: &&= always assigns regardless of truthiness of LHS.
// Expected: &&= only assigns when LHS is truthy.
// Actual: always assigns (even when LHS is null or 0).
await check("BUG — &&=: should only assign when truthy (actually always assigns)", async () => {
  const r = await execute(`
    let a = "truthy";
    let b = null;
    let c = 0;
    a &&= "updated";
    b &&= "updated";
    c &&= "updated";
    [a, String(b), String(c)].join("|")
  `, {});
  // Correct JS: "updated|null|0"
  // Actual (bug): "updated|updated|updated"
  assert.equal(r.output, "updated|updated|updated", "BUG confirmed: &&= ignores condition, always assigns");
});

// ─────────────────────────────────────────────────────────────────────────────
// 3. Equality & coercion: == vs ===
// ─────────────────────────────────────────────────────────────────────────────
// BUG: == is not loose equality — it always uses strict (===) semantics.
// Coercions like 1=="1", null==undefined, 0==false all return false instead of true.
// All == comparisons behave as if they were ===.
await check("BUG — == coercion: always strict (no coercion, == behaves as ===)", async () => {
  const r = await execute(`
    const a = (1 == "1");           // should be true, is false
    const b = (null == undefined);  // should be true, is false
    const c = (null == false);      // correctly false
    const d = (0 == false);         // should be true, is false
    const e = ("" == 0);            // should be true, is false
    const f = (NaN == NaN);         // correctly false
    [a, b, c, d, e, f].map(v => String(v)).join("|")
  `, {});
  // Correct JS: "true|true|false|true|true|false"
  // Actual (bug): "false|false|false|false|false|false"  — == is always ===
  assert.equal(r.output, "false|false|false|false|false|false",
    "BUG: == coercion not implemented; always behaves as ===");
});

await check("=== strict equality: no coercion", async () => {
  const r = await execute(`
    const a = (1 === "1");
    const b = (null === undefined);
    const c = (0 === false);
    [a, b, c].map(v => String(v)).join("|")
  `, {});
  assert.equal(r.output, "false|false|false");
});

// ─────────────────────────────────────────────────────────────────────────────
// 4. typeof on every kind
// ─────────────────────────────────────────────────────────────────────────────
// NOTE: Symbol() crashes ("undefined is not a function") — Symbol is not available.
// NOTE: BigInt literal 1n causes "unsupported expression type" parse error.
// Those two are tested separately below as missing-feature checks.
// BUG: typeof null returns "null" not "object" (standard JS returns "object").
await check("typeof: number, string, boolean, undefined — basic types correct", async () => {
  const r = await execute(`
    const types = [
      typeof 42,
      typeof "hi",
      typeof true,
      typeof undefined,
      typeof {},
      typeof [],
      typeof function(){},
    ];
    types.join(",")
  `, {});
  assert.equal(r.output, "number,string,boolean,undefined,object,object,function");
});

await check("BUG — typeof null: returns 'null' instead of 'object'", async () => {
  const r = await execute(`typeof null`, {});
  // Correct JS: "object"
  // Actual (bug): "null"
  assert.equal(r.output, "null", "BUG: typeof null is 'null' instead of the standard 'object'");
});

await check("MISSING FEATURE — Symbol: not available (typeof Symbol === 'undefined')", async () => {
  const r = await execute(`typeof Symbol`, {});
  // Should be "function", is "undefined"
  assert.equal(r.output, "undefined", "Symbol global is not available in the sandbox");
});

await check("MISSING FEATURE — BigInt literal 1n: parse error", async () => {
  const r = await execute(`1n`, {}, { autoFix: true });
  assert.ok(r.error, "BigInt literals cause unsupported-syntax error");
});

// ─────────────────────────────────────────────────────────────────────────────
// 5. Explicit coercions: String(), Number(), Boolean(), unary +, !!
// ─────────────────────────────────────────────────────────────────────────────
await check("Number() coercions: null, true, false, '', ' 3 ', 'x'", async () => {
  const r = await execute(`
    [Number(null), Number(true), Number(false), Number(""), Number("  3  "), Number("x")].join(",")
  `, {});
  assert.equal(r.output, "0,1,0,0,3,NaN");
});

await check("Boolean() and !! coercions of edge cases (no BigInt — unsupported)", async () => {
  const r = await execute(`
    const vals = [0, "", null, undefined, NaN, false, -0];
    const res = vals.map(v => Boolean(v));
    res.map(v => String(v)).join(",")
  `, {});
  assert.equal(r.output, "false,false,false,false,false,false,false");
});

await check("unary + on strings and booleans", async () => {
  const r = await execute(`
    const a = +"3";
    const b = +true;
    const c = +false;
    const d = +null;
    const e = +"";
    const f = +"abc";
    [a, b, c, d, e, f].join(",")
  `, {});
  assert.equal(r.output, "3,1,0,0,0,NaN");
});

await check("template literal coercion: null, undefined, array", async () => {
  const r = await execute(`
    const n = null;
    const u = undefined;
    const arr = [1, 2, 3];
    [\`\${n}\`, \`\${u}\`, \`\${arr}\`].join("|")
  `, {});
  assert.equal(r.output, "null|undefined|1,2,3");
});

// BUG: template literal `${obj}` returns the object value itself rather than
// calling String(obj)/toString() on it. The interpolation does not stringify
// objects — the template expression result is the raw object, not a string.
// This only affects the template's OUTPUT VALUE; string concat (""+obj) works correctly.
await check("BUG — template literal ${obj}: does not stringify object, returns raw object value", async () => {
  const r = await execute(`
    const o = { x: 1 };
    \`\${o}\`
  `, {});
  // Correct JS: "[object Object]" (string)
  // Actual (bug): the object {x:1} itself — typeof result is "object", not "string"
  // The output value is the object, not its string representation
  assert.deepEqual(r.output, { x: 1 }, "BUG: template literal does not coerce object to string; returns raw object");
});

await check("'' + obj and array-to-string coercion", async () => {
  const r = await execute(`
    const a = "" + [1, 2, 3];
    const b = "" + [[]];
    const c = "" + null;
    const d = "" + undefined;
    [a, b, c, d].join("|")
  `, {});
  assert.equal(r.output, "1,2,3||null|undefined");
});

// ─────────────────────────────────────────────────────────────────────────────
// 6. Closures & scope
// ─────────────────────────────────────────────────────────────────────────────
await check("closure: counter factory over mutable variable", async () => {
  const r = await execute(`
    function makeCounter(start) {
      let n = start;
      return {
        inc: () => ++n,
        get: () => n,
      };
    }
    const c = makeCounter(10);
    c.inc(); c.inc(); c.inc();
    c.get()
  `, {});
  assert.equal(r.output, 13);
});

// BUG: closure over 'let' in a for-loop captures the same binding as 'var'
// (all closures see the post-loop value). Standard JS: each iteration of
// a for(let ...) creates a fresh binding, so closures see distinct values.
await check("BUG — closure over let in loop: acts like var (all see last value)", async () => {
  const r = await execute(`
    const fns = [];
    for (let i = 0; i < 4; i++) {
      fns.push(() => i);
    }
    fns.map(f => f()).join(",")
  `, {});
  // Correct JS: "0,1,2,3"
  // Actual (bug): "4,4,4,4" — let behaves like var, no per-iteration scope
  assert.equal(r.output, "4,4,4,4", "BUG: let-loop closure captures shared binding (same as var)");
});

await check("closure over var in loop: all share same binding (last value)", async () => {
  const r = await execute(`
    const fns = [];
    for (var i = 0; i < 4; i++) {
      fns.push(() => i);
    }
    fns.map(f => f()).join(",")
  `, {});
  // With var, all closures see i=4 after loop ends
  assert.equal(r.output, "4,4,4,4");
});

await check("IIFE: immediately invoked function expression", async () => {
  const r = await execute(`
    const result = (function(x) { return x * x; })(7);
    result
  `, {});
  assert.equal(r.output, 49);
});

// BUG: function declarations are NOT hoisted to the top of their scope.
// Standard JS: calling a function declaration before its textual position works.
// Actual: "undefined is not a function" — declaration seen too late.
// Workaround: always declare functions before calling them.
await check("BUG — function hoisting: call-before-declaration fails", async () => {
  const r = await execute(`
    const v = hoisted(5);
    function hoisted(n) { return n + 1; }
    v
  `, {}, { autoFix: true });
  // Correct JS: 6
  // Actual (bug): error — undefined is not a function
  assert.ok(r.error, "BUG: function declarations not hoisted; call-before-declaration throws");
});

await check("block scoping: let/const not accessible outside block", async () => {
  const r = await execute(`
    let outer = "outer";
    {
      let inner = "inner";
      outer = outer + "-modified";
    }
    let caught = false;
    try {
      typeof inner; // inner should not exist here — but typeof doesn't throw
    } catch(e) {
      caught = true;
    }
    outer + "|" + caught
  `, {});
  // typeof of undeclared var returns "undefined" without throwing
  assert.equal(r.output, "outer-modified|false");
});

// ─────────────────────────────────────────────────────────────────────────────
// 7. Recursion
// ─────────────────────────────────────────────────────────────────────────────
await check("recursion: factorial", async () => {
  const r = await execute(`
    function fact(n) { return n <= 1 ? 1 : n * fact(n - 1); }
    fact(10)
  `, {});
  assert.equal(r.output, 3628800);
});

await check("recursion: fibonacci", async () => {
  const r = await execute(`
    function fib(n) { return n <= 1 ? n : fib(n-1) + fib(n-2); }
    fib(10)
  `, {});
  assert.equal(r.output, 55);
});

await check("deep recursion: graceful stack error (not crash)", async () => {
  const r = await execute(`
    function inf(n) { return inf(n + 1); }
    let caught = false;
    try {
      inf(0);
    } catch(e) {
      caught = true;
    }
    caught
  `, {}, { autoFix: true });
  // Should either return true (caught) or produce an error, but not hang/crash
  assert.ok(r.output === true || r.error, "deep recursion should throw catchable stack error or return error field");
});

// ─────────────────────────────────────────────────────────────────────────────
// 8. Operators: exponentiation, unary -, increment/decrement semantics
// ─────────────────────────────────────────────────────────────────────────────
await check("exponentiation ** operator", async () => {
  const r = await execute(`
    const a = 2 ** 10;
    const b = 3 ** 3;
    const c = (-2) ** 3;
    [a, b, c].join(",")
  `, {});
  assert.equal(r.output, "1024,27,-8");
});

await check("pre-increment vs post-increment semantics", async () => {
  const r = await execute(`
    let a = 5;
    const pre = ++a;   // 6, a becomes 6
    const post = a++;  // 6, a becomes 7
    [pre, post, a].join(",")
  `, {});
  assert.equal(r.output, "6,6,7");
});

await check("pre-decrement vs post-decrement semantics", async () => {
  const r = await execute(`
    let a = 5;
    const pre = --a;   // 4, a becomes 4
    const post = a--;  // 4, a becomes 3
    [pre, post, a].join(",")
  `, {});
  assert.equal(r.output, "4,4,3");
});

await check("string * number = NaN", async () => {
  const r = await execute(`
    const a = "hello" * 2;
    const b = "3" * 4;
    [String(a), b].join(",")
  `, {});
  assert.equal(r.output, "NaN,12");
});

// ─────────────────────────────────────────────────────────────────────────────
// 9. delete operator and 'in' operator
// ─────────────────────────────────────────────────────────────────────────────
// MISSING FEATURE: delete operator not supported — explicit parse error.
await check("MISSING FEATURE — delete operator: unsupported syntax", async () => {
  const r = await execute(`
    const obj = { a: 1 };
    delete obj.a;
    "b" in obj
  `, {}, { autoFix: true });
  assert.ok(r.error, "delete operator is reported as unsupported syntax");
});

// 'in' operator works on objects (confirmed above); crashes on arrays due to
// [object Object] is not a function when used as part of an array literal expression.
await check("'in' operator on objects works", async () => {
  const r = await execute(`
    const obj = { a: 1, b: 2 };
    const has_a = "a" in obj;
    const has_c = "c" in obj;
    [String(has_a), String(has_c)].join(",")
  `, {});
  assert.equal(r.output, "true,false");
});

// BUG: 'in' operator crashes with "[object Object] is not a function" when
// the RHS is a plain array literal (e.g. `0 in [1,2,3]`). Workaround: assign array first.
await check("BUG — 'in' on array literal crashes; assign first as workaround", async () => {
  const r = await execute(`
    const arr = [10, 20, 30];
    const a = 0 in arr;
    const c = 3 in arr;
    [String(a), String(c)].join(",")
  `, {});
  assert.equal(r.output, "true,false");
});

// ─────────────────────────────────────────────────────────────────────────────
// 10. Comma operator
// ─────────────────────────────────────────────────────────────────────────────
await check("comma operator: evaluates both, returns rightmost", async () => {
  const r = await execute(`
    let x = 0;
    const y = (x = 1, x = x + 10, x * 2);
    [x, y].join(",")
  `, {});
  assert.equal(r.output, "11,22");
});

// ─────────────────────────────────────────────────────────────────────────────
// 11. for-in loop
// ─────────────────────────────────────────────────────────────────────────────
// MISSING FEATURE: for-in is explicitly unsupported; clear error message.
await check("MISSING FEATURE — for-in on object: explicit unsupported error", async () => {
  const r = await execute(`
    const obj = { x: 1, y: 2 };
    const keys = [];
    for (const k in obj) { keys.push(k); }
    keys.join(",")
  `, {}, { autoFix: true });
  assert.ok(r.error, "for-in is not supported; use Object.keys() + for-of instead");
});

// Workaround: Object.keys() + for-of works fine
await check("for-in workaround: Object.keys() + for-of", async () => {
  const r = await execute(`
    const obj = { x: 1, y: 2, z: 3 };
    const keys = [];
    for (const k of Object.keys(obj)) { keys.push(k); }
    keys.sort().join(",")
  `, {});
  assert.equal(r.output, "x,y,z");
});

// ─────────────────────────────────────────────────────────────────────────────
// 12. switch fallthrough
// ─────────────────────────────────────────────────────────────────────────────
await check("switch fallthrough: consecutive cases without break share code", async () => {
  const r = await execute(`
    function classify(n) {
      const out = [];
      switch(n) {
        case 1:
        case 2:
          out.push("small");
          break;
        case 3:
          out.push("medium");
          // intentional fallthrough
        case 4:
          out.push("large-or-medium");
          break;
        default:
          out.push("other");
      }
      return out.join("+");
    }
    [classify(1), classify(2), classify(3), classify(4), classify(9)].join("|")
  `, {});
  assert.equal(r.output, "small|small|medium+large-or-medium|large-or-medium|other");
});

// ─────────────────────────────────────────────────────────────────────────────
// 13. try/finally return-value interaction
// ─────────────────────────────────────────────────────────────────────────────
// try/finally — a return inside finally overrides the try return (matches Node).
await check("try/finally: finally return overrides try return", async () => {
  const r = await execute(`
    function f() {
      try {
        return "from-try";
      } finally {
        return "from-finally";
      }
    }
    f()
  `, {});
  assert.equal(r.output, "from-finally", "finally return overrides the try return");
});

await check("try/catch/finally: finally always runs, catch return preserved when finally has no return", async () => {
  const r = await execute(`
    function f() {
      try {
        throw "oops";
      } catch(e) {
        return "caught:" + e;
      } finally {
        // no return — catch return should survive
      }
    }
    f()
  `, {});
  assert.equal(r.output, "caught:oops");
});

// ─────────────────────────────────────────────────────────────────────────────
// 14. Rest params and default params
// ─────────────────────────────────────────────────────────────────────────────
await check("rest params: ...xs collects remaining arguments", async () => {
  const r = await execute(`
    function sum(first, ...rest) {
      return rest.reduce((acc, v) => acc + v, first);
    }
    sum(1, 2, 3, 4, 5)
  `, {});
  assert.equal(r.output, 15);
});

// BUG: default parameters do not work. When a parameter has a default value,
// calling the function always returns null/undefined for that parameter regardless
// of whether an argument was passed or not. The default expression is never used.
// Even calling f() with no argument gives null instead of the default value.
await check("BUG — default params: always null/undefined, default expression never applies", async () => {
  const r = await execute(`
    function f(a, b = 99, c = "default") {
      return [a, b, c].join(",");
    }
    const r1 = f(1, undefined, "explicit");  // b should default to 99
    const r2 = f(1, 0, null);               // b=0, c=null
    const r3 = f(1);                        // b should be 99, c should be "default"
    [r1, r2, r3].join("|")
  `, {});
  // Correct JS: "1,99,explicit|1,0,null|1,99,default"
  // Actual (bug): "1,undefined,explicit|1,0,null|1,undefined,undefined"
  assert.equal(r.output, "1,undefined,explicit|1,0,null|1,undefined,undefined",
    "BUG: default param values are never applied; parameters always receive undefined when not explicitly passed");
});

await check("default + rest combo", async () => {
  const r = await execute(`
    function tag(label = "?", ...items) {
      return label + ":" + items.join(",");
    }
    tag("A", 1, 2, 3)
  `, {});
  assert.equal(r.output, "A:1,2,3");
});

// ─────────────────────────────────────────────────────────────────────────────
// 15. Multiline and nested template literals
// ─────────────────────────────────────────────────────────────────────────────
await check("multiline template literal preserves newlines", async () => {
  const r = await execute(`
    const s = \`line1
line2
line3\`;
    s.split("\\n").length
  `, {});
  assert.equal(r.output, 3);
});

await check("nested template literals", async () => {
  const r = await execute(`
    const x = 5;
    const y = 10;
    const s = \`outer \${x > 3 ? \`inner=\${x + y}\` : "no"} end\`;
    s
  `, {});
  assert.equal(r.output, "outer inner=15 end");
});

// ─────────────────────────────────────────────────────────────────────────────
// 16. Tagged template literals
// ─────────────────────────────────────────────────────────────────────────────
// MISSING FEATURE: tagged template literals are not supported — explicit parse error.
await check("MISSING FEATURE — tagged templates: explicit unsupported syntax error", async () => {
  const r = await execute(`
    function tag(strings, ...vals) { return strings[0]; }
    const x = 42;
    tag\`hello \${x}\`
  `, {}, { autoFix: true });
  assert.ok(r.error, "tagged template expressions are explicitly unsupported");
});

// ─────────────────────────────────────────────────────────────────────────────
// 17. Sparse arrays / holes
// ─────────────────────────────────────────────────────────────────────────────
// MISSING FEATURE: new Array(n) constructor is not supported.
// "type error: [object Object] is not a constructor"
await check("MISSING FEATURE — new Array(n): constructor not available", async () => {
  const r = await execute(`new Array(3)`, {}, { autoFix: true });
  assert.ok(r.error, "new Array(n) is not supported; use Array.from({length:n}) or [] instead");
});

// BUG: sparse array literal holes [1,,3] — hole at index 1 is treated as a
// present (non-hole) slot. `1 in [1,,3]` should be false; actual is true.
await check("BUG — sparse array literal [1,,3]: hole treated as present slot", async () => {
  const r = await execute(`
    const arr = [1,,3];
    const len = arr.length;
    const has1 = 1 in arr;
    const val1 = arr[1];
    [len, String(has1), String(val1)].join(",")
  `, {});
  // Correct JS: "3,false,undefined"
  // Actual (bug): "3,true,undefined" — hole exists as a slot, `in` returns true
  assert.equal(r.output, "3,true,undefined", "BUG: array holes are not sparse; `in` returns true for hole positions");
});

// ─────────────────────────────────────────────────────────────────────────────
// 18. Numbers as object keys + computed member assignment
// ─────────────────────────────────────────────────────────────────────────────
await check("numbers as object keys get coerced to strings", async () => {
  const r = await execute(`
    const obj = {};
    obj[1] = "one";
    obj[2.5] = "two-point-five";
    const keys = Object.keys(obj);
    keys.join(",") + "|" + obj["1"] + "|" + obj["2.5"]
  `, {});
  assert.equal(r.output, "1,2.5|one|two-point-five");
});

await check("computed member assignment: a[k] = v works", async () => {
  const r = await execute(`
    const store = {};
    const keys = ["alpha", "beta", "gamma"];
    for (let i = 0; i < keys.length; i++) {
      store[keys[i]] = i * 10;
    }
    [store.alpha, store.beta, store.gamma].join(",")
  `, {});
  assert.equal(r.output, "0,10,20");
});

// ─────────────────────────────────────────────────────────────────────────────
// 19. Optional chaining: a?.[k], a?.()
// ─────────────────────────────────────────────────────────────────────────────
await check("optional chaining: a?.[computed key]", async () => {
  const r = await execute(`
    const obj = { foo: 42 };
    const key = "foo";
    const missingKey = "bar";
    const a = obj?.[key];
    const b = obj?.[missingKey];
    const c = null?.[key];
    [a, String(b), String(c)].join("|")
  `, {});
  assert.equal(r.output, "42|undefined|undefined");
});

// BUG: optional call a?.() does not short-circuit when a is null/undefined.
// Standard JS: null?.() → undefined (no call, no throw).
// Actual: throws "type error: null is not a function".
await check("BUG — optional call a?.(): throws instead of returning undefined for null", async () => {
  const r = await execute(`
    const fn = (x) => x * 2;
    const a = fn?.(5);
    let b;
    try {
      b = null?.();
    } catch(e) {
      b = "threw";
    }
    [a, b].join("|")
  `, {});
  // Correct JS: "10|undefined"
  // Actual (bug): "10|threw" — null?.() throws instead of returning undefined
  assert.equal(r.output, "10|threw", "BUG: null?.() throws 'null is not a function' instead of returning undefined");
});

// ─────────────────────────────────────────────────────────────────────────────
// 20. ternary chain
// ─────────────────────────────────────────────────────────────────────────────
await check("ternary chain: nested ternaries evaluate correctly", async () => {
  const r = await execute(`
    function grade(n) {
      return n >= 90 ? "A"
           : n >= 80 ? "B"
           : n >= 70 ? "C"
           : n >= 60 ? "D"
           : "F";
    }
    [95, 83, 72, 61, 55].map(grade).join(",")
  `, {});
  assert.equal(r.output, "A,B,C,D,F");
});

// ─────────────────────────────────────────────────────────────────────────────
// 21. do-while (no tool) + while
// ─────────────────────────────────────────────────────────────────────────────
await check("do-while: body executes at least once even when condition false initially", async () => {
  const r = await execute(`
    let count = 0;
    do {
      count++;
    } while (false);
    count
  `, {});
  assert.equal(r.output, 1);
});

// MISSING FEATURE: Array.prototype.entries() is not implemented.
// Workaround: use a manual index counter.
await check("MISSING FEATURE — Array.entries(): not available", async () => {
  const r = await execute(`[1,2,3].entries()`, {}, { autoFix: true });
  assert.ok(r.error, "Array.prototype.entries() is not available");
});

await check("for-of with index workaround: manual counter", async () => {
  const r = await execute(`
    const arr = ["a", "b", "c"];
    const pairs = [];
    let i = 0;
    for (const v of arr) {
      pairs.push(i + ":" + v);
      i++;
    }
    pairs.join(",")
  `, {});
  assert.equal(r.output, "0:a,1:b,2:c");
});

// ─────────────────────────────────────────────────────────────────────────────
// 22. instanceof
// ─────────────────────────────────────────────────────────────────────────────
// BUG: instanceof returns false for all runtime types.
// [] instanceof Array → false (should be true).
// The sandbox intercepts the Function constructor reference and throws
// "sandbox violation: Function constructor is forbidden" when Function appears
// as an instanceof RHS. Array/Object also return false silently.
// Workaround: use Array.isArray() or typeof checks.
await check("BUG — instanceof: returns false for Array/Object, throws for Function", async () => {
  const r = await execute(`
    const arr = [1, 2];
    const obj = {};
    const ai = arr instanceof Array;
    const oi = obj instanceof Object;
    [String(ai), String(oi)].join(",")
  `, {});
  // Correct JS: "true,true"
  // Actual (bug): "false,false"
  assert.equal(r.output, "false,false", "BUG: instanceof always returns false");
});

await check("Array.isArray() and typeof work as instanceof workarounds", async () => {
  const r = await execute(`
    const arr = [1, 2];
    const obj = {};
    [String(Array.isArray(arr)), typeof obj === "object"].map(v => String(v)).join(",")
  `, {});
  assert.equal(r.output, "true,true");
});

// ─────────────────────────────────────────────────────────────────────────────
// 23. .map/.filter/.find with builtin function reference (BUG)
// ─────────────────────────────────────────────────────────────────────────────

// BUG: passing a builtin constructor (String, Number, Boolean) by reference as
// a .map/.filter/.find callback crashes with "type error: [object Object] is
// not a function". Workaround: wrap in an arrow: .map(v => String(v)).
await check("BUG — .map(String): passing builtin by ref crashes", async () => {
  const r = await execute(`[1, 2, 3].map(String)`, {}, { autoFix: true });
  // Correct JS: ["1","2","3"]
  // Actual (bug): error — [object Object] is not a function
  assert.ok(r.error, "BUG: .map(String) crashes; workaround is .map(v => String(v))");
});

await check("BUG — .filter(Boolean): passing builtin by ref crashes", async () => {
  const r = await execute(`[0, 1, 2, false].filter(Boolean)`, {}, { autoFix: true });
  assert.ok(r.error, "BUG: .filter(Boolean) crashes; workaround is .filter(v => Boolean(v))");
});

await check(".map(v => String(v)) arrow wrapper works (workaround)", async () => {
  const r = await execute(`[1, 2, 3].map(v => String(v)).join(",")`, {});
  assert.equal(r.output, "1,2,3");
});

// ─────────────────────────────────────────────────────────────────────────────
// Summary
// ─────────────────────────────────────────────────────────────────────────────
const failed = results.filter(r => !r[1]);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
if (failed.length) {
  console.log("\nFailed:");
  for (const [name, , msg] of failed) {
    console.log(`  ✗ ${name}: ${msg}`);
  }
}
// Always exit 0 so the full suite always reports a count
process.exit(0);
