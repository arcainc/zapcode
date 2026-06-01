/**
 * e2e stress tests — real TypeScript through the sandbox, probing durability,
 * parallel batching, error handling, and language breadth under load. Mixes the
 * high-level zapcode-ai API with the raw binding (to drive serialization on
 * every hop, like a Temporal workflow would).
 */
import assert from "node:assert/strict";
import { execute } from "../dist/index.js";
import {
  Zapcode,
  ZapcodeSnapshotHandle,
  ZapcodeSessionHandle,
} from "@unchartedfr/zapcode";

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

console.log("stress e2e");

// --- Durability -----------------------------------------------------------

await test("deep suspend/resume chain serializes on every hop", () => {
  // 25 sequential tool calls, dumping + reloading the snapshot each time —
  // the per-activity serialization pattern. State must survive every hop.
  const sandbox = new Zapcode(
    `
    let acc = 0;
    for (let i = 0; i < 25; i++) {
      acc += await step(i);
    }
    acc
    `,
    { externalFunctions: ["step"] }
  );
  let state = sandbox.start();
  let hops = 0;
  while (!state.completed) {
    assert.equal(state.functionName, "step");
    const arg = state.args[0];
    // Round-trip the snapshot bytes through dump/load on every hop.
    const bytes = state.snapshot;
    state = ZapcodeSnapshotHandle.load(bytes).resume(arg * 2);
    hops++;
  }
  assert.equal(hops, 25);
  // sum(i*2 for i in 0..25) = 2 * (24*25/2) = 600
  assert.equal(state.output, 600);
});

await test("closure captured before a suspend works after resume", async () => {
  const result = await execute(
    `
    const base = await getBase();           // suspends here
    const add = (x) => x + base;            // closure captures 'base'
    const more = await getBase();           // suspends again
    add(more)
    `,
    {
      getBase: {
        description: "Get a base number.",
        parameters: {},
        execute: async () => 10,
      },
    }
  );
  assert.equal(result.output, 20);
});

await test("class instance mutated across a mid-method suspend", async () => {
  const result = await execute(
    `
    class Counter {
      constructor() { this.n = 0; }
      async bump() { this.n += await delta(); return this.n; }
    }
    const c = new Counter();
    await c.bump();
    await c.bump();
    c.n
    `,
    {
      delta: {
        description: "Return an increment.",
        parameters: {},
        execute: async () => 5,
      },
    }
  );
  assert.equal(result.output, 10);
});

await test("try/finally runs its finally block after a resumed call", async () => {
  const result = await execute(
    `
    let log = [];
    try {
      log.push("before");
      await work();
      log.push("after");
    } finally {
      log.push("finally");
    }
    log.join(",")
    `,
    { work: { description: "do work", parameters: {}, execute: async () => 1 } }
  );
  assert.equal(result.output, "before,after,finally");
});

// --- Parallel batching ----------------------------------------------------

await test("Promise.all of 25 calls preserves order across dump/load", () => {
  const session = ZapcodeSessionHandle.create({ externalFunctions: ["load"] });
  const keys = Array.from({ length: 25 }, (_, i) => `k${i}`);
  const suspended = session.runChunk(
    `await Promise.all([${keys.map(k => `load("${k}")`).join(", ")}])`
  );
  assert.equal(suspended.kind, "suspended_many");
  assert.equal(suspended.calls.length, 25);
  // Ship across a boundary, resume with results in order.
  const results = suspended.calls.map((c, i) => `v${i}:${c.args[0]}`);
  const done = ZapcodeSessionHandle.load(suspended.session).resumeMany(results);
  assert.equal(done.completed, true);
  assert.deepEqual(done.output, results);
});

await test("mixed Promise.all unwraps inner promises and keeps plain values", async () => {
  const result = await execute(
    `
    const out = await Promise.all([
      fetchOne("a"),
      Promise.resolve(42),
      "literal",
      fetchOne("b"),
    ]);
    out
    `,
    {
      fetchOne: {
        description: "fetch",
        parameters: { k: { type: "string" } },
        execute: async ({ k }) => `got:${k}`,
      },
    }
  );
  assert.deepEqual(result.output, ["got:a", 42, "literal", "got:b"]);
});

await test("sequential Promise.all batches in one chunk", async () => {
  const result = await execute(
    `
    const a = await Promise.all([f(1), f(2)]);
    const b = await Promise.all([f(3), f(4)]);
    [...a, ...b]
    `,
    { f: { description: "f", parameters: { n: { type: "number" } }, execute: async ({ n }) => n * 10 } }
  );
  assert.deepEqual(result.output, [10, 20, 30, 40]);
});

await test("Promise.all inside a loop batches each iteration", async () => {
  const result = await execute(
    `
    const rows = [];
    for (let i = 0; i < 3; i++) {
      const pair = await Promise.all([f(i), f(i + 100)]);
      rows.push(pair[0] + pair[1]);
    }
    rows
    `,
    { f: { description: "f", parameters: { n: { type: "number" } }, execute: async ({ n }) => n } }
  );
  // i + (i+100) for i in 0..3 → 100, 102, 104
  assert.deepEqual(result.output, [100, 102, 104]);
});

