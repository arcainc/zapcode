/**
 * E2E: tools that perform REAL host-side side effects, asserted on the host
 * state after the run. This is the actual contract a host depends on — the
 * suspend/resume bridge invokes the registered tool, the tool mutates external
 * state (a store, a counter, an ordered audit log), and the agent code
 * orchestrates those effects. We assert the host state, not just the returned
 * value, across: ordering, loops, conditionals, data flow between calls, error
 * paths, parallel batches, durability across a serialized suspend/resume, and a
 * realistic mini-workflow over a host "database".
 */
import assert from "node:assert/strict";
import { execute } from "../dist/index.js";
import { ZapcodeSessionHandle } from "@unchartedfr/zapcode";

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

/** A host-side mutable world the tools act on. */
function makeWorld() {
  const world = {
    db: new Map(), // id -> record
    log: [], // ordered audit trail of every effect
    counter: 0,
  };
  const tools = {
    insert: {
      description: "Insert a record.",
      parameters: { id: { type: "number" }, value: { type: "string" } },
      returns: "{ ok: boolean; size: number }",
      execute: async ({ id, value }) => {
        world.db.set(id, value);
        world.log.push(`insert:${id}=${value}`);
        return { ok: true, size: world.db.size };
      },
    },
    get: {
      description: "Read a record (null if absent).",
      parameters: { id: { type: "number" } },
      returns: "string | null",
      execute: async ({ id }) => {
        world.log.push(`get:${id}`);
        return world.db.has(id) ? world.db.get(id) : null;
      },
    },
    remove: {
      description: "Delete a record.",
      parameters: { id: { type: "number" } },
      returns: "{ removed: boolean }",
      execute: async ({ id }) => {
        const removed = world.db.delete(id);
        world.log.push(`remove:${id}`);
        return { removed };
      },
    },
    bump: {
      description: "Increment the host counter, returning the new value.",
      parameters: {},
      returns: "number",
      execute: async () => {
        world.counter += 1;
        world.log.push(`bump:${world.counter}`);
        return world.counter;
      },
    },
    emit: {
      description: "Append a tagged event to the audit log.",
      parameters: { tag: { type: "string" } },
      execute: async ({ tag }) => {
        world.log.push(`emit:${tag}`);
        return null;
      },
    },
  };
  return { world, tools };
}

// ── ordering ───────────────────────────────────────────────────────────────

await test("side effects fire in source order, interleaved with computation", async () => {
  const { world, tools } = makeWorld();
  await execute(
    "await emit('a'); const n = 1 + 1; await emit('b' + n); if (n === 2) await emit('c'); 'done'",
    tools
  );
  assert.deepEqual(world.log, ["emit:a", "emit:b2", "emit:c"]);
});

// ── loops ────────────────────────────────────────────────────────────────

await test("a loop drives one side effect per iteration, in order", async () => {
  const { world, tools } = makeWorld();
  await execute(
    "for (const id of [10, 20, 30]) { await insert({ id, value: 'v' + id }); } 'ok'",
    tools
  );
  assert.equal(world.db.size, 3);
  assert.equal(world.db.get(20), "v20");
  assert.deepEqual(world.log, ["insert:10=v10", "insert:20=v20", "insert:30=v30"]);
});

// ── conditional: a not-taken branch performs NO effect ─────────────────────

await test("a side effect in a not-taken branch never fires", async () => {
  const { world, tools } = makeWorld();
  await execute(
    "const flag = false; if (flag) { await emit('should-not-happen'); } else { await emit('taken'); } 'ok'",
    tools
  );
  assert.deepEqual(world.log, ["emit:taken"]);
});

// ── data flow: a tool result decides the next call's args ──────────────────

await test("a tool result feeds the next tool call (read-modify-write)", async () => {
  const { world, tools } = makeWorld();
  world.db.set(1, "5"); // seed
  const r = await execute(
    "const cur = await get({ id: 1 }); const next = String(Number(cur) * 2); await insert({ id: 1, value: next }); next",
    tools
  );
  assert.equal(r.output, "10");
  assert.equal(world.db.get(1), "10");
  assert.deepEqual(world.log, ["get:1", "insert:1=10"]);
});

// ── error path: effects before a throw persist; catch lets effects continue ─

