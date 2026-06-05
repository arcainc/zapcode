/**
 * Regression tests for host-boundary marshalling (stress-pass cluster L).
 * A tool returning BigInt/Infinity/NaN/undefined used to abort the whole
 * process (uncatchable Rust panic / serde error); a no-arg tool called as
 * `tool({})` was wrongly rejected.
 *
 * Run: npm run build && node tests/marshalling.mjs
 */
import assert from "node:assert/strict";
import { execute } from "../dist/index.js";

let passed = 0;
async function test(name, fn) {
  await fn();
  passed++;
  console.log(`  PASS ${name}`);
}

const tool = execute_ => ({ description: "", parameters: {}, execute: execute_ });

console.log("host-boundary marshalling");

await test("L1: BigInt return marshals to a number (no process abort)", async () => {
  const r = await execute(`const x = await f(); typeof x + ':' + x`, { f: tool(async () => 10n) });
  assert.equal(r.output, "number:10");
});

await test("L2: non-finite return marshals to null", async () => {
  const inf = await execute(`(await f()) === null`, { f: tool(async () => Infinity) });
  assert.equal(inf.output, true);
  const nan = await execute(`const x = await f(); x.score`, { f: tool(async () => ({ ok: true, score: NaN })) });
  assert.equal(nan.output, null);
});

await test("L3: undefined return is usable (void tools)", async () => {
  const r = await execute(`const x = await save(); x === null`, { save: tool(async () => undefined) });
  assert.equal(r.output, true);
});

await test("L3: undefined object properties are dropped", async () => {
  const r = await execute(`const x = await f(); Object.keys(x).join(',')`, {
    f: tool(async () => ({ a: 1, b: undefined, c: 3 })),
  });
  assert.equal(r.output, "a,c");
});

await test("L5: a no-arg tool may be called as tool({})", async () => {
  const r = await execute(`await ping({}); 'ok'`, { ping: tool(async () => ({ done: true })) });
  assert.equal(r.output, "ok");
});

await test("Date return marshals to an ISO string", async () => {
  const r = await execute(`await f()`, { f: tool(async () => new Date(1700000000123)) });
  assert.equal(r.output, "2023-11-14T22:13:20.123Z");
});

// ── Reference-cycle / deep-recursion DoS hardening (must never abort) ──
// Each of these used to SIGSEGV the host process (exit 139) via unbounded
// native recursion. They must now surface a catchable error or a bounded value,
// and the host must survive — which is implicit in this script reaching the end.

await test("cycle returned as result is a catchable error, not a host abort", async () => {
  await assert.rejects(
    () => execute(`const a = []; a.push(a); a`, {}),
    /circular structure/i,
  );
});

await test("JSON.stringify(cycle) throws the JS circular-structure error in-guest", async () => {
  const r = await execute(
    `const a=[]; a.push(a); let ok=false; try { JSON.stringify(a); } catch(e){ ok = (''+e).indexOf('circular') >= 0; } ok`,
    {},
  );
  assert.equal(r.output, true);
});

await test("String()/template/join of a cycle are bounded (no abort)", async () => {
  for (const expr of ["String(a)", "`${a}`", "a.join(',')"]) {
    const r = await execute(`const a=[]; a.push(a); typeof (${expr}) === 'string'`, {});
    assert.equal(r.output, true, `expected ${expr} to produce a bounded string`);
  }
});

await test("structuredClone(cycle) round-trips, preserving the self-reference", async () => {
  const r = await execute(`const a=[]; a.push(a); const c = structuredClone(a); (c !== a) && (c[0] === c)`, {});
  assert.equal(r.output, true);
});

await test("a cyclic value passed to a tool is a catchable error, not a host abort", async () => {
  await assert.rejects(
    () => execute(`const a=[]; a.push(a); await echo(a); 1`, {
      echo: { description: "", parameters: { x: { type: "object", optional: true } }, execute: async (args) => args.x },
    }),
    /circular structure/i,
  );
});

