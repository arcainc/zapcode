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

// ── structured typecheck() API (tsgo-preferred, tsc fallback) ─────────────
import { typecheck, formatDiagnosticsForModel } from "../dist/index.js";

const typedTools = {
  getWeather: {
    description: "Weather.",
    parameters: { city: { type: "string" } },
    returns: "{ temp: number; city: string }",
    execute: async ({ city }) => ({ temp: 26, city }),
  },
  ping: { description: "Ping.", parameters: {}, execute: async () => "pong" },
};

for (const engine of ["typescript", "tsgo"]) {
  let available = true;
  if (engine === "tsgo") {
    try { await import("@typescript/native-preview/package.json", { with: { type: "json" } }); }
    catch { available = false; }
  }
  if (!available) { console.log(`  - skipping ${engine} (not installed)`); continue; }

  await test(`[${engine}] structured diagnostics with agent-relative positions`, async () => {
    const code = 'const w = await getWeather({ city: 42 });\nreturn w.temp.toUpperCase();';
    const r = await typecheck(code, typedTools, { engine });
    assert.equal(r.engine, engine);
    assert.equal(r.ok, false);
    assert.equal(r.diagnostics.length, 2);
    assert.equal(r.diagnostics[0].line, 1);
    assert.equal(r.diagnostics[0].severity, "error");
    assert.match(r.diagnostics[0].message, /not assignable/);
    // The typed return makes downstream misuse a pre-execution failure.
    assert.equal(r.diagnostics[1].line, 2);
    assert.match(r.diagnostics[1].message, /toUpperCase/);
  });

  await test(`[${engine}] both call shapes type-check for single-param tools`, async () => {
    for (const call of ['getWeather({ city: "sf" })', 'getWeather("sf")', "ping()", "ping({})"]) {
      const r = await typecheck(`const x = await ${call}; return 1;`, typedTools, { engine });
      assert.equal(r.ok, true, `${call}: ${JSON.stringify(r.diagnostics)}`);
    }
  });

  await test(`[${engine}] unknown tool fails the type pass`, async () => {
    const r = await typecheck("return await dropTables();", typedTools, { engine });
    assert.equal(r.ok, false);
  });
}

await test("formatDiagnosticsForModel cites the offending line with a caret", async () => {
  const code = 'const w = await getWeather({ city: 42 });\nreturn w;';
  const r = await typecheck(code, typedTools);
  const text = formatDiagnosticsForModel(r, code);
  assert.match(text, /Type error at line 1, column \d+/);
  assert.match(text, /getWeather\(\{ city: 42 \}\)/);
  assert.match(text, /\^/);
});

console.log(`\n${passed} type-check tests passed.`);