await test("effects before a thrown tool persist; catch continues effects", async () => {
  const { world, tools } = makeWorld();
  // A failing tool bound to the same world.
  tools.boom = {
    description: "always throws",
    parameters: {},
    execute: async () => {
      world.log.push("boom-attempted");
      throw new Error("kaboom");
    },
  };
  // NB: a tool error surfaces in-guest as a STRING (documented L4 residual,
  // see e2e-tool-contract), so `String(e)` is the message; `e.message` would
  // be undefined.
  const r = await execute(
    "await emit('before'); let caught = ''; \
     try { await boom(); await emit('unreached'); } catch (e) { caught = String(e); } \
     await emit('after'); caught",
    tools
  );
  assert.equal(r.output, "kaboom");
  // The effect before the throw and the boom attempt landed; the post-throw
  // 'unreached' did not; the catch let 'after' proceed.
  assert.deepEqual(world.log, ["emit:before", "boom-attempted", "emit:after"]);
});

// ── parallel batch: all effects happen ─────────────────────────────────────

await test("Promise.all over tools performs every side effect", async () => {
  const { world, tools } = makeWorld();
  const r = await execute(
    "const results = await Promise.all([bump(), bump(), bump()]); results.join(',')",
    tools
  );
  assert.equal(world.counter, 3);
  // Three bump effects recorded (order is the batch's settle order).
  assert.equal(world.log.filter((l) => l.startsWith("bump:")).length, 3);
  assert.equal(r.output, "1,2,3");
});

// ── durability: effects + state survive a serialized suspend/resume ────────

await test("side effects persist across a dump/load/resume boundary", () => {
  const world = { log: [], db: new Map() };
  const session = ZapcodeSessionHandle.create({ externalFunctions: ["step"] });

  // The agent does an effect, suspends, (ship state elsewhere), resumes, does
  // more — the host performs each effect as it resolves the suspension.
  const s1 = session.runChunk(
    "const a = await step('one'); const b = await step('two'); a + '+' + b"
  );
  assert.equal(s1.completed, false);
  assert.deepEqual(s1.args, ["one"]);
  // Host performs effect #1 and resolves.
  world.log.push("did:one");
  world.db.set("one", 1);

  const s2 = ZapcodeSessionHandle.load(s1.session).resume("R1");
  assert.equal(s2.completed, false);
  assert.deepEqual(s2.args, ["two"]);
  // Effect #1 must still be visible after shipping the state across a boundary.
  assert.deepEqual(world.log, ["did:one"]);
  world.log.push("did:two");
  world.db.set("two", 2);

  const done = ZapcodeSessionHandle.load(s2.session).resume("R2");
  assert.equal(done.completed, true);
  assert.equal(done.output, "R1+R2");
  // Both effects landed, in order, across two serialization boundaries.
  assert.deepEqual(world.log, ["did:one", "did:two"]);
  assert.equal(world.db.size, 2);
});

// ── realistic mini-workflow over a host "database" ─────────────────────────

await test("an agent orchestrates a real CRUD workflow; host DB ends correct", async () => {
  const { world, tools } = makeWorld();
  // Seed: two existing rows.
  world.db.set(1, "alpha");
  world.db.set(2, "beta");

  const r = await execute(
    `
    // Insert a new row, update an existing one (read-modify-write), delete one,
    // and report the final size — a realistic agent data-maintenance pass.
    await insert({ id: 3, value: 'gamma' });
    const existing = await get({ id: 1 });
    await insert({ id: 1, value: existing.toUpperCase() });
    await remove({ id: 2 });
    const ids = [];
    for (const id of [1, 2, 3]) {
      const v = await get({ id });
      if (v !== null) ids.push(id + ':' + v);
    }
    ids.join(',')
  `,
    tools
  );

  // Assert the FINAL host-DB state, not just the returned value.
  assert.equal(world.db.size, 2);
  assert.equal(world.db.get(1), "ALPHA");
  assert.equal(world.db.has(2), false);
  assert.equal(world.db.get(3), "gamma");
  assert.equal(r.output, "1:ALPHA,3:gamma");
  // The tool-call trace is the audit log of what actually executed.
  assert.deepEqual(r.toolCalls.map((c) => c.name), [
    "insert",
    "get",
    "insert",
    "remove",
    "get",
    "get",
    "get",
  ]);
});

console.log(`\n${passed} tool side-effect checks passed.`);
