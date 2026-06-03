/**
 * Realistic, end-to-end agentic tool-use workflows for Zapcode stress-testing.
 *
 * Tests 12–16 multi-step workflow snippets covering:
 *   customer support triage, calendar/SLA, data pipeline, finance approval,
 *   search/RAG, form building, inventory/order, and more.
 *
 * Run: node tests/scenarios2-workflows.mjs
 *
 * NEW BUGS FOUND (sections 101+):
 *   BUG-G  Default function parameters are never applied — `f(x=10); f()` → x is null
 *   BUG-H  Closure scope sharing — two closures from same factory share captured vars
 *          `function wrap(x){return ()=>x}; f1=wrap(1); f2=wrap(2); f1()` → 2 not 1
 *   BUG-I  obj[varKey].push(v) — push result discarded; array in object unchanged
 *   BUG-J  FIXED (cluster C): class getter `get prop() {...}` now runs on read
 *   BUG-K  FIXED (cluster C): class setter `set v(x) {...}` now runs on assignment
 *   BUG-L  instanceof returns false for Array/Object — `[] instanceof Array` → false
 *   BUG-M  Destructuring defaults not applied — `const {a=10}={}; a` → null
 *   BUG-N  Computed property name in object literal is null — `{[expr]: val}` → null
 *   BUG-O  `??` operator in for-of body after Promise.all batch → "invalid iterator state"
 *          Minimal: `await Promise.all([t1(), t2()]); for (const x of arr) { y ?? 0; await t3(); }`
 *          Workaround: use `|| 0` or a ternary instead of `??` in such loops.
 *
 * MISSING (sections 201+):
 *   MISSING-2-A  Promise.allSettled is not a function
 *   MISSING-2-B  Promise.race is not a function
 *   MISSING-2-C  Object.hasOwn is not a function
 *   MISSING-2-D  new Array(N) / Array(N) — "not a constructor" / "not a function"
 *   MISSING-2-E  Array.prototype.entries() is not a function
 *   MISSING-2-F  Array.prototype.keys() is not a function
 *   MISSING-2-G  Array.from({length:N}, mapFn) — mapFn ignored (returns [])
 *   MISSING-2-H  Number.prototype.toLocaleString — "undefined is not a function"
 *   MISSING-2-I  Object.prototype.hasOwnProperty on instances — "undefined is not a function"
 *
 * ALSO NOTED (not reported separately — narrow scoped):
 *   JSON.parse("{bad}") silently returns {} instead of throwing SyntaxError
 *   for...in loops: explicitly unsupported (good error message)
 *   Closure over let in for-loop: all closures share last value (no per-iteration binding)
 *   Array destructuring assignment `[a,b]=[b,a]` → "unsupported assignment target"
 *   Symbol.iterator on custom objects → "object is not iterable"
 *   Promise constructor `new Promise(...)` → "[object Object] is not a constructor"
 *
 * ALREADY FIXED / WORKING (do NOT report):
 *   spread, regex, destructuring, Set/Map/Error, numeric builtins, Object.fromEntries,
 *   structuredClone, top-level switch, labeled break/continue, Date getUTC*, BUG-A..F
 *
 * KNOWN LIMITATIONS (do NOT report):
 *   tool calls inside .map/.filter/.forEach/.reduce → clear error, use for...of or
 *   await Promise.all([toolA(), toolB(), ...]) with direct array elements.
 *   for...in loops: unsupported (explicit error).
 */

import assert from "node:assert/strict";
import { execute } from "../dist/index.js";

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

// ===========================================================================
// SECTION 1 — Realistic agentic workflow scenarios (expected to pass)
// ===========================================================================

// --- 1. Customer-support triage -------------------------------------------
// fetch ticket → enrich account (parallel Promise.all) → classify → escalate
await check("workflow-01: support triage with parallel enrichment and escalation", async () => {
  const escalations = [];
  const tools = {
    fetchTicket: {
      description: "Fetch a support ticket by ID.",
      parameters: { ticketId: { type: "string" } },
      execute: async ({ ticketId }) => ({
        ticketId,
        subject: "Login broken",
        severity: ticketId === "T-001" ? 5 : 2,
        accountId: "acct-" + ticketId.slice(-3),
        tags: ["auth"],
      }),
    },
    getAccount: {
      description: "Look up an account.",
      parameters: { accountId: { type: "string" } },
      execute: async ({ accountId }) => ({
        accountId,
        tier: accountId.endsWith("001") ? "enterprise" : "free",
        openTickets: accountId.endsWith("001") ? 9 : 1,
        region: "us-east",
      }),
    },
    addTag: {
      description: "Add a tag to a ticket.",
      parameters: { ticketId: { type: "string" }, tag: { type: "string" } },
      execute: async ({ ticketId, tag }) => ({ ticketId, tag, ok: true }),
    },
    routeTicket: {
      description: "Route a ticket to a support queue.",
      parameters: {
        ticketId: { type: "string" },
        queue: { type: "string" },
        priority: { type: "number" },
        reason: { type: "string" },
      },
      execute: async (input) => { escalations.push(input); return { routed: true }; },
    },
  };

  const result = await execute(
    `
    const [ticket, account] = await Promise.all([
      fetchTicket({ ticketId: "T-001" }),
      getAccount({ accountId: "acct-001" })
    ]);
    const isEnterprise = account.tier === "enterprise";
    const isCritical   = ticket.severity >= 4;
    const priority     = isEnterprise && isCritical ? 1 : isEnterprise ? 2 : 3;
    const queue        = priority === 1 ? "vip-critical" : priority === 2 ? "enterprise" : "standard";

    if (isEnterprise) await addTag({ ticketId: ticket.ticketId, tag: "enterprise" });

    await routeTicket({
      ticketId: ticket.ticketId,
      queue,
      priority,
      reason: "tier=" + account.tier + " severity=" + ticket.severity,
    });

    ({ priority, queue, openTickets: account.openTickets })
    `,
    tools
  );

  assert.deepEqual(result.output, { priority: 1, queue: "vip-critical", openTickets: 9 });
  assert.equal(escalations.length, 1);
  assert.equal(escalations[0].queue, "vip-critical");
  assert.equal(escalations[0].priority, 1);
  assert.equal(result.toolCalls.length, 4); // fetchTicket, getAccount, addTag, routeTicket
});