await test("deeply nested literal is a catchable parse error, not a host abort", async () => {
  const code = `const x = ${"[".repeat(5000)}${"]".repeat(5000)}; 1`;
  await assert.rejects(() => execute(code, {}), /nesting depth/i);
});

await test("runtime-built deep JSON.stringify is catchable, not a host abort", async () => {
  const code = `let a={x:1}; for(let i=0;i<9000;i++){a={n:a};} JSON.stringify(a).length`;
  await assert.rejects(() => execute(code, {}, { memoryLimitMb: 64, timeLimitMs: 20000 }), /nesting depth/i);
});

await test("Array.from({length: huge}) hits the memory limit (no untracked OOM)", async () => {
  await assert.rejects(
    () => execute(`const a = Array.from({length: 50000000}); a.length`, {}, { memoryLimitMb: 8, timeLimitMs: 20000 }),
    /memory limit|allocation limit/i,
  );
});

// ── String/array indexing & flatten host-abort regressions (audit C1/C2/C3) ──
// Each of these aborted the host process on guest input (SIGSEGV/SIGABRT) and is
// uncatchable from JS; they must now return the correct value or a catchable
// error, with the host surviving.

await test("substring/slice on multibyte input do not SIGABRT the host", async () => {
  // C2: substring across a multibyte boundary used to panic on a non-char byte index.
  const a = await execute(`"ééé".substring(1, 2)`, {});
  assert.equal(a.output, "é");
  // C3: indexOf returns a char index; slice must consume it as a char index, not bytes.
  const b = await execute(`const u = "é://host"; u.slice(0, u.indexOf(":"))`, {});
  assert.equal(b.output, "é");
});

await test("flat(Infinity) on a self-cyclic array is catchable, not a host abort", async () => {
  const r = await execute(
    `const a = []; a.push(a); let ok = false; try { a.flat(Infinity); } catch (e) { ok = true; } ok`,
    {},
  );
  assert.equal(r.output, true);
  // acyclic deep flatten is unaffected
  const ok = await execute(`[1,[2,[3,[4,[5]]]]].flat(Infinity).join(",")`, {});
  assert.equal(ok.output, "1,2,3,4,5");
});

await test("padStart/join past the cap hit the memory limit", async () => {
  await assert.rejects(
    () => execute(`'x'.padStart(200000000, 'ab').length`, {}, { memoryLimitMb: 8, timeLimitMs: 20000 }),
    /memory limit/i,
  );
  await assert.rejects(
    () => execute(`const s='x'.repeat(1000000); const a=[]; let i=0; while(i<300){a.push(s);i=i+1;} a.join('').length`, {}, { memoryLimitMb: 8, timeLimitMs: 20000 }),
    /memory limit/i,
  );
});

// ── User `__`-prefixed keys must survive the host boundary ──
// value_to_json hides only the EXACT reserved internal markers, never a blanket
// `__`-prefix, so real user keys like `__id__`/`__typename`/`__v` marshal out
// while VM brands (e.g. a class instance's `__class__`) never leak.
await test("user __-keys survive marshalling to the host", async () => {
  const r = await execute(`({__id__: 42, name: "x", __typename: "User", __v: 1})`, {});
  assert.deepEqual(r.output, { __id__: 42, name: "x", __typename: "User", __v: 1 });
});

await test("class instance brands do NOT leak across the host boundary", async () => {
  const r = await execute(`class C { constructor(){ this.a = 1; this.b = 2; } } new C()`, {});
  assert.deepEqual(r.output, { a: 1, b: 2 });
});

await test("user __-keys round-trip through a tool argument and back", async () => {
  const r = await execute(
    `const x = await echo({payload: {__id__: 7, q: "hi"}}); x.__id__ + ":" + x.q`,
    {
      echo: {
        description: "",
        parameters: { payload: { type: "object" } },
        execute: async (args) => args.payload,
      },
    },
  );
  assert.equal(r.output, "7:hi");
});

