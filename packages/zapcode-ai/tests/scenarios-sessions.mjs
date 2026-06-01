// EXPLORATORY stress-pass catalog (not part of the green test:e2e gate; run via `npm run test:scenarios`).
// Checks named BUG/MISSING document gaps found during the realistic-scenario pass; see ../../KNOWN_GAPS.md.
// Some were fixed after this file was written, so those checks now intentionally show as failing-to-flag-fixed.
/**
 * Stress-test: realistic durable-workflow scenarios across serialization
 * boundaries. Probes state fidelity (objects, arrays, Maps, class instances,
 * closures, nested data), resumeMany ordering, per-chunk inputs/stdout, and
 * end-to-end multi-hop pipelines.
 *
 * Run: node tests/scenarios-sessions.mjs
 * Never calls npm run build — dist is assumed current.
 */
import assert from "node:assert/strict";
import { createSession, loadSession } from "../dist/index.js";
import { ZapcodeSessionHandle } from "@unchartedfr/zapcode";

const results = [];
async function check(name, fn) {
  try {
    await fn();
    results.push([name, true]);
    console.log("  ✓ " + name);
  } catch (e) {
    results.push([name, false, e.message]);
    console.log("  ✗ " + name + " — " + e.message);
  }
}

// ─── helpers ─────────────────────────────────────────────────────────────────

/** Simulate a Temporal activity boundary: dump → reload with fresh tool impls. */
function hop(session, tools) {
  return loadSession(session.dump(), { tools });
}

// ─── SCENARIO 1 ──────────────────────────────────────────────────────────────
// Define helpers/config in chunk 1; call them in later chunks AFTER dump/load.
// Proves functions + closures survive serialization.
await check("S1: helpers defined pre-dump are callable post-load", async () => {
  const tools = {
    multiply: {
      description: "Multiply two numbers.",
      parameters: { a: { type: "number" }, b: { type: "number" } },
      execute: async ({ a, b }) => a * b,
    },
  };
  let s = createSession({ tools });

  // Chunk 1: define helper + config constant
  await s.runChunk(`
    const TAX_RATE = 0.08;
    async function priceWithTax(base) {
      const raw = await multiply({ a: base, b: 1 });
      return raw + raw * TAX_RATE;
    }
    "ready"
  `);

  // Activity boundary
  s = hop(s, tools);

  // Chunk 2: use the helper
  const r = await s.runChunk(`await priceWithTax(100)`);
  assert.strictEqual(r.output, 108);
});

// ─── SCENARIO 2 ──────────────────────────────────────────────────────────────
// Stateful accumulation across chunks — running total + collected list.
// NOTE: uses top-level arrays (which work fine) rather than nested arrays
// (which are affected by BUG-1, see below).
await check("S2: running total survives multiple dump/load hops", async () => {
  const tools = {
    getAmount: {
      description: "Return an amount for an invoice.",
      parameters: { invoiceId: { type: "string" } },
      execute: async ({ invoiceId }) =>
        ({ inv_1: 100, inv_2: 250, inv_3: 75 })[invoiceId] ?? 0,
    },
  };
  let s = createSession({ tools });

  // Chunk 1: initialize accumulators (top-level arrays — not nested)
  await s.runChunk(`
    let runningTotal = 0;
    const processedIds = [];
  `);

  // Chunk 2 — hop 1
  s = hop(s, tools);
  await s.runChunk(`
    const a1 = await getAmount({ invoiceId: "inv_1" });
    runningTotal += a1;
    processedIds.push("inv_1");
  `);

  // Chunk 3 — hop 2
  s = hop(s, tools);
  await s.runChunk(`
    const a2 = await getAmount({ invoiceId: "inv_2" });
    runningTotal += a2;
    processedIds.push("inv_2");
  `);

  // Chunk 4 — hop 3
  s = hop(s, tools);
  const r = await s.runChunk(`
    const a3 = await getAmount({ invoiceId: "inv_3" });
    runningTotal += a3;
    processedIds.push("inv_3");
    ({ total: runningTotal, ids: processedIds })
  `);

  assert.strictEqual(r.output.total, 425);
  assert.deepStrictEqual(r.output.ids, ["inv_1", "inv_2", "inv_3"]);
});

