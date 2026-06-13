/**
 * e2e: content-addressed snapshots (v17) AND sessions (v18) —
 * dumpReferenced / loadWithPrograms. The program bytecode is elided and supplied
 * at load, so a fleet of parked snapshots/sessions of one workflow stores the
 * program once. The core round-trip + validation is covered by Rust tests; this
 * pins the napi + high-level (createSession/loadSession) surface.
 */
import assert from "node:assert/strict";
import { ZapcodeSnapshotHandle, ZapcodeProgramHandle, ZapcodeSessionHandle } from "@unchartedfr/zapcode";
import { createSession, loadSession } from "../dist/index.js";

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

console.log("referencing e2e");

const CODE = `let s = 0; for (let i = 0; i < 40; i++) { s += i * 2 + 1; } const r = await f(); s + r`;
const EXPECT = (() => { let s = 0; for (let i = 0; i < 40; i++) s += i * 2 + 1; return s + 7; })();

function suspend() {
  const prog = ZapcodeProgramHandle.compile(CODE, { externalFunctions: ["f"] });
  return { prog, sus: prog.start() };
}

await test("referenced dump is smaller and resumes with the supplied program", () => {
  const { prog, sus } = suspend();
  const h = ZapcodeSnapshotHandle.load(sus.snapshot);
  const ref = h.dumpReferenced();
  assert.ok(ref.length < sus.snapshot.length, `referenced ${ref.length} < self ${sus.snapshot.length}`);
  const resumed = ZapcodeSnapshotHandle.loadWithPrograms(ref, [prog.dump()]).resume(7);
  assert.equal(resumed.completed, true);
  assert.equal(resumed.output, EXPECT);
});

await test("a recompile of the same source resumes a referenced blob (deterministic)", () => {
  const { sus } = suspend();
  const ref = ZapcodeSnapshotHandle.load(sus.snapshot).dumpReferenced();
  const recompiled = ZapcodeProgramHandle.compile(CODE, { externalFunctions: ["f"] });
  const resumed = ZapcodeSnapshotHandle.loadWithPrograms(ref, [recompiled.dump()]).resume(7);
  assert.equal(resumed.output, EXPECT);
});

await test("plain load() rejects a referenced blob", () => {
  const { sus } = suspend();
  const ref = ZapcodeSnapshotHandle.load(sus.snapshot).dumpReferenced();
  assert.throws(() => ZapcodeSnapshotHandle.load(ref), /referenced/);
});

await test("a mismatched program is rejected (fingerprint), never a crash", () => {
  const { sus } = suspend();
  const ref = ZapcodeSnapshotHandle.load(sus.snapshot).dumpReferenced();
  const wrong = ZapcodeProgramHandle.compile(`const r = await f(); r + 1`, { externalFunctions: ["f"] });
  assert.throws(() => ZapcodeSnapshotHandle.loadWithPrograms(ref, [wrong.dump()]), /fingerprint mismatch/);
});

await test("wrong program count is rejected", () => {
  const { sus } = suspend();
  const ref = ZapcodeSnapshotHandle.load(sus.snapshot).dumpReferenced();
  assert.throws(() => ZapcodeSnapshotHandle.loadWithPrograms(ref, []), /needs 1 program|but 0/);
});

// ── Sessions (v18) — raw binding ───────────────────────────────────────────

await test("session: dumpReferenced round-trips across a definition + use", () => {
  let s = ZapcodeSessionHandle.create({});
  const r1 = s.runChunk(`function calc(n){ let t=0; for(let i=0;i<n;i++) t+=i*2+1; return t; } const base=5;`);
  const sess = ZapcodeSessionHandle.load(r1.session);
  const ref = sess.dumpReferenced();
  const restored = ZapcodeSessionHandle.loadWithPrograms(ref.session, ref.programs);
  assert.equal(restored.runChunk(`calc(10) + base`).output, (() => { let t = 0; for (let i = 0; i < 10; i++) t += i * 2 + 1; return t + 5; })());
});

await test("session: plain load rejects a referenced blob; mismatched bundle rejected", () => {
  let s = ZapcodeSessionHandle.create({});
  const ref = ZapcodeSessionHandle.load(s.runChunk(`const x = 1;`).session).dumpReferenced();
  assert.throws(() => ZapcodeSessionHandle.load(ref.session), /referenced/);
  // a different session's bundle (same 1-program count, different bytecode)
  let s2 = ZapcodeSessionHandle.create({});
  const ref2 = ZapcodeSessionHandle.load(s2.runChunk(`const y = 9 + 8 + 7;`).session).dumpReferenced();
  assert.throws(() => ZapcodeSessionHandle.loadWithPrograms(ref.session, ref2.programs), /mismatch/);
});

// ── Sessions (v18) — high-level createSession / loadSession ────────────────

await test("createSession.dumpReferenced + loadSession({ programs }) survives a tool call", async () => {
  const db = new Map([["42", { id: "42", owner: "ada" }]]);
  const tools = {
    fetchRow: { description: "load", parameters: { id: { type: "string" } }, execute: async ({ id }) => db.get(id) ?? null },
  };
  const session = createSession({ tools });
  await session.runChunk(`async function step(id){ return await fetchRow({ id }); } const tag = "v18";`);
  const { session: sBytes, programs } = session.dumpReferenced();

  // ...elsewhere: reload from program-free bytes + the stored bundle.
  const resumed = loadSession(sBytes, { tools, programs });
  const out = await resumed.runChunk(`(await step("42")).owner + ":" + tag`);
  assert.equal(out.output, "ada:v18");
});

console.log(`\n${passed} referencing checks passed.`);
