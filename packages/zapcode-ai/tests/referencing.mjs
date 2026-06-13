/**
 * e2e: content-addressed snapshots (dumpReferenced / loadWithPrograms) — v17.
 * The program bytecode is elided from the snapshot and supplied at load, so a
 * fleet of parked snapshots of one workflow stores the program once. The core
 * round-trip + validation is covered by Rust tests; this pins the napi surface.
 */
import assert from "node:assert/strict";
import { ZapcodeSnapshotHandle, ZapcodeProgramHandle } from "@unchartedfr/zapcode";

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

console.log("referencing e2e");

const CODE = `let s = 0; for (let i = 0; i < 40; i++) { s += i * 2 + 1; } const r = await f(); s + r`;
const EXPECT = (() => { let s = 0; for (let i = 0; i < 40; i++) s += i * 2 + 1; return s + 7; })();

function suspend() {
  const prog = ZapcodeProgramHandle.compile(CODE, { externalFunctions: ["f"] });
  return { prog, sus: prog.start() };
}

test("referenced dump is smaller and resumes with the supplied program", () => {
  const { prog, sus } = suspend();
  const h = ZapcodeSnapshotHandle.load(sus.snapshot);
  const ref = h.dumpReferenced();
  assert.ok(ref.length < sus.snapshot.length, `referenced ${ref.length} < self ${sus.snapshot.length}`);
  const resumed = ZapcodeSnapshotHandle.loadWithPrograms(ref, [prog.dump()]).resume(7);
  assert.equal(resumed.completed, true);
  assert.equal(resumed.output, EXPECT);
});

test("a recompile of the same source resumes a referenced blob (deterministic)", () => {
  const { sus } = suspend();
  const ref = ZapcodeSnapshotHandle.load(sus.snapshot).dumpReferenced();
  const recompiled = ZapcodeProgramHandle.compile(CODE, { externalFunctions: ["f"] });
  const resumed = ZapcodeSnapshotHandle.loadWithPrograms(ref, [recompiled.dump()]).resume(7);
  assert.equal(resumed.output, EXPECT);
});

test("plain load() rejects a referenced blob", () => {
  const { sus } = suspend();
  const ref = ZapcodeSnapshotHandle.load(sus.snapshot).dumpReferenced();
  assert.throws(() => ZapcodeSnapshotHandle.load(ref), /referenced/);
});

test("a mismatched program is rejected (fingerprint), never a crash", () => {
  const { sus } = suspend();
  const ref = ZapcodeSnapshotHandle.load(sus.snapshot).dumpReferenced();
  const wrong = ZapcodeProgramHandle.compile(`const r = await f(); r + 1`, { externalFunctions: ["f"] });
  assert.throws(() => ZapcodeSnapshotHandle.loadWithPrograms(ref, [wrong.dump()]), /fingerprint mismatch/);
});

test("wrong program count is rejected", () => {
  const { sus } = suspend();
  const ref = ZapcodeSnapshotHandle.load(sus.snapshot).dumpReferenced();
  assert.throws(() => ZapcodeSnapshotHandle.loadWithPrograms(ref, []), /needs 1 program|but 0/);
});

console.log(`\n${passed} referencing checks passed.`);
