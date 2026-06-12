/**
 * e2e: realistic durable agent workflows across chunks and dump/load
 * boundaries. These scenarios exercise the zapcode-ai session bridge, so tool
 * results/errors are resumed through the wrapper's validation and resume loop.
 */
import assert from "node:assert/strict";
import { createSession, loadSession } from "../dist/index.js";

let passed = 0;
async function test(name, fn) {
  try {
    await fn();
    passed++;
    console.log(`  PASS ${name}`);
  } catch (err) {
    console.error(`  FAIL ${name}`);
    throw err;
  }
}

function reload(session, tools) {
  return loadSession(session.dump(), { tools });
}

function createSupportTools(events) {
  return {
    lookupCustomer: {
      description: "Load CRM customer facts.",
      parameters: { customerId: { type: "string" } },
      execute: async ({ customerId }) => {
        events.push({ type: "lookupCustomer", customerId });
        const customers = {
          "cust-ent": { id: customerId, tier: "enterprise", owner: "guy Y" },
          "cust-free": { id: customerId, tier: "free", owner: "guy X" },
        };
        if (!customers[customerId]) throw new Error(`customer ${customerId} not found`);
        return customers[customerId];
      },
    },
    searchKb: {
      description: "Search support knowledge base.",
      parameters: {
        query: { type: "string" },
        limit: { type: "number", optional: true },
      },
      execute: async ({ query, limit = 2 }) => {
        events.push({ type: "searchKb", query, limit });
        return {
          hits: Array.from({ length: limit }, (_, i) => ({
            id: `kb-${i + 1}`,
            title: `${query} runbook ${i + 1}`,
          })),
        };
      },
    },
    riskScore: {
      description: "Get automated risk score.",
      parameters: { customerId: { type: "string" } },
      execute: async ({ customerId }) => {
        events.push({ type: "riskScore", customerId });
        if (customerId === "cust-free") throw new Error("risk service timeout");
        return { level: "high", confidence: 0.92 };
      },
    },
    openCase: {
      description: "Open a support case.",
      parameters: {
        customerId: { type: "string" },
        severity: { type: "string" },
        summary: { type: "string" },
        metadata: { type: "object", optional: true },
      },
      execute: async input => {
        const id = `case-${events.filter(e => e.type === "openCase").length + 1}`;
        events.push({ type: "openCase", id, input });
        return { id, created: true };
      },
    },
    notifyOwner: {
      description: "Notify the customer owner.",
      parameters: {
        owner: { type: "string" },
        caseId: { type: "string" },
        message: { type: "string" },
      },
      execute: async input => {
        events.push({ type: "notifyOwner", input });
        return { delivered: true };
      },
    },
  };
}

console.log("scenarios3 durable e2e");