// ─── SCENARIO 3 ──────────────────────────────────────────────────────────────
// Class instance with SCALAR fields mutates across chunks + dump/load.
// (Array fields on class instances are affected by BUG-1; this test uses scalars.)
// (Number primitive methods like .toFixed() are affected by BUG-5; uses integer
// prices so that no float formatting is needed.)
await check("S3: class instance scalar fields survive dump/load method calls", async () => {
  const tools = {
    getPrice: {
      description: "Return current price for a SKU.",
      parameters: { sku: { type: "string" } },
      // Integer prices — avoids BUG-5 (number primitive methods unavailable).
      execute: async ({ sku }) =>
        ({ apple: 2, banana: 1, cherry: 3 })[sku] ?? 0,
    },
  };
  let s = createSession({ tools });

  // Chunk 1: define Cart class + create instance (tracks totals via scalars only)
  // summary() uses string concatenation, not toFixed, to avoid BUG-5.
  await s.runChunk(`
    class Cart {
      constructor(owner) {
        this.owner = owner;
        this.itemCount = 0;
        this.total = 0;
      }
      async addItem(sku, qty) {
        const price = await getPrice({ sku });
        this.total += price * qty;
        this.itemCount++;
      }
      summary() {
        return this.owner + ":" + this.itemCount + " items,$" + this.total;
      }
    }
    const cart = new Cart("alice");
    "cart_ready"
  `);

  // Hop then add first item
  s = hop(s, tools);
  await s.runChunk(`await cart.addItem("apple", 3)`);

  // Hop then add second item
  s = hop(s, tools);
  await s.runChunk(`await cart.addItem("cherry", 2)`);

  // Hop then read summary
  s = hop(s, tools);
  const r = await s.runChunk(`cart.summary()`);

  // apple:2*3=6 + cherry:3*2=6 = $12
  assert.strictEqual(r.output, "alice:2 items,$12");
});

// ─── SCENARIO 4 ──────────────────────────────────────────────────────────────
// Parallel fan-out via Promise.all before and after dump/load.
await check("S4: Promise.all fan-out results stay ordered across dump/load", async () => {
  const tools = {
    enrichUser: {
      description: "Enrich a user record.",
      parameters: { userId: { type: "string" } },
      execute: async ({ userId }) => ({
        id: userId,
        score: { u1: 90, u2: 45, u3: 72 }[userId] ?? 0,
        plan: { u1: "pro", u2: "free", u3: "pro" }[userId] ?? "free",
      }),
    },
  };
  let s = createSession({ tools });

  // Chunk 1: parallel enrich
  const r1 = await s.runChunk(`
    const users = await Promise.all([
      enrichUser({ userId: "u1" }),
      enrichUser({ userId: "u2" }),
      enrichUser({ userId: "u3" }),
    ]);
    users.map(u => u.id + ":" + u.score).join(",")
  `);
  assert.strictEqual(r1.output, "u1:90,u2:45,u3:72");

  // Hop then use the results again (they were not persisted — use a new computation)
  s = hop(s, tools);
  const r2 = await s.runChunk(`
    const more = await Promise.all([
      enrichUser({ userId: "u1" }),
      enrichUser({ userId: "u3" }),
    ]);
    more.map(u => u.plan).join(",")
  `);
  assert.strictEqual(r2.output, "pro,pro");
});

