/**
 * e2e: durable multi-chunk SESSION workflows (high-level `createSession` API).
 *
 * Complements `e2e-durable-serialization.mjs` (which drills the low-level
 * `ZapcodeSessionHandle` suspend/resume bytes). This suite exercises the
 * developer-facing durable-session contract an orchestrator actually uses:
 *
 *   - a workflow defined in chunk 1 (functions, classes, top-level state) keeps
 *     accumulating across MANY `dump()` → `loadSession()` reload cycles, possibly
 *     in a fresh process, with state threaded through top-level bindings;
 *   - tool-using async workflows run across reloads and re-bind the same tools;
 *   - per-chunk results (`output`, `stdout`, `toolCalls`) are scoped to that chunk
 *     (toolCalls are NOT cumulative);
 *   - `runChunk(code, inputs)` injects inputs as bare globals;
 *   - replay determinism: the same chunk script over the same reload path yields
 *     the same output every time;
 *   - error in a chunk is catchable at the host and leaves the LAST good state
 *     usable; `dump()` is idempotent.
 *
 * It also pins the documented session boundaries, asserted at ACTUAL behavior:
 *   - re-declaring a top-level `let`/`const` in a later chunk throws a clean
 *     compile error (P2 — chunks share one top-level scope);
 *   - a *live generator instance* held in a top-level binding cannot be persisted,
 *     so it makes the session un-dumpable (P1); a generator *function* with no
 *     live instance reloads fine and can be re-driven with `for…of`.
 */
import assert from "node:assert/strict";
import { createSession, loadSession } from "../dist/index.js";

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

// Dump the current session and load a brand-new one (simulating a process hop).
function reload(session, tools = {}) {
  return loadSession(session.dump(), { tools });
}

console.log("e2e durable session workflows");

// ---------------------------------------------------------------------------
// Workflow definition + accumulating state across reloads
// ---------------------------------------------------------------------------

await test("a workflow defined in chunk 1 keeps running across many reloads", async () => {
  let s = createSession({ tools: {} });
  await s.runChunk(
    `let log = []; function step(name) { log.push(name); return log.length; }`
  );
  // Five process hops, each appending one step.
  for (const name of ["a", "b", "c", "d", "e"]) {
    s = reload(s);
    await s.runChunk(`step('${name}');`);
  }
  s = reload(s);
  const r = await s.runChunk(`log.join(',') + ':' + log.length`);
  assert.equal(r.output, "a,b,c,d,e:5");
});

await test("top-level numeric accumulator threads through reloads", async () => {
  let s = createSession({ tools: {} });
  await s.runChunk(`let acc = 0; function add(n) { acc += n; return acc; }`);
  let last = 0;
  for (let i = 1; i <= 4; i++) {
    s = reload(s);
    last = (await s.runChunk(`add(${i})`)).output;
  }
  assert.equal(last, 10); // 1+2+3+4
});

await test("a class instance accumulates state across reloads", async () => {
  let s = createSession({ tools: {} });
  await s.runChunk(
    `class Acc { constructor() { this.items = []; } add(x) { this.items.push(x); return this; } total() { return this.items.reduce((a, b) => a + b, 0); } } const a = new Acc();`
  );
  s = reload(s);
  await s.runChunk(`a.add(3).add(4);`);
  s = reload(s);
  await s.runChunk(`a.add(5);`);
  s = reload(s);
  const r = await s.runChunk(`a.total() + ':' + a.items.length`);
  assert.equal(r.output, "12:3");
});

await test("a generator FUNCTION (no live instance) reloads and re-drives via for-of", async () => {
  let s = createSession({ tools: {} });
  await s.runChunk(`function* range(n) { for (let i = 0; i < n; i++) yield i; }`);
  s = reload(s);
  const r = await s.runChunk(`let sum = 0; for (const v of range(4)) sum += v; sum`);
  assert.equal(r.output, 6); // 0+1+2+3
});