// --- 2. Calendar / scheduling with SLA -------------------------------------------
// get current time, compute SLA deadline, determine urgency, notify
await check("workflow-02: calendar SLA routing with deadline computation", async () => {
  const NOW = Date.UTC(2026, 4, 31); // 2026-05-31T00:00:00Z
  const notifications = [];
  const tools = {
    getNowMs: {
      description: "Return current epoch milliseconds.",
      parameters: {},
      execute: async () => NOW,
    },
    parseDateMs: {
      description: "Parse ISO date to epoch ms.",
      parameters: { date: { type: "string" } },
      execute: async ({ date }) => Date.parse(date + "T00:00:00.000Z"),
    },
    notify: {
      description: "Send a notification.",
      parameters: {
        to: { type: "string" },
        message: { type: "string" },
        urgency: { type: "string" },
      },
      execute: async (input) => { notifications.push(input); return { sent: true }; },
    },
  };

  const result = await execute(
    `
    const now = await getNowMs();
    const dayMs = 24 * 60 * 60 * 1000;

    const requests = [
      { id: "R1", due: "2026-06-01", slaHours: 48, owner: "alice" },
      { id: "R2", due: "2026-06-10", slaHours: 72, owner: "bob" },
      { id: "R3", due: "2026-05-31", slaHours: 24, owner: "carol" },
    ];

    const report = [];
    for (const req of requests) {
      const dueMs    = await parseDateMs({ date: req.due });
      const diffDays = (dueMs - now) / dayMs;
      const slaDays  = req.slaHours / 24;
      const overdue  = diffDays < 0;
      const urgent   = !overdue && diffDays <= slaDays;
      const urgency  = overdue ? "overdue" : urgent ? "urgent" : "normal";

      if (urgency !== "normal") {
        await notify({
          to: req.owner,
          message: req.id + " is " + urgency + " (due " + req.due + ")",
          urgency,
        });
      }
      report.push({ id: req.id, urgency, diffDays });
    }
    report.map(r => r.id + ":" + r.urgency).join(",")
    `,
    tools
  );

  // R3 due=2026-05-31=now, diffDays=0 (NOT < 0, so NOT overdue), slaDays=1, 0<=1 → urgent
  assert.equal(result.output, "R1:urgent,R2:normal,R3:urgent");
  assert.equal(notifications.length, 2);
  assert.equal(notifications[0].to, "alice");
  assert.equal(notifications[0].urgency, "urgent");
  assert.equal(notifications[1].urgency, "urgent"); // R3 is urgent, not overdue
});

// --- 3. Data pipeline: paginated fetch → validate → transform → write ---
await check("workflow-03: paginated ETL pipeline with validation and write", async () => {
  const pages = [
    { rows: [{ id: 1, email: "alice@test.com", amount: "19.99" }, { id: 2, email: "bad-email", amount: "5.00" }], next: "p2" },
    { rows: [{ id: 3, email: "carol@example.org", amount: "abc" },  { id: 4, email: "dave@co.io", amount: "100.00" }], next: null },
  ];
  let pageIdx = 0;
  const written = [];
  const tools = {
    fetchPage: {
      description: "Fetch a page of rows.",
      parameters: { cursor: { type: "string", optional: true } },
      execute: async () => pages[pageIdx++],
    },
    writeRecord: {
      description: "Write a validated record.",
      parameters: { id: { type: "number" }, email: { type: "string" }, amountCents: { type: "number" } },
      execute: async (rec) => { written.push(rec); return { ok: true }; },
    },
  };

  const result = await execute(
    `
    const emailRe = /^[\\w.+-]+@[\\w-]+\\.[a-z]{2,}$/;
    const allRows = [];
    let cursor = null;
    while (true) {
      const page = await fetchPage({ cursor });
      for (const row of page.rows) {
        allRows.push(row);
      }
      if (!page.next) break;
      cursor = page.next;
    }

    const ok = [];
    const errors = [];
    for (const row of allRows) {
      const emailValid  = emailRe.test(row.email);
      const amountParsed = parseFloat(row.amount);
      const amountValid  = !isNaN(amountParsed) && amountParsed >= 0;

      if (!emailValid)  { errors.push({ id: row.id, reason: "bad email" }); continue; }
      if (!amountValid) { errors.push({ id: row.id, reason: "bad amount" }); continue; }

      const amountCents = Math.round(amountParsed * 100);
      await writeRecord({ id: row.id, email: row.email, amountCents });
      ok.push(row.id);
    }
    ({ written: ok.length, failed: errors.length, errors })
    `,
    tools
  );

  // id:1 ok, id:2 bad email, id:3 bad amount, id:4 ok → written=2, failed=2
  assert.equal(result.output.written, 2);
  assert.equal(result.output.failed, 2);
  assert.equal(result.output.errors[0].id, 2);
  assert.equal(written.length, 2);
  assert.equal(written[0].amountCents, 1999);
  assert.equal(written[1].amountCents, 10000);
});

// --- 4. Finance approval: totals, thresholds, per-item approvals -------------
await check("workflow-04: finance approval workflow with toFixed and per-item policy", async () => {
  const approvalRequests = [];
  const tools = {
    getLineItems: {
      description: "Return expense line items.",
      parameters: {},
      execute: async () => [
        { id: "E1", category: "travel",    amountUsd: 450.00, vendor: "Delta" },
        { id: "E2", category: "software",  amountUsd: 1200.50, vendor: "Acme" },
        { id: "E3", category: "meals",     amountUsd: 85.25, vendor: "Bistro" },
        { id: "E4", category: "equipment", amountUsd: 2400.00, vendor: "TechCo" },
      ],
    },
    requestApproval: {
      description: "Request approval for an expense.",
      parameters: {
        expenseId: { type: "string" },
        amountUsd: { type: "number" },
        approver: { type: "string" },
        reason: { type: "string" },
      },
      execute: async (req) => { approvalRequests.push(req); return { pending: true, requestId: "AR-" + req.expenseId }; },
    },
    submitExpenseReport: {
      description: "Submit an expense report.",
      parameters: {
        totalUsd: { type: "number" },
        autoApprovedCount: { type: "number" },
        pendingApprovalCount: { type: "number" },
        summary: { type: "string" },
      },
      execute: async (report) => ({ submitted: true, report }),
    },
  };

  const result = await execute(
    `
    const MANAGER_THRESHOLD = 500;
    const DIRECTOR_THRESHOLD = 2000;

    const items = await getLineItems();
    const autoApproved = [];
    const needsApproval = [];

    for (const item of items) {
      if (item.amountUsd < MANAGER_THRESHOLD) {
        autoApproved.push(item);
      } else {
        const approver = item.amountUsd >= DIRECTOR_THRESHOLD ? "director" : "manager";
        const resp = await requestApproval({
          expenseId: item.id,
          amountUsd: item.amountUsd,
          approver,
          reason: item.category + " from " + item.vendor,
        });
        needsApproval.push({ item, approver, requestId: resp.requestId });
      }
    }

    const total = items.reduce((s, i) => s + i.amountUsd, 0);
    const totalStr = Number(total.toFixed(2));

    await submitExpenseReport({
      totalUsd: totalStr,
      autoApprovedCount: autoApproved.length,
      pendingApprovalCount: needsApproval.length,
      summary: autoApproved.length + " auto-approved, " + needsApproval.length + " pending",
    });

    ({
      total: totalStr,
      autoApproved: autoApproved.map(i => i.id),
      pending: needsApproval.map(n => n.approver + ":" + n.item.id),
    })
    `,
    tools
  );

  assert.deepEqual(result.output.autoApproved, ["E1", "E3"]);
  assert.deepEqual(result.output.pending, ["manager:E2", "director:E4"]);
  assert.equal(result.output.total, 4135.75);
  assert.equal(approvalRequests.length, 2);
  assert.equal(approvalRequests[0].approver, "manager");
  assert.equal(approvalRequests[1].approver, "director");
});

