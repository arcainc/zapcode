/**
 * e2e: a failing host tool surfaces to agent-written code as a catchable
 * runtime error (so the agent can retry / fall back), while an uncaught tool
 * failure still aborts the execution. Runs real TypeScript in the sandbox.
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

console.log("tool-errors e2e");

// A flaky tool: throws on the first call, succeeds on the second.
function flakyTools() {
  let calls = 0;
  return {
    fetchData: {
      description: "Fetch some data; fails intermittently.",
      parameters: { key: { type: "string" } },
      execute: async ({ key }) => {
        calls++;
        if (calls === 1) throw new Error("503 service unavailable");
        return `${key}-ok`;
      },
    },
  };
}

await test("agent code can try/catch a failing tool and retry", async () => {
  const result = await execute(
    `
    let out;
    try {
      out = await fetchData("a");
    } catch (e) {
      // tool failed — retry once
      out = await fetchData("a");
    }
    out
  `,
    flakyTools()
  );
  assert.equal(result.output, "a-ok");
  // Both attempts are recorded; the first carries an error.
  assert.equal(result.toolCalls.length, 2);
  assert.equal(result.toolCalls[0].error, "503 service unavailable");
  assert.equal(result.toolCalls[0].result, undefined);
  assert.equal(result.toolCalls[1].result, "a-ok");
});

await test("an uncaught tool failure aborts execution", async () => {
  await assert.rejects(
    () =>
      execute(`const v = await fetchData("a"); v`, {
        fetchData: {
          description: "always fails",
          parameters: { key: { type: "string" } },
          execute: async () => {
            throw new Error("permanent failure");
          },
        },
      }),
    /permanent failure/
  );
});

await test("autoFix surfaces an uncaught tool failure as a structured error", async () => {
  const result = await execute(
    `const v = await fetchData("a"); v`,
    {
      fetchData: {
        description: "always fails",
        parameters: { key: { type: "string" } },
        execute: async () => {
          throw new Error("permanent failure");
        },
      },
    },
    { autoFix: true }
  );
  assert.equal(result.output, null);
  assert.match(String(result.error), /permanent failure/);
});

console.log(`\n${passed} tool-error checks passed.`);
