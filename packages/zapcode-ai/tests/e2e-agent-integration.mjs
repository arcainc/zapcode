/**
 * e2e: end-to-end agent integration through the high-level `execute` API.
 *
 * Exercises the realistic "an LLM wrote this TypeScript, run it with these tools"
 * path: tool-call recording (name/args/input/result), async/await + Promise
 * combinators driven by the host bridge, tool-error handling, deferred-promise
 * `.then`/`.catch`/`.finally`, and multi-tool agent workflows. Asserts the actual
 * contract of this interpreter (e.g. a caught tool error surfaces as a real Error
 * object with name/message — see `tool_error_is_a_real_error`), verified against
 * the built local binding.
 */
import assert from "node:assert/strict";
import { execute } from "../dist/index.js";

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

console.log("e2e agent integration");

// ---------------------------------------------------------------------------
// Tool-call recording
// ---------------------------------------------------------------------------

await test("sequential tool calls produce the result and a recorded call log", async () => {
  const r = await execute(
    `const a = await getX({ n: 3 }); const b = await getX({ n: 4 }); a + b`,
    { getX: tool(async ({ n }) => n * 10, { n: { type: "number" } }) }
  );
  assert.equal(r.output, 70);
  assert.equal(r.toolCalls.length, 2);
  assert.equal(r.toolCalls[0].name, "getX");
  assert.deepEqual(r.toolCalls[0].input, { n: 3 });
  assert.equal(r.toolCalls[0].result, 30);
  assert.equal(r.toolCalls[1].result, 40);
});

await test("tool that returns an object is usable as a structured value", async () => {
  const r = await execute(
    `const row = await fetchRow({ id: 7 }); row.id + ':' + row.amount`,
    { fetchRow: tool(async ({ id }) => ({ id: "r" + id, amount: id * 10 }), { id: { type: "number" } }) }
  );
  assert.equal(r.output, "r7:70");
});

await test("a tool result flows through array methods and JSON", async () => {
  const r = await execute(
    `const items = await listItems({}); JSON.stringify(items.filter(x => x.active).map(x => x.id))`,
    { listItems: tool(async () => [
      { id: 1, active: true }, { id: 2, active: false }, { id: 3, active: true },
    ]) }
  );
  assert.equal(r.output, "[1,3]");
});

await test("no tool calls => empty call log", async () => {
  const r = await execute(`[1, 2, 3].reduce((a, b) => a + b, 0)`, {});
  assert.equal(r.output, 6);
  assert.equal(r.toolCalls.length, 0);
});

// ---------------------------------------------------------------------------
// Async / await sequencing
// ---------------------------------------------------------------------------

await test("await sequences dependent tool calls", async () => {
  const r = await execute(
    `const user = await getUser({ id: 1 });
     const orders = await getOrders({ user: user.name });
     user.name + ' has ' + orders.length + ' orders'`,
    {
      getUser: tool(async ({ id }) => ({ id, name: "u" + id }), { id: { type: "number" } }),
      getOrders: tool(async ({ user }) => [user + "-o1", user + "-o2"], { user: { type: "string" } }),
    }
  );
  assert.equal(r.output, "u1 has 2 orders");
});

await test("async function defined and awaited in the same program", async () => {
  const r = await execute(
    `async function pipeline(id) {
       const a = await step({ id });
       const b = await step({ id: a });
       return b;
     }
     await pipeline(2)`,
    { step: tool(async ({ id }) => id * 3, { id: { type: "number" } }) }
  );
  assert.equal(r.output, 18); // 2 -> 6 -> 18
});

// ---------------------------------------------------------------------------
// Promise combinators (host-driven parallel batches)
// ---------------------------------------------------------------------------

await test("Promise.all runs tool calls and preserves element order", async () => {
  const r = await execute(
    `const [a, b, c] = await Promise.all([f({ i: 1 }), f({ i: 2 }), f({ i: 3 })]); a + ',' + b + ',' + c`,
    { f: tool(async ({ i }) => i * 100, { i: { type: "number" } }) }
  );
  assert.equal(r.output, "100,200,300");
});

await test("Promise.all of mixed tool calls + literals", async () => {
  const r = await execute(
    `const [a, b] = await Promise.all([f({ i: 5 }), Promise.resolve(99)]); a + b`,
    { f: tool(async ({ i }) => i, { i: { type: "number" } }) }
  );
  assert.equal(r.output, 104);
});

await test("Promise.race resolves with the first settled tool", async () => {
  const r = await execute(
    `await Promise.race([f({ i: 1 }), f({ i: 2 })])`,
    { f: tool(async ({ i }) => i, { i: { type: "number" } }) }
  );
  assert.ok(r.output === 1 || r.output === 2);
});

await test("Promise.allSettled reports per-element status", async () => {
  const r = await execute(
    `const res = await Promise.allSettled([ok({}), bad({})]);
     JSON.stringify(res.map(x => x.status))`,
    { ok: tool(async () => 1), bad: tool(async () => { throw new Error("x"); }) }
  );
  assert.equal(r.output, '["fulfilled","rejected"]');
});

