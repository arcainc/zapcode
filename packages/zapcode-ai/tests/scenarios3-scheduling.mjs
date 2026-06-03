/**
 * Realistic scheduling/escalation E2E pass for agent-authored code.
 *
 * Run from packages/zapcode-ai:
 *   npm run build && node tests/scenarios3-scheduling.mjs
 */
import assert from "node:assert/strict";
import { execute, zapcode } from "../dist/index.js";

const NOW_MS = Date.parse("2026-06-01T12:00:00.000Z");
const WEEK_MS = 7 * 24 * 60 * 60 * 1000;

let passed = 0;

async function test(name, fn) {
  try {
    await fn();
    passed++;
    console.log(`  PASS ${name}`);
  } catch (error) {
    console.error(`  FAIL ${name}`);
    throw error;
  }
}

function createSchedulingTools(escalations) {
  return {
    getCurrentTimeMs: {
      description: "Return the deterministic current time as epoch milliseconds.",
      parameters: {},
      execute: async () => NOW_MS,
    },
    parseScheduleDate: {
      description:
        "Parse a user-provided scheduled date. Accepts YYYY-MM-DD or ISO strings with timezone offsets.",
      parameters: {
        date: { type: "string", description: "User-provided date or datetime." },
        timezone: { type: "string", optional: true, description: "User timezone hint." },
      },
      execute: async ({ date, timezone }) => {
        const normalized = /^\d{4}-\d{2}-\d{2}$/.test(date) ? `${date}T00:00:00.000Z` : date;
        const parsed = Date.parse(normalized);
        if (Number.isNaN(parsed)) {
          throw new Error(
            `invalid schedule date '${date}'${timezone ? ` for timezone '${timezone}'` : ""}`
          );
        }
        return parsed;
      },
    },
    recordEscalation: {
      description: "Record the escalation selected by scheduling logic.",
      parameters: {
        contact: { type: "string", description: "Escalation contact, either contact Y or contact X." },
        dueAtMs: { type: "number", description: "Scheduled time as epoch milliseconds." },
        reason: { type: "string", description: "Why this contact was selected." },
        priority: { type: "number", optional: true, description: "Priority from 1 high to 5 low." },
        notes: { type: "string", optional: true, description: "Free-form user notes." },
      },
      execute: async input => {
        escalations.push(input);
        return { ok: true, contact: input.contact };
      },
    },
  };
}

async function expectNoEscalationError(code, expectedMessage) {
  const escalations = [];
  await assert.rejects(
    () => execute(code, createSchedulingTools(escalations)),
    error => {
      assert.match(String(error.message), expectedMessage);
      return true;
    }
  );
  assert.deepEqual(escalations, []);
}

function routeCode(userDateExpression, extras = "") {
  return `
    const userDate = ${userDateExpression};
    const nowMs = await getCurrentTimeMs();
    const dueAtMs = await parseScheduleDate({ date: userDate, timezone: "America/New_York" });
    const withinWeek = dueAtMs >= nowMs && dueAtMs <= nowMs + ${WEEK_MS};
    const contact = withinWeek ? "contact Y" : "contact X";
    await recordEscalation({
      contact,
      dueAtMs,
      reason: withinWeek ? "scheduled within one week" : "scheduled outside one week",
      ${extras}
    });
    contact
  `;
}

console.log("scenarios3 scheduling e2e");

