import assert from "node:assert/strict";
import { execute } from "../dist/index.js";

const NOW_MS = Date.UTC(2026, 4, 31);

function createRoutingTools(escalations) {
  return {
    getNowMs: {
      description: "Return the current time as epoch milliseconds.",
      parameters: {},
      execute: async () => NOW_MS,
    },
    parseDateMs: {
      description: "Parse an ISO date string and return epoch milliseconds.",
      parameters: {
        date: { type: "string", description: "ISO date string, such as 2026-06-03" },
      },
      execute: async ({ date }) => {
        const parsed = Date.parse(`${date}T00:00:00.000Z`);
        if (Number.isNaN(parsed)) {
          throw new Error(`invalid ISO date '${date}'`);
        }
        return parsed;
      },
    },
    escalateTo: {
      description: "Escalate the request to an assignee.",
      parameters: {
        assignee: { type: "string", description: "The assignee name." },
        dueAtMs: { type: "number", description: "Due date as epoch milliseconds." },
        reason: { type: "string", description: "Why this assignee was chosen." },
        metadata: { type: "object", optional: true },
      },
      execute: async input => {
        escalations.push(input);
        return { ok: true, assignee: input.assignee };
      },
    },
  };
}

async function expectExecutionError(code, expectedMessage) {
  const escalations = [];
  await assert.rejects(
    () => execute(code, createRoutingTools(escalations)),
    error => {
      assert.match(String(error.message), expectedMessage);
      return true;
    }
  );
  assert.deepEqual(escalations, []);
}

{
  const escalations = [];
  const result = await execute(
    `
    const userDate = "2026-06-03";
    const now = await getNowMs();
    const dueAtMs = await parseDateMs({ date: userDate });
    const weekMs = 7 * 24 * 60 * 60 * 1000;
    const assignee = dueAtMs >= now && dueAtMs <= now + weekMs ? "guy Y" : "guy X";
    await escalateTo({
      assignee,
      dueAtMs,
      reason: "User date is within one week from now",
      metadata: { userDate },
    });
    assignee
    `,
    createRoutingTools(escalations)
  );

  assert.equal(result.output, "guy Y");
  assert.equal(escalations.length, 1);
  assert.equal(escalations[0].assignee, "guy Y");
  assert.equal(escalations[0].dueAtMs, Date.UTC(2026, 5, 3));
  assert.deepEqual(result.toolCalls.at(-1).input, escalations[0]);
}

{
  const escalations = [];
  const result = await execute(
    `
    const userDate = "2026-06-20";
    const now = await getNowMs();
    const dueAtMs = await parseDateMs({ date: userDate });
    const weekMs = 7 * 24 * 60 * 60 * 1000;
    const assignee = dueAtMs >= now && dueAtMs <= now + weekMs ? "guy Y" : "guy X";
    await escalateTo({ assignee, dueAtMs, reason: "User date is not within one week from now" });
    assignee
    `,
    createRoutingTools(escalations)
  );

  assert.equal(result.output, "guy X");
  assert.equal(escalations[0].assignee, "guy X");
}

await expectExecutionError(
  `
  await escalateTo({
    assignee: "guy Y",
    dueAtMs: "2026-06-03",
    reason: "wrong due date type",
  })
  `,
  /Invalid arguments for tool 'escalateTo': parameter 'dueAtMs' expected number, got string/
);

await expectExecutionError(
  `
  await escalateTo({
    assignee: "guy Y",
    reason: "missing due date",
  })
  `,
  /Invalid arguments for tool 'escalateTo': missing required parameter 'dueAtMs'/
);

await expectExecutionError(
  `
  await escalateTo({
    assignee: "guy Y",
    dueAtMs: 1780444800000,
    reason: "extra typo field",
    assigne: "guy X",
  })
  `,
  /Invalid arguments for tool 'escalateTo': unexpected parameter 'assigne'/
);

await expectExecutionError(
  `await escalateTo("guy Y", 1780444800000, "ok", {}, "extra")`,
  /Invalid arguments for tool 'escalateTo': received 5 positional arguments but expected 4/
);

console.log("agent scenarios ok");