// ─── SCENARIO 5 ──────────────────────────────────────────────────────────────
// Pagination loop across chunks: each chunk fetches one page, accumulates.
// NOTE: session is one growing program, so each chunk must use a DISTINCT
// variable name for the page result (known limitation: redeclaring a top-level
// const/let is rejected). Uses top-level array (not nested), which works.
await check("S5: paginated accumulation across chunks with dump/load", async () => {
  const pages = [
    ["item_1", "item_2"],
    ["item_3", "item_4"],
    ["item_5"],
  ];
  const tools = {
    fetchPage: {
      description: "Fetch a page of items.",
      parameters: { page: { type: "number" } },
      execute: async ({ page }) => ({
        items: pages[page] ?? [],
        hasMore: page < pages.length - 1,
      }),
    },
  };
  let s = createSession({ tools });

  // Chunk 1: init accumulator (top-level array)
  await s.runChunk(`
    const allItems = [];
    let pageIdx = 0;
  `);

  // Each "activity" uses a distinct binding name for its page result
  // (session = one growing scope; redeclaring const pageResult would fail)
  s = hop(s, tools);
  await s.runChunk(`
    const pageResult0 = await fetchPage({ page: pageIdx });
    for (const item of pageResult0.items) { allItems.push(item); }
    pageIdx++;
  `);

  s = hop(s, tools);
  await s.runChunk(`
    const pageResult1 = await fetchPage({ page: pageIdx });
    for (const item of pageResult1.items) { allItems.push(item); }
    pageIdx++;
  `);

  s = hop(s, tools);
  const r = await s.runChunk(`
    const pageResult2 = await fetchPage({ page: pageIdx });
    for (const item of pageResult2.items) { allItems.push(item); }
    ({ items: allItems, done: !pageResult2.hasMore })
  `);

  assert.strictEqual(r.output.done, true);
  assert.deepStrictEqual(r.output.items, ["item_1", "item_2", "item_3", "item_4", "item_5"]);
});

// ─── SCENARIO 6 ──────────────────────────────────────────────────────────────
// Realistic end-to-end: ingest → enrich (parallel) → classify → act/escalate.
// 4 chunks, dump/load between each.
await check("S6: ingest→enrich→classify→escalate pipeline across 4 hops", async () => {
  const escalations = [];
  const tools = {
    fetchEvent: {
      description: "Fetch a raw event.",
      parameters: { eventId: { type: "string" } },
      execute: async ({ eventId }) => ({
        id: eventId,
        type: "payment_failed",
        accountId: "acc_42",
        amount: 8500,
      }),
    },
    enrichAccount: {
      description: "Enrich an account.",
      parameters: { accountId: { type: "string" } },
      execute: async ({ accountId }) => ({ accountId, tier: "enterprise", mrr: 12000 }),
    },
    enrichRiskScore: {
      description: "Fetch risk score for an account.",
      parameters: { accountId: { type: "string" } },
      execute: async ({ accountId }) => ({ accountId, score: 78 }),
    },
    escalateEvent: {
      description: "Escalate an event.",
      parameters: {
        eventId: { type: "string" },
        severity: { type: "string" },
        assignee: { type: "string" },
        reason: { type: "string" },
      },
      execute: async (input) => {
        escalations.push(input);
        return { ok: true };
      },
    },
  };
  let s = createSession({ tools });

  // Chunk 1 — define helpers
  await s.runChunk(`
    function classifyEvent(event, account, risk) {
      if (account.tier === "enterprise" && event.amount > 5000 && risk.score > 70) {
        return { severity: "critical", assignee: "vip_support" };
      }
      return { severity: "normal", assignee: "general_queue" };
    }
    "helpers_ready"
  `);

  // Chunk 2 — ingest
  s = hop(s, tools);
  await s.runChunk(`
    const rawEvent = await fetchEvent({ eventId: "evt_101" });
  `);

  // Chunk 3 — parallel enrich
  s = hop(s, tools);
  await s.runChunk(`
    const [enrichedAccount, riskData] = await Promise.all([
      enrichAccount({ accountId: rawEvent.accountId }),
      enrichRiskScore({ accountId: rawEvent.accountId }),
    ]);
    const classification = classifyEvent(rawEvent, enrichedAccount, riskData);
  `);

  // Chunk 4 — act
  s = hop(s, tools);
  const r = await s.runChunk(`
    await escalateEvent({
      eventId: rawEvent.id,
      severity: classification.severity,
      assignee: classification.assignee,
      reason: "auto-classified by pipeline",
    });
    classification.severity
  `);

  assert.strictEqual(r.output, "critical");
  assert.strictEqual(escalations.length, 1);
  assert.strictEqual(escalations[0].assignee, "vip_support");
  assert.strictEqual(escalations[0].severity, "critical");
});