// ── Date edge-correctness marshals across the host boundary (no abort) ──
// Out-of-range ISO clock fields, far-future bare-integer strings, non-finite
// Date.UTC components, two-digit-year MakeFullYear, and the ±8.64e15 time clip
// must each reach the host as the Node-matching value, never crash the process.
await test("date conformance edges marshal to the host like Node", async () => {
  // ISO hour > 24 is Invalid Date (NaN time value), not a rollover.
  let r = await execute(`isNaN(new Date("2020-01-01T25:00:00Z").getTime())`, {});
  assert.equal(r.output, true);
  // Hour 24 (only as 24:00:00.000) is the one allowed over-23 value.
  r = await execute(`new Date("2020-01-01T24:00:00Z").toISOString()`, {});
  assert.equal(r.output, "2020-01-02T00:00:00.000Z");
  // A bare far-future integer string overflows the Date window -> NaN.
  r = await execute(`isNaN(new Date("1234567890123").getTime())`, {});
  assert.equal(r.output, true);
  // A non-finite Date.UTC component yields NaN, not a silent 0.
  r = await execute(`isNaN(Date.UTC(2020, NaN, 1))`, {});
  assert.equal(r.output, true);
  // Two-digit year maps to 1900+yy (MakeFullYear) through Date.UTC.
  r = await execute(`new Date(Date.UTC(99, 0, 1)).getUTCFullYear()`, {});
  assert.equal(r.output, 1999);
  // ±8.64e15 ms is the valid boundary; one past it clips to Invalid Date.
  r = await execute(`new Date(8640000000000000).getTime()`, {});
  assert.equal(r.output, 8640000000000000);
  r = await execute(`isNaN(new Date(8640000000000001).getTime())`, {});
  assert.equal(r.output, true);
});

// ── ASCII regex shorthands + sticky /y marshal across the host boundary ──
// JS shorthands (\d \w …) are ASCII-only, unlike the regex crate's Unicode
// default, and /y must anchor at lastIndex. A non-ASCII subject (Arabic-Indic
// digits, accented letters) must reach the host as the Node-matching boolean,
// never crash the process (no exit 139/134).
await test("ascii regex shorthands + sticky /y marshal to the host like Node", async () => {
  // \d is ASCII-only: an Arabic-Indic digit string is rejected by ID/ZIP rules.
  let r = await execute(`/^\\d{5}$/.test("١٢٣٤٥")`, {});
  assert.equal(r.output, false);
  r = await execute(`/^\\d{5}$/.test("12345")`, {});
  assert.equal(r.output, true);
  // \w does not match accented letters; a slug with "café" is rejected.
  r = await execute(`/^[\\w-]+$/.test("café-slug")`, {});
  assert.equal(r.output, false);
  r = await execute(`/^[\\w-]+$/.test("my-slug-1")`, {});
  assert.equal(r.output, true);
  // Sticky /y anchors at lastIndex: 'a' is not at index 0 of "baa".
  r = await execute(`const re = /a/y; re.lastIndex = 0; re.test("baa")`, {});
  assert.equal(r.output, false);
  r = await execute(`const re = /a/y; re.lastIndex = 1; re.test("baa")`, {});
  assert.equal(r.output, true);
});

