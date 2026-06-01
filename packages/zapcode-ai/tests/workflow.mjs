/**
 * e2e: a realistic agent-authored workflow that runs across multiple session
 * chunks, serializing the whole VM state between each (as a Temporal workflow
 * would across activity boundaries). The agent defines helpers in one chunk,
 * then drives a multi-step triage — parallel enrichment, classification,
 * error recovery, escalation — in later chunks, re-loading the session each hop.
 */
import assert from "node:assert/strict";
import { createSession, loadSession } from "../dist/index.js";
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

console.log("workflow e2e");

const NOW = Date.UTC(2026, 5, 1);

function triageTools(state) {
  const accounts = {
    acc_1: { tier: "enterprise", openTickets: 4 },
    acc_2: { tier: "free", openTickets: 1 },
    acc_3: { tier: "enterprise", openTickets: 9 },
  };
  return {
    getNowMs: { description: "now", parameters: {}, execute: async () => NOW },
    fetchTicket: {
      description: "Fetch a ticket by id.",
      parameters: { id: { type: "string" } },
      execute: async ({ id }) => {
        const map = {
          t1: { id: "t1", accountId: "acc_1", severity: 3 },
          t2: { id: "t2", accountId: "acc_2", severity: 1 },
          t3: { id: "t3", accountId: "acc_3", severity: 5 },
        };
        return map[id];
      },
    },
    enrichAccount: {
      description: "Look up account details.",
      parameters: { accountId: { type: "string" } },
      execute: async ({ accountId }) => {
        if (!accounts[accountId]) throw new Error(`unknown account ${accountId}`);
        return accounts[accountId];
      },
    },
    escalate: {
      description: "Escalate a ticket to a queue.",
      parameters: {
        ticketId: { type: "string" },
        queue: { type: "string" },
        reason: { type: "string" },
        priority: { type: "number" },
      },
      execute: async (input) => {
        state.escalations.push(input);
        return { ok: true };
      },
    },
  };
}

// Simulate persisting the session across an activity boundary: dump bytes,
// then resume from those bytes with fresh tool implementations.
function hop(session, state) {
  const bytes = session.dump();
  return loadSession(bytes, { tools: triageTools(state) });
}

await test("multi-step triage workflow across serialized session chunks", async () => {
  const state = { escalations: [] };
  let session = createSession({ tools: triageTools(state) });

  // Chunk 1 — the agent defines the workflow logic (functions + config).
  const setup = await session.runChunk(`
    const URGENT_SEVERITY = 4;
    async function loadTicket(id) {
      const ticket = await fetchTicket(id);
      const account = await enrichAccount(ticket.accountId);
      return { ...ticket, tier: account.tier, load: account.openTickets };
    }
    function queueFor(t) {
      if (t.severity >= URGENT_SEVERITY) return "urgent";
      return t.tier === "enterprise" ? "priority" : "standard";
    }
    "ready"
  `);
  assert.equal(setup.output, "ready");

  // Persist + reload (activity boundary 1).
  session = hop(session, state);

  // Chunk 2 — fan out ticket loading in parallel, then classify.
  const classified = await session.runChunk(`
    const tickets = await Promise.all([loadTicket("t1"), loadTicket("t2"), loadTicket("t3")]);
    const withQueue = tickets.map(t => ({ ...t, queue: queueFor(t) }));
    withQueue.map(t => t.id + ":" + t.queue).join(",")
  `);
  assert.equal(classified.output, "t1:priority,t2:standard,t3:urgent");

  // Persist + reload (activity boundary 2).
  session = hop(session, state);

  // Chunk 3 — escalate the urgent/priority tickets.
  // (Distinct binding names: a session is one growing program, so top-level
  // const/let persist across chunks and can't be redeclared. Tool calls over a
  // dynamic list go through a for...of loop; the parallel form is
  // Promise.all([...]) with calls written directly as elements.)
  const escalated = await session.runChunk(`
    const toEscalate = await Promise.all([loadTicket("t1"), loadTicket("t3")]);
    let count = 0;
    for (const t of toEscalate) {
      await escalate({ ticketId: t.id, queue: queueFor(t), reason: "auto", priority: t.severity });
      count++;
    }
    count
  `);
  assert.equal(escalated.output, 2);

  // The host observed both escalations, with the workflow's computed queue/priority.
  assert.equal(state.escalations.length, 2);
  const byId = Object.fromEntries(state.escalations.map(e => [e.ticketId, e]));
  assert.equal(byId.t1.queue, "priority");
  assert.equal(byId.t3.queue, "urgent");
  assert.equal(byId.t3.priority, 5);
});

await test("workflow recovers from a failed enrichment mid-flight", async () => {
  const state = { escalations: [] };
  let session = createSession({ tools: triageTools(state) });

  await session.runChunk(`
    async function safeLoad(id) {
      const ticket = await fetchTicket(id);
      try {
        const account = await enrichAccount(ticket.accountId);
        return { id: ticket.id, tier: account.tier };
      } catch (e) {
        return { id: ticket.id, tier: "unknown", error: String(e) };
      }
    }
    "ready"
  `);
  session = hop(session, state);

  // t_bad references an account that doesn't exist → enrichment throws → caught.
  const out = await session.runChunk(`
    const t = await fetchTicket("t1");
    const bad = { id: "t_bad", accountId: "acc_missing", severity: 2 };
    const a = await safeLoad("t1");
    let b;
    try {
      await enrichAccount(bad.accountId);
      b = "ok";
    } catch (e) {
      b = "fallback";
    }
    a.tier + "/" + b
  `);
  assert.equal(out.output, "enterprise/fallback");
});

await test("raw session handle: dump/load/resume batch across a boundary", () => {
  // Lower-level mirror of the above, asserting the wire-level contract.
  const session = ZapcodeSessionHandle.create({ externalFunctions: ["load"] });
  const suspended = session.runChunk(`await Promise.all([load("a"), load("b")])`);
  assert.equal(suspended.kind, "suspended_many");
  const bytes = suspended.session;
  const reloaded = ZapcodeSessionHandle.load(bytes);
  const done = reloaded.resumeMany(["A", "B"]);
  assert.equal(done.completed, true);
  assert.deepEqual(done.output, ["A", "B"]);
});

console.log(`\n${passed} workflow checks passed.`);
