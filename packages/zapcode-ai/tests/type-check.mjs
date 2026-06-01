/**
 * e2e: the optional type-check pre-pass catches malformed tool calls (unknown
 * tool, wrong argument type) as compile errors before running, and lets valid
 * code through to execute normally.
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

const tools = {
  getWeather: {
    description: "Get weather for a city.",
    parameters: { city: { type: "string" } },
    execute: async ({ city }) => ({ temp: 26, city }),
  },
  escalateTo: {
    description: "Escalate to an assignee.",
    parameters: {
      assignee: { type: "string" },
      dueAtMs: { type: "number" },
      reason: { type: "string" },
    },
    execute: async (input) => ({ ok: true, ...input }),
  },
};

console.log("type-check e2e");

await test("valid code passes the type-check and runs", async () => {
  const result = await execute(
    `const w = await getWeather("Tokyo"); w.temp`,
    tools,
    { typeCheck: true }
  );
  assert.equal(result.output, 26);
});

await test("wrong argument type is caught before execution", async () => {
  let ran = false;
  const spyTools = {
    getWeather: {
      ...tools.getWeather,
      execute: async (a) => {
        ran = true;
        return tools.getWeather.execute(a);
      },
    },
  };
  await assert.rejects(
    () => execute(`await getWeather(123)`, spyTools, { typeCheck: true }),
    /Type error before execution/
  );
  assert.equal(ran, false, "the tool must not run when the type-check fails");
});

await test("unknown tool name is caught before execution", async () => {
  await assert.rejects(
    () => execute(`await getWether("Tokyo")`, tools, { typeCheck: true }),
    /Type error before execution/
  );
});

await test("missing required argument on a multi-param tool is caught", async () => {
  await assert.rejects(
    () => execute(`await escalateTo({ assignee: "x", dueAtMs: 1 })`, tools, { typeCheck: true }),
    /Type error before execution/
  );
});

await test("autoFix returns the type error as a structured result", async () => {
  const result = await execute(`await getWeather(123)`, tools, {
    typeCheck: true,
    autoFix: true,
  });
  assert.equal(result.output, null);
  assert.match(String(result.error), /Type error before execution/);
});

console.log(`\n${passed} type-check checks passed.`);
