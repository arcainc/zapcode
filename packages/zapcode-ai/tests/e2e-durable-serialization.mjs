/**
 * e2e: durable-session SERIALIZATION conformance.
 *
 * The headline durability promise: an agent defines a workflow now; its VM state
 * serializes to bytes; it is reloaded — possibly in another process / activity —
 * and resumes with state intact. This suite drives that boundary hard:
 *
 *   - top-level bindings / functions / classes survive dump→load and stay live
 *     across MANY serialization cycles;
 *   - heap values (arrays, objects, Map, Set) preserve identity & mutations;
 *   - a mid-tool-call SUSPENSION serializes and resumes in a fresh handle (single
 *     calls AND `Promise.all` batches), including error injection at the boundary;
 *   - byte-stability: re-dumping an idle state and reloading is value-preserving.
 *
 * It also covers the nested-closure capture case: a closure returned from a
 * nested function call DOES retain its captured function-local environment
 * across dump/load — the shared upvalue cell backing the capture travels in the
 * idle-session snapshot — just like top-level/module-level state. See
 * `nested_closure_capture_survives_serialization`.
 */
import assert from "node:assert/strict";
import { createSession, loadSession } from "../dist/index.js";
import { ZapcodeSessionHandle } from "@unchartedfr/zapcode";

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

// Reload helper: dump the current session and load a brand-new one (no tools).
function reload(session, tools = {}) {
  return loadSession(session.dump(), { tools });
}

console.log("e2e durable serialization");

// ---------------------------------------------------------------------------
// Top-level bindings & program refs across chunks
// ---------------------------------------------------------------------------

await test("a function declared in chunk 1 is callable in a later, reloaded chunk", async () => {
  let s = createSession({ tools: {} });
  const c1 = await s.runChunk(`function double(x){ return x * 2; } const base = 21; base`);
  assert.equal(c1.output, 21);
  s = reload(s);
  const c2 = await s.runChunk(`double(base)`);
  assert.equal(c2.output, 42);
});

await test("top-level let accumulates across MANY dump/load cycles", async () => {
  let s = createSession({ tools: {} });
  await s.runChunk(`let total = 0; function add(n){ total += n; return total; }`);
  let last = 0;
  for (let i = 1; i <= 5; i++) {
    s = reload(s);
    const r = await s.runChunk(`add(${i})`);
    last = r.output;
  }
  assert.equal(last, 15); // 1+2+3+4+5
});

await test("a top-level closure over a module-level binding survives reload", async () => {
  let s = createSession({ tools: {} });
  await s.runChunk(`let counter = 0; function inc(){ counter++; return counter; } inc();`);
  s = reload(s);
  const r = await s.runChunk(`inc() + ',' + inc()`);
  assert.equal(r.output, "2,3");
});

// ---------------------------------------------------------------------------
// Classes & heap values across serialization
// ---------------------------------------------------------------------------

await test("a class instance keeps its fields & methods across reload", async () => {
  let s = createSession({ tools: {} });
  await s.runChunk(`class Counter { constructor(){ this.n = 0; } inc(){ this.n++; return this; } } const c = new Counter(); c.inc(); c.inc();`);
  s = reload(s);
  const r = await s.runChunk(`c.inc(); c.n`);
  assert.equal(r.output, 3);
});

await test("array mutations persist across reloads (heap identity preserved)", async () => {
  let s = createSession({ tools: {} });
  await s.runChunk(`const state = { items: [], count: 0 };`);
  s = reload(s);
  await s.runChunk(`state.items.push('a'); state.items.push('b'); state.count = state.items.length;`);
  s = reload(s);
  const r = await s.runChunk(`state.items.join(',') + ':' + state.count`);
  assert.equal(r.output, "a,b:2");
});

await test("aliased arrays keep shared identity across reload", async () => {
  let s = createSession({ tools: {} });
  await s.runChunk(`const a = [1, 2]; const b = a;`); // b aliases a
  s = reload(s);
  const r = await s.runChunk(`b.push(3); a.length + ':' + (a === b)`);
  assert.equal(r.output, "3:true");
});

await test("Map and Set survive reload with their contents", async () => {
  let s = createSession({ tools: {} });
  await s.runChunk(`const m = new Map([['x', 1]]); const set = new Set([1, 2]);`);
  s = reload(s);
  const r = await s.runChunk(`m.set('y', 2); set.add(3); m.size + ':' + set.size + ':' + m.get('x')`);
  assert.equal(r.output, "2:3:1");
});

await test("nested data structure round-trips byte-stably", async () => {
  let s = createSession({ tools: {} });
  await s.runChunk(`const tree = { id: 1, children: [{ id: 2, children: [] }, { id: 3, children: [{ id: 4 }] }] };`);
  s = reload(s);
  const r = await s.runChunk(`JSON.stringify(tree)`);
  assert.equal(r.output, '{"id":1,"children":[{"id":2,"children":[]},{"id":3,"children":[{"id":4}]}]}');
});

// ---------------------------------------------------------------------------
// Mid-suspension serialization (low-level handle)
// ---------------------------------------------------------------------------