await test("system prompt exposes named-object scheduling signatures", async () => {
  const { system } = zapcode({ tools: createSchedulingTools([]) });
  assert.match(
    system,
    /declare function parseScheduleDate\(input: \{ date: string; timezone\?: string \}\): Promise<unknown>;/
  );
  assert.match(
    system,
    /declare function recordEscalation\(input: \{ contact: string; dueAtMs: number; reason: string; priority\?: number; notes\?: string \}\): Promise<unknown>;/
  );
  assert.match(system, /Call shape: await recordEscalation\(\{ contact: string, dueAtMs: number/);
});

await test("date within one week routes to escalation contact Y with notes and priority", async () => {
  const escalations = [];
  const result = await execute(
    routeCode(JSON.stringify("2026-06-05"), 'priority: 1,\n      notes: "VIP customer asked for same-week scheduling",'),
    createSchedulingTools(escalations)
  );

  assert.equal(result.output, "contact Y");
  assert.equal(escalations.length, 1);
  assert.equal(escalations[0].contact, "contact Y");
  assert.equal(escalations[0].dueAtMs, Date.parse("2026-06-05T00:00:00.000Z"));
  assert.equal(escalations[0].priority, 1);
  assert.equal(escalations[0].notes, "VIP customer asked for same-week scheduling");
  assert.deepEqual(result.toolCalls.at(-1).input, escalations[0]);
});

await test("date outside one week routes to escalation contact X with optional fields omitted", async () => {
  const escalations = [];
  const result = await execute(routeCode(JSON.stringify("2026-06-20")), createSchedulingTools(escalations));

  assert.equal(result.output, "contact X");
  assert.equal(escalations.length, 1);
  assert.equal(escalations[0].contact, "contact X");
  assert.equal(Object.hasOwn(escalations[0], "priority"), false);
  assert.equal(Object.hasOwn(escalations[0], "notes"), false);
});

await test("timezone-offset ISO string inside the one-week window routes to contact Y", async () => {
  const escalations = [];
  const result = await execute(
    routeCode(JSON.stringify("2026-06-08T00:30:00-04:00"), 'priority: 2,'),
    createSchedulingTools(escalations)
  );

  assert.equal(result.output, "contact Y");
  assert.equal(escalations[0].contact, "contact Y");
  assert.equal(escalations[0].dueAtMs, Date.parse("2026-06-08T00:30:00-04:00"));
});

await test("timezone-offset ISO string just beyond one week routes to contact X", async () => {
  const escalations = [];
  const result = await execute(
    routeCode(JSON.stringify("2026-06-08T12:00:01Z"), 'notes: "one second past the SLA window",'),
    createSchedulingTools(escalations)
  );

  assert.equal(result.output, "contact X");
  assert.equal(escalations[0].contact, "contact X");
  assert.equal(escalations[0].notes, "one second past the SLA window");
});

await test("boundary matrix covers past, now-day, seven-day, and eight-day routing", async () => {
  const cases = [
    ["2026-05-31", "contact X"],
    ["2026-06-01T12:00:00Z", "contact Y"],
    ["2026-06-08T12:00:00Z", "contact Y"],
    ["2026-06-09", "contact X"],
  ];

  for (const [userDate, expectedContact] of cases) {
    const escalations = [];
    const result = await execute(routeCode(JSON.stringify(userDate)), createSchedulingTools(escalations));
    assert.equal(result.output, expectedContact);
    assert.equal(escalations.length, 1);
    assert.equal(escalations[0].contact, expectedContact);
  }
});

await test("missing date argument produces a crisp validation error before host execution", async () => {
  await expectNoEscalationError(
    `await parseScheduleDate({ timezone: "America/New_York" })`,
    /Invalid arguments for tool 'parseScheduleDate': missing required parameter 'date'/
  );
});

await test("invalid date argument type produces a crisp validation error", async () => {
  await expectNoEscalationError(
    `await parseScheduleDate({ date: 20260605, timezone: "America/New_York" })`,
    /Invalid arguments for tool 'parseScheduleDate': parameter 'date' expected string, got number/
  );
});

await test("semantic invalid date string is surfaced as a catchable tool error", async () => {
  await expectNoEscalationError(
    `await parseScheduleDate({ date: "next Friday", timezone: "America/New_York" })`,
    /invalid schedule date 'next Friday' for timezone 'America\/New_York'/
  );
});

await test("missing dueAtMs on escalation produces a crisp validation error", async () => {
  await expectNoEscalationError(
    `
    await recordEscalation({
      contact: "contact Y",
      reason: "agent forgot to pass the parsed schedule",
      priority: 1,
    })
    `,
    /Invalid arguments for tool 'recordEscalation': missing required parameter 'dueAtMs'/
  );
});

await test("optional priority and notes fields reject wrong types", async () => {
  await expectNoEscalationError(
    `
    await recordEscalation({
      contact: "contact Y",
      dueAtMs: 1780660800000,
      reason: "priority as string",
      priority: "high",
    })
    `,
    /Invalid arguments for tool 'recordEscalation': parameter 'priority' expected number, got string/
  );

  await expectNoEscalationError(
    `
    await recordEscalation({
      contact: "contact Y",
      dueAtMs: 1780660800000,
      reason: "notes as object",
      notes: { text: "call customer first" },
    })
    `,
    /Invalid arguments for tool 'recordEscalation': parameter 'notes' expected string, got object/
  );
});

await test("positional multi-argument scheduling calls are rejected with the named-object guidance", async () => {
  await expectNoEscalationError(
    `await recordEscalation("contact Y", 1780660800000, "positional call")`,
    /Invalid arguments for tool 'recordEscalation': expected one named object argument/
  );
});

console.log(`\n${passed} scheduling checks passed.`);