// ─── SCENARIO 7 ──────────────────────────────────────────────────────────────
// Raw binding: single suspension → resume(value) after dump/load.
await check("S7 (raw): single suspension → dump → load → resume(value)", () => {
  const s = ZapcodeSessionHandle.create({ externalFunctions: ["lookupConfig"] });
  const suspended = s.runChunk(`
    const cfg = await lookupConfig("db_url");
    "got:" + cfg
  `);
  assert.strictEqual(suspended.completed, false);
  assert.strictEqual(suspended.kind, "suspended");
  assert.strictEqual(suspended.functionName, "lookupConfig");
  assert.deepStrictEqual(suspended.args, ["db_url"]);

  const done = ZapcodeSessionHandle.load(suspended.session).resume("postgres://localhost/db");
  assert.strictEqual(done.completed, true);
  assert.strictEqual(done.output, "got:postgres://localhost/db");
});

// ─── SCENARIO 8 ──────────────────────────────────────────────────────────────
// Raw binding: Promise.all → kind:"suspended_many" → dump → load → resumeMany.
await check("S8 (raw): suspended_many → dump → load → resumeMany ordering", () => {
  const s = ZapcodeSessionHandle.create({ externalFunctions: ["fetch"] });
  const suspended = s.runChunk(`
    const [a, b, c] = await Promise.all([fetch("x"), fetch("y"), fetch("z")]);
    a + "|" + b + "|" + c
  `);
  assert.strictEqual(suspended.kind, "suspended_many");
  assert.strictEqual(suspended.calls.length, 3);
  assert.deepStrictEqual(suspended.calls.map(c => c.args[0]), ["x", "y", "z"]);

  // Dump the suspended session, reload in a "new process", resume with results
  const bytes = suspended.session;
  const done = ZapcodeSessionHandle.load(bytes).resumeMany(["R_x", "R_y", "R_z"]);
  assert.strictEqual(done.completed, true);
  assert.strictEqual(done.output, "R_x|R_y|R_z");
});

// ─── SCENARIO 9 ──────────────────────────────────────────────────────────────
// Raw binding: tool failure path → resumeError → caught by guest try/catch.
await check("S9 (raw): resumeError caught by guest try/catch after dump/load", () => {
  const s = ZapcodeSessionHandle.create({ externalFunctions: ["callApi"] });
  const suspended = s.runChunk(`
    let result;
    try {
      const v = await callApi("endpoint_1");
      result = "success:" + v;
    } catch (e) {
      result = "error:" + e;
    }
    result
  `);
  assert.strictEqual(suspended.completed, false);

  // Simulate the API failing — feed back an error after a boundary hop
  const done = ZapcodeSessionHandle.load(suspended.session).resumeError("502 Bad Gateway");
  assert.strictEqual(done.completed, true);
  assert.strictEqual(done.output, "error:502 Bad Gateway");
});