// ── Non-constructible Function global + in/instanceof guards (no abort) ──
// Referencing `Function` (typeof / instanceof) used to raise an UNCATCHABLE
// sandbox violation that killed the whole guest program at parse time. It must
// now resolve to a non-constructible value: typeof === "function", a function
// literal is `instanceof Function`, and a forbidden CALL is a catchable error.
// `in`/`instanceof` with a bad RHS must throw a catchable TypeError, not abort.
await test("Function global + in/instanceof guards marshal to the host like Node", async () => {
  // `typeof Function` is "function"; referencing it no longer aborts.
  let r = await execute(`typeof Function`, {});
  assert.equal(r.output, "function");
  // A function literal is an instance of Function (and Object).
  r = await execute(`(function f(){}) instanceof Function`, {});
  assert.equal(r.output, true);
  r = await execute(`(() => 1) instanceof Object`, {});
  assert.equal(r.output, true);
  // `instanceof` with a non-callable RHS is a catchable TypeError.
  r = await execute(`let ok=false; try{({}) instanceof 5}catch(e){ok=(e instanceof TypeError)} ok`, {});
  assert.equal(r.output, true);
  // `in` on a primitive RHS is a catchable TypeError (not silent false).
  r = await execute(`let ok=false; try{"length" in "abc"}catch(e){ok=(e instanceof TypeError)} ok`, {});
  assert.equal(r.output, true);
  // Actually calling Function is still forbidden, but the violation is now
  // catchable in-guest and the program runs to completion (no exit 139/134).
  r = await execute(`let ok=false; try{ Function("return 1") }catch(e){ ok=true } ok`, {});
  assert.equal(r.output, true);
  r = await execute(`let ok=false; try{ new Function("return 1") }catch(e){ ok=true } ok`, {});
  assert.equal(r.output, true);
});

// ── Strict JSON.parse grammar + escapes + reviver `this` marshal to host ──
// JSON.parse must reject the inputs Node rejects (Infinity/NaN, unquoted keys,
// trailing commas, …) as CATCHABLE errors, decode \uXXXX / short escapes via a
// single left-to-right scan, and bind the reviver's `this` to the holder — all
// without aborting the process (no exit 139/134).
await test("strict JSON.parse + escapes + reviver this marshal to the host like Node", async () => {
  // Each malformed input is a catchable error, and the program completes.
  for (const bad of [
    "Infinity", "NaN", "+1", "01", "1.", ".5", "0x1F",
    "[1,2,]", "[1,,2]", '{"a":1,}', "{a:1}", "'abc'", "[[1,],2]", "",
  ]) {
    const src = `let ok=false; try{ JSON.parse(${JSON.stringify(bad)}) }catch(e){ ok=true } ok`;
    const r = await execute(src, {});
    assert.equal(r.output, true, `expected JSON.parse(${JSON.stringify(bad)}) to throw`);
  }
  // Valid numbers parse (no spurious rejection).
  let r = await execute(`JSON.parse("1e3")`, {});
  assert.equal(r.output, 1000);
  // \uXXXX decodes to its code point (length 1), not left literal.
  r = await execute(`JSON.parse('"\\u0041"')`, {});
  assert.equal(r.output, "A");
  r = await execute(`JSON.parse('"\\u0041"').length`, {});
  assert.equal(r.output, 1);
  r = await execute(`JSON.parse('"\\u0041\\u0042"')`, {});
  assert.equal(r.output, "AB");
  // A doubled backslash then `n` is a literal backslash + n (NOT a newline).
  r = await execute(`JSON.stringify(JSON.parse('"a\\\\nb"'))`, {});
  assert.equal(r.output, '"a\\nb"');
  // Reviver `this` is the holder, so a reviver can read its sibling keys.
  r = await execute(
    `JSON.stringify(JSON.parse('{"a":1,"b":2}', function(k,v){ return k==='a' ? this.b : v; }))`,
    {},
  );
  assert.equal(r.output, '{"a":2,"b":2}');
  // A reviver returning undefined deletes the property (no cyclic holder splice).
  r = await execute(
    `JSON.stringify(JSON.parse('{"a":1,"b":2}', (k, v) => k === 'b' ? undefined : v))`,
    {},
  );
  assert.equal(r.output, '{"a":1}');
});