// ---------------------------------------------------------------------------
// Tool-using workflows across reloads
// ---------------------------------------------------------------------------

await test("a tool-using async workflow accumulates results across reloads", async () => {
  const makeTools = () => ({
    fetchScore: tool(async ({ id }) => id * 10, { id: { type: "number" } }),
  });
  let s = createSession({ tools: makeTools() });
  await s.runChunk(
    `let results = []; async function collect(id) { results.push(await fetchScore({ id })); return results.length; }`
  );
  s = loadSession(s.dump(), { tools: makeTools() });
  await s.runChunk(`await collect(1);`);
  s = loadSession(s.dump(), { tools: makeTools() });
  await s.runChunk(`await collect(2);`);
  s = loadSession(s.dump(), { tools: makeTools() });
  const r = await s.runChunk(`await collect(3); results.join(',')`);
  assert.equal(r.output, "10,20,30");
});

await test("a parallel tool batch inside a reloaded workflow resolves in order", async () => {
  const makeTools = () => ({
    load: tool(async ({ k }) => k.toUpperCase(), { k: { type: "string" } }),
  });
  let s = createSession({ tools: makeTools() });
  await s.runChunk(
    `async function loadAll(keys) { return await Promise.all(keys.map(k => load({ k }))); }`
  );
  s = loadSession(s.dump(), { tools: makeTools() });
  const r = await s.runChunk(`(await loadAll(['a', 'b', 'c'])).join('-')`);
  assert.equal(r.output, "A-B-C");
});

// ---------------------------------------------------------------------------
// Per-chunk result scoping
// ---------------------------------------------------------------------------

await test("toolCalls are scoped per chunk, not cumulative", async () => {
  const makeTools = () => ({ ping: tool(async () => "p") });
  const s = createSession({ tools: makeTools() });
  const c1 = await s.runChunk(`await ping()`);
  const c2 = await s.runChunk(`await ping(); await ping()`);
  const c3 = await s.runChunk(`'no tools here'`);
  assert.equal(c1.toolCalls.length, 1);
  assert.equal(c2.toolCalls.length, 2);
  assert.equal(c3.toolCalls.length, 0);
});

await test("stdout is captured per chunk", async () => {
  const s = createSession({ tools: {} });
  const c1 = await s.runChunk(`console.log("first"); 1`);
  const c2 = await s.runChunk(`console.log("second"); 2`);
  assert.equal(c1.stdout, "first\n");
  assert.equal(c1.output, 1);
  assert.equal(c2.stdout, "second\n");
  assert.equal(c2.output, 2);
});

// ---------------------------------------------------------------------------
// runChunk inputs
// ---------------------------------------------------------------------------

await test("runChunk inputs are injected as bare globals", async () => {
  const s = createSession({ tools: {} });
  const r = await s.runChunk(`x + y`, { x: 3, y: 4 });
  assert.equal(r.output, 7);
});

await test("structured inputs flow into a workflow function", async () => {
  const s = createSession({ tools: {} });
  await s.runChunk(`function summarize(rows) { return rows.reduce((a, r) => a + r.n, 0); }`);
  const r = await s.runChunk(`summarize(payload.rows)`, {
    payload: { rows: [{ n: 1 }, { n: 2 }, { n: 3 }] },
  });
  assert.equal(r.output, 6);
});

// ---------------------------------------------------------------------------
// Replay determinism & idempotent dump
// ---------------------------------------------------------------------------

await test("the same chunk path replays to the same output every time", async () => {
  const runOnce = async () => {
    let s = createSession({ tools: {} });
    await s.runChunk(`let acc = 0; function add(n) { acc += n; return acc; }`);
    s = reload(s);
    await s.runChunk(`add(10);`);
    s = reload(s);
    return (await s.runChunk(`add(5); acc`)).output;
  };
  const a = await runOnce();
  const b = await runOnce();
  assert.equal(a, 15);
  assert.equal(b, 15);
});

