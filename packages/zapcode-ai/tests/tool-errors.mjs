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

await test("a caught tool failure is a real Error (message/name/instanceof) in the guest", async () => {
  // Idiomatic agent code inspects `e.message` and branches on `e.name`; a thrown
  // tool must surface as a real Error, and the host error's subclass name (here
  // RangeError) must survive the boundary.
  const result = await execute(
    `
    try {
      await boom("x");
      "no-throw"
    } catch (e) {
      [e instanceof Error, e.name, e.message].join("|")
    }
    `,
    {
      boom: {
        description: "always throws a RangeError",
        parameters: { key: { type: "string" } },
        execute: async () => {
          throw new RangeError("out of range");
        },
      },
    }
  );
  assert.equal(result.output, "true|RangeError|out of range");
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

// N4: a tool (external) call inside a .then/.catch/.finally callback must be
// able to suspend and resume — the common `primary().catch(() => fallback())`
// retry/fallback pattern. Previously the array-callback guard blocked this.
await test(".catch callback can call a tool (fallback pattern)", async () => {
  const result = await execute(
    `
    const out = await Promise.reject("primary down")
      .catch(() => fetchData("fallback"));
    out
  `,
    {
      fetchData: {
        description: "Fetch fallback data.",
        parameters: { key: { type: "string" } },
        execute: async ({ key }) => `${key}-ok`,
      },
    }
  );
  assert.equal(result.output, "fallback-ok");
  assert.equal(result.toolCalls.length, 1);
  assert.equal(result.toolCalls[0].result, "fallback-ok");
});

await test(".then callback can call a tool threading the resolved value", async () => {
  const result = await execute(
    `
    const out = await Promise.resolve("ctx")
      .then((v) => fetchData(v));
    out
  `,
    {
      fetchData: {
        description: "Fetch data.",
        parameters: { key: { type: "string" } },
        execute: async ({ key }) => `${key}-ok`,
      },
    }
  );
  assert.equal(result.output, "ctx-ok");
});

await test(".finally callback runs a tool but passes the value through", async () => {
  let cleaned = 0;
  const result = await execute(
    `
    const out = await Promise.resolve(7)
      .finally(() => cleanup("done"));
    out
  `,
    {
      cleanup: {
        description: "Cleanup side effect.",
        parameters: { what: { type: "string" } },
        execute: async () => {
          cleaned++;
          return 999; // discarded by finally
        },
      },
    }
  );
  assert.equal(result.output, 7);
  assert.equal(cleaned, 1);
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
