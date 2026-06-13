/**
 * E2E from the AGENT AUTHOR's seat. Agent-written code is untrusted,
 * unreviewed, and run the instant it's produced — so what matters is the
 * QUALITY of the feedback around each run, not just that correct code works.
 *
 * These tests assert the developer experience:
 *   - transparency: structured run report, tool-call trace with timing
 *   - catch-before-commit: dryRun (typecheck + side-effect-free smoke run)
 *   - legible failure: errors carry a source location, no silent hangs
 *   - self-verification: console.assert into stderr without aborting
 *   - recoverable state: fork a suspension into independent branches
 */
import assert from "node:assert/strict";
import { execute, dryRun, forkSnapshot, prepare, createSession } from "../dist/index.js";
import { Zapcode } from "@unchartedfr/zapcode";

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
  getUser: {
    description: "Fetch a user by id.",
    parameters: { id: { type: "number" } },
    returns: "{ name: string; age: number }",
    execute: async ({ id }) => ({ name: "Ada", age: 36, id }),
  },
  audit: {
    description: "Record an audit event (side-effecting).",
    parameters: { event: { type: "string" } },
    execute: async ({ event }) => ({ logged: event }),
  },
};

// ── transparency: structured run report + tool-call trace ──────────────────

await test("run report summarizes a successful run", async () => {
  const r = await execute("const u = await getUser({ id: 1 }); u.name", tools);
  assert.equal(r.output, "Ada");
  assert.equal(r.report.completed, true);
  assert.equal(r.report.toolCallCount, 1);
  assert.equal(typeof r.report.durationMs, "number");
  assert.equal(r.report.error, undefined);
});

await test("tool-call trace records name, validated input, result, timing", async () => {
  const r = await execute(
    "await getUser({ id: 7 }); await audit({ event: 'seen' }); 1",
    tools
  );
  assert.equal(r.toolCalls.length, 2);
  assert.equal(r.toolCalls[0].name, "getUser");
  assert.deepEqual(r.toolCalls[0].input, { id: 7 });
  assert.equal(r.toolCalls[0].result.name, "Ada");
  assert.equal(typeof r.toolCalls[0].durationMs, "number");
  assert.equal(r.toolCalls[1].name, "audit");
});

await test("a failed run reports the throw site (line) in the report", async () => {
  // autoFix mode returns a structured result instead of throwing.
  const r = await execute("const x = 1;\nconst y = null;\ny.field", tools, {
    autoFix: true,
  });
  assert.equal(r.report.completed, false);
  assert.ok(r.report.error, "expected a structured error");
  assert.equal(r.report.error.line, 3, `expected line 3, got ${r.report.error.line}`);
});

// ── provenance: scriptName names which script erred ────────────────────────

await test("scriptName labels the failure across autoFix, throw, and report.error", async () => {
  const BAD = "const x = nope.field; x"; // property access on undefined → throws

  // autoFix: the label rides on report.error.script and the human error string,
  // without disturbing the line/column the core attached.
  const r = await execute(BAD, {}, { scriptName: "planner", autoFix: true });
  assert.equal(r.report.error.script, "planner");
  assert.equal(r.report.error.line, 1);
  assert.match(r.error, /\[planner\]/);

  // throw path: the thrown Error's message is prefixed with the label.
  await assert.rejects(
    () => execute(BAD, {}, { scriptName: "planner" }),
    (e) => e.message.startsWith("[planner] ")
  );

  // opt-in: with no scriptName the message is unchanged (no bracket prefix).
  await assert.rejects(() => execute(BAD, {}), (e) => !e.message.startsWith("["));
});

await test("a session names the erring chunk by index in the thrown error", async () => {
  const session = createSession({ tools: {}, scriptName: "agent-7" });
  await session.runChunk("const a = 1;"); // chunk #1 — fine
  // chunk #2 throws; the label carries both the session name and chunk index.
  await assert.rejects(
    () => session.runChunk("const b = nope.field; b"),
    (e) => e.message.startsWith("[agent-7 #2] ")
  );
});

await test("console output survives across tool-call suspensions", async () => {
  // Regression: the snapshot/resume path used to drop ALL console output from
  // any program that called a tool. stdout/stderr must capture every segment.
  const t = (fn) => ({ description: "t", parameters: {}, execute: fn });
  const r = await execute(
    `
    console.log("start"); console.warn("w0");
    await tick();
    console.log("middle"); console.error("e1");
    await tick();
    console.log("end");
    "done";
    `,
    { tick: t(async () => 1) }
  );
  assert.equal(r.output, "done");
  assert.equal(r.stdout, "start\nmiddle\nend\n");
  assert.equal(r.stderr, "w0\ne1\n");
});

// ── catch-before-commit: dryRun ────────────────────────────────────────────