await test("Promise.any resolves with the first fulfilled tool", async () => {
  const r = await execute(
    `await Promise.any([bad({}), ok({})])`,
    { ok: tool(async () => "won"), bad: tool(async () => { throw new Error("x"); }) }
  );
  assert.equal(r.output, "won");
});

// ---------------------------------------------------------------------------
// Tool errors
// ---------------------------------------------------------------------------

await test("tool_error_is_a_real_error: a caught tool error is an Error with name/message", async () => {
  // CONTRACT: when a tool throws, the value caught in the guest is a real Error
  // object — `e instanceof Error` holds, `e.message` is the message, and the
  // host error's subclass name (e.g. TypeError) is preserved.
  const r = await execute(
    `let out; try { await boom({}); out = 'no'; } catch (e) { out = [e instanceof Error, e.name, e.message].join(':'); } out`,
    { boom: tool(async () => { throw new TypeError("kaboom"); }) }
  );
  assert.equal(r.output, "true:TypeError:kaboom");
});

await test("a tool error short-circuits the program when uncaught", async () => {
  await assert.rejects(
    () => execute(`const v = await boom({}); v`, {
      boom: tool(async () => { throw new Error("upstream failure"); }),
    }),
    /upstream failure/
  );
});

await test("catch + fallback tool: primary fails, fallback succeeds", async () => {
  const r = await execute(
    `let v;
     try { v = await primary({}); }
     catch (e) { v = await fallback({}); }
     v`,
    {
      primary: tool(async () => { throw new Error("down"); }),
      fallback: tool(async () => "fallback-value"),
    }
  );
  assert.equal(r.output, "fallback-value");
});

// ---------------------------------------------------------------------------
// Deferred promises (.then / .catch / .finally)
// ---------------------------------------------------------------------------

await test("a bare tool call is a deferred promise; .then drives the call", async () => {
  const r = await execute(
    `let v = 0; await getX({ n: 5 }).then(x => { v = x * 2; return v; }); v`,
    { getX: tool(async ({ n }) => n, { n: { type: "number" } }) }
  );
  assert.equal(r.output, 10);
});

await test(".catch on a rejected deferred tool promise recovers", async () => {
  const r = await execute(
    `await boom({}).catch(() => 'recovered')`,
    { boom: tool(async () => { throw new Error("nope"); }) }
  );
  assert.equal(r.output, "recovered");
});

await test(".then chaining across two tools", async () => {
  const r = await execute(
    `await a({ n: 2 }).then(x => b({ n: x }))`,
    {
      a: tool(async ({ n }) => n + 1, { n: { type: "number" } }),
      b: tool(async ({ n }) => n * 10, { n: { type: "number" } }),
    }
  );
  assert.equal(r.output, 30); // 2 -> 3 -> 30
});

await test("typeof a bare (un-awaited) tool call is object (a promise)", async () => {
  const r = await execute(
    `const p = getX({ n: 1 }); const t = typeof p; await p; t`,
    { getX: tool(async ({ n }) => n, { n: { type: "number" } }) }
  );
  assert.equal(r.output, "object");
});

// ---------------------------------------------------------------------------
// Realistic multi-tool workflows
// ---------------------------------------------------------------------------

await test("ETL-style workflow: extract -> transform -> load", async () => {
  const loaded = [];
  const r = await execute(
    `const rows = await extract({});
     const transformed = rows.map(r => ({ id: r.id, total: r.qty * r.price }));
     for (const t of transformed) { await load(t); }
     transformed.reduce((s, t) => s + t.total, 0)`,
    {
      extract: tool(async () => [
        { id: 1, qty: 2, price: 5 }, { id: 2, qty: 3, price: 10 },
      ]),
      load: tool(async (row) => { loaded.push(row); return "ok"; }, {
        id: { type: "number" }, total: { type: "number" },
      }),
    }
  );
  assert.equal(r.output, 40); // 10 + 30
  assert.equal(loaded.length, 2);
});

await test("retry loop: tool fails twice then succeeds", async () => {
  let attempts = 0;
  const r = await execute(
    `let result, lastErr;
     for (let i = 0; i < 5; i++) {
       try { result = await flaky({ attempt: i }); break; }
       catch (e) { lastErr = String(e); }
     }
     result`,
    {
      flaky: tool(async () => {
        attempts++;
        if (attempts < 3) throw new Error("transient");
        return "success";
      }, { attempt: { type: "number" } }),
    }
  );
  assert.equal(r.output, "success");
  assert.equal(attempts, 3);
});

await test("fan-out then aggregate via Promise.all + reduce", async () => {
  const r = await execute(
    `const ids = [1, 2, 3, 4];
     const results = await Promise.all(ids.map(id => score({ id })));
     results.reduce((a, b) => a + b, 0)`,
    { score: tool(async ({ id }) => id * id, { id: { type: "number" } }) }
  );
  assert.equal(r.output, 30); // 1 + 4 + 9 + 16
});

console.log(`\n${passed} e2e agent-integration checks passed.`);
