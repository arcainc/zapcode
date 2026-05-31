/**
 * e2e: durable sessions through the napi binding, exercising the
 * "agent defines a workflow now, it runs (and resumes) later across a
 * serialization boundary" path. Runs real TypeScript in the sandbox.
 *
 * This file grows as the durability features land. It must always pass
 * against the freshly-built local binding (see `npm run sync-local-binding`).
 */
import assert from "node:assert/strict";
import { ZapcodeSessionHandle } from "@unchartedfr/zapcode";

let passed = 0;
function test(name, fn) {
  try {
    fn();
    passed++;
    console.log(`  ✓ ${name}`);
  } catch (err) {
    console.error(`  ✗ ${name}`);
    throw err;
  }
}

console.log("durable-sessions e2e");

test("define a workflow in one chunk, run it in a later chunk", () => {
  const session = ZapcodeSessionHandle.create({ externalFunctions: [] });
  // Chunk 1: the agent "writes" the workflow (a function + a binding).
  const first = session.runChunk(`
    function double(x) { return x * 2; }
    const base = 21;
    base
  `);
  assert.equal(first.completed, true);
  assert.equal(first.output, 21);

  // Chunk 2: call the earlier function — proves cross-chunk program refs survive.
  const second = ZapcodeSessionHandle.load(first.session).runChunk(`double(base)`);
  assert.equal(second.completed, true);
  assert.equal(second.output, 42);
});

test("suspend on a tool call, then dump/load/resume across a boundary", () => {
  const session = ZapcodeSessionHandle.create({ externalFunctions: ["lookup"] });
  const suspended = session.runChunk(`
    const a = await lookup("first");
    const b = await lookup("second");
    a + "/" + b
  `);
  assert.equal(suspended.completed, false);
  assert.equal(suspended.functionName, "lookup");
  assert.deepEqual(suspended.args, ["first"]);

  // Serialize the whole VM state and "ship it to another activity".
  const wireBytes = suspended.session;
  const resumedHandle = ZapcodeSessionHandle.load(wireBytes);
  const next = resumedHandle.resume("A");
  assert.equal(next.completed, false);
  assert.deepEqual(next.args, ["second"]);

  // Ship again, resume again — different process, same state.
  const finalHandle = ZapcodeSessionHandle.load(next.session);
  const done = finalHandle.resume("B");
  assert.equal(done.completed, true);
  assert.equal(done.output, "A/B");
});

test("resumeError raises a catchable error at the suspension point", () => {
  const session = ZapcodeSessionHandle.create({ externalFunctions: ["callTool"] });
  const suspended = session.runChunk(`
    let outcome;
    try {
      const v = await callTool("x");
      outcome = "ok:" + v;
    } catch (e) {
      outcome = "caught:" + e;
    }
    outcome
  `);
  assert.equal(suspended.completed, false);

  // The host tool failed — feed the error back across the boundary.
  const done = ZapcodeSessionHandle.load(suspended.session).resumeError("upstream 500");
  assert.equal(done.completed, true);
  assert.equal(done.output, "caught:upstream 500");
});

test("uncaught resumeError propagates to the host", () => {
  const session = ZapcodeSessionHandle.create({ externalFunctions: ["callTool"] });
  const suspended = session.runChunk(`const v = await callTool("x"); v`);
  assert.throws(
    () => ZapcodeSessionHandle.load(suspended.session).resumeError("upstream 500"),
    /upstream 500/
  );
});

console.log(`\n${passed} durable-session checks passed.`);
