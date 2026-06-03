/**
 * e2e: durable-session INPUTS, state threading, and chunk recovery.
 *
 * Complements `e2e-durable-serialization.mjs` (which pins the byte-level
 * dump/load/resume of heap & suspension state) by exercising the *driver-level*
 * session contract an orchestrator relies on across many chunks:
 *
 *   - per-chunk `inputs` are injected as bare globals (scalars, objects, arrays)
 *     and carry full reference semantics inside the chunk;
 *   - state derived from inputs persists into later chunks (the input itself is
 *     a per-chunk binding; what you store in top-level state survives);
 *   - the SAME input name can be re-supplied with a new value on each reloaded
 *     chunk, while top-level accumulator state keeps growing;
 *   - `toolCalls` are per-chunk (not cumulative) and an input value flowing into
 *     a tool argument is recorded as the post-validation arg, not as extra calls;
 *   - a chunk that throws does NOT corrupt the session: state from *before* the
 *     failing chunk is intact and the failing chunk's partial mutations roll back
 *     to the last good checkpoint, so the next chunk runs normally;
 *   - re-dumping an idle session is byte-identical (deterministic serialization);
 *   - large payloads (long strings, big arrays) round-trip across reload.
 *
 * The `output` marshalling contract (Date→ISO via `toISOString`, Map/Set as a
 * plain value, NaN/Infinity→null) is also pinned at its ACTUAL behavior; the
 * documented residuals (NaN/Infinity→null on output) are asserted as actual, not
 * the JS answer.
 */
import assert from "node:assert/strict";
import { execute, createSession, loadSession } from "../dist/index.js";

let passed = 0;
async function test(name, fn) {
  try {
    await fn();
    passed++;
    console.log(`  ✓ ${name}`);
  } catch (err) {
    console.error(`  ✗ ${name}`);
    throw err;
  }
}

function tool(fn, parameters = {}) {
  return { description: "t", parameters, execute: fn };
}

// Reload helper: dump the current session and load a brand-new one.
function reload(session, tools = {}) {
  return loadSession(session.dump(), { tools });
}

console.log("e2e session inputs & state threading");

// ---------------------------------------------------------------------------
// Inputs as bare globals
// ---------------------------------------------------------------------------

await test("scalar inputs are visible as bare globals in the chunk", async () => {
  const s = createSession({ tools: {} });
  const r = await s.runChunk(`x + y`, { x: 10, y: 5 });
  assert.equal(r.output, 15);
});

await test("an object input carries reference semantics inside the chunk", async () => {
  const s = createSession({ tools: {} });
  const r = await s.runChunk(
    `config.items.push(99); config.items.length`,
    { config: { items: [1, 2, 3] } }
  );
  assert.equal(r.output, 4);
});

await test("an array input is iterable and indexable", async () => {
  const s = createSession({ tools: {} });
  const r = await s.runChunk(
    `rows.filter(r => r.active).map(r => r.id).join(',')`,
    { rows: [{ id: 1, active: true }, { id: 2, active: false }, { id: 3, active: true }] }
  );
  assert.equal(r.output, "1,3");
});

await test("nested input structures read through deep paths", async () => {
  const s = createSession({ tools: {} });
  const r = await s.runChunk(
    `payload.meta.tags.length + ':' + payload.meta.tags[0]`,
    { payload: { meta: { tags: ["a", "b", "c"] } } }
  );
  assert.equal(r.output, "3:a");
});

// ---------------------------------------------------------------------------
// State derived from inputs persists; inputs can be re-supplied
// ---------------------------------------------------------------------------

await test("state derived from an input persists into a later chunk", async () => {
  const s = createSession({ tools: {} });
  await s.runChunk(`const doubled = base * 2;`, { base: 21 });
  const r = await s.runChunk(`doubled`);
  assert.equal(r.output, 42);
});

await test("the same input name can be re-supplied with a new value each reloaded chunk", async () => {
  let s = createSession({ tools: {} });
  await s.runChunk(
    `let history = []; function record(v) { history.push(v); return history.length; }`
  );
  s = reload(s);
  const r1 = await s.runChunk(`record(cursor)`, { cursor: 10 });
  assert.equal(r1.output, 1);
  s = reload(s);
  const r2 = await s.runChunk(`record(cursor)`, { cursor: 20 });
  assert.equal(r2.output, 2);
  const r3 = await s.runChunk(`history.join(',')`);
  assert.equal(r3.output, "10,20");
});

await test("an input feeding a top-level accumulator threads across many reloads", async () => {
  let s = createSession({ tools: {} });
  await s.runChunk(`let total = 0; function add(n) { total += n; return total; }`);
  let last = 0;
  for (const n of [3, 5, 7, 11]) {
    s = reload(s);
    const r = await s.runChunk(`add(delta)`, { delta: n });
    last = r.output;
  }
  assert.equal(last, 26); // 3+5+7+11
});

// ---------------------------------------------------------------------------
// toolCalls: per-chunk, input-fed args recorded correctly
// ---------------------------------------------------------------------------

await test("toolCalls reset per chunk (not cumulative)", async () => {
  const s = createSession({ tools: { ping: tool(async () => "pong") } });
  const r1 = await s.runChunk(`await ping({})`);
  const r2 = await s.runChunk(`await ping({})`);
  assert.equal(r1.toolCalls.length, 1);
  assert.equal(r2.toolCalls.length, 1);
});

