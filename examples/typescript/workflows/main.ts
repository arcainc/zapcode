/**
 * Half-deterministic, half-agentic workflows with @unchartedfr/zapcode-ai.
 *
 * Every demo here is the kind of TypeScript an LLM writes in "code mode": real
 * control flow (loops, conditionals, retries, compensation) with tool calls
 * woven through it. It runs in the Zapcode sandbox — no containers, no network,
 * no filesystem; the only outside effects are the tools you register.
 *
 * This example is OFFLINE — it uses mock tools and asserts the results, so it
 * runs with no API key. The same code is what `zapcode({ tools })` would feed
 * to a model. To see a model actually generate this code, see ../ai-agent.
 *
 * Prerequisites: npm install
 * Run with: npm start
 */

import { execute, createSession, loadSession, dryRun } from "@unchartedfr/zapcode-ai";
import assert from "node:assert/strict";

const tool = (
  description: string,
  parameters: Record<string, { type: "string" | "number" | "boolean" | "object" | "array" }>,
  execute: (args: any) => unknown | Promise<unknown>,
) => ({ description, parameters, execute });

// ---------------------------------------------------------------------------
// 1. Parallel fan-out — Promise.all over tools suspends ONCE; the host runs
//    them concurrently. Deterministic aggregation, real parallelism, one turn.
// ---------------------------------------------------------------------------
async function fanOut() {
  console.log("\n=== 1. Parallel fan-out (map-reduce over tools) ===");
  const docs: Record<string, { severity: string }> = {
    d1: { severity: "high" }, d2: { severity: "low" }, d3: { severity: "high" }, d4: { severity: "low" },
  };
  const tools = {
    searchDocs: tool("Find doc ids for a query.", { query: { type: "string" } }, async () => Object.keys(docs)),
    fetchDoc: tool("Fetch a doc by id.", { id: { type: "string" } }, async ({ id }) => docs[id]),
  };

  const r = await execute(
    `
    const ids  = await searchDocs({ query: "Q3 incidents" });
    const got  = await Promise.all(ids.map(id => fetchDoc({ id })));
    const bySeverity = {};
    for (const d of got) bySeverity[d.severity] = (bySeverity[d.severity] ?? 0) + 1;
    bySeverity;
    `,
    tools,
  );
  console.log("  output:", r.output, `(${r.toolCalls.length} tool calls)`);
  assert.deepEqual(r.output, { high: 2, low: 2 });
}

// ---------------------------------------------------------------------------
// 2. Retry with fallback — a thrown tool rejects the guest's await with a REAL
//    Error (e.message / e.name / instanceof all work), so idiomatic recovery
//    code just works. Console output is captured across the suspensions.
// ---------------------------------------------------------------------------
async function retryFallback() {
  console.log("\n=== 2. Retry with fallback ===");
  let attempts = 0;
  const tools = {
    primaryModel: tool("Flaky primary.", { prompt: { type: "string" } }, async () => {
      attempts++;
      throw new Error("503 unavailable (attempt " + attempts + ")");
    }),
    fallbackModel: tool("Reliable fallback.", { prompt: { type: "string" } }, async () => "fallback summary"),
  };

  const r = await execute(
    `
    async function withRetry(fn, n) {
      let last;
      for (let i = 0; i < n; i++) {
        try { return await fn(); }
        catch (e) { last = e; console.warn("retry " + (i + 1) + ": " + e.message); }
      }
      throw last;
    }
    try { await withRetry(() => primaryModel({ prompt: "summarize" }), 2); }
    catch (e) { await fallbackModel({ prompt: "summarize" }); }
    `,
    tools,
  );
  console.log("  output:", JSON.stringify(r.output));
  console.log("  stderr (retry log):", JSON.stringify(r.stderr));
  assert.equal(r.output, "fallback summary");
  assert.match(r.stderr, /retry 1:.*retry 2:/s);
}

