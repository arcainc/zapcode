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

console.log(`\n${passed} marshalling checks passed.`);