// --- 5. Search/RAG: query → dedup + rank → top-N → format Markdown ----------
await check("workflow-05: search/RAG dedup rank top-N and format Markdown answer", async () => {
  const tools = {
    searchDocs: {
      description: "Search documentation chunks.",
      parameters: { query: { type: "string" }, limit: { type: "number" } },
      execute: async ({ query, limit }) => [
        { id: "d1", title: "Auth guide",      score: 0.92, content: "Use OAuth2 for auth." },
        { id: "d2", title: "Quickstart",      score: 0.88, content: "Run npm install first." },
        { id: "d3", title: "Auth guide",      score: 0.85, content: "Use OAuth2 for auth." }, // dup title
        { id: "d4", title: "API reference",   score: 0.79, content: "See /api/v1 docs." },
        { id: "d5", title: "Quickstart",      score: 0.71, content: "Run npm install first." }, // dup title
        { id: "d6", title: "Troubleshooting", score: 0.65, content: "Check error logs." },
      ].slice(0, limit),
    },
  };

  const result = await execute(
    `
    const raw = await searchDocs({ query: "how to authenticate", limit: 6 });

    // Sort by score descending
    const sorted = raw.slice().sort((a, b) => b.score - a.score);

    // Dedup by title, keep highest score
    const seen = {};
    const deduped = [];
    for (const doc of sorted) {
      if (!seen[doc.title]) {
        seen[doc.title] = true;
        deduped.push(doc);
      }
    }

    // Top 3
    const top = deduped.slice(0, 3);

    // Format Markdown
    const sections = top.map((doc, i) =>
      (i + 1) + ". **" + doc.title + "** (score: " + doc.score.toFixed(2) + ")\\n   " + doc.content
    );
    const answer = "## Search Results\\n\\n" + sections.join("\\n");

    ({ topTitles: top.map(d => d.title), answerLength: answer.length, firstScore: top[0].score })
    `,
    tools
  );

  assert.deepEqual(result.output.topTitles, ["Auth guide", "Quickstart", "API reference"]);
  assert.ok(result.output.answerLength > 50);
  assert.equal(result.output.firstScore, 0.92);
});

// --- 6. Form / payload building with fallback on tool failure ----------------
await check("workflow-06: form payload assembly with try/catch fallback on tool failure", async () => {
  const submissions = [];
  const tools = {
    getUserProfile: {
      description: "Fetch a user profile by ID.",
      parameters: { userId: { type: "string" } },
      execute: async ({ userId }) => {
        if (userId === "u-missing") throw new Error("user not found");
        return { userId, name: "Alice", email: "alice@test.com", orgId: "org-1" };
      },
    },
    getOrgSettings: {
      description: "Fetch org settings.",
      parameters: { orgId: { type: "string" } },
      execute: async ({ orgId }) => ({
        orgId,
        plan: "pro",
        maxSeats: 50,
        features: ["sso", "audit-log"],
      }),
    },
    submitForm: {
      description: "Submit a registration form.",
      parameters: {
        userId: { type: "string" },
        name: { type: "string" },
        email: { type: "string" },
        orgId: { type: "string" },
        plan: { type: "string" },
        hasSso: { type: "boolean" },
      },
      execute: async (form) => { submissions.push(form); return { submittedAt: 1748649600000 }; },
    },
  };

  const result = await execute(
    `
    const userId = "u-001";

    let profile;
    try {
      profile = await getUserProfile({ userId });
    } catch (e) {
      profile = { userId, name: "Unknown", email: "noreply@internal", orgId: "org-default" };
    }

    let orgSettings;
    try {
      orgSettings = await getOrgSettings({ orgId: profile.orgId });
    } catch (e) {
      orgSettings = { plan: "free", maxSeats: 5, features: [] };
    }

    const hasSso = orgSettings.features.includes("sso");
    const required = ["userId", "name", "email", "orgId", "plan", "hasSso"];
    const form = {
      userId:  profile.userId,
      name:    profile.name,
      email:   profile.email,
      orgId:   profile.orgId,
      plan:    orgSettings.plan,
      hasSso,
    };
    const missing = required.filter(k => form[k] === undefined || form[k] === null);
    if (missing.length > 0) throw new Error("Missing fields: " + missing.join(", "));

    await submitForm(form);
    ({ name: form.name, plan: form.plan, hasSso: form.hasSso, submitted: true })
    `,
    tools
  );

  assert.equal(result.output.name, "Alice");
  assert.equal(result.output.plan, "pro");
  assert.equal(result.output.hasSso, true);
  assert.equal(result.output.submitted, true);
  assert.equal(submissions.length, 1);
  assert.equal(submissions[0].hasSso, true);
});