await test("an input value flowing into a tool arg is recorded as the validated arg", async () => {
  const s = createSession({ tools: { calc: tool(async ({ n }) => n * 2, { n: { type: "number" } }) } });
  const r = await s.runChunk(`await calc({ n: seed })`, { seed: 21 });
  assert.equal(r.output, 42);
  assert.equal(r.toolCalls.length, 1);
  assert.deepEqual(r.toolCalls[0].input, { n: 21 });
});

await test("a chunk with no tool calls has an empty toolCalls log", async () => {
  const s = createSession({ tools: { unused: tool(async () => 1) } });
  const r = await s.runChunk(`[1, 2, 3].reduce((a, b) => a + b, 0)`);
  assert.equal(r.output, 6);
  assert.equal(r.toolCalls.length, 0);
});

// ---------------------------------------------------------------------------
// Chunk error recovery
// ---------------------------------------------------------------------------

await test("a throwing chunk does not corrupt pre-existing session state", async () => {
  const s = createSession({ tools: {} });
  await s.runChunk(`let items = []; items.push('a');`);
  await assert.rejects(() => s.runChunk(`items.push('b'); null.crash;`));
  // 'a' (committed in chunk 1) survives; 'b' (in the failed chunk) rolled back.
  const r = await s.runChunk(`items.join(',')`);
  assert.equal(r.output, "a");
});

await test("the session keeps running normally after a recovered error", async () => {
  const s = createSession({ tools: {} });
  await s.runChunk(`let counter = 100;`);
  await assert.rejects(() => s.runChunk(`throw new Error("boom")`));
  const r = await s.runChunk(`counter += 1; counter`);
  assert.equal(r.output, 101);
});

await test("a tool error inside a chunk is catchable and does not brick the session", async () => {
  const s = createSession({
    tools: { risky: tool(async () => { throw new Error("nope"); }) },
  });
  const r1 = await s.runChunk(
    `let msg; try { await risky({}); msg = 'ok'; } catch (e) { msg = 'caught'; } msg`
  );
  assert.equal(r1.output, "caught");
  const r2 = await s.runChunk(`'still alive'`);
  assert.equal(r2.output, "still alive");
});

// ---------------------------------------------------------------------------
// Deterministic serialization & large payloads
// ---------------------------------------------------------------------------

await test("re-dumping an idle session is byte-identical", async () => {
  const s = createSession({ tools: {} });
  await s.runChunk(`let acc = [1, 2, 3]; const total = acc.reduce((a, b) => a + b, 0);`);
  const d1 = s.dump();
  const s2 = loadSession(d1, { tools: {} });
  const d2 = s2.dump();
  assert.equal(Buffer.compare(Buffer.from(d1), Buffer.from(d2)), 0);
});

await test("large string and array payloads round-trip across reload", async () => {
  let s = createSession({ tools: {} });
  await s.runChunk(
    `const big = 'x'.repeat(20000); const arr = Array.from({ length: 5000 }, (_, i) => i);`
  );
  s = reload(s);
  const r = await s.runChunk(`big.length + ':' + arr.length + ':' + arr[4999]`);
  assert.equal(r.output, "20000:5000:4999");
});

await test("a deep nested structure built from an input survives reload", async () => {
  let s = createSession({ tools: {} });
  await s.runChunk(
    `const tree = { root: { children: seedIds.map(id => ({ id, kids: [] })) } };`,
    { seedIds: [1, 2, 3] }
  );
  s = reload(s);
  const r = await s.runChunk(
    `tree.root.children[1].kids.push('x'); tree.root.children.length + ':' + tree.root.children[1].kids[0]`
  );
  assert.equal(r.output, "3:x");
});

// ---------------------------------------------------------------------------
// Output marshalling contract (incl. documented residuals)
// ---------------------------------------------------------------------------

await test("Date output marshals to an ISO string via toISOString", async () => {
  const r = await execute(`new Date(1700000000000).toISOString()`, {});
  assert.equal(r.output, "2023-11-14T22:13:20.000Z");
});

await test("Map contents are usable as a plain output value", async () => {
  const r = await execute(`const m = new Map([['a', 1], ['b', 2]]); [...m.entries()]`, {});
  assert.deepEqual(r.output, [["a", 1], ["b", 2]]);
});

await test("NaN / Infinity output marshal to null (documented residual, asserted as actual)", async () => {
  // F4: when the output VALUE itself is NaN/Infinity it is marshalled to null
  // (correct inside the sandbox; this is the output-boundary behavior).
  const nan = await execute(`0 / 0`, {});
  assert.equal(nan.output, null);
  const inf = await execute(`1 / 0`, {});
  assert.equal(inf.output, null);
  const arr = await execute(`[1, 1 / 0, 3]`, {});
  assert.deepEqual(arr.output, [1, null, 3]);
});

await test("structured output object preserves key order and nesting", async () => {
  const r = await execute(`({ id: 7, tags: ['a', 'b'], meta: { ok: true } })`, {});
  assert.deepEqual(r.output, { id: 7, tags: ["a", "b"], meta: { ok: true } });
});

console.log(`\n${passed} session-inputs checks passed.`);