// ── i64 arithmetic degrades to f64 past 2^53 (host-boundary numeric parity) ──
// Past Number.MAX_SAFE_INTEGER an Int result must round like a JS double, and
// the value must marshal across the napi boundary as a plain JS number matching
// Node's own arithmetic (it used to stay an exact i64 → the wrong value).
await test("large-integer arithmetic + parseInt overflow marshal to the host like Node", async () => {
  let r = await execute(`9007199254740991 + 2`, {});
  assert.equal(typeof r.output, "number");
  assert.equal(r.output, 9007199254740991 + 2); // 9007199254740992 in JS
  assert.equal(r.output, 9007199254740992);

  r = await execute(`9007199254740992 + 1 === 9007199254740992`, {});
  assert.equal(r.output, true);

  r = await execute(`(function(){let x=9007199254740992; return x+1===x})()`, {});
  assert.equal(r.output, true);

  // parseInt past i64 range returns the f64, not NaN.
  r = await execute(`parseInt("9999999999999999999")`, {});
  assert.equal(typeof r.output, "number");
  assert.equal(r.output, parseInt("9999999999999999999")); // 1e19
  assert.equal(r.output, 1e19);

  r = await execute(`String(parseInt("9999999999999999999"))`, {});
  assert.equal(r.output, "10000000000000000000");
});

// ── ECMA-262 Number::toString: exponential notation crosses the host boundary ──
// The shared formatter drives String()/template/join/JSON.stringify inside the
// VM. Guest stdout (console.log) and JSON output must match Node's String()
// exactly — previously large/small magnitudes printed full positional decimals.
await test("number stringification uses JS exponential notation like Node", async () => {
  let r = await execute(`console.log(String(1e21), String(1e-7), String(2 ** 70))`, {});
  assert.equal(r.stdout.trim(), "1e+21 1e-7 1.1805916207174113e+21");

  // JSON.stringify (runs in-VM) must serialize numbers the same way.
  r = await execute(`JSON.stringify({ big: 1e21, small: 1e-7, mid: 123.456 })`, {});
  assert.equal(r.output, JSON.stringify({ big: 1e21, small: 1e-7, mid: 123.456 }));
  assert.equal(r.output, '{"big":1e+21,"small":1e-7,"mid":123.456}');

  // Array.join routes through the same formatter.
  r = await execute(`[1e21, 1e-7, 12345678].join(",")`, {});
  assert.equal(r.output, [1e21, 1e-7, 12345678].join(","));
});

// ── IEEE-754 negative zero is preserved through the host boundary ──
// Unary `-0` yields a real Float(-0.0) (not the integer 0), so its sign is
// observable in division and Object.is/SameValue while ToString still drops it.
await test("negative zero keeps its sign across the host boundary like Node", async () => {
  let r = await execute(`String(1 / -0)`, {});
  assert.equal(r.output, "-Infinity");

  r = await execute(`Object.is(-0, 0)`, {});
  assert.equal(r.output, false);

  r = await execute(`Object.is(-0, -0)`, {});
  assert.equal(r.output, true);

  // 0 / -5 is -0; its sign is visible to Object.is but ToString renders "0".
  r = await execute(`[Object.is(0 / -5, -0), String(0 / -5)]`, {});
  assert.deepEqual(r.output, [true, "0"]);

  // -0 used as an index/key behaves exactly like +0.
  r = await execute(`(function(){ const o = {}; o[-0] = 7; return o[0]; })()`, {});
  assert.equal(r.output, 7);
});

// ── labeled break out of a plain block does not run away ──
// `break label` on a labeled non-loop block used to emit an unpatched jump to
// instruction 0 → infinite loop → allocation-limit error on guest input. It
// must now complete promptly with the JS-correct result.
await test("labeled break on a plain block completes (no runaway) like Node", async () => {
  let r = await execute(`(function(){ let r=''; foo:{ r+='a'; break foo; r+='b'; } return r+'c'; })()`, {});
  assert.equal(r.output, "ac");

  // An unlabeled break inside such a block, nested in a loop, breaks the loop.
  r = await execute(`(function(){ let r=''; for(let i=0;i<3;i++){ blk:{ r+=i; break; } r+='x'; } return r; })()`, {});
  assert.equal(r.output, "0");
});