// --- 7. Inventory / order: check stock, decide fulfill/backorder, place orders
await check("workflow-07: inventory check and order fulfillment with backorders", async () => {
  const orders = [];
  const tools = {
    checkStock: {
      description: "Check stock for a SKU.",
      parameters: { sku: { type: "string" } },
      execute: async ({ sku }) => {
        const stock = { "SKU-A": 50, "SKU-B": 0, "SKU-C": 5, "SKU-D": 100 };
        return { sku, qty: stock[sku] ?? 0 };
      },
    },
    placeOrder: {
      description: "Place a fulfillment or backorder.",
      parameters: {
        sku: { type: "string" },
        qty: { type: "number" },
        type: { type: "string" },
        warehouseId: { type: "string", optional: true },
      },
      execute: async (o) => { orders.push(o); return { orderId: "ORD-" + o.sku, ok: true }; },
    },
  };

  const result = await execute(
    `
    const requested = [
      { sku: "SKU-A", qty: 10 },
      { sku: "SKU-B", qty: 5 },
      { sku: "SKU-C", qty: 3 },
      { sku: "SKU-D", qty: 200 },
    ];

    const [stockA, stockB, stockC, stockD] = await Promise.all([
      checkStock({ sku: "SKU-A" }),
      checkStock({ sku: "SKU-B" }),
      checkStock({ sku: "SKU-C" }),
      checkStock({ sku: "SKU-D" }),
    ]);
    const stockMap = {};
    stockMap[stockA.sku] = stockA.qty;
    stockMap[stockB.sku] = stockB.qty;
    stockMap[stockC.sku] = stockC.qty;
    stockMap[stockD.sku] = stockD.qty;

    const report = [];
    for (const req of requested) {
      // NOTE: use || 0 (not ?? 0) — see BUG-O: ?? in for-of after Promise.all batch
      // triggers "invalid iterator state" in the interpreter
      const available = stockMap[req.sku] || 0;
      if (available >= req.qty) {
        const resp = await placeOrder({ sku: req.sku, qty: req.qty, type: "fulfill", warehouseId: "WH-1" });
        report.push({ sku: req.sku, status: "fulfilled", orderId: resp.orderId });
      } else if (available > 0) {
        const fulfillQty   = available;
        const backorderQty = req.qty - available;
        const r1 = await placeOrder({ sku: req.sku, qty: fulfillQty,   type: "fulfill",   warehouseId: "WH-1" });
        const r2 = await placeOrder({ sku: req.sku, qty: backorderQty, type: "backorder" });
        report.push({ sku: req.sku, status: "partial", fulfillQty, backorderQty });
      } else {
        const plResp = await placeOrder({ sku: req.sku, qty: req.qty, type: "backorder" });
        report.push({ sku: req.sku, status: "backorder", orderId: plResp.orderId });
      }
    }
    report.map(r => r.sku + ":" + r.status).join(",")
    `,
    tools
  );

  assert.equal(result.output, "SKU-A:fulfilled,SKU-B:backorder,SKU-C:fulfilled,SKU-D:partial");
  // SKU-A: 50 >= 10 → fulfilled
  // SKU-B: 0 >= 5 → backorder
  // SKU-C: 5 >= 3 → fulfilled
  // SKU-D: 100 < 200, > 0 → partial
  assert.ok(orders.length >= 5); // SKU-A(1), SKU-B(1), SKU-C(1), SKU-D(2)
});

// --- 8. Customer churn prediction: enrich + classify + action ----------------
await check("workflow-08: churn risk classification with multi-tool enrichment", async () => {
  const alerts = [];
  const tools = {
    listAccounts: {
      description: "List account IDs.",
      parameters: {},
      execute: async () => ["acc-1", "acc-2", "acc-3", "acc-4"],
    },
    getUsageStats: {
      description: "Get usage stats for an account.",
      parameters: { accountId: { type: "string" } },
      execute: async ({ accountId }) => {
        const stats = {
          "acc-1": { logins: 45, apiCalls: 1200, lastActiveMs: Date.UTC(2026, 4, 30) },
          "acc-2": { logins: 2,  apiCalls: 10,   lastActiveMs: Date.UTC(2026, 3, 1) },
          "acc-3": { logins: 30, apiCalls: 800,  lastActiveMs: Date.UTC(2026, 4, 28) },
          "acc-4": { logins: 0,  apiCalls: 0,    lastActiveMs: Date.UTC(2026, 2, 15) },
        };
        return stats[accountId];
      },
    },
    sendChurnAlert: {
      description: "Send a churn risk alert.",
      parameters: {
        accountId: { type: "string" },
        riskLevel: { type: "string" },
        daysSinceActive: { type: "number" },
      },
      execute: async (alert) => { alerts.push(alert); return { sent: true }; },
    },
  };

  const result = await execute(
    `
    const NOW_MS = ${Date.UTC(2026, 4, 31)};
    const DAY_MS = 24 * 60 * 60 * 1000;
    const accountIds = await listAccounts();

    const results = [];
    for (const accountId of accountIds) {
      const stats = await getUsageStats({ accountId });
      const daysSinceActive = Math.floor((NOW_MS - stats.lastActiveMs) / DAY_MS);
      const riskLevel =
        daysSinceActive > 60 ? "critical" :
        daysSinceActive > 30 ? "high" :
        stats.logins < 5     ? "medium" : "low";

      if (riskLevel === "critical" || riskLevel === "high") {
        await sendChurnAlert({ accountId, riskLevel, daysSinceActive });
      }
      results.push({ accountId, riskLevel, daysSinceActive });
    }

    results.map(r => r.accountId + ":" + r.riskLevel).join(",")
    `,
    tools
  );

  assert.equal(result.output, "acc-1:low,acc-2:high,acc-3:low,acc-4:critical");
  assert.equal(alerts.length, 2);
  assert.equal(alerts.find(a => a.accountId === "acc-2").riskLevel, "high");
  assert.equal(alerts.find(a => a.accountId === "acc-4").riskLevel, "critical");
});

