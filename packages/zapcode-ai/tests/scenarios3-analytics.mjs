/**
 * Independent E2E pass for an agent-authored product/usage ANALYTICS & REPORTING
 * workflow over deeply-nested, partially-missing event/account records.
 *
 * REGRESSION FOCUS: optional member/index access (a?.b?.c, a?.[k], a?.b?.[i],
 * mixed a?.b.c?.[i]) combined with nullish coalescing (??) used DIRECTLY as
 * tool-call argument values and as values inside object literals passed to tools.
 * This is exactly the compiler stack-leak class fixed in this PR (an expression
 * like `owner?.ownerId ?? "x"` used to corrupt the surrounding object shape).
 *
 * Every tool-call `.input` shape is asserted to be EXACTLY correct so a leaked
 * `null`/`[object Object]` key or a corrupted shape fails loudly.
 *
 * Run from packages/zapcode-ai:
 *   node tests/scenarios3-analytics.mjs
 */
import assert from "node:assert/strict";
import { execute, zapcode } from "../dist/index.js";

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

console.log("scenarios3 analytics e2e");

// ────────────────────────────────────────────────────────────────────────────
// Heterogeneously-shaped, deeply-nested, partially-missing records.
// Some are fully populated; some have null/undefined/missing intermediate fields.
// ────────────────────────────────────────────────────────────────────────────
const EVENT_RECORDS = [
  {
    eventId: "ev_001",
    account: { accountId: "acct_full", plan: "enterprise", owner: { ownerId: "owner_ava", contact: { email: "ava@x.io" } } },
    geo: [{ region: "us-east", city: "nyc" }, { region: "us-west" }],
    metrics: { events: 120, sessions: 14, errors: { count: 2 } },
    feature: { flags: { beta: true } },
  },
  {
    // owner missing entirely; geo present but empty region on [0]; metrics partial
    eventId: "ev_002",
    account: { accountId: "acct_noowner", plan: "growth", owner: null },
    geo: [{ city: "sf" }],
    metrics: { events: 30 },
  },
  {
    // account null; geo undefined; metrics undefined
    eventId: "ev_003",
    account: null,
  },
  {
    // deeply nested present, owner present but no contact
    eventId: "ev_004",
    account: { accountId: "acct_nocontact", plan: "enterprise", owner: { ownerId: "owner_ben" } },
    geo: [],
    metrics: { events: 0, sessions: 5, errors: { count: 0 } },
    feature: { flags: {} },
  },
  {
    // missing account key altogether; only metrics
    eventId: "ev_005",
    metrics: { events: 9, sessions: 1 },
    geo: [{ region: "eu-central", city: "berlin" }, { region: "eu-west", city: "dublin" }],
  },
];

function createAnalyticsTools(state = {}) {
  const fetched = [];
  const saved = [];
  const reports = [];
  const tools = {
    fetchEvents: {
      description: "Fetch raw, partially-shaped usage event records.",
      parameters: {
        window: { type: "string", optional: true, description: "Time window label." },
      },
      execute: async ({ window }) => {
        fetched.push(window ?? "default");
        return JSON.parse(JSON.stringify(EVENT_RECORDS));
      },
    },
    saveRollup: {
      description: "Persist one rolled-up per-account analytics row.",
      parameters: {
        eventId: { type: "string" },
        accountId: { type: "string" },
        plan: { type: "string" },
        ownerId: { type: "string" },
        ownerEmail: { type: "string" },
        primaryRegion: { type: "string" },
        events: { type: "number" },
        sessions: { type: "number" },
        errorCount: { type: "number" },
        betaFlag: { type: "boolean" },
        metadata: { type: "object", optional: true },
      },
      execute: async input => {
        saved.push(input);
        return { ok: true, accountId: input.accountId };
      },
    },
    publishReport: {
      description: "Publish the final aggregated analytics report.",
      parameters: {
        title: { type: "string" },
        totals: { type: "object" },
        byPlan: { type: "object" },
        regions: { type: "array" },
        summary: { type: "string" },
      },
      execute: async input => {
        reports.push(input);
        return { published: true, title: input.title };
      },
    },
  };
  return { tools, fetched, saved, reports };
}

