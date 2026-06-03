/**
 * e2e: the AI-SDK INTEGRATION surface produced by `zapcode({ tools })`.
 *
 * This is the boundary an application wires into an LLM SDK. `zapcode()` turns a
 * tool registry into: a system prompt that documents the callable tools, the tool
 * shapes for the Vercel AI SDK / OpenAI / Anthropic, a universal `handleToolCall`
 * that runs LLM-emitted code, and a `custom` adapter extension point. This suite
 * pins all of those:
 *
 *   - the system prompt enumerates each tool as a `declare function` signature
 *     with a call shape and the tool-level description;
 *   - the SDK tool shapes all expose a single `execute_code(code: string)` tool
 *     (OpenAI `function`, Anthropic `input_schema`, Vercel callable), with `code`
 *     required;
 *   - `handleToolCall(code)` runs the code, resolves tool calls, and returns the
 *     full ExecutionResult (`code`, `output`, `toolCalls`); pure computation needs
 *     no tools; a guest runtime error rejects (autoFix off by default);
 *   - a custom adapter receives the AdapterContext and its output is keyed into
 *     `custom`;
 *   - multi-param tools render a one-named-object call shape and optional params
 *     are marked `?` and stripped from the recorded input when omitted.
 *
 * Documented residual asserted as actual: per-PARAMETER descriptions are NOT
 * rendered into the system prompt (only the tool-level description), and the
 * generated signature return type is `Promise<unknown>` (cluster L8/L9).
 */
import assert from "node:assert/strict";
import { zapcode, createAdapter } from "../dist/index.js";

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

function tool(description, parameters, execute) {
  return { description, parameters, execute };
}

console.log("e2e AI SDK adapters");

const weatherTools = {
  getWeather: tool(
    "Get current weather for a city",
    { city: { type: "string", description: "City name" } },
    async ({ city }) => `sunny in ${city}`
  ),
  add: tool(
    "Add two numbers",
    { a: { type: "number" }, b: { type: "number" } },
    async ({ a, b }) => a + b
  ),
};

// ---------------------------------------------------------------------------
// System prompt
// ---------------------------------------------------------------------------

await test("system prompt is a non-empty string documenting each tool", async () => {
  const { system } = zapcode({ tools: weatherTools });
  assert.equal(typeof system, "string");
  assert.ok(system.length > 0);
  assert.ok(system.includes("getWeather"));
  assert.ok(system.includes("add"));
  // tool-level descriptions are surfaced
  assert.ok(system.includes("Get current weather for a city"));
  assert.ok(system.includes("Add two numbers"));
});

await test("system prompt renders a declare-function signature and call shape", async () => {
  const { system } = zapcode({
    tools: {
      multi: tool("Multi-arg tool", { a: { type: "number" }, b: { type: "string" } }, async () => 0),
    },
  });
  assert.ok(system.includes("declare function multi(input: { a: number; b: string }): Promise<unknown>;"));
  assert.ok(system.includes("await multi({ a: number, b: string })"));
});

await test("optional params render with `?` in the signature", async () => {
  const { system } = zapcode({
    tools: {
      opt: tool("Optional tool", { req: { type: "number" }, maybe: { type: "string", optional: true } }, async () => 0),
    },
  });
  assert.ok(system.includes("req: number; maybe?: string"));
});

await test("per-parameter descriptions are NOT rendered (documented L8 residual)", async () => {
  const { system } = zapcode({
    tools: {
      look: tool("Look something up", { id: { type: "string", description: "UNIQUE-PARAM-DESC-MARKER" } }, async () => 0),
    },
  });
  // The tool-level description shows; the per-param description marker does not.
  assert.ok(system.includes("Look something up"));
  assert.equal(system.includes("UNIQUE-PARAM-DESC-MARKER"), false); // JS/ideal: would be included
});

await test("a no-tools registry still produces a usable system prompt", async () => {
  const { system, openaiTools } = zapcode({ tools: {} });
  assert.ok(system.length > 0);
  // the execute_code tool is always present even with no domain tools
  assert.equal(openaiTools.length, 1);
});

// ---------------------------------------------------------------------------
// SDK tool shapes
// ---------------------------------------------------------------------------

await test("OpenAI tool shape exposes execute_code(code: string) with code required", async () => {
  const { openaiTools } = zapcode({ tools: weatherTools });
  assert.equal(openaiTools.length, 1);
  const t = openaiTools[0];
  assert.equal(t.type, "function");
  assert.equal(t.function.name, "execute_code");
  assert.equal(t.function.parameters.type, "object");
  assert.equal(t.function.parameters.properties.code.type, "string");
  assert.deepEqual(t.function.parameters.required, ["code"]);
});