// --- 9. Batch notifications: group users by region, send per-region message --
await check("workflow-09: group users by region and send batched notifications", async () => {
  const sent = [];
  const tools = {
    getUsers: {
      description: "Return all users.",
      parameters: {},
      execute: async () => [
        { id: "u1", name: "Alice",  region: "us", email: "alice@test.com" },
        { id: "u2", name: "Bob",    region: "eu", email: "bob@test.com" },
        { id: "u3", name: "Carol",  region: "us", email: "carol@test.com" },
        { id: "u4", name: "Dave",   region: "ap", email: "dave@test.com" },
        { id: "u5", name: "Eve",    region: "eu", email: "eve@test.com" },
        { id: "u6", name: "Frank",  region: "us", email: "frank@test.com" },
      ],
    },
    sendBatch: {
      description: "Send a notification batch to a region.",
      parameters: {
        region: { type: "string" },
        recipients: { type: "array" },
        subject: { type: "string" },
      },
      execute: async (batch) => { sent.push(batch); return { region: batch.region, count: batch.recipients.length }; },
    },
  };

  const result = await execute(
    `
    const users = await getUsers();

    // Group users by region using object map
    const byRegion = {};
    for (const user of users) {
      const r = user.region;
      if (!byRegion[r]) byRegion[r] = [];
      const arr = byRegion[r];
      arr.push(user.email);
      byRegion[r] = arr;
    }

    // Send a batch per region
    const regions = Object.keys(byRegion).sort();
    const summary = [];
    for (const region of regions) {
      const recipients = byRegion[region];
      const resp = await sendBatch({
        region,
        recipients,
        subject: "Update for " + region.toUpperCase() + " region",
      });
      summary.push(region + ":" + resp.count);
    }
    summary.join(",")
    `,
    tools
  );

  assert.equal(result.output, "ap:1,eu:2,us:3");
  assert.equal(sent.length, 3);
  const usBatch = sent.find(b => b.region === "us");
  assert.equal(usBatch.recipients.length, 3);
  assert.ok(usBatch.recipients.includes("alice@test.com"));
});

// --- 10. Multi-step report generation: fetch metrics → compute → format ------
await check("workflow-10: metrics report with computed derived fields and Markdown formatting", async () => {
  const tools = {
    getMetrics: {
      description: "Fetch daily metrics for a service.",
      parameters: { service: { type: "string" }, days: { type: "number" } },
      execute: async ({ service, days }) => {
        const data = [];
        for (let i = 0; i < days; i++) {
          data.push({
            date: "2026-05-" + String(29 + i).padStart(2, "0"),
            requests: 10000 + i * 1500,
            errors: 50 + i * 10,
            p99LatencyMs: 120 + i * 5,
          });
        }
        return data;
      },
    },
  };

  const result = await execute(
    `
    const metrics = await getMetrics({ service: "api-gateway", days: 3 });

    // Compute derived fields
    const enriched = metrics.map(m => ({
      date: m.date,
      requests: m.requests,
      errorRate: Number((m.errors / m.requests * 100).toFixed(3)),
      p99LatencyMs: m.p99LatencyMs,
    }));

    const avgErrorRate = enriched.reduce((s, m) => s + m.errorRate, 0) / enriched.length;
    const maxLatency   = enriched.reduce((max, m) => Math.max(max, m.p99LatencyMs), 0);
    const totalRequests = enriched.reduce((s, m) => s + m.requests, 0);

    // Format rows — note: toLocaleString() is not available; use String() instead
    const rows = enriched.map(m =>
      "| " + m.date + " | " + String(m.requests) + " | " + m.errorRate + "% | " + m.p99LatencyMs + "ms |"
    );
    const table = "| Date | Requests | Error Rate | P99 |\\n|---|---|---|---|\\n" + rows.join("\\n");

    ({
      table,
      avgErrorRate: Number(avgErrorRate.toFixed(3)),
      maxLatency,
      totalRequests,
      rowCount: enriched.length,
    })
    `,
    tools
  );

  assert.equal(result.output.rowCount, 3);
  assert.equal(result.output.totalRequests, 10000 + 11500 + 13000);
  assert.equal(result.output.maxLatency, 130);
  assert.ok(result.output.table.includes("2026-05-29"));
  assert.ok(result.output.table.includes("Error Rate"));
  // avgErrorRate: (50/10000 + 60/11500 + 70/13000) / 3 * 100
  assert.ok(result.output.avgErrorRate > 0.4 && result.output.avgErrorRate < 0.6);
});

// --- 11. Retry logic: retry a failing tool up to N times --------------------
await check("workflow-11: retry loop with exponential-like backoff attempt tracking", async () => {
  let attemptCount = 0;
  const tools = {
    unreliableApi: {
      description: "API that fails the first two times.",
      parameters: { payload: { type: "string" } },
      execute: async ({ payload }) => {
        attemptCount++;
        if (attemptCount < 3) throw new Error("service unavailable (attempt " + attemptCount + ")");
        return { ok: true, data: payload.toUpperCase(), attempts: attemptCount };
      },
    },
    log: {
      description: "Log a message.",
      parameters: { msg: { type: "string" } },
      execute: async ({ msg }) => ({ logged: msg }),
    },
  };

  const result = await execute(
    `
    const MAX_RETRIES = 4;
    let lastError = null;
    let response = null;

    for (let attempt = 1; attempt <= MAX_RETRIES; attempt++) {
      try {
        response = await unreliableApi({ payload: "hello" });
        lastError = null;
        break;
      } catch (e) {
        lastError = String(e);
        await log({ msg: "Attempt " + attempt + " failed: " + lastError });
        if (attempt === MAX_RETRIES) throw new Error("Exhausted retries: " + lastError);
      }
    }

    ({ data: response.data, attempts: response.attempts, lastError })
    `,
    tools
  );

  assert.equal(result.output.data, "HELLO");
  assert.equal(result.output.attempts, 3);
  assert.equal(result.output.lastError, null);
  assert.equal(attemptCount, 3);
  // verify tool calls include 2 failures and 1 success
  const apiCalls = result.toolCalls.filter(c => c.name === "unreliableApi");
  assert.equal(apiCalls.length, 3);
  assert.ok(apiCalls[0].error);
  assert.ok(apiCalls[1].error);
  assert.ok(!apiCalls[2].error);
});