// ── Error.cause + no phantom methods on constructed instances ──
// `new Error(msg,{cause})` exposes `cause`; a chained read on any `new
// Builtin()` resolves against the instance, not the global constructor.
await test("Error cause and instance property reads marshal like Node", async () => {
  let r = await execute(`new Error("e", { cause: "c" }).cause`, {});
  assert.equal(r.output, "c");

  r = await execute(`new Error("e").cause === undefined`, {});
  assert.equal(r.output, true);

  // No phantom function for an arbitrary key on a freshly-constructed instance.
  r = await execute(`[typeof new Error("e").zzz, typeof new Map().zzz, typeof new Date().zzz]`, {});
  assert.deepEqual(r.output, ["undefined", "undefined", "undefined"]);

  // Real members and toString still work.
  r = await execute(`[new Error("boom").message, String(new TypeError("x"))]`, {});
  assert.deepEqual(r.output, ["boom", "TypeError: x"]);
});

// ── Map/Set iterators have a real .next() and stay iterable across the boundary ──
await test("Map/Set iterators expose next() and feed for-of/spread/Array.from like Node", async () => {
  let r = await execute(`JSON.stringify(new Map([["a", 1]]).entries().next())`, {});
  assert.equal(r.output, JSON.stringify({ value: ["a", 1], done: false }));

  r = await execute(`(function(){ let it = new Set([5,6]).values(); return [it.next().value, it.next().value, it.next().done]; })()`, {});
  assert.deepEqual(r.output, [5, 6, true]);

  r = await execute(`[...new Map([["a",1],["b",2]]).keys()]`, {});
  assert.deepEqual(r.output, ["a", "b"]);

  r = await execute(`Array.from(new Set([5,6]).values())`, {});
  assert.deepEqual(r.output, [5, 6]);

  // Map/Set built from another collection's iterator.
  r = await execute(`(function(){ let m = new Map([["a",1]]); return new Map(m.entries()).get("a"); })()`, {});
  assert.equal(r.output, 1);
});

// ── object-literal getters/setters work and enumerate like Node ──
await test("object-literal accessors invoke + enumerate across the host boundary", async () => {
  let r = await execute(`({ get x() { return 42; } }).x`, {});
  assert.equal(r.output, 42);

  r = await execute(`(function(){ let o = { _x: 0, set x(v) { this._x = v * 2; } }; o.x = 7; return o._x; })()`, {});
  assert.equal(r.output, 14);

  // Enumerable in source order; JSON invokes the getter.
  r = await execute(`Object.keys({ a: 1, get b() { return 2; }, c: 3 })`, {});
  assert.deepEqual(r.output, ["a", "b", "c"]);

  r = await execute(`JSON.stringify({ a: 1, get b() { return 2; } })`, {});
  assert.equal(r.output, '{"a":1,"b":2}');

  r = await execute(`JSON.stringify({ ...{ a: 1, get b() { return 7; } } })`, {});
  assert.equal(r.output, '{"a":1,"b":7}');
});

// ── Number.MIN_VALUE constant + Object.fromEntries over any iterable ──
await test("Number.MIN_VALUE and Object.fromEntries(iterable) match Node", async () => {
  let r = await execute(`Number.MIN_VALUE`, {});
  assert.equal(r.output, 5e-324);
  assert.equal(r.output, Number.MIN_VALUE);

  r = await execute(`JSON.stringify(Object.fromEntries(new Map([["a",1],["b",2]])))`, {});
  assert.equal(r.output, '{"a":1,"b":2}');

  r = await execute(`JSON.stringify(Object.fromEntries(Object.entries({a:1,b:2,c:3}).filter(([k,v]) => v > 1)))`, {});
  assert.equal(r.output, '{"b":2,"c":3}');
});

// ── ECMA property key order (integer-index ascending, then insertion) ──
await test("property enumeration order matches Node across the boundary", async () => {
  let r = await execute(`Object.keys({ 2: "a", 1: "b", 10: "c", z: "d", a: "e" })`, {});
  assert.deepEqual(r.output, ["1", "2", "10", "z", "a"]);

  r = await execute(`JSON.stringify({ 2: "a", 1: "b", z: "c" })`, {});
  assert.equal(r.output, '{"1":"b","2":"a","z":"c"}');

  // for-in (desugars to Object.keys) sees the same order.
  r = await execute(`(function(){ let o = {}; o[3] = 1; o[1] = 1; o[2] = 1; let k = ""; for (const x in o) k += x; return k; })()`, {});
  assert.equal(r.output, "123");
});