// ─── SCENARIO 10 ─────────────────────────────────────────────────────────────
// Per-chunk inputs are visible in the chunk; stdout is per-chunk (step-local).
await check("S10: per-chunk inputs are visible; stdout is step-local", async () => {
  const tools = {};
  let s = createSession({ tools });

  // Chunk 1 has no inputs
  const r1 = await s.runChunk(`
    console.log("chunk1");
    const greeting = "hello";
  `);
  assert.strictEqual(r1.stdout, "chunk1\n");

  // Chunk 2 receives per-chunk inputs
  const r2 = await s.runChunk(`
    console.log("chunk2", userId);
    userId + "_processed"
  `, { userId: "user_42" });
  assert.strictEqual(r2.stdout, "chunk2 user_42\n");
  assert.strictEqual(r2.output, "user_42_processed");

  // stdout is per-chunk — chunk 1's "chunk1" must not appear in chunk 2's stdout
  assert.strictEqual(r2.stdout.includes("chunk1"), false);

  // Persist and reload; inject different inputs on the next chunk
  s = hop(s, tools);
  const r3 = await s.runChunk(`
    console.log("chunk3", userId);
    greeting + " " + userId
  `, { userId: "user_99" });
  assert.strictEqual(r3.stdout, "chunk3 user_99\n");
  assert.strictEqual(r3.output, "hello user_99");
});

// ─── SCENARIO 11 ─────────────────────────────────────────────────────────────
// Map and deeply nested object fidelity across dump/load.
// Note: Set is unavailable in the sandbox (typeof Set === "undefined").
// Note: Map.size always returns null (BUG-3 below); use Map.get/has instead.
// Note: nested-array mutation via push corrupts the parent object (BUG-1 below);
//       use spread reassignment instead.
await check("S11: Map and deeply nested objects survive dump/load", async () => {
  const tools = {
    getTag: {
      description: "Get a tag for a record.",
      parameters: { id: { type: "string" } },
      execute: async ({ id }) => "tag_" + id,
    },
  };
  let s = createSession({ tools });

  // Chunk 1: build Map + deeply nested plain object
  await s.runChunk(`
    const registry = new Map();
    const meta = { created: 1748736000000, nested: { deep: { value: 42 } } };
  `);

  s = hop(s, tools);

  // Chunk 2: populate the Map with tool-fetched values
  await s.runChunk(`
    const tag1 = await getTag({ id: "1" });
    const tag2 = await getTag({ id: "2" });
    registry.set("rec_1", { name: "alpha", tag: tag1 });
    registry.set("rec_2", { name: "beta",  tag: tag2 });
  `);

  s = hop(s, tools);

  // Chunk 3: read back via Map.get/has (not .size — that's broken)
  const r = await s.runChunk(`
    const r1 = registry.get("rec_1");
    const r2 = registry.get("rec_2");
    ({
      r1Name: r1.name,
      r1Tag:  r1.tag,
      r2Tag:  r2.tag,
      hasRec1: registry.has("rec_1"),
      deepVal: meta.nested.deep.value,
    })
  `);

  assert.strictEqual(r.output.r1Name, "alpha");
  assert.strictEqual(r.output.r1Tag, "tag_1");
  assert.strictEqual(r.output.r2Tag, "tag_2");
  assert.strictEqual(r.output.hasRec1, true);
  assert.strictEqual(r.output.deepVal, 42);
});

// ─── SCENARIO 12 ─────────────────────────────────────────────────────────────
// Closure over mutable state — capture before suspend, verify after resume.
await check("S12: closure captures survive across suspend/resume boundaries", async () => {
  const tools = {
    getMultiplier: {
      description: "Return a multiplier.",
      parameters: {},
      execute: async () => 7,
    },
  };
  let s = createSession({ tools });

  // Chunk 1: create closure that captures a value fetched via tool
  await s.runChunk(`
    const multiplier = await getMultiplier();
    const scaleBy = (n) => n * multiplier;
  `);

  s = hop(s, tools);

  // Chunk 2: invoke the closure
  const r = await s.runChunk(`[scaleBy(3), scaleBy(10), scaleBy(0)]`);
  assert.deepStrictEqual(r.output, [21, 70, 0]);
});

