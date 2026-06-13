/**
 * e2e: ZapcodeDriver — the in-process suspend/resume driver that keeps the VM
 * resident across tool hops (no dump+load per hop). This is what `execute` /
 * `prepare` drive internally; these tests pin the public binding API directly,
 * including the `dump()` escape hatch for cross-process durability.
 */
import assert from "node:assert/strict";
import { ZapcodeDriver, ZapcodeSnapshotHandle } from "@unchartedfr/zapcode";

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

console.log("driver e2e");

test("drives a multi-hop run, resuming each external call in memory", () => {
  const d = ZapcodeDriver.fromCode(
    `let sum = 0; for (let i = 0; i < 3; i++) { sum += await step({ i }); } sum`,
    { externalFunctions: ["step"] }
  );
  let s = d.start();
  let hops = 0;
  while (!s.completed) {
    assert.equal(s.kind, "suspended");
    assert.equal(s.functionName, "step");
    s = d.resume(s.args[0].i * 10);
    hops++;
  }
  assert.equal(hops, 3);
  assert.equal(s.output, 30); // 0 + 10 + 20
});

test("cumulative stdout/stderr survive across hops", () => {
  const d = ZapcodeDriver.fromCode(
    `console.log("a"); await f(); console.warn("b"); await f(); console.log("c"); "done"`,
    { externalFunctions: ["f"] }
  );
  let s = d.start();
  while (!s.completed) s = d.resume(null);
  assert.equal(s.output, "done");
  assert.equal(s.stdout, "a\nc\n");
  assert.equal(s.stderr, "b\n");
});

test("resumeMany settles a Promise.all batch", () => {
  const d = ZapcodeDriver.fromCode(
    `const xs = await Promise.all([f("a"), f("b"), f("c")]); xs.join("/")`,
    { externalFunctions: ["f"] }
  );
  let s = d.start();
  assert.equal(s.kind, "suspended_many");
  assert.equal(s.combinator, "all");
  assert.equal(s.calls.length, 3);
  s = d.resumeMany(["A", "B", "C"]);
  assert.equal(s.completed, true);
  assert.equal(s.output, "A/B/C");
});

test("resumeErrorObject raises a real Error the guest can catch", () => {
  const d = ZapcodeDriver.fromCode(
    `try { await f(); "no"; } catch (e) { [e instanceof Error, e.name, e.message].join(":"); }`,
    { externalFunctions: ["f"] }
  );
  let s = d.start();
  s = d.resumeErrorObject("kaboom", "TypeError");
  assert.equal(s.output, "true:TypeError:kaboom");
});

test("dump() at a suspension hands off to a snapshot handle (durability)", () => {
  const d = ZapcodeDriver.fromCode(`const r = await f(); "got:" + r`, { externalFunctions: ["f"] });
  const s = d.start();
  assert.equal(s.completed, false);
  // Persist the in-memory suspension to bytes and resume via the snapshot handle
  // — the same bytes a cross-process / fork path would use.
  const bytes = d.dump();
  const resumed = ZapcodeSnapshotHandle.load(bytes).resume("R");
  assert.equal(resumed.completed, true);
  assert.equal(resumed.output, "got:R");
});

console.log(`\n${passed} driver checks passed.`);