// ────────────────────────────────────────────────────────────────────────────
// 1. System-prompt assertion.
// ────────────────────────────────────────────────────────────────────────────
await test("system prompt exposes named-object analytics tool signatures", async () => {
  const { system } = zapcode({ tools: createAnalyticsTools().tools });
  assert.match(system, /declare function fetchEvents\(window\?: string\): Promise<unknown>;/);
  assert.match(
    system,
    /declare function saveRollup\(input: \{ eventId: string; accountId: string; plan: string; ownerId: string; ownerEmail: string; primaryRegion: string; events: number; sessions: number; errorCount: number; betaFlag: boolean; metadata\?: object \}\): Promise<unknown>;/
  );
  assert.match(system, /Call shape: await publishReport\(\{ title: string, totals: object/);
});

// ────────────────────────────────────────────────────────────────────────────
// 2. Optional chain + ?? DIRECTLY as tool-call argument values (single record,
//    fully-populated branch). Asserts the PRESENT branch of every optional chain.
// ────────────────────────────────────────────────────────────────────────────
await test("deep optional chains + ?? as direct tool args resolve the PRESENT branch with exact shape", async () => {
  const harness = createAnalyticsTools();
  const result = await execute(
    `
    const events = await fetchEvents({ window: "7d" });
    const rec = events[0];
    await saveRollup({
      eventId: rec?.eventId ?? "unknown",
      accountId: rec?.account?.accountId ?? "none",
      plan: rec?.account?.plan ?? "free",
      ownerId: rec?.account?.owner?.ownerId ?? "unowned",
      ownerEmail: rec?.account?.owner?.contact?.email ?? "no-email",
      primaryRegion: rec?.geo?.[0]?.region ?? "unknown",
      events: rec?.metrics?.events ?? 0,
      sessions: rec?.metrics?.sessions ?? 0,
      errorCount: rec?.metrics?.errors?.count ?? 0,
      betaFlag: rec?.feature?.flags?.beta ?? false,
      metadata: { secondRegion: rec?.geo?.[1]?.region ?? "n/a", lastRegion: rec?.geo?.[(rec?.geo?.length ?? 0) - 1]?.region ?? "n/a" },
    });
    "done"
    `,
    harness.tools
  );

  assert.equal(result.output, "done");
  assert.equal(harness.saved.length, 1);
  const expected = {
    eventId: "ev_001",
    accountId: "acct_full",
    plan: "enterprise",
    ownerId: "owner_ava",
    ownerEmail: "ava@x.io",
    primaryRegion: "us-east",
    events: 120,
    sessions: 14,
    errorCount: 2,
    betaFlag: true,
    metadata: { secondRegion: "us-west", lastRegion: "us-west" },
  };
  assert.deepEqual(harness.saved[0], expected);
  // Exact tool-call input shape — guards against leaked keys / corrupted shape.
  assert.deepEqual(result.toolCalls.at(-1).input, expected);
  // Exact key set (order-independent) — guards against leaked/missing keys.
  assert.deepEqual(Object.keys(result.toolCalls.at(-1).input).sort(), Object.keys(expected).sort());
});

// ────────────────────────────────────────────────────────────────────────────
// 3. Same arg shape but a DEEPLY-NULL record — asserts the NULL branch of every
//    optional chain, and that no stray null / [object Object] keys leaked.
// ────────────────────────────────────────────────────────────────────────────
await test("deep optional chains + ?? as direct tool args resolve the NULL branch with exact shape", async () => {
  const harness = createAnalyticsTools();
  const result = await execute(
    `
    const events = await fetchEvents({ window: "7d" });
    const rec = events[2]; // ev_003: account null, no geo/metrics/feature
    await saveRollup({
      eventId: rec?.eventId ?? "unknown",
      accountId: rec?.account?.accountId ?? "none",
      plan: rec?.account?.plan ?? "free",
      ownerId: rec?.account?.owner?.ownerId ?? "unowned",
      ownerEmail: rec?.account?.owner?.contact?.email ?? "no-email",
      primaryRegion: rec?.geo?.[0]?.region ?? "unknown",
      events: rec?.metrics?.events ?? 0,
      sessions: rec?.metrics?.sessions ?? 0,
      errorCount: rec?.metrics?.errors?.count ?? 0,
      betaFlag: rec?.feature?.flags?.beta ?? false,
      metadata: { secondRegion: rec?.geo?.[1]?.region ?? "n/a", lastRegion: rec?.geo?.[(rec?.geo?.length ?? 0) - 1]?.region ?? "n/a" },
    });
    "done"
    `,
    harness.tools
  );

  assert.equal(result.output, "done");
  const expected = {
    eventId: "ev_003",
    accountId: "none",
    plan: "free",
    ownerId: "unowned",
    ownerEmail: "no-email",
    primaryRegion: "unknown",
    events: 0,
    sessions: 0,
    errorCount: 0,
    betaFlag: false,
    metadata: { secondRegion: "n/a", lastRegion: "n/a" },
  };
  assert.deepEqual(harness.saved[0], expected);
  assert.deepEqual(result.toolCalls.at(-1).input, expected);
  // No leaked keys, and no value is `null` or the literal "[object Object]".
  for (const [k, v] of Object.entries(result.toolCalls.at(-1).input)) {
    assert.notEqual(v, null, `key ${k} leaked null`);
    assert.notEqual(v, "[object Object]", `key ${k} leaked stringified object`);
  }
});

// ────────────────────────────────────────────────────────────────────────────
// 4. Mixed optional/non-optional links (a?.b.c?.[i]) + ternary gated on chains +
//    object spread merging defaults with optional values.
// ────────────────────────────────────────────────────────────────────────────
await test("mixed optional links, spread defaults, and ternaries gated on optional chains", async () => {
  const harness = createAnalyticsTools();
  const result = await execute(
    `
    const events = await fetchEvents({ window: "all" });
    const rec = events[1]; // ev_002: owner null, geo[0] has no region
    const defaults = { ownerId: "unowned", ownerEmail: "no-email", primaryRegion: "unknown", betaFlag: false };
    const resolved = {
      ...defaults,
      eventId: rec.eventId,
      accountId: rec?.account?.accountId ?? "none",
      plan: rec?.account?.plan ?? "free",
      // mixed: account is present, .owner optional, ownerId via optional chain
      ownerId: rec?.account?.owner?.ownerId ?? defaults.ownerId,
      // a?.b.c?.[i]: geo present (non-optional .length implied), first region missing -> fallback
      primaryRegion: rec?.geo?.[0]?.region ?? defaults.primaryRegion,
      events: rec?.metrics?.events ?? 0,
      sessions: rec?.metrics?.sessions ?? 0,
      errorCount: rec?.metrics?.errors?.count ?? 0,
      // ternary gated on optional chain presence
      betaFlag: rec?.feature?.flags?.beta ? true : false,
      metadata: { hasOwner: rec?.account?.owner?.ownerId ? "yes" : "no", city: rec?.geo?.[0]?.city ?? "n/a" },
    };
    await saveRollup(resolved);
    resolved
    `,
    harness.tools
  );

  const expected = {
    ownerId: "unowned",
    ownerEmail: "no-email",
    primaryRegion: "unknown",
    betaFlag: false,
    eventId: "ev_002",
    accountId: "acct_noowner",
    plan: "growth",
    events: 30,
    sessions: 0,
    errorCount: 0,
    metadata: { hasOwner: "no", city: "sf" },
  };
  assert.deepEqual(result.output, expected);
  assert.deepEqual(harness.saved[0], expected);
  assert.deepEqual(result.toolCalls.at(-1).input, expected);
});

// ────────────────────────────────────────────────────────────────────────────
// 5. Optional chains nested inside template literals used as a string tool arg.
// ────────────────────────────────────────────────────────────────────────────
await test("optional chains inside template literals used as a string tool arg", async () => {
  const harness = createAnalyticsTools();
  const result = await execute(
    `
    const events = await fetchEvents({ window: "all" });
    const rec = events[3]; // ev_004: owner present, no contact, geo empty
    const summaryStr = \`\${rec?.account?.accountId ?? "none"}/\${rec?.account?.owner?.ownerId ?? "unowned"}/\${rec?.account?.owner?.contact?.email ?? "no-email"}/\${rec?.geo?.[0]?.region ?? "no-region"}\`;
    await saveRollup({
      eventId: rec?.eventId ?? "unknown",
      accountId: rec?.account?.accountId ?? "none",
      plan: rec?.account?.plan ?? "free",
      ownerId: rec?.account?.owner?.ownerId ?? "unowned",
      ownerEmail: rec?.account?.owner?.contact?.email ?? "no-email",
      primaryRegion: rec?.geo?.[0]?.region ?? "no-region",
      events: rec?.metrics?.events ?? 0,
      sessions: rec?.metrics?.sessions ?? 0,
      errorCount: rec?.metrics?.errors?.count ?? 0,
      betaFlag: rec?.feature?.flags?.beta ?? false,
      metadata: { label: summaryStr },
    });
    summaryStr
    `,
    harness.tools
  );

  assert.equal(result.output, "acct_nocontact/owner_ben/no-email/no-region");
  const saved = harness.saved[0];
  assert.equal(saved.ownerId, "owner_ben");
  assert.equal(saved.ownerEmail, "no-email");
  assert.equal(saved.primaryRegion, "no-region");
  assert.equal(saved.events, 0);
  assert.equal(saved.errorCount, 0);
  assert.equal(saved.betaFlag, false);
  assert.deepEqual(saved.metadata, { label: "acct_nocontact/owner_ben/no-email/no-region" });
  assert.deepEqual(result.toolCalls.at(-1).input, saved);
});

// ────────────────────────────────────────────────────────────────────────────
// 6. Computed-key index access + ?? as direct tool args (a?.[k]).
// ────────────────────────────────────────────────────────────────────────────
await test("computed-key optional index access a?.[k] as direct tool args", async () => {
  const harness = createAnalyticsTools();
  const result = await execute(
    `
    const events = await fetchEvents({ window: "all" });
    const rec = events[0];
    const accountKey = "accountId";
    const metricKey = "events";
    const missingKey = "nope";
    await saveRollup({
      eventId: rec?.eventId ?? "unknown",
      accountId: rec?.account?.[accountKey] ?? "none",
      plan: rec?.account?.["plan"] ?? "free",
      ownerId: rec?.account?.owner?.["ownerId"] ?? "unowned",
      ownerEmail: rec?.account?.owner?.contact?.["email"] ?? "no-email",
      primaryRegion: rec?.geo?.[0]?.["region"] ?? "unknown",
      events: rec?.metrics?.[metricKey] ?? 0,
      sessions: rec?.metrics?.[missingKey] ?? -1,
      errorCount: rec?.metrics?.errors?.["count"] ?? 0,
      betaFlag: rec?.feature?.flags?.["beta"] ?? false,
    });
    "ok"
    `,
    harness.tools
  );

  const expected = {
    eventId: "ev_001",
    accountId: "acct_full",
    plan: "enterprise",
    ownerId: "owner_ava",
    ownerEmail: "ava@x.io",
    primaryRegion: "us-east",
    events: 120,
    sessions: -1, // missingKey -> undefined -> ?? -1
    errorCount: 2,
    betaFlag: true,
  };
  assert.deepEqual(harness.saved[0], expected);
  assert.deepEqual(result.toolCalls.at(-1).input, expected);
});

// ────────────────────────────────────────────────────────────────────────────
// 7. STRESS: roll up the whole heterogeneous array with optional chains in args,
//    asserting one saveRollup per record with EXACT shapes for all 5.
// ────────────────────────────────────────────────────────────────────────────
await test("STRESS: per-record rollup over heterogeneous array — exact shape for every saveRollup", async () => {
  const harness = createAnalyticsTools();
  const result = await execute(
    `
    const events = await fetchEvents({ window: "30d" });
    const rows = [];
    for (const rec of events) {
      const row = {
        eventId: rec?.eventId ?? "unknown",
        accountId: rec?.account?.accountId ?? "none",
        plan: rec?.account?.plan ?? "free",
        ownerId: rec?.account?.owner?.ownerId ?? "unowned",
        ownerEmail: rec?.account?.owner?.contact?.email ?? "no-email",
        primaryRegion: rec?.geo?.[0]?.region ?? "unknown",
        events: rec?.metrics?.events ?? 0,
        sessions: rec?.metrics?.sessions ?? 0,
        errorCount: rec?.metrics?.errors?.count ?? 0,
        betaFlag: rec?.feature?.flags?.beta ?? false,
      };
      await saveRollup(row);
      rows.push(row);
    }
    rows
    `,
    harness.tools
  );

  const expectedRows = [
    { eventId: "ev_001", accountId: "acct_full", plan: "enterprise", ownerId: "owner_ava", ownerEmail: "ava@x.io", primaryRegion: "us-east", events: 120, sessions: 14, errorCount: 2, betaFlag: true },
    { eventId: "ev_002", accountId: "acct_noowner", plan: "growth", ownerId: "unowned", ownerEmail: "no-email", primaryRegion: "unknown", events: 30, sessions: 0, errorCount: 0, betaFlag: false },
    { eventId: "ev_003", accountId: "none", plan: "free", ownerId: "unowned", ownerEmail: "no-email", primaryRegion: "unknown", events: 0, sessions: 0, errorCount: 0, betaFlag: false },
    { eventId: "ev_004", accountId: "acct_nocontact", plan: "enterprise", ownerId: "owner_ben", ownerEmail: "no-email", primaryRegion: "unknown", events: 0, sessions: 5, errorCount: 0, betaFlag: false },
    { eventId: "ev_005", accountId: "none", plan: "free", ownerId: "unowned", ownerEmail: "no-email", primaryRegion: "eu-central", events: 9, sessions: 1, errorCount: 0, betaFlag: false },
  ];
  assert.deepEqual(result.output, expectedRows);
  assert.equal(harness.saved.length, 5);
  assert.deepEqual(harness.saved, expectedRows);
  const rollupCalls = result.toolCalls.filter(c => c.name === "saveRollup");
  assert.equal(rollupCalls.length, 5);
  for (let i = 0; i < expectedRows.length; i++) {
    assert.deepEqual(rollupCalls[i].input, expectedRows[i]);
  }
});

// ────────────────────────────────────────────────────────────────────────────
// 8. STRESS aggregate: Object.entries/map/filter/reduce roll-up into ONE report,
//    with optional chains feeding the aggregation, asserted exactly.
// ────────────────────────────────────────────────────────────────────────────
await test("STRESS: aggregate report via map/filter/reduce with optional chains feeding totals", async () => {
  const harness = createAnalyticsTools();
  const result = await execute(
    `
    const events = await fetchEvents({ window: "30d" });
    const totalEvents = events.reduce((acc, rec) => acc + (rec?.metrics?.events ?? 0), 0);
    const totalSessions = events.reduce((acc, rec) => acc + (rec?.metrics?.sessions ?? 0), 0);
    const totalErrors = events.reduce((acc, rec) => acc + (rec?.metrics?.errors?.count ?? 0), 0);
    const ownedCount = events.filter(rec => (rec?.account?.owner?.ownerId ?? null) !== null).length;

    const byPlan = {};
    for (const rec of events) {
      const plan = rec?.account?.plan ?? "free";
      byPlan[plan] = (byPlan[plan] ?? 0) + (rec?.metrics?.events ?? 0);
    }

    // all primary regions across records (first region per record), filter missing
    const regions = events
      .map(rec => rec?.geo?.[0]?.region ?? null)
      .filter(r => r !== null);

    const totals = { events: totalEvents, sessions: totalSessions, errors: totalErrors, ownedAccounts: ownedCount, records: events.length };
    const summary = \`\${events.length} records, \${totalEvents} events across \${Object.keys(byPlan).length} plans\`;
    await publishReport({ title: "Usage 30d", totals, byPlan, regions, summary });
    ({ totals, byPlan, regions, summary })
    `,
    harness.tools
  );

  const expectedTotals = { events: 159, sessions: 20, errors: 2, ownedAccounts: 2, records: 5 };
  const expectedByPlan = { enterprise: 120, growth: 30, free: 9 };
  const expectedRegions = ["us-east", "eu-central"];
  const expectedSummary = "5 records, 159 events across 3 plans";

  assert.deepEqual(result.output.totals, expectedTotals);
  assert.deepEqual(result.output.byPlan, expectedByPlan);
  assert.deepEqual(result.output.regions, expectedRegions);
  assert.equal(result.output.summary, expectedSummary);
  assert.equal(harness.reports.length, 1);
  assert.deepEqual(harness.reports[0], {
    title: "Usage 30d",
    totals: expectedTotals,
    byPlan: expectedByPlan,
    regions: expectedRegions,
    summary: expectedSummary,
  });
  assert.deepEqual(result.toolCalls.at(-1).input, harness.reports[0]);
});

// ────────────────────────────────────────────────────────────────────────────
// 9. logical &&/|| mixing with optional chains as a direct boolean tool arg.
// ────────────────────────────────────────────────────────────────────────────
await test("logical &&/|| mixed with optional chains as direct tool args", async () => {
  const harness = createAnalyticsTools();
  const result = await execute(
    `
    const events = await fetchEvents({ window: "all" });
    const a = events[0]; // beta true, has owner
    const b = events[1]; // no beta, no owner
    // betaFlag via && short-circuit on optional chain; ownerId via || fallback
    await saveRollup({
      eventId: a?.eventId ?? "unknown",
      accountId: a?.account?.accountId ?? "none",
      plan: a?.account?.plan ?? "free",
      ownerId: (a?.account?.owner?.ownerId || "unowned"),
      ownerEmail: a?.account?.owner?.contact?.email ?? "no-email",
      primaryRegion: a?.geo?.[0]?.region ?? "unknown",
      events: a?.metrics?.events ?? 0,
      sessions: a?.metrics?.sessions ?? 0,
      errorCount: a?.metrics?.errors?.count ?? 0,
      betaFlag: (a?.feature?.flags?.beta && a?.metrics?.errors?.count < 5) ? true : false,
    });
    await saveRollup({
      eventId: b?.eventId ?? "unknown",
      accountId: b?.account?.accountId ?? "none",
      plan: b?.account?.plan ?? "free",
      ownerId: (b?.account?.owner?.ownerId || "unowned"),
      ownerEmail: b?.account?.owner?.contact?.email ?? "no-email",
      primaryRegion: b?.geo?.[0]?.region ?? "unknown",
      events: b?.metrics?.events ?? 0,
      sessions: b?.metrics?.sessions ?? 0,
      errorCount: b?.metrics?.errors?.count ?? 0,
      betaFlag: (b?.feature?.flags?.beta && true) ? true : false,
    });
    "ok"
    `,
    harness.tools
  );

  assert.equal(harness.saved.length, 2);
  assert.equal(harness.saved[0].ownerId, "owner_ava");
  assert.equal(harness.saved[0].betaFlag, true);
  assert.equal(harness.saved[1].ownerId, "unowned");
  assert.equal(harness.saved[1].betaFlag, false);
  assert.deepEqual(result.toolCalls.at(-1).input, harness.saved[1]);
});

// ────────────────────────────────────────────────────────────────────────────
// 10 & 11. Invalid-argument validation — rejection BEFORE host side effects.
// ────────────────────────────────────────────────────────────────────────────
await test("invalid arg: optional chain producing wrong type is rejected before side effects", async () => {
  const harness = createAnalyticsTools();
  // events resolves to a number for ev_001, but we mis-route it into `accountId`
  // (string param). The ?? fallback never fires because events is present (120).
  await assert.rejects(
    () =>
      execute(
        `
        const events = await fetchEvents({ window: "all" });
        const rec = events[0];
        await saveRollup({
          eventId: rec?.eventId ?? "unknown",
          accountId: rec?.metrics?.events ?? 0,
          plan: rec?.account?.plan ?? "free",
          ownerId: rec?.account?.owner?.ownerId ?? "unowned",
          ownerEmail: rec?.account?.owner?.contact?.email ?? "no-email",
          primaryRegion: rec?.geo?.[0]?.region ?? "unknown",
          events: rec?.metrics?.events ?? 0,
          sessions: rec?.metrics?.sessions ?? 0,
          errorCount: rec?.metrics?.errors?.count ?? 0,
          betaFlag: rec?.feature?.flags?.beta ?? false,
        })
        `,
        harness.tools
      ),
    /Invalid arguments for tool 'saveRollup': parameter 'accountId' expected string, got number/
  );
  assert.deepEqual(harness.saved, []);
});

await test("invalid arg: missing required param (null branch left unguarded) rejected before side effects", async () => {
  const harness = createAnalyticsTools();
  // ev_003 has null account; we omit ownerId entirely (forgot the ?? fallback).
  await assert.rejects(
    () =>
      execute(
        `
        const events = await fetchEvents({ window: "all" });
        const rec = events[2];
        await saveRollup({
          eventId: rec?.eventId ?? "unknown",
          accountId: rec?.account?.accountId ?? "none",
          plan: rec?.account?.plan ?? "free",
          ownerEmail: rec?.account?.owner?.contact?.email ?? "no-email",
          primaryRegion: rec?.geo?.[0]?.region ?? "unknown",
          events: rec?.metrics?.events ?? 0,
          sessions: rec?.metrics?.sessions ?? 0,
          errorCount: rec?.metrics?.errors?.count ?? 0,
          betaFlag: rec?.feature?.flags?.beta ?? false,
        })
        `,
        harness.tools
      ),
    /Invalid arguments for tool 'saveRollup': missing required parameter 'ownerId'/
  );
  assert.deepEqual(harness.saved, []);
});

// ────────────────────────────────────────────────────────────────────────────
// 12. Determinism: identical program -> identical outputs and tool inputs.
// ────────────────────────────────────────────────────────────────────────────
await test("determinism: repeated identical run yields identical rollups and report", async () => {
  const PROGRAM = `
    const events = await fetchEvents({ window: "30d" });
    const rows = events.map(rec => ({
      eventId: rec?.eventId ?? "unknown",
      accountId: rec?.account?.accountId ?? "none",
      ownerId: rec?.account?.owner?.ownerId ?? "unowned",
      region: rec?.geo?.[(rec?.geo?.length ?? 0) - 1]?.region ?? "unknown",
      events: rec?.metrics?.events ?? 0,
    }));
    const total = rows.reduce((acc, r) => acc + r.events, 0);
    ({ rows, total })
  `;
  const a = await execute(PROGRAM, createAnalyticsTools().tools);
  const b = await execute(PROGRAM, createAnalyticsTools().tools);
  assert.deepEqual(a.output, b.output);
  assert.deepEqual(a.output, {
    rows: [
      { eventId: "ev_001", accountId: "acct_full", ownerId: "owner_ava", region: "us-west", events: 120 },
      { eventId: "ev_002", accountId: "acct_noowner", ownerId: "unowned", region: "unknown", events: 30 },
      { eventId: "ev_003", accountId: "none", ownerId: "unowned", region: "unknown", events: 0 },
      { eventId: "ev_004", accountId: "acct_nocontact", ownerId: "owner_ben", region: "unknown", events: 0 },
      { eventId: "ev_005", accountId: "none", ownerId: "unowned", region: "eu-west", events: 9 },
    ],
    total: 159,
  });
});

console.log(`\n${passed} analytics checks passed.`);