// --- 12. Multi-source aggregation: weighted scoring --------------------------
await check("workflow-12: multi-source candidate scoring with weighted aggregation", async () => {
  const tools = {
    getResumeScore: {
      description: "Score a resume.",
      parameters: { candidateId: { type: "string" } },
      execute: async ({ candidateId }) => {
        const scores = { "c1": 85, "c2": 72, "c3": 91, "c4": 60 };
        return { candidateId, score: scores[candidateId] ?? 50 };
      },
    },
    getInterviewScore: {
      description: "Get interview assessment score.",
      parameters: { candidateId: { type: "string" } },
      execute: async ({ candidateId }) => {
        const scores = { "c1": 78, "c2": 88, "c3": 70, "c4": 95 };
        return { candidateId, score: scores[candidateId] ?? 50 };
      },
    },
    getTechTestScore: {
      description: "Get technical test score.",
      parameters: { candidateId: { type: "string" } },
      execute: async ({ candidateId }) => {
        const scores = { "c1": 90, "c2": 65, "c3": 88, "c4": 75 };
        return { candidateId, score: scores[candidateId] ?? 50 };
      },
    },
  };

  const result = await execute(
    `
    const candidateIds = ["c1", "c2", "c3", "c4"];
    const WEIGHTS = { resume: 0.25, interview: 0.40, techTest: 0.35 };
    const PASS_THRESHOLD = 78;

    const scored = [];
    for (const candidateId of candidateIds) {
      const [resume, interview, techTest] = await Promise.all([
        getResumeScore({ candidateId }),
        getInterviewScore({ candidateId }),
        getTechTestScore({ candidateId }),
      ]);
      const weighted =
        resume.score    * WEIGHTS.resume    +
        interview.score * WEIGHTS.interview +
        techTest.score  * WEIGHTS.techTest;
      const final = Number(weighted.toFixed(1));
      scored.push({ candidateId, final, pass: final >= PASS_THRESHOLD });
    }

    scored.sort((a, b) => b.final - a.final);
    ({
      ranking: scored.map(s => s.candidateId),
      passing: scored.filter(s => s.pass).map(s => s.candidateId),
      topScore: scored[0].final,
    })
    `,
    tools
  );

  // c1: 0.25*85 + 0.40*78 + 0.35*90 = 21.25+31.2+31.5 = 83.95 → 84.0  pass
  // c2: 0.25*72 + 0.40*88 + 0.35*65 = 18+35.2+22.75 = 75.95 → 76.0   fail
  // c3: 0.25*91 + 0.40*70 + 0.35*88 = 22.75+28+30.8 = 81.55 → 81.6   pass
  // c4: 0.25*60 + 0.40*95 + 0.35*75 = 15+38+26.25 = 79.25 → 79.3     pass
  assert.equal(result.output.ranking[0], "c1"); // highest
  assert.ok(result.output.passing.includes("c1"));
  assert.ok(result.output.passing.includes("c3"));
  assert.ok(!result.output.passing.includes("c2"));
  assert.ok(result.output.topScore >= 83 && result.output.topScore <= 85);
});

// --- 13. Config validation and patching with conditional tool calls -----------
await check("workflow-13: config validation with field-level patching", async () => {
  const patches = [];
  const tools = {
    loadConfig: {
      description: "Load a service configuration.",
      parameters: { service: { type: "string" } },
      execute: async ({ service }) => ({
        service,
        timeout: 5000,
        retries: null,          // should be number
        logLevel: "WARN",
        maxConnections: 200,
        featureFlags: { beta: true, experimental: null },
      }),
    },
    patchConfig: {
      description: "Patch a config field.",
      parameters: {
        service: { type: "string" },
        field: { type: "string" },
        value: { type: "string" },
        reason: { type: "string" },
      },
      execute: async (p) => { patches.push(p); return { patched: true }; },
    },
  };

  const result = await execute(
    `
    const RULES = [
      { field: "retries",        required: true,  defaultVal: "3",     type: "number" },
      { field: "logLevel",       allowed: ["DEBUG","INFO","WARN","ERROR"] },
      { field: "maxConnections", max: 100,         defaultVal: "50" },
    ];

    const cfg = await loadConfig({ service: "payments" });
    const issues = [];
    const patchOps = [];

    for (const rule of RULES) {
      const val = cfg[rule.field];
      if (rule.required && (val === null || val === undefined)) {
        issues.push(rule.field + " is missing");
        patchOps.push({ field: rule.field, value: rule.defaultVal, reason: "required field missing" });
      } else if (rule.allowed && !rule.allowed.includes(val)) {
        issues.push(rule.field + " has invalid value: " + val);
      } else if (rule.max !== undefined && typeof val === "number" && val > rule.max) {
        issues.push(rule.field + " exceeds max " + rule.max);
        patchOps.push({ field: rule.field, value: rule.defaultVal, reason: "exceeds max" });
      }
    }

    for (const op of patchOps) {
      await patchConfig({ service: cfg.service, field: op.field, value: op.value, reason: op.reason });
    }

    ({ issues, patched: patchOps.map(p => p.field) })
    `,
    tools
  );

  assert.ok(result.output.issues.some(i => i.includes("retries")));
  assert.ok(result.output.issues.some(i => i.includes("maxConnections")));
  assert.ok(result.output.patched.includes("retries"));
  assert.ok(result.output.patched.includes("maxConnections"));
  assert.equal(patches.length, 2);
});

// --- 14. E-commerce checkout: apply coupons, compute taxes, place order ------
await check("workflow-14: e-commerce checkout with coupon, tax, and order placement", async () => {
  const orders = [];
  const tools = {
    getCart: {
      description: "Get current cart contents.",
      parameters: { cartId: { type: "string" } },
      execute: async ({ cartId }) => ({
        cartId,
        items: [
          { sku: "BOOK-01", name: "Clean Code", price: 39.99, qty: 1 },
          { sku: "BOOK-02", name: "SICP",       price: 49.99, qty: 2 },
        ],
      }),
    },
    validateCoupon: {
      description: "Validate and return a coupon discount.",
      parameters: { code: { type: "string" }, subtotal: { type: "number" } },
      execute: async ({ code, subtotal }) => {
        const coupons = { "SAVE10": 0.10, "SAVE20": 0.20 };
        const rate = coupons[code];
        if (!rate) throw new Error("Invalid coupon: " + code);
        return { valid: true, discountRate: rate, discountAmt: Number((subtotal * rate).toFixed(2)) };
      },
    },
    placeOrder: {
      description: "Place an order.",
      parameters: {
        cartId: { type: "string" },
        subtotal: { type: "number" },
        discount: { type: "number" },
        tax: { type: "number" },
        total: { type: "number" },
        couponCode: { type: "string", optional: true },
      },
      execute: async (o) => { orders.push(o); return { orderId: "ORD-" + Date.now(), status: "confirmed" }; },
    },
  };

  const result = await execute(
    `
    const TAX_RATE = 0.08;
    const cart = await getCart({ cartId: "cart-42" });
    const subtotal = cart.items.reduce((s, i) => s + i.price * i.qty, 0);

    let discount = 0;
    let couponUsed = null;
    try {
      const coupon = await validateCoupon({ code: "SAVE10", subtotal });
      discount = coupon.discountAmt;
      couponUsed = "SAVE10";
    } catch(e) {
      // coupon invalid, no discount
    }

    const taxable = subtotal - discount;
    const tax     = Number((taxable * TAX_RATE).toFixed(2));
    const total   = Number((taxable + tax).toFixed(2));

    const resp = await placeOrder({
      cartId:     cart.cartId,
      subtotal:   Number(subtotal.toFixed(2)),
      discount,
      tax,
      total,
      couponCode: couponUsed,
    });

    ({
      subtotal:   Number(subtotal.toFixed(2)),
      discount,
      tax,
      total,
      orderId:    resp.orderId,
      status:     resp.status,
    })
    `,
    tools
  );

  // subtotal: 39.99 + 49.99*2 = 139.97, discount: 13.997→14.00, taxable: 125.97, tax: 10.08, total: 136.05
  assert.equal(result.output.subtotal, 139.97);
  assert.equal(result.output.discount, 14.00);
  assert.equal(result.output.status, "confirmed");
  assert.ok(result.output.orderId.startsWith("ORD-"));
  assert.equal(orders[0].couponCode, "SAVE10");
});