// ── class declared inside a function constructs (no runaway recursion) ──
// Class members compiled inside a function body used to dangle into the wrong
// global function slots, so instantiating recursed to the stack-depth limit.
await test("class declared inside a function works across the host boundary", async () => {
  let r = await execute(`function make(){ class C { constructor(){ this.v = 5; } } return new C().v; } make()`, {});
  assert.equal(r.output, 5);

  r = await execute(`(function(){ class C { m(){ return 9; } } return new C().m(); })()`, {});
  assert.equal(r.output, 9);

  // Factory pattern with per-instance state.
  r = await execute(`(function(){ function mk(){ class K { constructor(){ this.n = 0; } inc(){ return ++this.n; } } return new K(); } const c = mk(); return c.inc() + c.inc(); })()`, {});
  assert.equal(r.output, 3);
});

// ── tagged templates call the tag with (strings, ...values) ──
await test("tagged templates work across the host boundary like Node", async () => {
  let r = await execute(`(function(){ function t(s, ...v){ return s.join("|") + "#" + v.join(","); } return t\`a\${1}b\${2}c\`; })()`, {});
  assert.equal(r.output, "a|b|c#1,2");

  // SQL-builder pattern.
  r = await execute(`(function(){ function sql(s, ...v){ return s.reduce((a,c,i)=>a+c+(i<v.length?"["+v[i]+"]":""),""); } let id=5; return sql\`id=\${id} x=\${id+1}\`; })()`, {});
  assert.equal(r.output, "id=[5] x=[6]");
});

// ── Number.toLocaleString grouping + private class fields ──
await test("toLocaleString and private class fields match Node across the boundary", async () => {
  let r = await execute(`(1234567).toLocaleString()`, {});
  assert.equal(r.output, "1,234,567");

  // Private fields work and are hidden from reflection.
  r = await execute(`(function(){ class C { #x = 42; pub = 1; getX(){ return this.#x; } } const c = new C(); return [c.getX(), JSON.stringify(c)]; })()`, {});
  assert.deepEqual(r.output, [42, '{"pub":1}']);
});

// ── Object.defineProperty (data/accessor descriptors + enumerable/writable) ──
await test("Object.defineProperty matches Node across the host boundary", async () => {
  // Non-enumerable data prop: readable, but hidden from keys/JSON.
  let r = await execute(`(function(){ const o = {a:1}; Object.defineProperty(o,'x',{value:5}); return [o.x, Object.keys(o), JSON.stringify(o)]; })()`, {});
  assert.deepEqual(r.output, [5, ["a"], '{"a":1}']);

  // Non-writable: assignment ignored.
  r = await execute(`(function(){ const o={}; Object.defineProperty(o,'x',{value:5,writable:false}); o.x=9; return o.x; })()`, {});
  assert.equal(r.output, 5);

  // Accessor descriptor invoked on read/write.
  r = await execute(`(function(){ const o={}; let v=0; Object.defineProperty(o,'x',{get(){return 9},set(n){v=n}}); const a=o.x; o.x=3; return [a,v]; })()`, {});
  assert.deepEqual(r.output, [9, 3]);

  // getOwnPropertyDescriptor of a plain prop.
  r = await execute(`Object.getOwnPropertyDescriptor({x:1},'x')`, {});
  assert.deepEqual(r.output, { value: 1, writable: true, enumerable: true, configurable: true });
});