await test("dump() is idempotent for an idle session", async () => {
  const s = createSession({ tools: {} });
  await s.runChunk(`const config = { retries: 3, region: 'us', tags: ['x', 'y'] };`);
  const d1 = s.dump();
  const d2 = s.dump();
  assert.equal(d1.length, d2.length);
  assert.ok(d1.length > 0);
  // and reloading either preserves the value
  const r = await loadSession(d2, { tools: {} }).runChunk(`config.retries + ':' + config.tags.join('')`);
  assert.equal(r.output, "3:xy");
});

await test("a large array survives a reload intact", async () => {
  let s = createSession({ tools: {} });
  await s.runChunk(`const big = Array.from({ length: 1000 }, (_, i) => i * 2);`);
  s = reload(s);
  const r = await s.runChunk(`big.length + ':' + big[0] + ':' + big[999]`);
  assert.equal(r.output, "1000:0:1998");
});

// ---------------------------------------------------------------------------
// Error recovery
// ---------------------------------------------------------------------------

await test("a throwing chunk is catchable at the host and leaves prior state usable", async () => {
  let s = createSession({ tools: {} });
  await s.runChunk(`let n = 5;`);
  await assert.rejects(() => s.runChunk(`throw new Error("boom")`));
  // The last good state is still readable.
  const r = await s.runChunk(`n`);
  assert.equal(r.output, 5);
});

await test("a failing tool argument in a chunk is catchable and state continues", async () => {
  const makeTools = () => ({ f: tool(async (a) => a.n, { n: { type: "number" } }) });
  let s = createSession({ tools: makeTools() });
  await s.runChunk(`let count = 0;`);
  await assert.rejects(() => s.runChunk(`await f({ n: 'not a number' })`));
  const r = await s.runChunk(`count = count + 1; count`);
  assert.equal(r.output, 1);
});

// ---------------------------------------------------------------------------
// Documented session boundaries (asserted at ACTUAL behavior)
// ---------------------------------------------------------------------------

await test("re-declaring a top-level binding in a later chunk throws (P2)", async () => {
  // DIVERGENCE/contract asserted as actual: chunks share one top-level scope, so a
  // second `let`/`const` of the same name is a compile error (a single-shot
  // `execute` of the concatenation would shadow it). Agents regenerating a whole
  // program across chunks must reuse bindings or assign, not re-declare.
  const s = createSession({ tools: {} });
  await s.runChunk(`let z = 1;`);
  await assert.rejects(() => s.runChunk(`let z = 2; z`), /already been declared/);
  // Re-assignment (no new declaration) is fine.
  const r = await s.runChunk(`z = 99; z`);
  assert.equal(r.output, 99);
});

await test("a live generator instance held at top level makes the session un-dumpable (P1)", async () => {
  // DIVERGENCE asserted as actual: a live generator OBJECT cannot be serialized,
  // so the wrapper's end-of-chunk persist throws. The same code runs fine under a
  // one-shot `execute`; durable workflows must avoid holding a live generator in a
  // top-level binding (drive it to completion inside the chunk instead).
  const s = createSession({ tools: {} });
  await assert.rejects(
    () => s.runChunk(`function* g() { yield 1; yield 2; } const it = g(); it.next().value`),
    /generators cannot be persisted/
  );
});

await test("driving a generator to completion inside one chunk is fine", async () => {
  // The supported pattern: consume the generator within the chunk, keep only plain
  // values at the top level.
  let s = createSession({ tools: {} });
  await s.runChunk(
    `function* g() { yield 1; yield 2; yield 3; } let collected = []; for (const v of g()) collected.push(v);`
  );
  s = reload(s);
  const r = await s.runChunk(`collected.join(',')`);
  assert.equal(r.output, "1,2,3");
});

console.log(`\n${passed} durable session-workflow checks passed.`);