// --- 15. Agent tool-call record introspection --------------------------------
await check("workflow-15: toolCalls introspection and audit log", async () => {
  const auditLog = [];
  const tools = {
    readSecret: {
      description: "Read a secret value.",
      parameters: { key: { type: "string" } },
      execute: async ({ key }) => ({ key, value: "secret-" + key }),
    },
    writeAudit: {
      description: "Write an audit log entry.",
      parameters: {
        action: { type: "string" },
        resource: { type: "string" },
        actor: { type: "string" },
      },
      execute: async (entry) => { auditLog.push(entry); return { id: "audit-1" }; },
    },
  };

  const result = await execute(
    `
    const keys = ["db-pass", "api-key", "jwt-secret"];
    const secrets = [];
    for (const key of keys) {
      const s = await readSecret({ key });
      secrets.push({ key: s.key, found: s.value !== null });
      await writeAudit({ action: "READ_SECRET", resource: key, actor: "agent" });
    }
    ({
      count: secrets.length,
      allFound: secrets.every(s => s.found),
      auditCount: keys.length,
    })
    `,
    tools
  );

  assert.equal(result.output.count, 3);
  assert.equal(result.output.allFound, true);
  assert.equal(result.output.auditCount, 3);
  assert.equal(auditLog.length, 3);
  assert.equal(result.toolCalls.length, 6); // 3 readSecret + 3 writeAudit
  const readCalls  = result.toolCalls.filter(c => c.name === "readSecret");
  const auditCalls = result.toolCalls.filter(c => c.name === "writeAudit");
  assert.equal(readCalls.length, 3);
  assert.equal(auditCalls.length, 3);
});

// --- 16. Set dedup + Map frequency + format report (pure data) ---------------
await check("workflow-16: Set dedup and frequency map from tool data", async () => {
  const tools = {
    getEvents: {
      description: "Return event stream.",
      parameters: {},
      execute: async () => [
        { id: "e1", type: "login",   userId: "u1" },
        { id: "e2", type: "purchase", userId: "u2" },
        { id: "e3", type: "login",   userId: "u1" },
        { id: "e4", type: "logout",  userId: "u1" },
        { id: "e5", type: "purchase", userId: "u3" },
        { id: "e6", type: "login",   userId: "u2" },
        { id: "e7", type: "purchase", userId: "u1" },
      ],
    },
  };

  const result = await execute(
    `
    const events = await getEvents();

    // Count event types
    const freq = {};
    for (const ev of events) {
      freq[ev.type] = (freq[ev.type] || 0) + 1;
    }

    // Unique users using Set
    const userSet = new Set();
    for (const ev of events) userSet.add(ev.userId);
    const uniqueUsers = Array.from(userSet).sort();

    // Users who made purchases
    const buyers = new Set();
    for (const ev of events) {
      if (ev.type === "purchase") buyers.add(ev.userId);
    }
    const buyerList = Array.from(buyers).sort();

    ({
      freq,
      uniqueUserCount: uniqueUsers.length,
      uniqueUsers,
      buyers: buyerList,
      mostCommon: Object.keys(freq).reduce((a, b) => freq[a] >= freq[b] ? a : b),
    })
    `,
    tools
  );

  assert.deepEqual(result.output.freq, { login: 3, purchase: 3, logout: 1 });
  assert.equal(result.output.uniqueUserCount, 3);
  assert.deepEqual(result.output.uniqueUsers, ["u1", "u2", "u3"]);
  assert.deepEqual(result.output.buyers, ["u1", "u2", "u3"]);
  // login and purchase both have 3 — either is valid as "mostCommon"
  assert.ok(["login", "purchase"].includes(result.output.mostCommon));
});

// ===========================================================================
// SECTION 101+ — CONFIRMED BUGS (expected to fail; document interpreter behavior)
// ===========================================================================

await check("BUG-G: default function parameters not applied (f(x=10); f() → x is null)", async () => {
  const r = await execute(`function f(x = 10) { return x; } f()`, {});
  // BUG: returns null instead of 10
  assert.equal(r.output, 10);
});

await check("BUG-G-arrow: default arrow param not applied (() => x=42 → null)", async () => {
  const r = await execute(`const fn = (x = 42) => x; fn()`, {});
  // BUG: returns null
  assert.equal(r.output, 42);
});

await check("BUG-H: closure scope sharing — two closures from same factory overwrite each other", async () => {
  // function wrap(x) { return () => x; }
  // f1 = wrap(10), f2 = wrap(20) → f1() should be 10, but returns 20 (shared scope)
  const r = await execute(
    `
    function wrap(x) { return () => x; }
    const f1 = wrap(10);
    const f2 = wrap(20);
    [f1(), f2()]
    `,
    {}
  );
  // BUG: returns [20, 20] — both closures see the last-written x
  assert.deepEqual(r.output, [10, 20]);
});

await check("BUG-I: obj[variableKey].push(v) — mutation is discarded", async () => {
  // obj["key"] = []; const k = "key"; obj[k].push(1); obj → {key: []} not {key:[1]}
  const r = await execute(
    `
    const o = {};
    o["eng"] = [];
    const k = "eng";
    o[k].push("Alice");
    o[k].push("Bob");
    o[k]
    `,
    {}
  );
  // BUG: returns [] — push via variable key discards result
  assert.deepEqual(r.output, ["Alice", "Bob"]);
});

await check("BUG-J: class getter always returns null", async () => {
  const r = await execute(
    `
    class Box {
      constructor(v) { this.v = v; }
      get doubled() { return this.v * 2; }
    }
    new Box(5).doubled
    `,
    {}
  );
  // FIXED (cluster C): the getter body runs on read, returning 10.
  assert.equal(r.output, 10);
});

