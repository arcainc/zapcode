import assert from "node:assert/strict";
import { execute, zapcode } from "../dist/index.js";

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

async function expectToolError(code, tools, expectedMessage) {
  await assert.rejects(
    () => execute(code, tools),
    error => {
      assert.match(String(error.message), expectedMessage);
      return true;
    }
  );
}

{
  const { system } = zapcode({ tools: createRoutingTools([]) });
  assert.match(
    system,
    /declare function escalateTo\(input: \{ assignee: string; dueAtMs: number; reason: string; metadata\?: object \}\): Promise<unknown>;/
  );
  assert.match(system, /Call shape: await escalateTo\(\{ assignee: string, dueAtMs: number/);
  assert.match(system, /Prefer the declared function signatures above exactly/);
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

{
  const cases = [
    ["2026-05-30", "guy X"],
    ["2026-05-31", "guy Y"],
    ["2026-06-07", "guy Y"],
    ["2026-06-08", "guy X"],
  ];

  for (const [userDate, expectedAssignee] of cases) {
    const escalations = [];
    const result = await execute(
      `
      const userDate = ${JSON.stringify(userDate)};
      const now = await getNowMs();
      const dueAtMs = await parseDateMs(userDate);
      const weekMs = 7 * 24 * 60 * 60 * 1000;
      const assignee = dueAtMs >= now && dueAtMs <= now + weekMs ? "guy Y" : "guy X";
      await escalateTo({ assignee, dueAtMs, reason: "calendar routing" });
      assignee
      `,
      createRoutingTools(escalations)
    );

    assert.equal(result.output, expectedAssignee);
    assert.equal(escalations.length, 1);
    assert.equal(escalations[0].assignee, expectedAssignee);
  }
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
  `await escalateTo("guy Y", 1780444800000, "looks valid")`,
  /Invalid arguments for tool 'escalateTo': expected one named object argument/
);

await expectExecutionError(
  `
  await escalateTo({
    assignee: "guy Y",
    dueAtMs: 0 / 0,
    reason: "NaN due date",
  })
  `,
  /Invalid arguments for tool 'escalateTo': parameter 'dueAtMs' expected number, got null/
);

await expectExecutionError(
  `
  await escalateTo({
    assignee: "guy Y",
    dueAtMs: 1780444800000,
    reason: "bad metadata",
    metadata: null,
  })
  `,
  /Invalid arguments for tool 'escalateTo': parameter 'metadata' expected object, got null/
);

await expectExecutionError(
  `
  await escalateTo({
    assignee: "guy Y",
    dueAtMs: 1780444800000,
    reason: "bad metadata",
    metadata: [],
  })
  `,
  /Invalid arguments for tool 'escalateTo': parameter 'metadata' expected object, got array/
);

{
  const escalations = [];
  await execute(
    `
    await escalateTo({
      assignee: "guy Y",
      dueAtMs: 1780444800000,
      reason: "metadata intentionally omitted",
    })
    `,
    createRoutingTools(escalations)
  );

  assert.equal(escalations.length, 1);
  assert.equal(Object.hasOwn(escalations[0], "metadata"), false);
}

{
  const routed = [];
  const supportTools = {
    lookupAccount: {
      description: "Lookup account details by account ID.",
      parameters: {
        accountId: { type: "string" },
      },
      execute: async ({ accountId }) => ({ accountId, tier: "enterprise", openTickets: 4 }),
    },
    routeTicket: {
      description: "Route a support ticket.",
      parameters: {
        ticketId: { type: "string" },
        accountId: { type: "string" },
        priority: { type: "number" },
        queue: { type: "string" },
        reason: { type: "string" },
        tags: { type: "array" },
        metadata: { type: "object", optional: true },
      },
      execute: async input => {
        routed.push(input);
        return { routed: true, queue: input.queue };
      },
    },
  };

  const result = await execute(
    `
    const account = await lookupAccount({ accountId: "acct_123" });
    const priority = account.tier === "enterprise" && account.openTickets > 3 ? 1 : 3;
    const queue = priority === 1 ? "vip" : "standard";
    await routeTicket({
      ticketId: "ticket_123",
      accountId: account.accountId,
      priority,
      queue,
      reason: "enterprise account with repeat contacts",
      tags: ["enterprise", "repeat-contact"],
      metadata: { openTickets: account.openTickets },
    });
    priority
    `,
    supportTools
  );

  assert.equal(result.output, 1);
  assert.equal(routed[0].ticketId, "ticket_123");
  assert.deepEqual(routed[0].tags, ["enterprise", "repeat-contact"]);
  assert.equal(routed[0].metadata.openTickets, 4);
  assert.deepEqual(result.toolCalls.at(-1).input, routed[0]);

  await expectToolError(
    `
    await routeTicket({
      ticketId: "ticket_123",
      accountId: "acct_123",
      priority: 1,
      queue: "vip",
      reason: "bad tags",
      tags: "enterprise",
    })
    `,
    supportTools,
    /Invalid arguments for tool 'routeTicket': parameter 'tags' expected array, got string/
  );

  await expectToolError(
    `
    await routeTicket({
      ticketId: "ticket_123",
      accountId: "acct_123",
      priority: 1,
      queue: "vip",
      reason: "bad metadata",
      tags: [],
      metadata: [],
    })
    `,
    supportTools,
    /Invalid arguments for tool 'routeTicket': parameter 'metadata' expected object, got array/
  );

  await expectToolError(
    `
    await routeTicket({
      ticketId: "ticket_123",
      accoundId: "acct_123",
      priority: 1,
      queue: "vip",
      reason: "typo account id",
      tags: [],
    })
    `,
    supportTools,
    /Invalid arguments for tool 'routeTicket': unexpected parameter 'accoundId'/
  );

  await expectToolError(
    `
    await routeTicket({
      accountId: "acct_123",
      priority: 1,
      queue: "vip",
      reason: "missing ticket id",
      tags: [],
    })
    `,
    supportTools,
    /Invalid arguments for tool 'routeTicket': missing required parameter 'ticketId'/
  );

  await expectToolError(
    `
    await routeTicket({
      ticketId: "ticket_123",
      accountId: "acct_123",
      priority: "1",
      queue: "vip",
      reason: "bad priority",
      tags: [],
    })
    `,
    supportTools,
    /Invalid arguments for tool 'routeTicket': parameter 'priority' expected number, got string/
  );
}

{
  const approvals = [];
  const approvalTools = {
    requestApproval: {
      description: "Request approval for an expense.",
      parameters: {
        amountUsd: { type: "number" },
        urgent: { type: "boolean" },
        approvers: { type: "array" },
        evidence: { type: "object", optional: true },
      },
      execute: async input => {
        approvals.push(input);
        return { ok: true, approverCount: input.approvers.length };
      },
    },
    savePayload: {
      description: "Store one arbitrary payload object.",
      parameters: {
        payload: { type: "object" },
      },
      execute: async ({ payload }) => ({ saved: true, keys: Object.keys(payload) }),
    },
  };

  const result = await execute(
    `
    await requestApproval({
      amountUsd: 1250.5,
      urgent: true,
      approvers: ["finance", "ops"],
      evidence: { invoiceId: "inv_1" },
    });
    const saved = await savePayload({ invoiceId: "inv_1", amountUsd: 1250.5 });
    saved.keys.sort().join(",")
    `,
    approvalTools
  );

  assert.equal(result.output, "amountUsd,invoiceId");
  assert.equal(approvals[0].urgent, true);

  await expectToolError(
    `await requestApproval({ amountUsd: 1 / 0, urgent: true, approvers: [] })`,
    approvalTools,
    /Invalid arguments for tool 'requestApproval': parameter 'amountUsd' expected number, got null/
  );

  await expectToolError(
    `await savePayload({ payload: { ok: true }, extra: true })`,
    approvalTools,
    /Invalid arguments for tool 'savePayload': unexpected parameter 'extra'/
  );
}

{
  const escalations = [];
  const result = await execute(
    `
    await escalateTo({
      assignee: "guy Y",
      dueAtMs: "2026-06-03",
      reason: "bad due date",
    })
    `,
    createRoutingTools(escalations),
    { autoFix: true }
  );

  assert.equal(result.output, null);
  assert.match(result.error, /parameter 'dueAtMs' expected number, got string/);
  assert.deepEqual(escalations, []);
  assert.equal(result.trace.status, "error");
  assert.match(result.trace.attributes["zapcode.error"], /expected number/);
  const failedToolSpan = result.trace.children.find(span => span.name === "tool_call");
  assert.equal(failedToolSpan.status, "error");
  assert.equal(failedToolSpan.attributes["zapcode.tool.name"], "escalateTo");
  assert.match(failedToolSpan.attributes["zapcode.tool.error"], /expected number/);
}

{
  const escalations = [];
  const result = await execute(
    `
    const now = await getNowMs();
    await escalateTo({ assignee: "guy Y", dueAtMs: now, reason: "trace success" });
    "ok"
    `,
    createRoutingTools(escalations),
    { autoFix: true }
  );

  assert.equal(result.output, "ok");
  const toolSpan = result.trace.children.find(
    span => span.name === "tool_call" && span.attributes["zapcode.tool.name"] === "escalateTo"
  );
  assert.equal(toolSpan.status, "ok");
  assert.match(toolSpan.attributes["zapcode.tool.args"], /guy Y/);
  assert.match(toolSpan.attributes["zapcode.tool.input"], /trace success/);
  assert.match(toolSpan.attributes["zapcode.tool.result"], /guy Y/);
}

{
  const { tools } = zapcode({ tools: createRoutingTools([]), autoFix: true });
  for (const input of [null, {}, { code: 123 }, []]) {
    const result = await tools.execute_code.execute(input);
    assert.equal(result.output, null);
    assert.match(result.error, /Invalid execute_code input: expected object with code: string/);
  }
}

{
  for (const name of ["console", "Math", "eval", "Function", "require", "process", "globalThis", "execute_code", "x-y"]) {
    assert.throws(
      () =>
        zapcode({
          tools: {
            [name]: {
              description: "bad tool",
              parameters: {},
              execute: async () => null,
            },
          },
        }),
      /Invalid tool name/
    );
  }
}

console.log("agent scenarios ok");
