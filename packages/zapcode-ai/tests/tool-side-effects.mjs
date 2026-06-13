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
  // A thrown tool now surfaces in-guest as a real Error (e.message works).
  const r = await execute(
    "await emit('before'); let caught = ''; \
     try { await boom(); await emit('unreached'); } catch (e) { caught = e.message; } \
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

// ── tool-error fidelity: a thrown tool is a real Error in the guest ────────

await test("a thrown tool is caught as a real Error (instanceof, name, message)", async () => {
  const tools = {
    fail: {
      description: "throws a TypeError",
      parameters: {},
      execute: async () => {
        throw new TypeError("nope");
      },
    },
  };
  const r = await execute(
    "let info = ''; try { await fail(); } catch (e) { \
       info = [e instanceof Error, e.name, e.message].join('|'); } info",
    tools
  );
  // The host's TypeError subclass name is preserved.
  assert.equal(r.output, "true|TypeError|nope");
});

await test("a caught tool error can be rethrown and re-caught with fidelity", async () => {
  const tools = {
    fail: {
      description: "throws",
      parameters: {},
      execute: async () => {
        throw new Error("original");
      },
    },
  };
  const r = await execute(
    "function wrap(e) { return new Error('wrapped: ' + e.message); } \
     let out = ''; \
     try { try { await fail(); } catch (e) { throw wrap(e); } } \
     catch (e2) { out = e2.message + '|' + (e2 instanceof Error); } out",
    tools
  );
  assert.equal(r.output, "wrapped: original|true");
});

await test("an uncaught tool error fails the run with the message (autoFix report)", async () => {
  const tools = {
    fail: {
      description: "throws",
      parameters: {},
      execute: async () => {
        throw new Error("boom-uncaught");
      },
    },
  };
  const r = await execute("await fail(); 'unreached'", tools, { autoFix: true });
  assert.equal(r.report.completed, false);
  assert.match(r.report.error.message, /boom-uncaught/);
});

// ── saga / compensation: a failing step triggers a rollback effect ─────────

await test("saga: a failed step triggers a compensating side effect", async () => {
  const { world, tools } = makeWorld();
  tools.charge = {
    description: "charge a payment — fails for amount > 100",
    parameters: { amount: { type: "number" } },
    execute: async ({ amount }) => {
      if (amount > 100) throw new Error("declined");
      world.log.push(`charge:${amount}`);
      return { ok: true };
    },
  };
  // The agent inserts a row, tries to charge, and on failure compensates by
  // removing the row — a realistic transactional pattern over host state.
  const r = await execute(
    "await insert({ id: 1, value: 'order' }); \
     let status; \
     try { await charge({ amount: 250 }); status = 'charged'; } \
     catch (e) { await remove({ id: 1 }); status = 'rolled-back:' + e.message; } \
     status",
    tools
  );
  assert.equal(r.output, "rolled-back:declined");
  // The insert happened, then was compensated; the DB is empty again.
  assert.equal(world.db.size, 0);
  assert.deepEqual(world.log, ["insert:1=order", "remove:1"]);
});

// ── nested tool calls inside a helper function ─────────────────────────────

await test("tool calls inside a helper function still drive host effects", async () => {
  const { world, tools } = makeWorld();
  const r = await execute(
    "async function store(id) { await insert({ id, value: 'n' + id }); return id; } \
     const ids = []; \
     for (const id of [1, 2, 3]) ids.push(await store(id)); \
     ids.join(',')",
    tools
  );
  assert.equal(r.output, "1,2,3");
  assert.equal(world.db.size, 3);
  assert.deepEqual(world.log, ["insert:1=n1", "insert:2=n2", "insert:3=n3"]);
});

// ── complex tool result drives a derived side effect ───────────────────────

await test("a structured tool result drives a derived side effect", async () => {
  const world = { log: [], db: new Map() };
  const tools = {
    fetchOrder: {
      description: "fetch an order with line items",
      parameters: { id: { type: "number" } },
      returns: "{ id: number; items: { sku: string; qty: number }[] }",
      execute: async ({ id }) => ({
        id,
        items: [
          { sku: "A", qty: 2 },
          { sku: "B", qty: 5 },
        ],
      }),
    },
    record: {
      description: "record a computed total",
      parameters: { total: { type: "number" } },
      execute: async ({ total }) => {
        world.log.push(`total:${total}`);
        world.db.set("total", total);
        return null;
      },
    },
  };
  const r = await execute(
    "const order = await fetchOrder({ id: 7 }); \
     const total = order.items.reduce((s, it) => s + it.qty, 0); \
     await record({ total }); total",
    tools
  );
  assert.equal(r.output, 7);
  assert.equal(world.db.get("total"), 7);
  assert.deepEqual(world.log, ["total:7"]);
});

// ── batch with one failing call: allSettled keeps the others' effects ──────

await test("Promise.allSettled over tools keeps successful effects when one fails", async () => {
  const { world, tools } = makeWorld();
  tools.maybe = {
    description: "succeeds for even ids, throws for odd",
    parameters: { id: { type: "number" } },
    execute: async ({ id }) => {
      if (id % 2 === 1) throw new Error("odd:" + id);
      world.log.push(`ok:${id}`);
      return id;
    },
  };
  // A BATCH (Promise.all/allSettled) rejection reason is a real Error in the
  // guest — `instanceof Error` holds and `.message` reads the message — exactly
  // like a direct `await tool()` rejection. (allSettled reasons travel through
  // resumeMany as `__error__`-branded objects so the VM treats them as Errors.)
  const r = await execute(
    "const rs = await Promise.allSettled([maybe({ id: 2 }), maybe({ id: 3 }), maybe({ id: 4 })]); \
     rs.map(x => x.status === 'fulfilled' ? 'f' + x.value : 'r:' + (x.reason instanceof Error) + ':' + x.reason.message).join(',')",
    tools
  );
  // The two even calls performed their effect; the odd one rejected with a real Error.
  assert.equal(r.output, "f2,r:true:odd:3,f4");
  assert.deepEqual(world.log.filter((l) => l.startsWith("ok:")).sort(), ["ok:2", "ok:4"]);
});

console.log(`\n${passed} tool side-effect checks passed.`);