// ---------------------------------------------------------------------------
// 3. Saga / compensation — if a later step fails, unwind the ones that already
//    succeeded. A classic distributed-transaction shape, as a try/catch.
// ---------------------------------------------------------------------------
async function saga() {
  console.log("\n=== 3. Saga / compensation (rollback) ===");
  const events: string[] = [];
  const step = (name: string, fail = false) =>
    tool(name, { orderId: { type: "string" } }, async ({ orderId }) => {
      if (fail) throw new Error(name + " failed");
      events.push(name + ":" + (orderId ?? "new"));
      return true;
    });
  const tools = {
    createOrder: tool("Create order.", { items: { type: "array" } }, async () => { events.push("createOrder"); return { id: "A1" }; }),
    reserveStock: step("reserveStock"),
    chargeCard: step("chargeCard", /* fail */ true),
    releaseStock: step("releaseStock"),
    cancelOrder: step("cancelOrder"),
  };

  const r = await execute(
    `
    const done = [];
    try {
      const order = await createOrder({ items: ["x"] }); done.push(["order", order.id]);
      await reserveStock({ orderId: order.id });          done.push(["stock", order.id]);
      await chargeCard({ orderId: order.id });            done.push(["charge", order.id]);
      "confirmed:" + order.id;
    } catch (e) {
      for (const [s, id] of done.reverse()) {
        if (s === "stock") await releaseStock({ orderId: id });
        if (s === "order") await cancelOrder({ orderId: id });
      }
      "rolled-back: " + e.message;
    }
    `,
    tools,
  );
  console.log("  output:", JSON.stringify(r.output));
  console.log("  host effects:", events.join(" → "));
  assert.equal(r.output, "rolled-back: chargeCard failed");
  assert.deepEqual(events, ["createOrder", "reserveStock:A1", "releaseStock:A1", "cancelOrder:A1"]);
}

// ---------------------------------------------------------------------------
// 4. Durable session — define a workflow now, serialize the whole VM to bytes,
//    resume it later (here: a fresh session, simulating another process).
// ---------------------------------------------------------------------------
async function durableSession() {
  console.log("\n=== 4. Durable session (dump → resume elsewhere) ===");
  const db = new Map<string, { id: string; owner: string }>([["42", { id: "42", owner: "ada" }]]);
  const tools = {
    fetchRow: tool("Load a row.", { id: { type: "string" } }, async ({ id }) => db.get(id) ?? null),
    enrich: tool("Enrich a row.", { row: { type: "object" } }, async ({ row }) => ({ ...row, enriched: true })),
  };

  const session = createSession({ tools, scriptName: "etl-job" });
  await session.runChunk(`
    async function step(id) { return await enrich({ row: await fetchRow({ id }) }); }
  `);
  const bytes = session.dump(); // store this anywhere — a DB, a queue, a Temporal activity
  console.log("  serialized session:", bytes.length, "bytes");

  // ...later, in a different process — only the bytes + the same tools are needed.
  const resumed = loadSession(bytes, { tools });
  const out = await resumed.runChunk(`await step("42")`);
  console.log("  resumed output:", JSON.stringify(out.output));
  assert.deepEqual(out.output, { id: "42", owner: "ada", enriched: true });
}

// ---------------------------------------------------------------------------
// 5. Pre-flight with dryRun — typecheck + run against side-effect-free stubs
//    BEFORE any real tool fires. The cheapest self-correction signal there is.
//    Tools are stubbed with a permissive empty object (field access yields
//    undefined), so a reported error is a real bug — e.g. dereferencing a field
//    chain the code only *assumed* would be there.
// ---------------------------------------------------------------------------
async function preflight() {
  console.log("\n=== 5. dryRun pre-flight ===");
  const tools = {
    getUser: { description: "Fetch a user.", parameters: { id: { type: "number" as const } }, returns: "{ name: string }", execute: async () => ({ name: "Ada" }) },
  };

  // Reaches completion under stubs: calls the tool, doesn't assume its shape.
  const good = await dryRun(`const u = await getUser({ id: 1 }); u ? "fetched" : "none"`, tools);
  console.log("  good code → reachedCompletion:", good.reachedCompletion, "| would call:", good.toolCalls.map(c => c.name));
  assert.equal(good.reachedCompletion, true);

  // Crashes instantly: dereferences `u.profile.name`, but `u.profile` is
  // undefined under the stub — exactly the kind of latent bug to catch pre-run.
  const bad = await dryRun(`const u = await getUser({ id: 1 }); u.profile.name`, tools);
  console.log("  bad code  → reachedCompletion:", bad.reachedCompletion, "| error:", bad.error?.message?.split("\n")[0]);
  assert.equal(bad.reachedCompletion, false);
}

async function main() {
  await fanOut();
  await retryFallback();
  await saga();
  await durableSession();
  await preflight();
  console.log("\nAll workflow demos passed ✓");
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