// ─── SCENARIO 13 ─────────────────────────────────────────────────────────────
// Reloaded session produces same cumulative output as a session that was
// never dumped.
await check("S13: reloaded session is behaviorally identical to never-dumped", async () => {
  function makeTools() {
    let callCount = 0;
    return {
      counter: {
        description: "Increment and return a counter.",
        parameters: {},
        execute: async () => ++callCount,
      },
    };
  }

  // Path A: no dump/load
  const toolsA = makeTools();
  const sA = createSession({ tools: toolsA });
  await sA.runChunk(`let totA = 0; const v1 = await counter(); totA += v1;`);
  const rA = await sA.runChunk(`const v2 = await counter(); totA += v2; totA`);

  // Path B: dump/load between chunks
  const toolsB = makeTools();
  let sB = createSession({ tools: toolsB });
  await sB.runChunk(`let totB = 0; const v3 = await counter(); totB += v3;`);
  sB = loadSession(sB.dump(), { tools: toolsB });
  const rB = await sB.runChunk(`const v4 = await counter(); totB += v4; totB`);

  // Both should be 1+2=3
  assert.strictEqual(rA.output, 3);
  assert.strictEqual(rB.output, 3);
});

// ─── SCENARIO 14 ─────────────────────────────────────────────────────────────
// resumeMany result order matches call-declaration order, not resolution order.
await check("S14 (raw): resumeMany result order matches call declaration order", () => {
  const s = ZapcodeSessionHandle.create({ externalFunctions: ["slow", "fast"] });
  const suspended = s.runChunk(`
    const results = await Promise.all([slow("first"), fast("second"), slow("third")]);
    results.join(",")
  `);
  assert.strictEqual(suspended.kind, "suspended_many");
  assert.strictEqual(suspended.calls.length, 3);

  const done = ZapcodeSessionHandle.load(suspended.session).resumeMany([
    "first_result",
    "second_result",
    "third_result",
  ]);
  assert.strictEqual(done.completed, true);
  assert.strictEqual(done.output, "first_result,second_result,third_result");
});

// ─── SCENARIO 15 ─────────────────────────────────────────────────────────────
// Multi-wave parallel → sequential → parallel with hops between each wave.
await check("S15: multi-wave parallel calls with hops between each wave", async () => {
  const tools = {
    resolve: {
      description: "Resolve a key.",
      parameters: { key: { type: "string" } },
      execute: async ({ key }) => key.toUpperCase(),
    },
  };
  let s = createSession({ tools });

  // Chunk 1 — wave 1
  await s.runChunk(`
    const wave1 = await Promise.all([
      resolve({ key: "a" }),
      resolve({ key: "b" }),
    ]);
  `);

  s = hop(s, tools);

  // Chunk 2 — sequential between waves
  await s.runChunk(`
    const mid = await resolve({ key: "mid" });
  `);

  s = hop(s, tools);

  // Chunk 3 — wave 2
  const r = await s.runChunk(`
    const wave2 = await Promise.all([
      resolve({ key: "c" }),
      resolve({ key: "d" }),
    ]);
    [...wave1, mid, ...wave2].join(",")
  `);

  assert.strictEqual(r.output, "A,B,MID,C,D");
});

// ─── SCENARIO 16 ─────────────────────────────────────────────────────────────
// Raw session handle — completed chunk session blob is loadable and extendable.
await check("S16 (raw): completed chunk session blob is loadable and extendable", () => {
  const s = ZapcodeSessionHandle.create({ externalFunctions: ["add"] });

  const r1 = s.runChunk(`
    function square(n) { return n * n; }
    const base = 5;
    base
  `);
  assert.strictEqual(r1.completed, true);
  assert.strictEqual(r1.output, 5);

  // Load from the completed session blob and run another chunk
  const s2 = ZapcodeSessionHandle.load(r1.session);
  const suspended = s2.runChunk(`await add(square(base), 1)`);
  assert.strictEqual(suspended.completed, false);
  assert.strictEqual(suspended.functionName, "add");
  // args: square(5)=25 and 1
  assert.deepStrictEqual(suspended.args, [25, 1]);

  const done = ZapcodeSessionHandle.load(suspended.session).resume(26);
  assert.strictEqual(done.completed, true);
  assert.strictEqual(done.output, 26);
});