await test("Anthropic tool shape mirrors OpenAI via input_schema", async () => {
  const { anthropicTools } = zapcode({ tools: weatherTools });
  assert.equal(anthropicTools.length, 1);
  const t = anthropicTools[0];
  assert.equal(t.name, "execute_code");
  assert.equal(t.input_schema.type, "object");
  assert.equal(t.input_schema.properties.code.type, "string");
  assert.deepEqual(t.input_schema.required, ["code"]);
});

await test("Vercel AI SDK tools expose a callable execute_code", async () => {
  const { tools } = zapcode({ tools: weatherTools });
  assert.deepEqual(Object.keys(tools), ["execute_code"]);
  assert.equal(typeof tools.execute_code.execute, "function");
});

await test("the three SDK shapes agree on the tool name", async () => {
  const z = zapcode({ tools: weatherTools });
  assert.equal(z.openaiTools[0].function.name, "execute_code");
  assert.equal(z.anthropicTools[0].name, "execute_code");
  assert.ok("execute_code" in z.tools);
});

// ---------------------------------------------------------------------------
// handleToolCall
// ---------------------------------------------------------------------------

await test("handleToolCall runs code, resolves tools, and returns the full result", async () => {
  const z = zapcode({ tools: weatherTools });
  const r = await z.handleToolCall(`const w = await getWeather({ city: "Paris" }); w`);
  assert.equal(r.output, "sunny in Paris");
  assert.equal(r.code, `const w = await getWeather({ city: "Paris" }); w`);
  assert.equal(r.toolCalls.length, 1);
  assert.equal(r.toolCalls[0].name, "getWeather");
  assert.equal(r.toolCalls[0].result, "sunny in Paris");
});

await test("handleToolCall composes multiple tool calls in emitted code", async () => {
  const z = zapcode({ tools: weatherTools });
  const r = await z.handleToolCall(
    `const a = await add({ a: 2, b: 3 }); const b = await add({ a: a, b: 10 }); b`
  );
  assert.equal(r.output, 15);
  assert.equal(r.toolCalls.length, 2);
});

await test("handleToolCall handles pure computation with no tools", async () => {
  const z = zapcode({ tools: weatherTools });
  const r = await z.handleToolCall(`[1, 2, 3, 4].reduce((a, b) => a + b, 0)`);
  assert.equal(r.output, 10);
  assert.equal(r.toolCalls.length, 0);
});

await test("the Vercel tool.execute path runs code and returns the result", async () => {
  const z = zapcode({ tools: weatherTools });
  const out = await z.tools.execute_code.execute({ code: `await add({ a: 4, b: 5 })` });
  assert.equal(out.output, 9);
  assert.equal(out.toolCalls[0].name, "add");
});

await test("a guest runtime error rejects handleToolCall (autoFix off by default)", async () => {
  const z = zapcode({ tools: weatherTools });
  await assert.rejects(() => z.handleToolCall(`null.x`), /Cannot read properties of null/);
});

await test("an optional param omitted by the LLM is stripped from the recorded input", async () => {
  const z = zapcode({
    tools: {
      look: tool("Look", { req: { type: "number" }, opt: { type: "string", optional: true } }, async (x) => x.req),
    },
  });
  const r = await z.handleToolCall(`await look({ req: 5 })`);
  assert.equal(r.output, 5);
  assert.deepEqual(r.toolCalls[0].input, { req: 5 });
});

// ---------------------------------------------------------------------------
// Custom adapters
// ---------------------------------------------------------------------------

await test("a custom adapter receives the context and is keyed into custom", async () => {
  const myAdapter = createAdapter("my-sdk", (ctx) => ({
    systemMessage: ctx.system,
    toolName: ctx.toolName,
    hasHandler: typeof ctx.handleToolCall === "function",
  }));
  const z = zapcode({ tools: weatherTools, adapters: [myAdapter] });
  assert.deepEqual(Object.keys(z.custom), ["my-sdk"]);
  assert.equal(z.custom["my-sdk"].toolName, "execute_code");
  assert.equal(z.custom["my-sdk"].hasHandler, true);
  assert.equal(typeof z.custom["my-sdk"].systemMessage, "string");
});

await test("a custom adapter's handleToolCall runs guest code end-to-end", async () => {
  let runner;
  const adapter = createAdapter("runner", (ctx) => {
    runner = ctx.handleToolCall;
    return { ok: true };
  });
  zapcode({ tools: weatherTools, adapters: [adapter] });
  const r = await runner(`await add({ a: 100, b: 1 })`);
  assert.equal(r.output, 101);
});

await test("multiple custom adapters each get their own custom key", async () => {
  const a1 = createAdapter("alpha", () => ({ tag: "a" }));
  const a2 = createAdapter("beta", () => ({ tag: "b" }));
  const z = zapcode({ tools: weatherTools, adapters: [a1, a2] });
  assert.deepEqual(Object.keys(z.custom).sort(), ["alpha", "beta"]);
  assert.equal(z.custom.alpha.tag, "a");
  assert.equal(z.custom.beta.tag, "b");
});

console.log(`\n${passed} AI SDK adapter checks passed.`);