await check("BUG-K: class setter has no effect — assigned value not stored", async () => {
  const r = await execute(
    `
    class Box {
      constructor() { this._v = 0; }
      set v(x) { this._v = x * 2; }
      get v() { return this._v; }
    }
    const b = new Box();
    b.v = 10;
    b._v
    `,
    {}
  );
  // FIXED (cluster C): the setter runs on assignment, so _v becomes 20.
  assert.equal(r.output, 20);
});

await check("BUG-L: instanceof returns false for Array and Object", async () => {
  const r = await execute(
    `
    [[] instanceof Array, {} instanceof Object, new Map() instanceof Map]
    `,
    {}
  );
  // BUG: returns [false, false, false]
  assert.deepEqual(r.output, [true, true, true]);
});

await check("BUG-M: destructuring defaults not applied when key is missing/undefined", async () => {
  const r = await execute(
    `
    const { a = 10, b = 20 } = { a: 5 };
    [a, b]
    `,
    {}
  );
  // BUG: returns [5, null] — default for b not applied
  assert.deepEqual(r.output, [5, 20]);
});

await check("BUG-N: computed property name in object literal resolves to null", async () => {
  const r = await execute(
    `
    const key = "myProp";
    const obj = { [key]: 99 };
    obj.myProp
    `,
    {}
  );
  // BUG: returns null instead of 99
  assert.equal(r.output, 99);
});

// ===========================================================================
// SECTION 201+ — MISSING BUILTINS
// ===========================================================================

await check("MISSING-2-A: Promise.allSettled is not a function", async () => {
  const r = await execute(
    `
    const results = await Promise.allSettled([Promise.resolve(1), Promise.reject("err"), Promise.resolve(3)]);
    results.map(r => r.status)
    `,
    {}
  );
  assert.deepEqual(r.output, ["fulfilled", "rejected", "fulfilled"]);
});

await check("MISSING-2-B: Promise.race is not a function", async () => {
  const r = await execute(
    `
    const winner = await Promise.race([Promise.resolve("fast"), Promise.resolve("slow")]);
    winner
    `,
    {}
  );
  assert.equal(r.output, "fast");
});

await check("MISSING-2-C: Object.hasOwn is not a function", async () => {
  const r = await execute(
    `
    const o = { a: 1 };
    [Object.hasOwn(o, "a"), Object.hasOwn(o, "b")]
    `,
    {}
  );
  assert.deepEqual(r.output, [true, false]);
});

await check("MISSING-2-D: new Array(N) — not a constructor", async () => {
  const r = await execute(`new Array(5).fill(0)`, {});
  assert.deepEqual(r.output, [0, 0, 0, 0, 0]);
});

await check("MISSING-2-E: Array.prototype.entries() — not a function", async () => {
  const r = await execute(
    `
    const arr = ["a", "b", "c"];
    const out = [];
    for (const [i, v] of arr.entries()) out.push(i + ":" + v);
    out
    `,
    {}
  );
  assert.deepEqual(r.output, ["0:a", "1:b", "2:c"]);
});

await check("MISSING-2-F: Array.prototype.keys() — not a function", async () => {
  const r = await execute(
    `
    const arr = [10, 20, 30];
    const keys = [];
    for (const k of arr.keys()) keys.push(k);
    keys
    `,
    {}
  );
  assert.deepEqual(r.output, [0, 1, 2]);
});

await check("MISSING-2-G: Array.from({length:N}, mapFn) — mapFn param ignored (returns empty)", async () => {
  // Array.from({length:3}, (_, i) => i) should return [0,1,2] but returns []
  const r = await execute(`Array.from({length: 3}, (_, i) => i * 2)`, {});
  assert.deepEqual(r.output, [0, 2, 4]);
});

await check("MISSING-2-H: Number.prototype.toLocaleString — undefined is not a function", async () => {
  const r = await execute(`(10000).toLocaleString()`, {});
  assert.equal(r.output, "10,000");
});

await check("MISSING-2-I: Object.prototype.hasOwnProperty on instances — undefined is not a function", async () => {
  const r = await execute(`({a:1}).hasOwnProperty("a")`, {});
  assert.equal(r.output, true);
});

await check("BUG-O: ?? (nullish coalescing) in for-of body after Promise.all → invalid iterator state", async () => {
  // Minimal repro:
  // await Promise.all([t1(), t2()]); for (const x of arr) { const v = y ?? 0; await t3(x); }
  // Crashes with "runtime error: invalid iterator state"
  // Workaround: use || 0 or a ternary instead
  const tools = {
    fetchPair: {
      description: "Return a pair.",
      parameters: { id: { type: "string" } },
      execute: async ({ id }) => ({ id, val: id.length }),
    },
    handleItem: {
      description: "Handle an item.",
      parameters: { item: { type: "string" } },
      execute: async ({ item }) => ({ done: true }),
    },
  };
  const r = await execute(
    `
    await Promise.all([fetchPair({ id: "a" }), fetchPair({ id: "b" })]);
    const items = ["x", "y"];
    for (const item of items) {
      const v = null ?? 0;
      await handleItem({ item });
    }
    "done"
    `,
    tools
  );
  // BUG: throws "runtime error: invalid iterator state"
  assert.equal(r.output, "done");
});

// ===========================================================================
// Summary
// ===========================================================================
const failed = results.filter(r => !r[1]);
const bugs    = results.filter(r => !r[1] && r[0].startsWith("BUG-"));
const missing = results.filter(r => !r[1] && r[0].startsWith("MISSING-"));
const workflows = results.filter(r => !r[1] && r[0].startsWith("workflow-"));

console.log(`\n${results.length - failed.length}/${results.length} passed`);
if (workflows.length)  console.log(`  Workflow scenario failures (unexpected): ${workflows.length}`);
if (bugs.length)       console.log(`  Confirmed interpreter bugs (expected to fail): ${bugs.length}`);
if (missing.length)    console.log(`  Missing builtins (expected to fail): ${missing.length}`);

if (workflows.length) {
  console.log("\nUnexpected workflow failures:");
  for (const [name, , msg] of workflows) {
    console.log(`  ✗ ${name}`);
    console.log(`    ${msg}`);
  }
  process.exit(1);
} else if (failed.length) {
  console.log("\n(All failures are known bugs or missing builtins — see file comments)");
}
