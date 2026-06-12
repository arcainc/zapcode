/**
 * Smoke test for two ergonomics additions:
 *   1. stdout / stderr split — console.error/warn land in `stderr`, never in
 *      `stdout`; console.log/info/debug stay on `stdout`. Checked through both
 *      the raw napi binding and the zapcode-ai `execute()` wrapper.
 *   2. prepare-once `ZapcodeProgramHandle` — compile once, run many; dump/load
 *      round-trips the compiled bytecode through a Buffer.
 *
 * Ground-truthed against Node: console.error/warn -> stderr; log/info/debug ->
 * stdout (see crates/zapcode-core/tests/stderr_split.rs for the same split).
 */
import assert from "node:assert/strict";
import { Zapcode, ZapcodeProgramHandle } from "@unchartedfr/zapcode";
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

// 1a. Binding-level split.
await test("console.error/warn -> stderr, log/info/debug -> stdout (binding)", () => {
  const zc = new Zapcode(`
    console.log("out-log");
    console.info("out-info");
    console.debug("out-debug");
    console.error("err-error");
    console.warn("err-warn");
    42
  `);
  const res = zc.run();
  assert.equal(res.completed, true);
  assert.equal(res.stdout, "out-log\nout-info\nout-debug\n");
  assert.equal(res.stderr, "err-error\nerr-warn\n");
  // Crucially: stderr content does NOT appear in stdout.
  assert.ok(!res.stdout.includes("err-"), "stderr must not leak into stdout");
});

// 1b. Same split surfaced through the zapcode-ai execute() wrapper.
await test("ExecutionResult carries a separate stderr stream", async () => {
  const result = await execute(
    `console.log("hello"); console.error("oops"); "done"`,
    {}
  );
  assert.equal(result.output, "done");
  assert.equal(result.stdout, "hello\n");
  assert.equal(result.stderr, "oops\n");
});

// 2a. Prepared program runs to completion, reusable across inputs.
await test("ZapcodeProgramHandle.compile + run (many inputs)", () => {
  const program = ZapcodeProgramHandle.compile(`x * 2`, { inputs: ["x"] });
  const a = program.run({ x: 21 });
  const b = program.run({ x: 5 });
  assert.equal(a.output, 42);
  assert.equal(b.output, 10);
});

// 2b. Prepared program preserves the stderr split.
await test("ZapcodeProgramHandle preserves stdout/stderr split", () => {
  const program = ZapcodeProgramHandle.compile(
    `console.log("L"); console.warn("W"); 1`
  );
  const res = program.run();
  assert.equal(res.stdout, "L\n");
  assert.equal(res.stderr, "W\n");
});

// 2c. dump() / load() round-trips the compiled bytecode through a Buffer.
await test("ZapcodeProgramHandle.dump() / load() round-trip", () => {
  const program = ZapcodeProgramHandle.compile(`a + b`, { inputs: ["a", "b"] });
  const bytes = program.dump();
  assert.ok(Buffer.isBuffer(bytes) && bytes.length > 0, "dump produced bytes");
  const reloaded = ZapcodeProgramHandle.load(bytes);
  const res = reloaded.run({ a: 3, b: 4 });
  assert.equal(res.output, 7);
});

console.log(`\n${passed} stderr/program smoke checks passed`);