await test("dryRun passes valid code and records the tool-call sequence", async () => {
  const d = await dryRun(
    "const u = await getUser({ id: 1 }); await audit({ event: 'x' }); u",
    tools
  );
  assert.equal(d.ok, true);
  assert.equal(d.typeErrors.length, 0);
  assert.equal(d.reachedCompletion, true);
  assert.deepEqual(
    d.toolCalls.map((c) => c.name),
    ["getUser", "audit"]
  );
});

await test("dryRun catches a type error BEFORE running anything", async () => {
  const d = await dryRun("await getUser({ id: 'not-a-number' })", tools);
  assert.equal(d.ok, false);
  assert.ok(d.typeErrors.length > 0);
  assert.match(d.typeErrors[0].message, /not assignable/);
});

await test("dryRun surfaces an instant runtime error with a location", async () => {
  // Code that typechecks but blows up at runtime; dryRun catches it with NO
  // real tool side effect (audit is never really called).
  let realAuditRan = false;
  const t = {
    ...tools,
    audit: {
      ...tools.audit,
      execute: async () => {
        realAuditRan = true;
        return {};
      },
    },
  };
  const d = await dryRun("await audit({ event: 'x' });\nundefined.boom", t);
  assert.equal(d.reachedCompletion, false);
  assert.ok(d.error);
  assert.equal(d.error.line, 2);
  assert.equal(realAuditRan, false, "dryRun must NOT run real tool side effects");
});

// ── self-verification: console.assert ──────────────────────────────────────

await test("console.assert writes to stderr on failure without aborting", async () => {
  const r = await execute(
    "console.assert(1 > 2, 'x must exceed y'); console.log('still running'); 'done'",
    tools
  );
  assert.equal(r.output, "done");
  assert.equal(r.stdout, "still running\n");
  assert.match(r.stderr, /Assertion failed: x must exceed y/);
});

await test("console.assert is silent when the condition holds", async () => {
  const r = await execute("console.assert(2 > 1, 'ok'); 'done'", tools);
  assert.equal(r.stderr, "");
});

// ── legible failure: no silent hangs ───────────────────────────────────────

await test("a runaway loop fails with a clear limit error, not a hang", async () => {
  const r = await execute("let n = 0; while (true) { n++; } n", tools, {
    autoFix: true,
    timeLimitMs: 500,
  });
  assert.equal(r.report.completed, false);
  assert.ok(r.report.error, "expected a structured error");
  assert.match(r.report.error.message.toLowerCase(), /time|limit|exceeded/);
});

// ── recoverable state: forking ─────────────────────────────────────────────

await test("fork: one suspension yields independent, deterministic branches", async () => {
  // Drive a suspension manually via the raw binding, then fork the snapshot.
  const code =
    "async function main() { const base = 100; const r = await decide(); return base + r * 10; } main();";
  const vm = new Zapcode(code, { externalFunctions: ["decide"] });
  const suspension = vm.start();
  assert.equal(suspension.functionName, "decide");

  const branchA = forkSnapshot(suspension.snapshot).resume(3);
  const branchB = forkSnapshot(suspension.snapshot).resume(7);
  assert.equal(branchA.output, 130);
  assert.equal(branchB.output, 170);

  // Deterministic: re-forking the same checkpoint with the same input repeats.
  const branchAagain = forkSnapshot(suspension.snapshot).resume(3);
  assert.equal(branchAagain.output, 130);
});

await test("fork: checkpoint/rollback — a bad branch leaves the checkpoint intact", async () => {
  const code =
    "async function main() { const r = await pick(); if (r < 0) throw new Error('bad branch'); return r * 2; } main();";
  const vm = new Zapcode(code, { externalFunctions: ["pick"] });
  const checkpoint = vm.start().snapshot;

  // Explore a branch that throws — it does not corrupt the checkpoint.
  let badThrew = false;
  try {
    forkSnapshot(checkpoint).resume(-1);
  } catch {
    badThrew = true;
  }
  assert.equal(badThrew, true);

  // Roll back: a fresh fork from the same checkpoint takes a good branch.
  const good = forkSnapshot(checkpoint).resume(21);
  assert.equal(good.output, 42);
});

// ── prepare: compile once, run many ────────────────────────────────────────

await test("prepare: a compiled program runs many times, identical to execute", async () => {
  const code = "const u = await getUser({ id: 1 }); u.age + 1";
  const program = prepare(code, tools);
  const r1 = await program.run();
  const r2 = await program.run();
  // Same ergonomics as execute(): output, trace, report.
  const direct = await execute(code, tools);
  assert.equal(r1.output, direct.output);
  assert.equal(r1.output, r2.output); // independent runs, identical result
  assert.equal(r1.report.completed, true);
  assert.equal(r1.toolCalls[0].name, "getUser");
});

await test("prepare: a runtime error still reports its location", async () => {
  const program = prepare("const y = null;\ny.field", tools);
  const r = await program.run({}, { autoFix: true });
  assert.equal(r.report.completed, false);
  assert.equal(r.report.error.line, 2);
});

console.log(`\n${passed} agent-DX checks passed.`);
