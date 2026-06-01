// EXPLORATORY stress-pass catalog (not part of the green test:e2e gate; run via `npm run test:scenarios`).
// Checks named BUG/MISSING document gaps found during the realistic-scenario pass; see ../../KNOWN_GAPS.md.
// Some were fixed after this file was written, so those checks now intentionally show as failing-to-flag-fixed.
/**
 * Stress-test: control-flow, error handling, retries, and tool-orchestration.
 *
 * Checks marked "BUG: ..." are confirmed or suspected interpreter bugs — they
 * are kept non-fatal so the full suite always reports a final count.
 *
 * Known limitations (NOT bugs, per spec):
 *   - tool calls inside .map/.filter/.forEach are unsupported
 *   - Math.random() is deterministic
 *   - Set is undefined (not available as a constructor)
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

// ---------------------------------------------------------------------------
// 1. Retry with attempt counter — flaky tool, loop with try/catch
// ---------------------------------------------------------------------------
await check("retry: flaky tool succeeds on 3rd attempt", async () => {
  let calls = 0;
  const result = await execute(
    `
    let lastErr;
    let value;
    for (let attempt = 0; attempt < 5; attempt++) {
      try {
        value = await fetchItem({ key: "key" });
        break;
      } catch (e) {
        lastErr = e;
      }
    }
    value ?? ("failed: " + lastErr)
    `,
    {
      fetchItem: {
        description: "Fetch an item; fails first 2 times.",
        parameters: { key: { type: "string" } },
        execute: async ({ key }) => {
          calls++;
          if (calls < 3) throw new Error(`transient-${calls}`);
          return `data-${key}`;
        },
      },
    }
  );
  assert.equal(result.output, "data-key");
  assert.equal(result.toolCalls.length, 3);
  assert.ok(result.toolCalls[0].error);
  assert.ok(result.toolCalls[1].error);
  assert.equal(result.toolCalls[2].result, "data-key");
});

// ---------------------------------------------------------------------------
// 2. try/catch/finally — finally must always run, even on tool throw
// ---------------------------------------------------------------------------
await check("finally: runs even when tool throws and error is caught", async () => {
  const result = await execute(
    `
    const log = [];
    let out = "none";
    try {
      log.push("try");
      out = await doWork({ x: "x" });
    } catch (e) {
      log.push("catch:" + e);
    } finally {
      log.push("finally");
    }
    log.join(",") + "|" + out
    `,
    {
      doWork: {
        description: "Always throws.",
        parameters: { x: { type: "string" } },
        execute: async () => { throw new Error("oops"); },
      },
    }
  );
  assert.equal(result.output, "try,catch:oops,finally|none");
});

await check("finally: runs on success path and output is preserved", async () => {
  const result = await execute(
    `
    let cleaned = false;
    let val;
    try {
      val = await getData();
    } finally {
      cleaned = true;
    }
    val + "|" + cleaned
    `,
    {
      getData: {
        description: "Returns a value.",
        parameters: {},
        execute: async () => 42,
      },
    }
  );
  assert.equal(result.output, "42|true");
});

await check("try...finally without catch — valid JS, should work", async () => {
  const result = await execute(
    `
    let cleaned = false;
    let val = "none";
    try {
      val = await doWork();
    } finally {
      cleaned = true;
    }
    val + "|" + cleaned
    `,
    {
      doWork: {
        description: "Does work.",
        parameters: {},
        execute: async () => "done",
      },
    }
  );
  assert.equal(result.output, "done|true");
});

// ---------------------------------------------------------------------------
// 3. Fallback chain — primary → secondary → hardcoded default
// ---------------------------------------------------------------------------
await check("fallback chain: primary fails, secondary fails, returns default", async () => {
  const result = await execute(
    `
    let result;
    try {
      result = await primary();
    } catch {
      try {
        result = await secondary();
      } catch {
        result = "default-value";
      }
    }
    result
    `,
    {
      primary: {
        description: "Always fails.",
        parameters: {},
        execute: async () => { throw new Error("primary-down"); },
      },
      secondary: {
        description: "Also always fails.",
        parameters: {},
        execute: async () => { throw new Error("secondary-down"); },
      },
    }
  );
  assert.equal(result.output, "default-value");
  assert.equal(result.toolCalls.length, 2);
  assert.ok(result.toolCalls[0].error);
  assert.ok(result.toolCalls[1].error);
});

await check("fallback chain: primary fails, secondary succeeds", async () => {
  const result = await execute(
    `
    let result;
    try {
      result = await primary();
    } catch {
      result = await secondary();
    }
    result
    `,
    {
      primary: {
        description: "Always fails.",
        parameters: {},
        execute: async () => { throw new Error("primary-down"); },
      },
      secondary: {
        description: "Returns backup.",
        parameters: {},
        execute: async () => "backup-ok",
      },
    }
  );
  assert.equal(result.output, "backup-ok");
});

// ---------------------------------------------------------------------------
// 4. Promise.all where one rejects — caught, toolCalls records the failure
// ---------------------------------------------------------------------------
await check("Promise.all: one rejection caught, toolCalls records .error", async () => {
  const result = await execute(
    `
    let outcome;
    try {
      const vals = await Promise.all([fetchOne({ key: "a" }), fetchOne({ key: "bad" }), fetchOne({ key: "c" })]);
      outcome = "ok:" + vals.join(",");
    } catch (e) {
      outcome = "caught:" + e;
    }
    outcome
    `,
    {
      fetchOne: {
        description: "Fetch one key; 'bad' throws.",
        parameters: { key: { type: "string" } },
        execute: async ({ key }) => {
          if (key === "bad") throw new Error("bad-key-error");
          return `v-${key}`;
        },
      },
    }
  );
  assert.equal(result.output, "caught:bad-key-error");
  // The failing tool call should be recorded with an error
  const failing = result.toolCalls.find(tc => tc.error);
  assert.ok(failing, "expected a toolCall with .error");
  assert.match(String(failing.error), /bad-key-error/);
});

// ---------------------------------------------------------------------------
// 5. Guard clauses — using separate variable (workaround for inline push bug)
// ---------------------------------------------------------------------------
await check("guard clauses: early-exit + nested conditionals (separate var pattern)", async () => {
  const result = await execute(
    `
    async function processItem(id) {
      const item = await getItem({ id });
      if (!item) return "not-found";
      if (item.status === "deleted") return "deleted";
      if (item.value < 0) return "invalid";
      return "ok:" + item.value;
    }
    const r0 = await processItem("missing");
    const r1 = await processItem("del");
    const r2 = await processItem("neg");
    const r3 = await processItem("good");
    [r0, r1, r2, r3].join(",")
    `,
    {
      getItem: {
        description: "Fetch an item by id.",
        parameters: { id: { type: "string" } },
        execute: async ({ id }) => {
          if (id === "missing") return null;
          if (id === "del") return { status: "deleted", value: 5 };
          if (id === "neg") return { status: "active", value: -1 };
          return { status: "active", value: 99 };
        },
      },
    }
  );
  assert.equal(result.output, "not-found,deleted,invalid,ok:99");
});

// ---------------------------------------------------------------------------
// 6. switch statements — only work inside function/IIFE, not top-level
//    (top-level switch causes "allocation limit exceeded" — confirmed bug)
// ---------------------------------------------------------------------------
await check("switch: works inside a function/IIFE", async () => {
  const result = await execute(
    `
    async function dispatch(cmd) {
      const data = await getCommand({ cmd });
      switch (data.type) {
        case "create": return "created:" + data.name;
        case "delete": return "deleted:" + data.name;
        case "update": return "updated:" + data.name;
        default: return "unknown:" + data.type;
      }
    }
    const r0 = await dispatch("c");
    const r1 = await dispatch("d");
    const r2 = await dispatch("u");
    const r3 = await dispatch("x");
    [r0, r1, r2, r3].join("|")
    `,
    {
      getCommand: {
        description: "Get a command object.",
        parameters: { cmd: { type: "string" } },
        execute: async ({ cmd }) => {
          const map = {
            c: { type: "create", name: "foo" },
            d: { type: "delete", name: "foo" },
            u: { type: "update", name: "foo" },
            x: { type: "noop", name: "bar" },
          };
          return map[cmd];
        },
      },
    }
  );
  assert.equal(result.output, "created:foo|deleted:foo|updated:foo|unknown:noop");
});

// ---------------------------------------------------------------------------
// 7. Ternary, &&/||/?? short-circuiting, optional chaining ?.
// ---------------------------------------------------------------------------
await check("ternary + logical operators: basic evaluation", async () => {
  const result = await execute(
    `
    const a = await getVal({ key: "a" });
    const b = await getVal({ key: "null" });
    const t = a > 5 ? "big" : "small";
    const and = a > 5 && "yes-and";
    const or = b || "fallback-or";
    const nul = b ?? "nullish-fallback";
    [t, and, or, nul].join(",")
    `,
    {
      getVal: {
        description: "Get a value.",
        parameters: { key: { type: "string" } },
        execute: async ({ key }) => key === "null" ? null : 10,
      },
    }
  );
  assert.equal(result.output, "big,yes-and,fallback-or,nullish-fallback");
});

await check("optional chaining: a?.b?.c on null/undefined chains", async () => {
  const result = await execute(
    `
    const obj = await getObj({ kind: "full" });
    const missing = await getObj({ kind: "null" });
    const shallow = await getObj({ kind: "shallow" });
    const r1 = obj?.meta?.count;
    const r2 = missing?.meta?.count;
    const r3 = shallow?.meta?.count;
    [r1, r2, r3].join(",")
    `,
    {
      getObj: {
        description: "Get an object.",
        parameters: { kind: { type: "string" } },
        execute: async ({ kind }) => {
          if (kind === "null") return null;
          if (kind === "shallow") return { meta: null };
          return { meta: { count: 42 } };
        },
      },
    }
  );
  assert.equal(result.output, "42,undefined,undefined");
});

// ---------------------------------------------------------------------------
// 8. Thrown-value fidelity — throw object, catch shape
// ---------------------------------------------------------------------------
await check("thrown-value fidelity: throw {code, msg} object, catch sees original shape", async () => {
  const result = await execute(
    `
    let codeVal, msgVal, isObj;
    try {
      throw { code: "RATE_LIMIT", msg: "too many requests" };
    } catch (e) {
      codeVal = e.code;
      msgVal = e.msg;
      isObj = typeof e === "object";
    }
    codeVal + "|" + msgVal + "|" + isObj
    `,
    {}
  );
  assert.equal(result.output, "RATE_LIMIT|too many requests|true");
});

await check("thrown-value fidelity: throw string, catch sees it as string", async () => {
  const result = await execute(
    `
    let caught;
    try {
      throw "simple-string-error";
    } catch (e) {
      caught = typeof e + ":" + e;
    }
    caught
    `,
    {}
  );
  assert.equal(result.output, "string:simple-string-error");
});

await check("thrown-value fidelity: throw number, catch sees number", async () => {
  const result = await execute(
    `
    let caught;
    try {
      throw 42;
    } catch (e) {
      caught = typeof e + ":" + e;
    }
    caught
    `,
    {}
  );
  assert.equal(result.output, "number:42");
});

// ---------------------------------------------------------------------------
// 9. Accumulate partial results across a loop where some iterations fail
// ---------------------------------------------------------------------------
await check("partial accumulation: collect successes and errors across loop", async () => {
  const result = await execute(
    `
    const ids = ["a", "bad1", "b", "bad2", "c"];
    const successes = [];
    const errors = [];
    for (const id of ids) {
      try {
        const v = await fetchItem({ id });
        successes.push(v);
      } catch (e) {
        errors.push(id + ":" + e);
      }
    }
    successes.join(",") + "|" + errors.join(",")
    `,
    {
      fetchItem: {
        description: "Fetch by id; bad* ids throw.",
        parameters: { id: { type: "string" } },
        execute: async ({ id }) => {
          if (id.startsWith("bad")) throw new Error("fetch-failed");
          return `val-${id}`;
        },
      },
    }
  );
  assert.equal(result.output, "val-a,val-b,val-c|bad1:fetch-failed,bad2:fetch-failed");
  assert.equal(result.toolCalls.length, 5);
});

// ---------------------------------------------------------------------------
// 10. Re-throw — wrap with string concatenation (Error constructor unavailable)
//     Note: new Error() is unsupported (typeof Error === "undefined").
//     Workaround: throw a string.
// ---------------------------------------------------------------------------
await check("re-throw: wrapped string propagates to host as rejection", async () => {
  await assert.rejects(
    () => execute(
      `
      try {
        await doOp();
      } catch (e) {
        throw "wrapped: " + e;
      }
      `,
      {
        doOp: {
          description: "Always fails.",
          parameters: {},
          execute: async () => { throw new Error("inner-error"); },
        },
      }
    ),
    /wrapped: inner-error/
  );
});

// ---------------------------------------------------------------------------
// 11. autoFix:true — runtime error returns {output:null, error} not throw
// ---------------------------------------------------------------------------
await check("autoFix: runtime error returns {output:null, error} not throw", async () => {
  const result = await execute(
    `
    const x = await alwaysFails();
    x
    `,
    {
      alwaysFails: {
        description: "Always throws.",
        parameters: {},
        execute: async () => { throw new Error("permanent-failure"); },
      },
    },
    { autoFix: true }
  );
  assert.equal(result.output, null);
  assert.match(String(result.error), /permanent-failure/);
});

await check("autoFix: caught error doesn't surface as autoFix error", async () => {
  const result = await execute(
    `
    let val;
    try {
      val = await alwaysFails();
    } catch {
      val = "recovered";
    }
    val
    `,
    {
      alwaysFails: {
        description: "Always throws.",
        parameters: {},
        execute: async () => { throw new Error("inner"); },
      },
    },
    { autoFix: true }
  );
  assert.equal(result.output, "recovered");
  assert.ok(!result.error, "should not have an error when exception was caught");
});

// ---------------------------------------------------------------------------
// 12. break/continue, do-while, while with awaited tool calls
// ---------------------------------------------------------------------------
await check("break: loop exits early on sentinel value", async () => {
  const result = await execute(
    `
    const results = [];
    for (let i = 0; i < 10; i++) {
      const v = await getVal({ i });
      if (v === "STOP") break;
      results.push(v);
    }
    results.join(",")
    `,
    {
      getVal: {
        description: "Returns val or STOP at i=3.",
        parameters: { i: { type: "number" } },
        execute: async ({ i }) => i === 3 ? "STOP" : `item-${i}`,
      },
    }
  );
  assert.equal(result.output, "item-0,item-1,item-2");
});

await check("continue: skips odd indices in loop", async () => {
  const result = await execute(
    `
    const results = [];
    for (let i = 0; i < 6; i++) {
      if (i % 2 !== 0) continue;
      const v = await getVal({ i });
      results.push(v);
    }
    results.join(",")
    `,
    {
      getVal: {
        description: "Returns item-i.",
        parameters: { i: { type: "number" } },
        execute: async ({ i }) => `item-${i}`,
      },
    }
  );
  assert.equal(result.output, "item-0,item-2,item-4");
});

await check("do-while: executes body at least once, tool called in condition check", async () => {
  const result = await execute(
    `
    let count = 0;
    let sentinel = false;
    do {
      count++;
      sentinel = await shouldStop({ count });
    } while (!sentinel && count < 10);
    count
    `,
    {
      shouldStop: {
        description: "Returns true when count >= 4.",
        parameters: { count: { type: "number" } },
        execute: async ({ count }) => count >= 4,
      },
    }
  );
  assert.equal(result.output, 4);
});

await check("while loop: sequential tool calls until condition met (pagination)", async () => {
  const result = await execute(
    `
    let page = 0;
    const allItems = [];
    let hasMore = true;
    while (hasMore) {
      const batch = await fetchPage({ page });
      for (const item of batch.items) {
        allItems.push(item);
      }
      hasMore = batch.hasMore;
      page++;
    }
    allItems.join(",")
    `,
    {
      fetchPage: {
        description: "Paginated fetch.",
        parameters: { page: { type: "number" } },
        execute: async ({ page }) => {
          if (page === 0) return { items: ["a", "b"], hasMore: true };
          if (page === 1) return { items: ["c", "d"], hasMore: true };
          return { items: ["e"], hasMore: false };
        },
      },
    }
  );
  assert.equal(result.output, "a,b,c,d,e");
});

// ---------------------------------------------------------------------------
// 13. Validate-then-act: throw custom error object, branch on shape
// ---------------------------------------------------------------------------
await check("validate-then-act: branch on error shape in catch", async () => {
  const result = await execute(
    `
    async function runWithValidation(input) {
      try {
        if (!input.name) throw { code: "MISSING_FIELD", field: "name" };
        if (input.amount < 0) throw { code: "INVALID_VALUE", field: "amount" };
        return await submitOrder(input);
      } catch (e) {
        if (e && e.code === "MISSING_FIELD") return "missing:" + e.field;
        if (e && e.code === "INVALID_VALUE") return "invalid:" + e.field;
        return "unknown-error:" + e;
      }
    }
    const r1 = await runWithValidation({ name: "", amount: 10 });
    const r2 = await runWithValidation({ name: "Alice", amount: -5 });
    const r3 = await runWithValidation({ name: "Alice", amount: 100 });
    [r1, r2, r3].join("|")
    `,
    {
      submitOrder: {
        description: "Submit an order.",
        parameters: { name: { type: "string" }, amount: { type: "number" } },
        execute: async ({ name, amount }) => `order-ok:${name}:${amount}`,
      },
    }
  );
  assert.equal(result.output, "missing:name|invalid:amount|order-ok:Alice:100");
});

// ---------------------------------------------------------------------------
// 14. Nullish coalescing ??: null vs false vs 0
// ---------------------------------------------------------------------------
await check("nullish coalescing ??: null vs false vs 0 vs empty-string", async () => {
  const result = await execute(
    `
    const data = await getData();
    const a = data.nullVal ?? "null-default";
    const b = data.falseVal ?? "false-default";
    const c = data.zeroVal ?? "zero-default";
    const d = data.emptyStr ?? "empty-default";
    [a, b, String(c), d].join("|")
    `,
    {
      getData: {
        description: "Returns mixed nullish values.",
        parameters: {},
        execute: async () => ({ nullVal: null, falseVal: false, zeroVal: 0, emptyStr: "" }),
      },
    }
  );
  // ?? only nullish-coalesces null/undefined, not false/0/""
  assert.equal(result.output, "null-default|false|0|");
});

// ---------------------------------------------------------------------------
// 15. Tool with object return — nested property access after tool call
// ---------------------------------------------------------------------------
await check("nested property access from tool return value", async () => {
  const result = await execute(
    `
    const resp = await fetchUser({ userId: "u1" });
    const name = resp?.profile?.displayName ?? "anonymous";
    const role = resp?.permissions?.role ?? "viewer";
    name + ":" + role
    `,
    {
      fetchUser: {
        description: "Fetch user details.",
        parameters: { userId: { type: "string" } },
        execute: async ({ userId }) => ({
          id: userId,
          profile: { displayName: "Alice" },
          permissions: { role: "admin" },
        }),
      },
    }
  );
  assert.equal(result.output, "Alice:admin");
});

// ---------------------------------------------------------------------------
// 16. Multi-step orchestration: sequential tool calls building on prior results
// ---------------------------------------------------------------------------
await check("multi-step orchestration: 4 sequential tool calls building context", async () => {
  const audit = [];
  const result = await execute(
    `
    const ticket = await createTicket({ title: "bug report" });
    const enriched = await enrichTicket({ ticketId: ticket.id });
    const assigned = await assignTicket({ ticketId: ticket.id, assignee: enriched.suggestedOwner });
    const notified = await notifyUser({ userId: enriched.reporterId, ticketId: ticket.id, assignee: assigned.assignee });
    [ticket.id, enriched.suggestedOwner, assigned.assignee, notified.ok].join("|")
    `,
    {
      createTicket: {
        description: "Create a ticket.",
        parameters: { title: { type: "string" } },
        execute: async ({ title }) => {
          const t = { id: "t-001", title };
          audit.push({ op: "create", ...t });
          return t;
        },
      },
      enrichTicket: {
        description: "Enrich a ticket.",
        parameters: { ticketId: { type: "string" } },
        execute: async ({ ticketId }) => ({ ticketId, suggestedOwner: "bob", reporterId: "u-99" }),
      },
      assignTicket: {
        description: "Assign a ticket.",
        parameters: { ticketId: { type: "string" }, assignee: { type: "string" } },
        execute: async ({ ticketId, assignee }) => ({ ticketId, assignee, assignedAt: 1000 }),
      },
      notifyUser: {
        description: "Notify a user.",
        parameters: { userId: { type: "string" }, ticketId: { type: "string" }, assignee: { type: "string" } },
        execute: async (args) => ({ ok: true, ...args }),
      },
    }
  );
  assert.equal(result.output, "t-001|bob|bob|true");
  assert.equal(result.toolCalls.length, 4);
});

// ---------------------------------------------------------------------------
// BUG 1: array.push(await fn_with_tool_call()) — "not a function" at runtime
//         Workaround: store result in a separate variable first, then push.
// ---------------------------------------------------------------------------
await check("BUG 1: inline push(await fn_with_tool()) crashes — confirmed bug", async () => {
  // Demonstrate the bug
  const bugResult = await execute(
    `
    async function processItem(id) {
      const item = await getItem({ id });
      return item ? "ok:" + id : "nf";
    }
    const results = [];
    results.push(await processItem("a"));
    results.join(",")
    `,
    {
      getItem: {
        description: "Get item.",
        parameters: { id: { type: "string" } },
        execute: async () => ({ value: 1 }),
      },
    },
    { autoFix: true }
  );
  // This should succeed but currently fails
  assert.equal(bugResult.output, "ok:a",
    "BUG: push(await fn_with_tool()) should work but fails with '__array__.push is not a function'");
});

await check("BUG 1 workaround: separate variable before push works fine", async () => {
  // Same logic but with separate variable — works correctly
  const result = await execute(
    `
    async function processItem(id) {
      const item = await getItem({ id });
      return item ? "ok:" + id : "nf";
    }
    const r0 = await processItem("missing");
    const r1 = await processItem("good");
    const results = [r0, r1];
    results.join(",")
    `,
    {
      getItem: {
        description: "Get item.",
        parameters: { id: { type: "string" } },
        execute: async ({ id }) => id === "missing" ? null : { value: 1 },
      },
    }
  );
  assert.equal(result.output, "nf,ok:good");
});

// ---------------------------------------------------------------------------
// BUG 2: top-level switch statement causes "allocation limit exceeded"
//         Works inside function/IIFE; broken at top level.
// ---------------------------------------------------------------------------
await check("BUG 2: top-level switch crashes with allocation limit exceeded", async () => {
  const result = await execute(
    `
    const x = 2;
    switch (x) {
      case 2: "two"; break;
      default: "other";
    }
    "done"
    `,
    {},
    { autoFix: true, timeLimitMs: 3000 }
  );
  assert.equal(result.output, "done",
    "BUG: top-level switch causes 'allocation limit exceeded'; only works inside function/IIFE");
});

// ---------------------------------------------------------------------------
// BUG 3: labeled break/continue — break outer exits inner but NOT outer loop
//         (outer loop continues with next iteration instead of stopping)
// ---------------------------------------------------------------------------
await check("BUG 3: labeled break — outer loop should exit, but continues", async () => {
  const result = await execute(
    `
    const found = [];
    outer: for (let i = 0; i < 3; i++) {
      for (let j = 0; j < 3; j++) {
        if (i === 1 && j === 1) break outer;
        found.push(i + ":" + j);
      }
    }
    found.join(",")
    `,
    {}
  );
  // Should be "0:0,0:1,0:2,1:0" but actually produces "0:0,0:1,0:2,1:0,2:0,2:1,2:2"
  assert.equal(result.output, "0:0,0:1,0:2,1:0",
    "BUG: labeled break outer only breaks inner loop, outer loop continues (label ignored)");
});

// ---------------------------------------------------------------------------
// BUG 4: Error constructor is undefined — new Error() throws "undefined is not a constructor"
//         typeof Error === "undefined" — the Error global is not available.
//         Workaround: throw strings or plain objects instead.
// ---------------------------------------------------------------------------
await check("BUG 4: Error constructor unavailable — typeof Error === 'undefined'", async () => {
  const result = await execute(`typeof Error`, {}, { autoFix: true });
  assert.equal(result.output, "function",
    "BUG: typeof Error should be 'function' but is 'undefined'; new Error() is unsupported");
});

await check("BUG 4 workaround: throw string, catch as string (no Error needed)", async () => {
  const result = await execute(
    `
    let caught = "none";
    try {
      throw "custom-error: something went wrong";
    } catch (e) {
      caught = e;
    }
    caught
    `,
    {}
  );
  assert.equal(result.output, "custom-error: something went wrong");
});

// ---------------------------------------------------------------------------
// BUG 5: Tool Error arrives in catch as a string, not an Error object.
//         e.message is undefined; String(e) gives the message text.
// ---------------------------------------------------------------------------
await check("BUG 5: tool Error in catch has no .message (e is string, not Error)", async () => {
  const result = await execute(
    `
    let msgProp = "none";
    let strValue = "none";
    try {
      await alwaysThrows();
    } catch (e) {
      msgProp = e.message;
      strValue = String(e);
    }
    msgProp + "|" + strValue
    `,
    {
      alwaysThrows: {
        description: "Throws an Error object with message 'tool-err-msg'.",
        parameters: {},
        execute: async () => { throw new Error("tool-err-msg"); },
      },
    }
  );
  // e.message should be "tool-err-msg", e should be an Error
  // BUG: e is actually the string "tool-err-msg", so e.message is undefined
  assert.equal(result.output, "tool-err-msg|tool-err-msg",
    "BUG: caught tool Error should have .message='tool-err-msg' but .message is undefined; e is a string not an Error object");
});

// ---------------------------------------------------------------------------
// BUG 6: instanceof Error returns false (Error not a constructor, errors are strings)
// ---------------------------------------------------------------------------
await check("BUG 6: instanceof Error always false, new Error() unsupported", async () => {
  const result = await execute(
    `
    let isErr = false;
    let msg = "";
    try {
      throw new Error("test-error");
    } catch (e) {
      isErr = e instanceof Error;
      msg = String(e);
    }
    isErr + "|" + msg
    `,
    {},
    { autoFix: true }
  );
  assert.equal(result.output, "true|test-error",
    "BUG: new Error() is unsupported (Error constructor is undefined); instanceof always false");
});

// ---------------------------------------------------------------------------
// &&-short-circuit with tool call on RHS — this works correctly
// ---------------------------------------------------------------------------
await check("&&-short-circuit skips tool when LHS is false", async () => {
  let toolCalled = false;
  const result = await execute(
    `
    const flag = await getFlag();
    const val = flag && await expensive();
    String(val)
    `,
    {
      getFlag: {
        description: "Returns false.",
        parameters: {},
        execute: async () => false,
      },
      expensive: {
        description: "Should not be called.",
        parameters: {},
        execute: async () => { toolCalled = true; return "expensive-result"; },
      },
    }
  );
  assert.equal(result.output, "false");
  assert.equal(toolCalled, false, "expensive tool should not be called when flag is false");
});

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------
const failed = results.filter(r => !r[1]);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
if (failed.length) {
  console.log("\nFailed:");
  for (const [name, , msg] of failed) {
    console.log(`  ✗ ${name}: ${msg}`);
  }
  process.exit(1);
}