// ─── SCENARIO 17 ─────────────────────────────────────────────────────────────
// Math.random() determinism across dump/load.
// NOTE: the test stores Math.random() results in top-level scalars first, then
// compares. Direct inline Math.random() as an argument to array.push() is
// affected by BUG-2 below, so we avoid that pattern here.
await check("S17: Math.random() scalar sequence is deterministic across dump/load", async () => {
  const tools = {};

  // Path A: no dump/load, sample 4 values via scalars
  const sA = createSession({ tools });
  await sA.runChunk(`
    const rA1 = Math.random();
    const rA2 = Math.random();
  `);
  const rA = await sA.runChunk(`
    const rA3 = Math.random();
    const rA4 = Math.random();
    [rA1, rA2, rA3, rA4]
  `);

  // Path B: dump/load between chunks
  let sB = createSession({ tools });
  await sB.runChunk(`
    const rB1 = Math.random();
    const rB2 = Math.random();
  `);
  sB = loadSession(sB.dump(), { tools });
  const rB = await sB.runChunk(`
    const rB3 = Math.random();
    const rB4 = Math.random();
    [rB1, rB2, rB3, rB4]
  `);

  // Both sequences must be identical (deterministic RNG state carried in dump)
  assert.deepStrictEqual(rA.output, rB.output,
    "Math.random() sequence diverged after dump/load — RNG state not serialized");
});

// ─── SCENARIO 18 ─────────────────────────────────────────────────────────────
// Error in one arm of Promise.all does NOT corrupt the session;
// a catch-and-recover followed by a dump/load still produces a valid state.
await check("S18: session is still usable after catching a Promise.all error", async () => {
  const tools = {
    okTool: {
      description: "Returns ok.",
      parameters: { v: { type: "string" } },
      execute: async ({ v }) => "ok:" + v,
    },
    failTool: {
      description: "Always fails.",
      parameters: {},
      // No params → call as failTool() not failTool({})
      execute: async () => { throw new Error("intentional failure"); },
    },
  };
  let s = createSession({ tools });

  // Chunk 1: catch the failure
  const r1 = await s.runChunk(`
    let recovered;
    try {
      await Promise.all([okTool({ v: "x" }), failTool()]);
      recovered = "no-error";
    } catch (e) {
      recovered = "caught";
    }
    recovered
  `);
  assert.match(r1.output, /caught/);

  // Dump/load after the failure path; session must still work
  s = hop(s, tools);

  const r2 = await s.runChunk(`
    const v = await okTool({ v: "after_failure" });
    v
  `);
  assert.strictEqual(r2.output, "ok:after_failure");
});

// ─── BUG PROBES ──────────────────────────────────────────────────────────────

// BUG-1: Mutating a NESTED array property (e.g. obj.items.push(x)) via in-place
// methods corrupts the parent object in every subsequent chunk. The push() call
// returns the correct new length, but then obj itself becomes the array: obj.items
// and all other properties of obj become null, and JSON.stringify(obj) returns
// the mutated array, not the original object.
// Affects: class instance array fields (this.items.push), plain object array
// properties, any in-place mutation of a nested array.
// Workaround: use spread reassignment — obj.items = [...obj.items, x].
// Does NOT affect top-level arrays declared directly at module scope.
await check("BUG-1: nested array push corrupts parent object (confirmed bug)", async () => {
  const s = createSession({ tools: {} });

  const r = await s.runChunk(`
    const obj = { items: [], count: 3 };
    obj.items.push("x");
    // After push: what is obj.count?
    obj.count
  `);
  // In real JS, obj.count is still 3. In the sandbox, it becomes null due to
  // the mutation corrupting the parent object binding.
  assert.notStrictEqual(r.output, 3,
    "BUG-1 regression: nested array push no longer corrupts parent object");
});

