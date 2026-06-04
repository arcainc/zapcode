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

console.log(`\n${passed} marshalling checks passed.`);
