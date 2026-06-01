/**
 * e2e: `Promise.all([tool(), tool(), ...])` in agent code suspends once with
 * the whole batch and the host runs the calls concurrently. Runs real
 * TypeScript in the sandbox.
 */
import assert from "node:assert/strict";
import { execute } from "../dist/index.js";
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

console.log("parallel e2e");

await test("Promise.all of tool calls runs them concurrently and preserves order", async () => {
  let inFlight = 0;
  let maxInFlight = 0;
  const tools = {
    fetchOne: {
      description: "Fetch one key.",
      parameters: { key: { type: "string" } },
      execute: async ({ key }) => {
        inFlight++;
        maxInFlight = Math.max(maxInFlight, inFlight);
        await new Promise(r => setTimeout(r, 20));
        inFlight--;
        return `v:${key}`;
      },
    },
  };

  const result = await execute(
    `
    const out = await Promise.all([fetchOne("a"), fetchOne("b"), fetchOne("c")]);
    out.join(",")
  `,
    tools
  );

  assert.equal(result.output, "v:a,v:b,v:c");
  assert.equal(result.toolCalls.length, 3);
  // All three were resolved in a single batch → they overlapped on the host.
  assert.ok(maxInFlight >= 2, `expected concurrent execution, max in-flight was ${maxInFlight}`);
});

await test("a failing call in Promise.all is catchable in guest code", async () => {
  const tools = {
    fetchOne: {
      description: "Fetch one key; 'bad' fails.",
      parameters: { key: { type: "string" } },
      execute: async ({ key }) => {
        if (key === "bad") throw new Error("boom");
        return `v:${key}`;
      },
    },
  };

  const result = await execute(
    `
    let out;
    try {
      await Promise.all([fetchOne("a"), fetchOne("bad")]);
      out = "no-error";
    } catch (e) {
      out = "caught:" + e;
    }
    out
  `,
    tools
  );
  assert.equal(result.output, "caught:boom");
});

await test("session batch survives dump/load and resumeMany", () => {
  const session = ZapcodeSessionHandle.create({ externalFunctions: ["fetchOne"] });
  const suspended = session.runChunk(
    `const out = await Promise.all([fetchOne("a"), fetchOne("b")]); out.join("/")`
  );
  assert.equal(suspended.completed, false);
  assert.equal(suspended.kind, "suspended_many");
  assert.equal(suspended.calls.length, 2);

  // Ship across a boundary, run the calls "in parallel" on the host, resume.
  const resumed = ZapcodeSessionHandle.load(suspended.session).resumeMany(["A", "B"]);
  assert.equal(resumed.completed, true);
  assert.equal(resumed.output, "A/B");
});

console.log(`\n${passed} parallel checks passed.`);