await test("stateful support triage survives reloads across definition, calls, and summary", async () => {
  const events = [];
  const tools = createSupportTools(events);
  let session = createSession({ tools });

  const defined = await session.runChunk(`
    const workflowState = {
      processed: [],
      ownerByTier: { enterprise: "guy Y", free: "guy X" },
    };

    async function triageCustomer(customerId, issueTitle) {
      const [customer, kb] = await Promise.all([
        lookupCustomer(customerId),
        searchKb({ query: issueTitle, limit: 2 }),
      ]);

      let score;
      try {
        score = await riskScore(customerId);
      } catch (e) {
        score = { level: "unknown", confidence: 0, note: String(e) };
      }

      const severity = customer.tier === "enterprise" || score.level === "high" ? "urgent" : "normal";
      const opened = await openCase({
        customerId,
        severity,
        summary: issueTitle + " / " + kb.hits[0].title,
        metadata: { scoreLevel: score.level, docCount: kb.hits.length },
      });
      const owner = workflowState.ownerByTier[customer.tier] || customer.owner;
      await notifyOwner({ owner, caseId: opened.id, message: severity + " support case" });
      workflowState.processed.push({ customerId, caseId: opened.id, severity, scoreLevel: score.level });
      return workflowState.processed.at(-1);
    }

    workflowState.processed.length
  `);
  assert.equal(defined.output, 0);
  assert.deepEqual(defined.toolCalls, []);

  session = reload(session, tools);
  const enterprise = await session.runChunk(`await triageCustomer("cust-ent", "SSO outage")`);
  assert.deepEqual(enterprise.output, {
    customerId: "cust-ent",
    caseId: "case-1",
    severity: "urgent",
    scoreLevel: "high",
  });
  assert.deepEqual(
    enterprise.toolCalls.map(call => call.name),
    ["lookupCustomer", "searchKb", "riskScore", "openCase", "notifyOwner"]
  );
  assert.deepEqual(enterprise.toolCalls[1].input, { query: "SSO outage", limit: 2 });
  assert.deepEqual(enterprise.toolCalls[3].input.metadata, { scoreLevel: "high", docCount: 2 });

  session = reload(session, tools);
  const free = await session.runChunk(`await triageCustomer("cust-free", "billing question")`);
  assert.deepEqual(free.output, {
    customerId: "cust-free",
    caseId: "case-2",
    severity: "normal",
    scoreLevel: "unknown",
  });
  assert.equal(free.toolCalls[2].name, "riskScore");
  assert.equal(free.toolCalls[2].error, "risk service timeout");
  assert.equal(free.toolCalls[3].input.metadata.scoreLevel, "unknown");

  session = reload(session, tools);
  const summary = await session.runChunk(`
    workflowState.processed.map(item => item.customerId + ":" + item.caseId + ":" + item.severity).join("|")
  `);
  assert.equal(summary.output, "cust-ent:case-1:urgent|cust-free:case-2:normal");
  assert.equal(events.filter(e => e.type === "openCase").length, 2);
});

await test("host tool errors resume into agent try/catch after a dump/load boundary", async () => {
  const events = [];
  const tools = createSupportTools(events);
  let session = createSession({ tools });

  await session.runChunk(`
    const audit = [];
    async function safeLookup(customerId) {
      try {
        const customer = await lookupCustomer(customerId);
        audit.push("ok:" + customer.id);
        return customer.owner;
      } catch (e) {
        audit.push("error:" + customerId + ":" + e.message);
        return "fallback-owner";
      }
    }
    audit.length
  `);

  session = reload(session, tools);
  const missing = await session.runChunk(`await safeLookup("missing")`);
  assert.equal(missing.output, "fallback-owner");
  assert.equal(missing.toolCalls.length, 1);
  assert.equal(missing.toolCalls[0].name, "lookupCustomer");
  assert.equal(missing.toolCalls[0].error, "customer missing not found");

  session = reload(session, tools);
  const audit = await session.runChunk(`audit.join("|")`);
  assert.equal(audit.output, "error:missing:customer missing not found");
});

await test("invalid durable tool arguments fail crisply and leave the last checkpoint usable", async () => {
  const events = [];
  const tools = createSupportTools(events);
  let session = createSession({ tools });

  await session.runChunk(`
    async function badCaseMissingSeverity() {
      return await openCase({
        customerId: "cust-ent",
        summary: "missing required severity",
      });
    }

    async function badCaseExtraField() {
      return await openCase({
        customerId: "cust-ent",
        severity: "urgent",
        summary: "typo field",
        severty: "normal",
      });
    }

    async function goodCase() {
      const opened = await openCase({
        customerId: "cust-ent",
        severity: "urgent",
        summary: "valid follow-up",
      });
      return opened.id;
    }
    "ready"
  `);

  const checkpoint = session.dump();
  await assert.rejects(
    () => session.runChunk(`await badCaseMissingSeverity()`),
    /Invalid arguments for tool 'openCase': missing required parameter 'severity'/
  );
  assert.equal(events.filter(e => e.type === "openCase").length, 0);

  session = loadSession(checkpoint, { tools });
  await assert.rejects(
    () => session.runChunk(`await badCaseExtraField()`),
    /Invalid arguments for tool 'openCase': unexpected parameter 'severty'/
  );
  assert.equal(events.filter(e => e.type === "openCase").length, 0);

  session = loadSession(checkpoint, { tools });
  const recovered = await session.runChunk(`await goodCase()`);
  assert.equal(recovered.output, "case-1");
  assert.equal(recovered.toolCalls.length, 1);
  assert.equal(recovered.toolCalls[0].name, "openCase");
});

console.log(`\n${passed} scenarios3 durable checks passed.`);