await test("empty and all-plain Promise.all resolve without a batch suspension", async () => {
  const r1 = await execute(`await Promise.all([])`, {});
  assert.deepEqual(r1.output, []);
  const r2 = await execute(`await Promise.all([1, 2, 3])`, {});
  assert.deepEqual(r2.output, [1, 2, 3]);
});

// --- Error handling -------------------------------------------------------

await test("rejection in a parallel batch is catchable and lets the workflow recover", async () => {
  const result = await execute(
    `
    let out;
    try {
      await Promise.all([ok("a"), boom("b"), ok("c")]);
      out = "no-error";
    } catch (e) {
      out = "recovered:" + (await ok("fallback"));
    }
    out
    `,
    {
      ok: { description: "ok", parameters: { k: { type: "string" } }, execute: async ({ k }) => k },
      boom: {
        description: "fails",
        parameters: { k: { type: "string" } },
        execute: async () => {
          throw new Error("kaboom");
        },
      },
    }
  );
  assert.equal(result.output, "recovered:fallback");
});

await test("a guest re-throw from catch propagates to the host", async () => {
  await assert.rejects(
    () =>
      execute(
        `
        try {
          await fail();
        } catch (e) {
          throw "wrapped: " + e;
        }
        `,
        { fail: { description: "fail", parameters: {}, execute: async () => { throw new Error("x"); } } }
      ),
    /wrapped/
  );
});

// --- Robustness / adversarial --------------------------------------------

await test("resumeMany with the wrong number of results is rejected", () => {
  const session = ZapcodeSessionHandle.create({ externalFunctions: ["f"] });
  const suspended = session.runChunk(`await Promise.all([f(1), f(2), f(3)])`);
  assert.throws(
    () => ZapcodeSessionHandle.load(suspended.session).resumeMany([1, 2]),
    /expected 3 results but got 2/
  );
});

await test("a tampered snapshot is rejected on load", () => {
  const sandbox = new Zapcode(`const x = await f(); x`, { externalFunctions: ["f"] });
  const state = sandbox.start();
  const bytes = Buffer.from(state.snapshot);
  bytes[bytes.length - 1] ^= 0x01; // flip a payload byte
  assert.throws(() => ZapcodeSnapshotHandle.load(bytes), /integrity/);
});

await test("loading a session blob as a plain snapshot is rejected", () => {
  const session = ZapcodeSessionHandle.create({ externalFunctions: [] });
  const dump = session.dump();
  assert.throws(() => ZapcodeSnapshotHandle.load(dump), /expected a snapshot blob but got a session blob/);
});

await test("an infinite loop is bounded by a resource limit (doesn't hang)", async () => {
  // Either the time or allocation guard may trip first — both bound the runaway.
  await assert.rejects(
    () => execute(`while (true) { acc = (acc || 0) + 1; }`, {}, { timeLimitMs: 100 }),
    /(time|allocation) limit/i
  );
});

await test("calling a tool inside .map gives an actionable error", async () => {
  // A known limitation: external calls can't suspend inside array-callback
  // methods. The error must steer the model to a for...of loop or Promise.all.
  await assert.rejects(
    () =>
      execute(`[1, 2, 3].map(n => f(n))`, {
        f: { description: "f", parameters: { n: { type: "number" } }, execute: async ({ n }) => n },
      }),
    /for\.\.\.of loop|Promise\.all/
  );
});

// --- Language breadth under await ----------------------------------------

await test("array methods, JSON, and nested destructuring over tool results", async () => {
  const result = await execute(
    `
    const rows = await Promise.all([row(1), row(2), row(3)]);
    const { total } = rows.reduce((acc, r) => ({ total: acc.total + r.amount }), { total: 0 });
    const top = rows
      .filter(r => r.amount > 10)
      .map(r => r.id)
      .sort();
    JSON.stringify({ total, top })
    `,
    {
      row: {
        description: "row",
        parameters: { id: { type: "number" } },
        execute: async ({ id }) => ({ id: "r" + id, amount: id * 10 }),
      },
    }
  );
  assert.deepEqual(JSON.parse(result.output), { total: 60, top: ["r2", "r3"] });
});

await test("object rest + spread merging tool results", async () => {
  const result = await execute(
    `
    const base = await getConfig();
    const merged = { ...base, env: "prod" };
    const { env, ...rest } = merged;
    env + ":" + JSON.stringify(rest)
    `,
    {
      getConfig: {
        description: "config",
        parameters: {},
        execute: async () => ({ region: "us", retries: 3 }),
      },
    }
  );
  assert.equal(result.output, 'prod:{"region":"us","retries":3}');
});

console.log(`\n${passed} stress checks passed.`);
