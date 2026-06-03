/**
 * Independent E2E pass for agent-authored CRM/support triage workflows.
 * Runs realistic TypeScript snippets through the zapcode-ai execute harness.
 */
import assert from "node:assert/strict";
import { execute } from "../dist/index.js";

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

console.log("support triage scenarios e2e");

const ACCOUNTS = {
  acct_enterprise: {
    accountId: "acct_enterprise",
    name: "Noble Bank",
    tier: "enterprise",
    ownerId: "owner_ava",
    health: "at_risk",
    openTickets: 7,
  },
  acct_growth: {
    accountId: "acct_growth",
    name: "Pine Labs",
    tier: "growth",
    ownerId: "owner_ben",
    health: "healthy",
    openTickets: 1,
  },
  acct_unowned: {
    accountId: "acct_unowned",
    name: "Quartz Health",
    tier: "enterprise",
    ownerId: null,
    health: "at_risk",
    openTickets: 3,
  },
  acct_broken: {
    accountId: "acct_broken",
    name: "Broken Lookup Co",
    tier: "enterprise",
    ownerId: "owner_cai",
    health: "at_risk",
    openTickets: 5,
  },
};

function createSupportTools(state = {}) {
  const lookups = [];
  const routed = [];
  const fallbacks = [];
  const tools = {
    batchLookupAccounts: {
      description: "Fetch CRM account records in one request.",
      parameters: {
        accountIds: { type: "array", description: "CRM account IDs to fetch." },
      },
      execute: async ({ accountIds }) => {
        lookups.push([...accountIds]);
        return accountIds.map(accountId => ACCOUNTS[accountId] ?? { accountId, tier: "unknown", ownerId: null, openTickets: 0 });
      },
    },
    getAccountOwner: {
      description: "Resolve the named owner for one account.",
      parameters: {
        accountId: { type: "string" },
      },
      execute: async ({ accountId }) => {
        if (state.failOwnerLookupFor === accountId) {
          throw new Error(`CRM owner lookup failed for ${accountId}`);
        }
        const account = ACCOUNTS[accountId];
        return account?.ownerId ? { ownerId: account.ownerId, displayName: account.ownerId.replace("owner_", "") } : null;
      },
    },
    routeTicket: {
      description: "Persist a triage routing decision.",
      parameters: {
        messageId: { type: "string" },
        accountId: { type: "string" },
        classification: { type: "string" },
        priority: { type: "number" },
        ownerId: { type: "string" },
        escalationPath: { type: "string" },
        reason: { type: "string" },
        metadata: { type: "object", optional: true },
      },
      execute: async input => {
        routed.push(input);
        return { routed: true, messageId: input.messageId, escalationPath: input.escalationPath };
      },
    },
    saveFallback: {
      description: "Save a structured manual-triage fallback when automation cannot route safely.",
      parameters: {
        messageId: { type: "string" },
        accountId: { type: "string" },
        status: { type: "string" },
        classification: { type: "string" },
        error: { type: "string" },
        recommendedQueue: { type: "string" },
        metadata: { type: "object", optional: true },
      },
      execute: async input => {
        fallbacks.push(input);
        return { saved: true, status: input.status };
      },
    },
  };

  return { tools, lookups, routed, fallbacks };
}

await test("classifies inbound messages and batches CRM account lookup before routing", async () => {
  const harness = createSupportTools();
  const result = await execute(
    `
    const messages = [
      {
        messageId: "msg_001",
        accountId: "acct_enterprise",
        subject: "checkout outage",
        body: "Our checkout is down and this is blocking all customer payments.",
      },
      {
        messageId: "msg_002",
        accountId: "acct_growth",
        subject: "refund status",
        body: "Can you help with a refund for invoice inv_992?",
      },
    ];

    const accounts = await batchLookupAccounts({
      accountIds: messages.map(message => message.accountId),
    });
    const accountById = new Map(accounts.map(account => [account.accountId, account]));

    const decisions = [];
    for (const message of messages) {
      const account = accountById.get(message.accountId);
      const text = \`\${message.subject} \${message.body}\`.toLowerCase();
      const classification = text.includes("outage") || text.includes("down")
        ? "technical_incident"
        : text.includes("refund")
          ? "billing_refund"
          : "general_support";
      const priority = classification === "technical_incident" && account.tier === "enterprise" ? 1 : 3;
      const owner = await getAccountOwner({ accountId: message.accountId });
      const escalationPath = priority === 1 ? "account-owner-and-oncall" : "billing-queue";

      const decision = {
        messageId: message.messageId,
        accountId: message.accountId,
        classification,
        priority,
        ownerId: owner.ownerId,
        escalationPath,
        reason: \`\${classification} for \${account.tier} account\`,
        metadata: { accountHealth: account.health, openTickets: account.openTickets },
      };
      await routeTicket(decision);
      decisions.push(decision);
    }

    decisions
    `,
    harness.tools
  );

  assert.equal(harness.lookups.length, 1);
  assert.deepEqual(harness.lookups[0], ["acct_enterprise", "acct_growth"]);
  assert.equal(harness.routed.length, 2);
  assert.equal(harness.routed[0].classification, "technical_incident");
  assert.equal(harness.routed[0].priority, 1);
  assert.equal(harness.routed[0].ownerId, "owner_ava");
  assert.equal(harness.routed[0].escalationPath, "account-owner-and-oncall");
  assert.equal(harness.routed[1].classification, "billing_refund");
  assert.equal(harness.routed[1].priority, 3);
  assert.equal(result.toolCalls.filter(call => call.name === "batchLookupAccounts").length, 1);
  assert.deepEqual(result.toolCalls.at(-1).input, harness.routed[1]);
});