await test("suspend on a tool call, ship the bytes, resume in a fresh handle", async () => {
  const h = ZapcodeSessionHandle.create({ externalFunctions: ["lookup"] });
  const sus = h.runChunk(`const a = await lookup("k1"); const b = await lookup("k2"); a + "/" + b`);
  assert.equal(sus.completed, false);
  assert.equal(sus.functionName, "lookup");
  assert.deepEqual(sus.args, ["k1"]);

  // Ship to "another process": load fresh, resume.
  const next = ZapcodeSessionHandle.load(sus.session).resume("V1");
  assert.equal(next.completed, false);
  assert.deepEqual(next.args, ["k2"]);

  const done = ZapcodeSessionHandle.load(next.session).resume("V2");
  assert.equal(done.completed, true);
  assert.equal(done.output, "V1/V2");
});

await test("suspended state survives MULTIPLE serialization hops before resume", async () => {
  const h = ZapcodeSessionHandle.create({ externalFunctions: ["lookup"] });
  const sus = h.runChunk(`const v = await lookup("key"); "got:" + v`);
  assert.equal(sus.completed, false);
  // Re-serialize the *suspended* state several times without resuming: each hop
  // loads the bytes and re-dumps them, simulating storage round-trips.
  let bytes = sus.session;
  for (let i = 0; i < 4; i++) {
    bytes = ZapcodeSessionHandle.load(bytes).dump();
  }
  const done = ZapcodeSessionHandle.load(bytes).resume("R");
  assert.equal(done.completed, true);
  assert.equal(done.output, "got:R");
});

await test("resumeError raises a catchable error at the suspension point", async () => {
  const h = ZapcodeSessionHandle.create({ externalFunctions: ["callTool"] });
  const sus = h.runChunk(`
    let outcome;
    try { outcome = "ok:" + await callTool("x"); }
    catch (e) { outcome = "caught:" + e; }
    outcome
  `);
  assert.equal(sus.completed, false);
  const done = ZapcodeSessionHandle.load(sus.session).resumeError("upstream 500");
  assert.equal(done.completed, true);
  assert.equal(done.output, "caught:upstream 500");
});

await test("a Promise.all batch suspension carries every call and resumes in order", async () => {
  const h = ZapcodeSessionHandle.create({ externalFunctions: ["f"] });
  const sus = h.runChunk(`const xs = await Promise.all([f("a"), f("b"), f("c")]); xs.join(',')`);
  assert.equal(sus.completed, false);
  assert.equal(sus.kind, "suspended_many");
  assert.equal(sus.calls.length, 3);
  assert.deepEqual(sus.calls.map(c => c.args[0]), ["a", "b", "c"]);
  // Resume the batch with the settled values, in a fresh handle.
  const done = ZapcodeSessionHandle.load(sus.session).resumeMany(["A", "B", "C"]);
  assert.equal(done.completed, true);
  assert.equal(done.output, "A,B,C");
});

// ---------------------------------------------------------------------------
// High-level session driving a tool across a reload
// ---------------------------------------------------------------------------

await test("a tool-using workflow defined in one chunk runs after reload", async () => {
  const makeTools = () => ({ fetchRow: tool(async ({ id }) => id * 100, { id: { type: "number" } }) });
  let s = createSession({ tools: makeTools() });
  await s.runChunk(`async function step(id){ return await fetchRow({ id }); }`);
  s = loadSession(s.dump(), { tools: makeTools() });
  const r = await s.runChunk(`await step(7)`);
  assert.equal(r.output, 700);
  assert.equal(r.toolCalls.length, 1);
  assert.equal(r.toolCalls[0].name, "fetchRow");
});

await test("idle re-dump is value-preserving (dump → load → dump → load)", async () => {
  let s = createSession({ tools: {} });
  await s.runChunk(`const config = { retries: 3, region: 'us' }; let seq = [];`);
  // round-trip twice with no work in between
  s = reload(s);
  s = reload(s);
  await s.runChunk(`seq.push(config.retries); seq.push(config.region);`);
  s = reload(s);
  const r = await s.runChunk(`seq.join('-')`);
  assert.equal(r.output, "3-us");
});

// ---------------------------------------------------------------------------
// Documented serialization boundary
// ---------------------------------------------------------------------------

await test("nested_closure_capture_survives_serialization", async () => {
  // A closure RETURNED from a nested function call retains its captured
  // function-local environment across dump/load — the shared upvalue cell that
  // backs the capture is carried in the idle-session snapshot and re-linked on
  // load, the same way top-level / module-level closures survive (see the
  // earlier test). Matches real Node.

  // Sanity: it works WITHOUT a reload.
  let s = createSession({ tools: {} });
  const same = await s.runChunk(`function mk(){ let n = 10; return () => ++n; } const inc = mk(); inc() + ',' + inc()`);
  assert.equal(same.output, "11,12");

  // After a reload, the captured `n` survives: the next ++n continues from 11.
  s = createSession({ tools: {} });
  await s.runChunk(`function mk(){ let n = 10; return () => ++n; } const inc = mk(); inc();`); // n = 11
  s = reload(s);
  const afterReload = await s.runChunk(`String(inc())`);
  assert.equal(afterReload.output, "12");
});

console.log(`\n${passed} durable-serialization checks passed.`);