// BUG-2: Calling any Array method when a Math.X() call appears as a direct
// argument produces "__array__.<method> is not a function", even though
// Math.X() works fine as a standalone expression.
// Minimal repro: `[].push(Math.random())` — same-chunk, no tool calls needed.
// Workaround: store Math result in a variable first, then pass the variable.
await check("BUG-2: array.push(Math.random()) fails with method-not-found (confirmed bug)", async () => {
  const s = createSession({ tools: {} });

  // Confirm the workaround works (proves Math itself is fine):
  const rOk = await s.runChunk(`
    const v = Math.random();
    const arr = [];
    arr.push(v);
    arr.length
  `);
  assert.strictEqual(rOk.output, 1);

  // Direct call as arg fails:
  let caught = null;
  try {
    await s.runChunk(`
      const arr2 = [];
      arr2.push(Math.random());
      arr2.length
    `);
  } catch (e) {
    caught = e.message;
  }
  assert.ok(caught !== null,
    "BUG-2 regression: arr.push(Math.random()) now works — remove this check");
  assert.match(caught, /__array__.*push.*function|push.*not.*function/i);
});

// BUG-3: Map.size always returns null regardless of how many entries were added.
// Map.get() and Map.has() work correctly. Map is not iterable (for...of fails).
// Set is entirely unavailable: typeof Set === "undefined".
await check("BUG-3: Map.size returns null instead of entry count (confirmed bug)", async () => {
  const s = createSession({ tools: {} });

  const r = await s.runChunk(`
    const m = new Map();
    m.set("a", 1);
    m.set("b", 2);
    ({
      size:   m.size,
      hasA:   m.has("a"),
      getB:   m.get("b"),
    })
  `);

  // Map.get and Map.has work:
  assert.strictEqual(r.output.hasA, true);
  assert.strictEqual(r.output.getB, 2);

  // Map.size is null (bug):
  assert.strictEqual(r.output.size, null,
    "BUG-3 regression: Map.size now returns correct count — remove this check");
});

// BUG-4: Set is completely unavailable — typeof Set === "undefined" and
// new Set() throws "undefined is not a constructor".
await check("BUG-4: Set constructor is unavailable (typeof Set === undefined)", async () => {
  const s = createSession({ tools: {} });

  const r = await s.runChunk(`typeof Set`);
  assert.strictEqual(r.output, "undefined",
    "BUG-4 regression: Set is now available — update S11 to use Set");

  let threw = false;
  try {
    await s.runChunk(`new Set()`);
  } catch (e) {
    threw = true;
  }
  assert.ok(threw, "BUG-4 regression: new Set() no longer throws");
});

// BUG-5: Number primitive methods (toFixed, toString, valueOf, toPrecision) are
// completely unavailable — typeof (9.6).toFixed === "undefined".
// String methods work fine. Only number primitives are affected.
// Workaround: use string concatenation ("" + n) or Math.round for formatting.
// Root cause appears related to BUG-2: the interpreter does not box numeric
// primitives when performing method dispatch.
await check("BUG-5: number primitive methods (toFixed, toString) are unavailable", async () => {
  const s = createSession({ tools: {} });

  const r = await s.runChunk(`
    const n = 9.6;
    ({
      toFixed_type:    typeof n.toFixed,
      toString_type:   typeof n.toString,
      toPrecision_type: typeof n.toPrecision,
      // string concatenation workaround works:
      via_concat:      "" + n,
    })
  `);

  // Methods are absent (bug):
  assert.strictEqual(r.output.toFixed_type, "undefined",
    "BUG-5 regression: n.toFixed is now available — remove this check");
  assert.strictEqual(r.output.toString_type, "undefined",
    "BUG-5 regression: n.toString is now available — remove this check");

  // Workaround works:
  assert.strictEqual(r.output.via_concat, "9.6");
});

// ─── results ──────────────────────────────────────────────────────────────────
const failed = results.filter(r => !r[1]);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
if (failed.length) {
  console.log("\nFailed:");
  for (const [name, , msg] of failed) console.log(`  ✗ ${name}: ${msg}`);
  process.exit(1);
}