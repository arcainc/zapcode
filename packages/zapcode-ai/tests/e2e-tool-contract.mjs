/**
 * e2e: the tool-call BOUNDARY contract — recording shape, argument validation,
 * coercion, and the records emitted for success / failure / skipped calls.
 *
 * This is the surface an orchestrator inspects after a run (audit log, billing,
 * replay). It pins:
 *
 *   - the `toolCalls` record shape: `{ name, args, input, result }` on success and
 *     a string `error` on failure (with `result` left `undefined`);
 *   - `args` is the RAW positional argument array the guest passed; `input` is the
 *     post-validation named-argument object with optional params stripped;
 *   - object keys crossing the boundary are emitted in SORTED (alphabetical) order
 *     — a deterministic normalization, asserted as ACTUAL (the historical
 *     "insertion order preserved" note does not hold for the current binding);
 *   - calls are recorded in execution order; a call in a not-taken branch is absent;
 *   - a tool error is recorded as a STRING `error` (the message) on the trace record,
 *     while in the guest catch it surfaces as a real Error (message/name/instanceof);
 *   - primitive type validation (number/string/boolean) accepts well-typed args and
 *     rejects mismatches / missing-required with a descriptive, abort-level error;
 *   - `array` / `object` typed params pass their structured payload through intact.
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

function tool(fn, parameters = {}, description = "t") {
  return { description, parameters, execute: fn };
}

console.log("e2e tool-call boundary contract");

// ---------------------------------------------------------------------------
// Record shape
// ---------------------------------------------------------------------------

await test("a successful call records name, args, input, and result", async () => {
  const r = await execute(`await echo({ a: 1, b: 'x' })`, {
    echo: tool(async (args) => args, { a: { type: "number" }, b: { type: "string" } }),
  });
  const rec = r.toolCalls[0];
  assert.equal(rec.name, "echo");
  assert.deepEqual(rec.input, { a: 1, b: "x" });
  assert.deepEqual(rec.result, { a: 1, b: "x" });
  assert.ok(Array.isArray(rec.args));
  assert.equal("error" in rec, false);
});

await test("args is the raw positional array; input is the validated named object", async () => {
  const r = await execute(`await f({ a: 1 })`, {
    f: tool(async (a) => a, { a: { type: "number" }, b: { type: "number", optional: true } }),
  });
  const rec = r.toolCalls[0];
  assert.deepEqual(rec.args, [{ a: 1 }]);
  // optional param not supplied → stripped from the validated input.
  assert.deepEqual(rec.input, { a: 1 });
});

await test("object keys crossing the boundary are normalized to sorted order", async () => {
  const r = await execute(`await echo({ zebra: 1, apple: 2, mango: 3 })`, {
    echo: tool(async (a) => a, {
      zebra: { type: "number" },
      apple: { type: "number" },
      mango: { type: "number" },
    }),
  });
  // Deterministic alphabetical normalization (asserted as actual behavior).
  assert.deepEqual(Object.keys(r.toolCalls[0].input), ["apple", "mango", "zebra"]);
  assert.deepEqual(Object.keys(r.toolCalls[0].result), ["apple", "mango", "zebra"]);
});

// ---------------------------------------------------------------------------
// Ordering & branch sensitivity
// ---------------------------------------------------------------------------

await test("calls are recorded in execution order", async () => {
  const r = await execute(`await a({}); await b({}); await a({})`, {
    a: tool(async () => 1),
    b: tool(async () => 2),
  });
  assert.deepEqual(r.toolCalls.map((c) => c.name), ["a", "b", "a"]);
});

await test("a call in a not-taken branch is absent from the log", async () => {
  const r = await execute(`if (false) { await a({}); } await b({})`, {
    a: tool(async () => 1),
    b: tool(async () => 2),
  });
  assert.deepEqual(r.toolCalls.map((c) => c.name), ["b"]);
});

await test("a loop records one entry per invocation", async () => {
  const r = await execute(`for (let i = 0; i < 3; i++) { await tick({ i }); }`, {
    tick: tool(async ({ i }) => i, { i: { type: "number" } }),
  });
  assert.equal(r.toolCalls.length, 3);
  assert.deepEqual(r.toolCalls.map((c) => c.input.i), [0, 1, 2]);
});

// ---------------------------------------------------------------------------
// Error recording + in-guest error fidelity
// ---------------------------------------------------------------------------

await test("a failed call records a string error and an undefined result", async () => {
  const r = await execute(`try { await fail({}); } catch (e) {} 'done'`, {
    fail: tool(async () => { throw new Error("boom"); }),
  });
  const rec = r.toolCalls[0];
  assert.equal(rec.name, "fail");
  // The trace record's `error` is the message string (host-loggable).
  assert.equal(rec.error, "boom");
  // result is present-but-undefined on the failure record.
  assert.equal(rec.result, undefined);
});

await test("a caught tool error is a real Error in the guest (message/name/instanceof)", async () => {
  // A thrown tool now rejects the guest's await with a real Error object, so
  // idiomatic `catch (e) { e.message }` works and the host error's subclass
  // name is preserved.
  const r = await execute(
    `try { await fail({}); 'no'; } catch (e) { [e instanceof Error, e.name, e.message].join(':'); }`,
    { fail: tool(async () => { throw new TypeError("kaboom"); }) }
  );
  assert.equal(r.output, "true:TypeError:kaboom");
});

// ---------------------------------------------------------------------------
// Argument validation & coercion
// ---------------------------------------------------------------------------

await test("well-typed primitive args validate and reach the tool with their types", async () => {
  const r = await execute(`await f({ n: 42, s: 'hi', flag: true })`, {
    f: tool(async (a) => `${typeof a.n}/${typeof a.s}/${typeof a.flag}`, {
      n: { type: "number" },
      s: { type: "string" },
      flag: { type: "boolean" },
    }),
  });
  assert.equal(r.output, "number/string/boolean");
});

await test("a wrong-typed argument is rejected before execution", async () => {
  await assert.rejects(
    () => execute(`await f({ n: 'not a number' })`, { f: tool(async (a) => a, { n: { type: "number" } }) }),
    /expected number, got string/
  );
});

await test("a missing required argument is rejected before execution", async () => {
  await assert.rejects(
    () => execute(`await f({})`, { f: tool(async (a) => a, { req: { type: "number" } }) }),
    /parameter 'req'/
  );
});

await test("calling an unregistered tool name throws (unbound ident is undefined)", async () => {
  // An unknown identifier reads as `undefined`, so invoking it is a TypeError
  // ("undefined is not a function") rather than a named-tool diagnostic.
  await assert.rejects(
    () => execute(`await doesNotExist({})`, { real: tool(async () => 1) }),
    /is not a function/
  );
});

await test("array and object typed params pass their payload through intact", async () => {
  const r = await execute(`await f({ items: [1, 2, 3], cfg: { x: 1, y: 2 } })`, {
    f: tool(async (a) => a.items.length + ":" + (a.cfg.x + a.cfg.y), {
      items: { type: "array" },
      cfg: { type: "object" },
    }),
  });
  assert.equal(r.output, "3:3");
  assert.deepEqual(r.toolCalls[0].input.items, [1, 2, 3]);
  assert.deepEqual(r.toolCalls[0].input.cfg, { x: 1, y: 2 });
});

await test("a no-arg tool accepts both tool() and tool({})", async () => {
  const r1 = await execute(`await ping()`, { ping: tool(async () => "pong") });
  assert.equal(r1.output, "pong");
  const r2 = await execute(`await ping({})`, { ping: tool(async () => "pong") });
  assert.equal(r2.output, "pong");
});

// ---------------------------------------------------------------------------
// Result flows back into the program
// ---------------------------------------------------------------------------

await test("a structured result is usable as a live value and recorded", async () => {
  const r = await execute(
    `const row = await fetch({ id: 7 }); row.tags.map(t => t.toUpperCase()).join(',')`,
    { fetch: tool(async ({ id }) => ({ id, tags: ["a", "b"] }), { id: { type: "number" } }) }
  );
  assert.equal(r.output, "A,B");
  assert.deepEqual(r.toolCalls[0].result, { id: 7, tags: ["a", "b"] });
});

console.log(`\n${passed} tool-contract checks passed.`);