// ── UTF-16 string semantics across the host boundary ──
// Strings are UTF-16-indexed: an astral char is 2 code units, charCodeAt yields
// surrogate halves, and a high+low surrogate re-pair on concatenation.
await test("strings are UTF-16-indexed (astral chars) like Node", async () => {
  let r = await execute(`["😀".length, "a😀b".length, "a😀b".indexOf("b")]`, {});
  assert.deepEqual(r.output, [2, 4, 3]);

  r = await execute(`["😀".charCodeAt(0), "😀".charCodeAt(1), "😀".codePointAt(0)]`, {});
  assert.deepEqual(r.output, [55357, 56832, 128512]);

  // Re-pairing: charAt(0)+charAt(1) reconstructs the astral char.
  r = await execute(`(function(){ const s = "😀"; return (s.charAt(0) + s.charAt(1)) === s; })()`, {});
  assert.equal(r.output, true);

  // A re-paired astral string marshals back to the host correctly.
  r = await execute(`String.fromCharCode(55357, 56832)`, {});
  assert.equal(r.output, "😀");
});

// ── const reassignment throws a catchable TypeError (mutation still allowed) ──
await test("const reassignment throws like Node across the host boundary", async () => {
  let r = await execute(`(function(){ try { const c = 1; c = 2; return "no"; } catch (e) { return e.name; } })()`, {});
  assert.equal(r.output, "TypeError");

  // Mutating a const object/array is allowed; only re-binding throws.
  r = await execute(`(function(){ const o = { x: 1 }; o.x = 2; const a = [1]; a.push(2); return [o.x, a.length]; })()`, {});
  assert.deepEqual(r.output, [2, 2]);
});

// ── let/const are block-scoped (shadow + restore, don't leak) ──
await test("let/const block scoping matches Node across the host boundary", async () => {
  // Inner-block shadow restores the outer binding.
  let r = await execute(`(function(){ let x = 1; { let x = 2; } return x; })()`, {});
  assert.equal(r.output, 1);

  // Block-scoped binding doesn't leak; typeof of an unbound name is "undefined".
  r = await execute(`(function(){ { let a = 1; } return typeof a; })()`, {});
  assert.equal(r.output, "undefined");

  // Duplicate let in the same scope is a (compile) error, surfaced to the host.
  await assert.rejects(
    () => execute(`(function(){ let a = 1; let a = 2; return a; })()`, {}),
    /has already been declared/,
  );
});

// ── .constructor on built-in instances resolves to the global constructor ──
await test("constructor property matches Node across the host boundary", async () => {
  let r = await execute(`[({}).constructor === Object, [].constructor === Array, new TypeError("x").constructor === TypeError]`, {});
  assert.deepEqual(r.output, [true, true, true]);

  r = await execute(`(function(){ try { null.x; } catch (e) { return e.constructor.name; } })()`, {});
  assert.equal(r.output, "TypeError");
});

// ── catch-param scoping + for-of/in per-iteration binding ──
await test("catch-param and for-of/in loop scoping match Node", async () => {
  // Catch param shadows an outer binding only inside the clause.
  let r = await execute(`(function(){ let e = "outer"; try { throw "x"; } catch (e) {} return e; })()`, {});
  assert.equal(r.output, "outer");

  // for-of / for-in loop variables get a fresh per-iteration binding.
  r = await execute(`(function(){ const fs = []; for (const v of [1,2,3]) fs.push(() => v); return fs.map(f => f()); })()`, {});
  assert.deepEqual(r.output, [1, 2, 3]);

  r = await execute(`(function(){ const fs = []; for (const k in {a:1,b:2}) fs.push(() => k); return fs.map(f => f()); })()`, {});
  assert.deepEqual(r.output, ["a", "b"]);
});

// ── Symbol.for global registry ──
await test("Symbol.for registry matches Node across the host boundary", async () => {
  let r = await execute(`[Symbol.for("x") === Symbol.for("x"), Symbol.for("x") === Symbol("x"), Symbol.keyFor(Symbol.for("hi"))]`, {});
  assert.deepEqual(r.output, [true, false, "hi"]);
});

console.log(`\n${passed} marshalling checks passed.`);