await test("validates required routing arguments before host side effects", async () => {
  const harness = createSupportTools();

  await assert.rejects(
    () =>
      execute(
        `
        await routeTicket({
          messageId: "msg_missing_owner",
          accountId: "acct_enterprise",
          classification: "technical_incident",
          priority: 1,
          escalationPath: "account-owner-and-oncall",
          reason: "missing ownerId should fail before persistence",
        })
        `,
        harness.tools
      ),
    /Invalid arguments for tool 'routeTicket': missing required parameter 'ownerId'/
  );

  assert.deepEqual(harness.routed, []);
});

await test("chooses owner escalation for enterprise incident and manager fallback for unowned accounts", async () => {
  const harness = createSupportTools();
  const result = await execute(
    `
    const cases = [
      { messageId: "msg_owned", accountId: "acct_enterprise", body: "production api outage for our users" },
      { messageId: "msg_unowned", accountId: "acct_unowned", body: "security review is blocking go live" },
    ];
    const accounts = await batchLookupAccounts({ accountIds: cases.map(item => item.accountId) });
    const accountById = new Map(accounts.map(account => [account.accountId, account]));

    const paths = [];
    for (const item of cases) {
      const account = accountById.get(item.accountId);
      const owner = await getAccountOwner({ accountId: item.accountId });
      const classification = item.body.includes("security") ? "security_review" : "technical_incident";
      const ownerId = owner ? owner.ownerId : "manager_pool";
      const escalationPath = owner
        ? "named-account-owner"
        : account.tier === "enterprise"
          ? "support-manager"
          : "standard-support";
      await routeTicket({
        messageId: item.messageId,
        accountId: item.accountId,
        classification,
        priority: account.tier === "enterprise" ? 1 : 3,
        ownerId,
        escalationPath,
        reason: owner ? "route to named owner" : "enterprise account has no active owner",
        metadata: { accountTier: account.tier },
      });
      paths.push(escalationPath);
    }

    paths
    `,
    harness.tools
  );

  assert.deepEqual(result.output, ["named-account-owner", "support-manager"]);
  assert.equal(harness.routed[0].ownerId, "owner_ava");
  assert.equal(harness.routed[1].ownerId, "manager_pool");
  assert.equal(harness.routed[1].classification, "security_review");
});

await test("catches failed tool calls and returns a structured manual-triage fallback", async () => {
  const harness = createSupportTools({ failOwnerLookupFor: "acct_broken" });
  const result = await execute(
    `
    const message = {
      messageId: "msg_broken",
      accountId: "acct_broken",
      body: "major outage, nobody can log in",
    };
    const classification = message.body.includes("outage") ? "technical_incident" : "general_support";
    let outcome;

    try {
      const owner = await getAccountOwner({ accountId: message.accountId });
      await routeTicket({
        messageId: message.messageId,
        accountId: message.accountId,
        classification,
        priority: 1,
        ownerId: owner.ownerId,
        escalationPath: "named-account-owner",
        reason: "owner lookup succeeded",
      });
      outcome = { status: "routed" };
    } catch (error) {
      const fallback = {
        messageId: message.messageId,
        accountId: message.accountId,
        status: "needs_manual_triage",
        classification,
        error: error instanceof Error ? error.message : String(error),
        recommendedQueue: "enterprise-support-manager",
        metadata: { failedStep: "getAccountOwner" },
      };
      await saveFallback(fallback);
      outcome = fallback;
    }
    outcome
    `,
    harness.tools
  );

  assert.deepEqual(harness.routed, []);
  assert.equal(harness.fallbacks.length, 1);
  assert.equal(harness.fallbacks[0].status, "needs_manual_triage");
  assert.equal(harness.fallbacks[0].classification, "technical_incident");
  assert.match(harness.fallbacks[0].error, /CRM owner lookup failed for acct_broken/);
  assert.equal(result.output.recommendedQueue, "enterprise-support-manager");
  const failedOwnerCall = result.toolCalls.find(call => call.name === "getAccountOwner");
  assert.match(failedOwnerCall.error, /CRM owner lookup failed for acct_broken/);
  assert.deepEqual(result.toolCalls.at(-1).input, harness.fallbacks[0]);
});

await test("optional chaining and nullish coalescing work inside tool argument objects", async () => {
  const harness = createSupportTools();
  const result = await execute(
    `
    const cases = [
      { messageId: "msg_owned", accountId: "acct_enterprise" },
      { messageId: "msg_unowned", accountId: "acct_unowned" },
    ];
    for (const item of cases) {
      const owner = await getAccountOwner({ accountId: item.accountId });
      await routeTicket({
        messageId: item.messageId,
        accountId: item.accountId,
        classification: "technical_incident",
        priority: 1,
        ownerId: owner?.ownerId ?? "manager_pool",
        escalationPath: owner?.ownerId ? "named-account-owner" : "support-manager",
        reason: "direct optional chain in tool argument object",
      });
    }
    true
    `,
    harness.tools
  );

  assert.equal(result.output, true);
  assert.equal(harness.routed.length, 2);
  assert.equal(harness.routed[0].ownerId, "owner_ava");
  assert.equal(harness.routed[0].escalationPath, "named-account-owner");
  assert.equal(harness.routed[1].ownerId, "manager_pool");
  assert.equal(harness.routed[1].escalationPath, "support-manager");
  assert.deepEqual(result.toolCalls.at(-1).input, harness.routed[1]);
});

console.log(`\n${passed} support triage checks passed.`);
