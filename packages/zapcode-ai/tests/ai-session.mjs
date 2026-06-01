/**
 * e2e: the zapcode-ai durable session bridge. An agent defines a workflow in
 * one chunk; it runs (with tool calls, errors, and parallel batches) in later
 * chunks; and the whole VM state survives a dump/load boundary — the
 * "define now, run later across a Temporal activity" path. Real TypeScript.
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

function rowTools(log) {
  return {
    fetchRow: {
      description: "Fetch a row by id.",
      parameters: { id: { type: "string" } },
      execute: async ({ id }) => {
        log?.push(id);
        if (id === "missing") throw new Error("not found");
        return { id, value: id.toUpperCase() };
      },
    },
  };
}

console.log("ai-session e2e");

await test("define a workflow in one chunk, invoke it in a later chunk", async () => {
  const session = createSession({ tools: rowTools() });
  // Chunk 1: the agent writes the workflow (a function + a binding).
  const first = await session.runChunk(`
    async function loadValue(id) {
      const row = await fetchRow(id);
      return row.value;
    }
    const prefix = "row:";
    prefix
  `);
  assert.equal(first.output, "row:");

  // Chunk 2: call the earlier function, which itself calls a tool.
  const second = await session.runChunk(`prefix + (await loadValue("a"))`);
  assert.equal(second.output, "row:A");
  assert.equal(second.toolCalls.length, 1);
  assert.equal(second.toolCalls[0].name, "fetchRow");
});

await test("session survives dump/load across an activity boundary", async () => {
  const session = createSession({ tools: rowTools() });
  await session.runChunk(`const base = 100; base`);

  // Serialize, "ship to another activity", reload with fresh tool impls.
  const bytes = session.dump();
  const resumed = loadSession(bytes, { tools: rowTools() });

  const out = await resumed.runChunk(`base + 1`);
  assert.equal(out.output, 101);
});

await test("parallel batch + tool errors work through the session bridge", async () => {
  const calls = [];
  const session = createSession({ tools: rowTools(calls) });

  const batched = await session.runChunk(`
    const rows = await Promise.all([fetchRow("a"), fetchRow("b"), fetchRow("c")]);
    rows.map(r => r.value).join(",")
  `);
  assert.equal(batched.output, "A,B,C");
  assert.equal(batched.toolCalls.length, 3);

  const recovered = await session.runChunk(`
    let out;
    try {
      await fetchRow("missing");
      out = "no-error";
    } catch (e) {
      out = "caught:" + e;
    }
    out
  `);
  assert.equal(recovered.output, "caught:not found");
});

console.log(`\n${passed} ai-session checks passed.`);
