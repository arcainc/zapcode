/**
 * Async / Promise / parallel-orchestration stress tests for the Zapcode sandbox.
 *
 * Covers: Promise.all fan-out, nested Promise.all, sequential for-of awaits,
 * mixed plain-values + tool-calls in Promise.all, retry-with-backoff, fallback
 * chains, partial-accumulation, Promise.resolve/await-non-promise,
 * Promise.allSettled / Promise.race / Promise.any probes, async helper fns,
 * chained .then()/.catch(), Error objects across async, enrich+filter workflow,
 * toolCalls metadata, and more.
 *
 * Run: node tests/scenarios2-async.mjs
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
// 1. Promise.all parallel fan-out: calls written directly as array elements
// ---------------------------------------------------------------------------
await check("Promise.all fan-out: 3 direct tool calls, order preserved", async () => {
  const result = await execute(
    `
    const [a, b, c] = await Promise.all([
      lookup({ key: "x" }),
      lookup({ key: "y" }),
      lookup({ key: "z" }),
    ]);
    a + "," + b + "," + c
    `,
    {
      lookup: {
        description: "Look up a value.",
        parameters: { key: { type: "string" } },
        execute: async ({ key }) => `val-${key}`,
      },
    }
  );
  assert.equal(result.output, "val-x,val-y,val-z");
  assert.equal(result.toolCalls.length, 3);
});

// ---------------------------------------------------------------------------
// 2. Nested Promise.all
//    CONFIRMED BUG: Promise.all([Promise.all([toolA(), toolB()]), ...]) —
//    inner Promise.all with tool calls doesn't resolve; results are undefined.
//    The inner batch leaks a pending internal promise object.
//    Workaround: await inner first, then include result in outer.
// ---------------------------------------------------------------------------
await check("BUG-NEST: nested Promise.all with tool calls — inner results are undefined", async () => {
  // Demonstrate the bug
  const bugResult = await execute(
    `
    const [groupA, groupB] = await Promise.all([
      Promise.all([fetch({ id: "a1" }), fetch({ id: "a2" })]),
      Promise.all([fetch({ id: "b1" }), fetch({ id: "b2" })]),
    ]);
    typeof groupA + "|" + typeof groupB
    `,
    {
      fetch: {
        description: "Fetch by id.",
        parameters: { id: { type: "string" } },
        execute: async ({ id }) => `r-${id}`,
      },
    },
    { autoFix: true }
  );
  // BUG: groupA and groupB are undefined because inner Promise.all doesn't resolve
  assert.equal(bugResult.output, "undefined|undefined",
    `BUG confirmed: nested Promise.all inner results should be arrays, got: "${bugResult.output}"`);

  // Workaround: await each inner group separately first
  const fixResult = await execute(
    `
    const groupA = await Promise.all([fetch({ id: "a1" }), fetch({ id: "a2" })]);
    const groupB = await Promise.all([fetch({ id: "b1" }), fetch({ id: "b2" })]);
    groupA.join("+") + "|" + groupB.join("+")
    `,
    {
      fetch: {
        description: "Fetch by id.",
        parameters: { id: { type: "string" } },
        execute: async ({ id }) => `r-${id}`,
      },
    }
  );
  assert.equal(fixResult.output, "r-a1+r-a2|r-b1+r-b2");
  assert.equal(fixResult.toolCalls.length, 4);
});

// ---------------------------------------------------------------------------
// 3. Sequential awaits in for-of loop
// ---------------------------------------------------------------------------
await check("for-of sequential awaits: each iteration calls tool and accumulates", async () => {
  const result = await execute(
    `
    const keys = ["p", "q", "r", "s"];
    const out = [];
    for (const k of keys) {
      const v = await get({ key: k });
      out.push(v);
    }
    out.join(",")
    `,
    {
      get: {
        description: "Get value for key.",
        parameters: { key: { type: "string" } },
        execute: async ({ key }) => `item-${key}`,
      },
    }
  );
  assert.equal(result.output, "item-p,item-q,item-r,item-s");
  assert.equal(result.toolCalls.length, 4);
});

// ---------------------------------------------------------------------------
// 4. Mix of sequential for-of AND Promise.all in same script
// ---------------------------------------------------------------------------
await check("mixed sequential + parallel in same script", async () => {
  const result = await execute(
    `
    const phases = ["init", "run", "done"];
    const log = [];
    for (const phase of phases) {
      const status = await checkPhase({ phase });
      log.push(phase + "=" + status);
    }
    const [u, v] = await Promise.all([fetch({ id: "u" }), fetch({ id: "v" })]);
    log.push("parallel=" + u + "+" + v);
    log.join("|")
    `,
    {
      checkPhase: {
        description: "Check a phase status.",
        parameters: { phase: { type: "string" } },
        execute: async ({ phase }) => phase === "run" ? "running" : "ok",
      },
      fetch: {
        description: "Fetch by id.",
        parameters: { id: { type: "string" } },
        execute: async ({ id }) => `r-${id}`,
      },
    }
  );
  assert.equal(result.output, "init=ok|run=running|done=ok|parallel=r-u+r-v");
  assert.equal(result.toolCalls.length, 5);
});

// ---------------------------------------------------------------------------
// 5. Promise.all mixing plain values, Promise.resolve, and tool calls
// ---------------------------------------------------------------------------
await check("Promise.all mixes plain values + Promise.resolve + tool call", async () => {
  const result = await execute(
    `
    const results = await Promise.all([
      42,
      Promise.resolve("resolved"),
      fetch({ id: "tool" }),
    ]);
    results.join(",")
    `,
    {
      fetch: {
        description: "Fetch by id.",
        parameters: { id: { type: "string" } },
        execute: async ({ id }) => `r-${id}`,
      },
    }
  );
  assert.equal(result.output, "42,resolved,r-tool");
});

// ---------------------------------------------------------------------------
// 6. await Promise.resolve(value) — awaiting a resolved promise
// ---------------------------------------------------------------------------
await check("await Promise.resolve returns the resolved value", async () => {
  const result = await execute(
    `
    const a = await Promise.resolve(99);
    const b = await Promise.resolve("hello");
    const c = await Promise.resolve(null);
    a + "|" + b + "|" + String(c)
    `,
    {}
  );
  assert.equal(result.output, "99|hello|null");
});

// ---------------------------------------------------------------------------
// 7. Awaiting a non-promise (plain value) — should return the value itself
// ---------------------------------------------------------------------------
await check("awaiting a plain number/string/object returns it unchanged", async () => {
  const result = await execute(
    `
    const n = await 123;
    const s = await "plain";
    const o = await { x: 7 };
    n + "|" + s + "|" + o.x
    `,
    {}
  );
  assert.equal(result.output, "123|plain|7");
});

// ---------------------------------------------------------------------------
// 8. Retry-with-backoff pattern: flaky tool throws first K times
// ---------------------------------------------------------------------------
await check("retry-with-backoff: tool succeeds on 3rd attempt (simulated delay)", async () => {
  let attempts = 0;
  const result = await execute(
    `
    let lastErr;
    let value;
    for (let attempt = 0; attempt < 5; attempt++) {
      try {
        value = await flaky({ id: "item" });
        break;
      } catch (e) {
        lastErr = String(e);
        // simulate backoff: wait 0ms (no real sleep in sandbox)
      }
    }
    value + "|attempts:" + attempts_global
    `,
    {
      flaky: {
        description: "Flaky tool that fails first 2 times.",
        parameters: { id: { type: "string" } },
        execute: async ({ id }) => {
          attempts++;
          if (attempts < 3) throw new Error(`transient-${attempts}`);
          return `ok-${id}`;
        },
      },
    }
  );
  // Note: attempts_global won't be defined in guest; just check value
  // We test the value portion
  assert.ok(String(result.output).startsWith("ok-item"), `got: ${result.output}`);
  assert.equal(attempts, 3);
  assert.equal(result.toolCalls.length, 3);
  assert.ok(result.toolCalls[0].error);
  assert.ok(result.toolCalls[1].error);
  assert.equal(result.toolCalls[2].result, "ok-item");
});

// ---------------------------------------------------------------------------
// 8b. Retry without depending on host-side variable in guest code
// ---------------------------------------------------------------------------
await check("retry: guest counts attempts with its own variable", async () => {
  let calls = 0;
  const result = await execute(
    `
    let value;
    let attemptCount = 0;
    for (let i = 0; i < 5; i++) {
      attemptCount++;
      try {
        value = await flaky({ id: "x" });
        break;
      } catch (e) {
        // retry
      }
    }
    value + "|attempts:" + attemptCount
    `,
    {
      flaky: {
        description: "Fails first 2 times.",
        parameters: { id: { type: "string" } },
        execute: async ({ id }) => {
          calls++;
          if (calls < 3) throw new Error("transient");
          return `done-${id}`;
        },
      },
    }
  );
  assert.equal(result.output, "done-x|attempts:3");
});

// ---------------------------------------------------------------------------
// 9. Fallback chain: primary → secondary → hardcoded default
//    (all three levels exercised in sub-checks)
// ---------------------------------------------------------------------------
await check("fallback chain: all fail → default", async () => {
  const result = await execute(
    `
    let out;
    try {
      out = await primary();
    } catch {
      try {
        out = await secondary();
      } catch {
        out = "hardcoded-default";
      }
    }
    out
    `,
    {
      primary: {
        description: "Always fails.",
        parameters: {},
        execute: async () => { throw new Error("primary-down"); },
      },
      secondary: {
        description: "Always fails.",
        parameters: {},
        execute: async () => { throw new Error("secondary-down"); },
      },
    }
  );
  assert.equal(result.output, "hardcoded-default");
  assert.equal(result.toolCalls.length, 2);
  assert.ok(result.toolCalls[0].error);
  assert.ok(result.toolCalls[1].error);
});

await check("fallback chain: primary fails, secondary succeeds", async () => {
  const result = await execute(
    `
    let out;
    try {
      out = await primary();
    } catch {
      out = await secondary();
    }
    out
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
// 10. Aggregate partial successes + errors across a loop
// ---------------------------------------------------------------------------
await check("partial accumulation: successes and failures collected from loop", async () => {
  const result = await execute(
    `
    const ids = ["ok1", "fail1", "ok2", "fail2", "ok3"];
    const successes = [];
    const errors = [];
    for (const id of ids) {
      try {
        const v = await handleItem({ id });
        successes.push(v);
      } catch (e) {
        errors.push(id + ":" + String(e));
      }
    }
    successes.join(",") + "|" + errors.join(",")
    `,
    {
      handleItem: {
        description: "Handle id; fails on ids starting with fail.",
        parameters: { id: { type: "string" } },
        execute: async ({ id }) => {
          if (id.startsWith("fail")) throw new Error("proc-error");
          return `res-${id}`;
        },
      },
    }
  );
  assert.equal(result.output, "res-ok1,res-ok2,res-ok3|fail1:proc-error,fail2:proc-error");
  assert.equal(result.toolCalls.length, 5);
});

// ---------------------------------------------------------------------------
// 11. Async helper functions defined then called
// ---------------------------------------------------------------------------
await check("async helper function: defined, called, result used by subsequent code", async () => {
  const result = await execute(
    `
    async function enrichRecord(id) {
      const base = await fetchBase({ id });
      const extra = await fetchExtra({ id });
      return { id, name: base.name, score: extra.score };
    }

    const r1 = await enrichRecord("alice");
    const r2 = await enrichRecord("bob");
    r1.name + ":" + r1.score + "|" + r2.name + ":" + r2.score
    `,
    {
      fetchBase: {
        description: "Fetch base record.",
        parameters: { id: { type: "string" } },
        execute: async ({ id }) => ({ name: id.charAt(0).toUpperCase() + id.slice(1) }),
      },
      fetchExtra: {
        description: "Fetch extra data.",
        parameters: { id: { type: "string" } },
        execute: async ({ id }) => ({ score: id.length * 10 }),
      },
    }
  );
  assert.equal(result.output, "Alice:50|Bob:30");
  assert.equal(result.toolCalls.length, 4);
});

// ---------------------------------------------------------------------------
// 12. Async function returning value used by later code (value flows through)
// ---------------------------------------------------------------------------
await check("async helper return value flows into chained computation", async () => {
  const result = await execute(
    `
    async function getScore(userId) {
      const user = await getUser({ userId });
      if (!user) return 0;
      return user.points * 2;
    }

    const s1 = await getScore("u1");
    const s2 = await getScore("missing");
    const total = s1 + s2;
    total
    `,
    {
      getUser: {
        description: "Get user by id.",
        parameters: { userId: { type: "string" } },
        execute: async ({ userId }) => {
          if (userId === "missing") return null;
          return { userId, points: 15 };
        },
      },
    }
  );
  assert.equal(result.output, 30);
});

// ---------------------------------------------------------------------------
// 13. Error objects across async: throw new Error, e.message, e instanceof Error
// ---------------------------------------------------------------------------
await check("throw new Error in guest: e.message accessible across await", async () => {
  const result = await execute(
    `
    async function riskyOp() {
      const data = await getData();
      if (!data.valid) {
        throw new Error("data-invalid:" + data.reason);
      }
      return data.value;
    }

    let msg = "none";
    try {
      await riskyOp();
    } catch (e) {
      msg = e.message;
    }
    msg
    `,
    {
      getData: {
        description: "Get data.",
        parameters: {},
        execute: async () => ({ valid: false, reason: "missing-field", value: null }),
      },
    }
  );
  assert.equal(result.output, "data-invalid:missing-field");
});

await check("e instanceof Error is true when guest throws new Error()", async () => {
  const result = await execute(
    `
    let isErr = false;
    let msg = "";
    try {
      throw new Error("guest-error");
    } catch (e) {
      isErr = e instanceof Error;
      msg = e.message;
    }
    isErr + "|" + msg
    `,
    {}
  );
  assert.equal(result.output, "true|guest-error");
});

// ---------------------------------------------------------------------------
// 14. Tool error arrives as Error in guest catch: e.message should work
//     CONFIRMED BUG: tool-thrown Error arrives as a plain string, not Error object.
//     e.message is undefined; String(e) gives the message text.
//     NOTE: avoid String(e.message ?? String(e)) — see BUG-NULLISH-IN-CALL below.
// ---------------------------------------------------------------------------
await check("BUG-TOOL-ERR: tool Error in catch is plain string (e.message undefined)", async () => {
  const result = await execute(
    `
    let msg = "none";
    let hasMsg = false;
    try {
      await throwingTool();
    } catch (e) {
      hasMsg = e.message !== undefined;
      // Workaround for BUG-NULLISH-IN-CALL: don't use ?? inside a function call arg
      const raw = e.message;
      msg = raw !== undefined ? String(raw) : String(e);
    }
    msg + "|hasMsg:" + hasMsg
    `,
    {
      throwingTool: {
        description: "Always throws.",
        parameters: {},
        execute: async () => { throw new Error("tool-thrown-error"); },
      },
    }
  );
  // Confirmed behavior: e is a plain string ("tool-thrown-error"), so e.message === undefined
  // hasMsg is false
  // BUG: should be hasMsg:true (e should be an Error with .message)
  const output = String(result.output);
  assert.equal(output, "tool-thrown-error|hasMsg:false",
    `BUG confirmed: tool Error arrives as plain string (no .message). Got: "${output}"`);
});

// ---------------------------------------------------------------------------
// 15. Promise.allSettled — probe existence and behavior
//     CONFIRMED BUG: typeof returns "function" but calling throws
//     "Promise.allSettled is not a function" — phantom/broken stub
// ---------------------------------------------------------------------------
await check("MISSING: Promise.allSettled is phantom stub (typeof=function but uncallable)", async () => {
  const result = await execute(
    `
    const typeStr = typeof Promise.allSettled;
    let callResult = "not-called";
    try {
      await Promise.allSettled([Promise.resolve("a")]);
      callResult = "called-ok";
    } catch(e) {
      callResult = "threw:" + String(e);
    }
    typeStr + "|" + callResult
    `,
    {},
    { autoFix: true }
  );
  // typeof shows "function" but calling throws — broken stub
  assert.equal(result.output, "function|threw:type error: Promise.allSettled is not a function",
    `MISSING: Promise.allSettled reports typeof="function" but throws when called. Got: "${result.output}"`);
});

// ---------------------------------------------------------------------------
// 16. Promise.race — probe existence and basic behavior
//     CONFIRMED BUG: same phantom-stub pattern
// ---------------------------------------------------------------------------
await check("MISSING: Promise.race is phantom stub (typeof=function but uncallable)", async () => {
  const result = await execute(
    `
    const typeStr = typeof Promise.race;
    let callResult = "not-called";
    try {
      await Promise.race([Promise.resolve("a")]);
      callResult = "called-ok";
    } catch(e) {
      callResult = "threw:" + String(e);
    }
    typeStr + "|" + callResult
    `,
    {},
    { autoFix: true }
  );
  assert.equal(result.output, "function|threw:type error: Promise.race is not a function",
    `MISSING: Promise.race reports typeof="function" but throws when called. Got: "${result.output}"`);
});

// ---------------------------------------------------------------------------
// 17. Promise.any — probe existence and behavior
//     CONFIRMED BUG: same phantom-stub pattern
// ---------------------------------------------------------------------------
await check("MISSING: Promise.any is phantom stub (typeof=function but uncallable)", async () => {
  const result = await execute(
    `
    const typeStr = typeof Promise.any;
    let callResult = "not-called";
    try {
      await Promise.any([Promise.resolve("a")]);
      callResult = "called-ok";
    } catch(e) {
      callResult = "threw:" + String(e);
    }
    typeStr + "|" + callResult
    `,
    {},
    { autoFix: true }
  );
  assert.equal(result.output, "function|threw:type error: Promise.any is not a function",
    `MISSING: Promise.any reports typeof="function" but throws when called. Got: "${result.output}"`);
});

// ---------------------------------------------------------------------------
// 18. Chained .then() on a resolved promise
// ---------------------------------------------------------------------------
await check(".then() chaining on Promise.resolve", async () => {
  const result = await execute(
    `
    const val = await Promise.resolve(5).then(x => x * 2).then(x => x + 1);
    val
    `,
    {}
  );
  assert.equal(result.output, 11);
});

// ---------------------------------------------------------------------------
// 19. .catch() on a rejected promise
// ---------------------------------------------------------------------------
await check(".catch() on Promise.reject recovers the value", async () => {
  const result = await execute(
    `
    const val = await Promise.reject("err-value").catch(e => "caught:" + e);
    val
    `,
    {}
  );
  assert.equal(result.output, "caught:err-value");
});

// ---------------------------------------------------------------------------
// 20. .then().catch() chain — success path skips catch
// ---------------------------------------------------------------------------
await check(".then().catch(): success path, catch not invoked", async () => {
  const result = await execute(
    `
    const val = await Promise.resolve("ok")
      .then(v => v.toUpperCase())
      .catch(e => "caught:" + e);
    val
    `,
    {}
  );
  assert.equal(result.output, "OK");
});

// ---------------------------------------------------------------------------
// 21. Chained .then() on a tool call result
//     CONFIRMED BUG: toolCall() without await returns the resolved VALUE, not a Promise.
//     So fetchNum({n:4}).then(fn) fails because 4 (number) has no .then method.
//     Workaround: await fetchNum({n:4}) first, or use Promise.resolve(toolCall).then().
// ---------------------------------------------------------------------------
await check("BUG-THEN: toolCall().then() fails — tool call is eager, returns value not Promise", async () => {
  // Confirm: unawaited tool call returns the resolved value, not a Promise
  const r1 = await execute(
    `typeof fetchNum({ n: 4 })`,
    {
      fetchNum: {
        description: "Return a number.",
        parameters: { n: { type: "number" } },
        execute: async ({ n }) => n,
      },
    }
  );
  assert.equal(r1.output, "number",
    `BUG: unawaited toolCall() should return a Promise (typeof "object") but returns the value directly (typeof "${r1.output}")`);

  // Workaround works: Promise.resolve(toolCall).then()
  const r2 = await execute(
    `
    const val = await Promise.resolve(fetchNum({ n: 4 })).then(x => x * 10);
    val
    `,
    {
      fetchNum: {
        description: "Return a number.",
        parameters: { n: { type: "number" } },
        execute: async ({ n }) => n,
      },
    }
  );
  assert.equal(r2.output, 40);
});

// ---------------------------------------------------------------------------
// 22. Realistic enrich + filter + sort + escalate workflow
//     NOTE: Array.sort() has a bug (doesn't mutate in-place — see BUG-SORT below).
//     Workaround: use the RETURN VALUE of sort(), not the original array.
// ---------------------------------------------------------------------------
await check("enrich-filter-sort-escalate workflow (uses sort return value workaround)", async () => {
  const escalated = [];
  const result = await execute(
    `
    const userIds = ["u1", "u2", "u3", "u4", "u5"];

    // Step 1: enrich all records sequentially
    const enriched = [];
    for (const uid of userIds) {
      const user = await getUser({ userId: uid });
      enriched.push(user);
    }

    // Step 2: filter to users needing review (score < 50)
    const flagged = enriched.filter(u => u.score < 50);

    // Step 3: sort by score ascending — MUST use return value (sandbox sort doesn't mutate)
    const sorted = flagged.sort((a, b) => a.score - b.score);

    // Step 4: escalate each flagged user in sorted order
    for (const user of sorted) {
      await escalate({ userId: user.id, score: user.score });
    }

    sorted.map(u => u.id + "=" + u.score).join(",")
    `,
    {
      getUser: {
        description: "Get user with score.",
        parameters: { userId: { type: "string" } },
        execute: async ({ userId }) => {
          const scores = { u1: 80, u2: 30, u3: 15, u4: 95, u5: 45 };
          return { id: userId, score: scores[userId] ?? 50 };
        },
      },
      escalate: {
        description: "Escalate a user.",
        parameters: { userId: { type: "string" }, score: { type: "number" } },
        execute: async (args) => {
          escalated.push(args);
          return { ok: true };
        },
      },
    }
  );
  assert.equal(result.output, "u3=15,u2=30,u5=45");
  assert.equal(escalated.length, 3);
  assert.equal(escalated[0].userId, "u3");
  assert.equal(escalated[1].userId, "u2");
  assert.equal(escalated[2].userId, "u5");
  // 5 getUser + 3 escalate = 8 tool calls
  assert.equal(result.toolCalls.length, 8);
});

// ---------------------------------------------------------------------------
// 23. Promise.all with one rejection caught via .catch() on the whole batch
//     CONFIRMED BUG: Promise.all([failingTool()]).catch(fn) does NOT invoke fn.
//     Tool rejections escape Promise.all and bypass .catch() handlers.
//     Pure Promise.reject() IS caught by .catch() — only tool failures escape.
// ---------------------------------------------------------------------------
await check("BUG-CATCH-CHAIN: Promise.all([failingTool()]).catch() does NOT catch tool rejection", async () => {
  // Confirm the bug: .catch() is NOT called when a tool rejects inside Promise.all
  const result = await execute(
    `
    let out = "none";
    try {
      out = await Promise.all([
        fetchItem({ id: "bad" }),
      ]).catch(e => "caught:" + String(e));
    } catch(e) {
      out = "outer-catch:" + String(e);
    }
    out
    `,
    {
      fetchItem: {
        description: "Fetch item; bad throws.",
        parameters: { id: { type: "string" } },
        execute: async ({ id }) => {
          if (id === "bad") throw new Error("fetch-error");
          return `r-${id}`;
        },
      },
    },
    { autoFix: true }
  );
  // BUG: .catch() is bypassed; error escapes to outer try/catch (or autoFix error)
  const output = String(result.output ?? result.error ?? "");
  assert.ok(
    output.includes("outer-catch") || output.includes("fetch-error"),
    `BUG confirmed: .catch() bypassed for tool rejection. Got: "${output}"`
  );
  // Ensure it did NOT say "caught:" (which would mean .catch() worked)
  assert.ok(!output.startsWith("caught:"), `BUG: .catch() should NOT intercept tool rejection, got: "${output}"`);

  // Contrast: pure Promise.reject IS caught by .catch()
  const r2 = await execute(
    `
    await Promise.all([Promise.reject("rej")]).catch(e => "pure-caught:" + e)
    `,
    {}
  );
  assert.equal(r2.output, "pure-caught:rej");
});

// ---------------------------------------------------------------------------
// 24. Multi-layer async nesting: async fn calls async fn calls tool
// ---------------------------------------------------------------------------
await check("multi-layer async nesting: 3 levels deep", async () => {
  const result = await execute(
    `
    async function level3(x) {
      return await compute({ value: x });
    }
    async function level2(x) {
      const a = await level3(x);
      return a * 2;
    }
    async function level1(x) {
      const b = await level2(x);
      return b + 1;
    }
    await level1(5)
    `,
    {
      compute: {
        description: "Compute a value.",
        parameters: { value: { type: "number" } },
        execute: async ({ value }) => value + 1,
      },
    }
  );
  // compute(5) => 6, *2 => 12, +1 => 13
  assert.equal(result.output, 13);
});

// ---------------------------------------------------------------------------
// 25. toolCalls metadata: args/input/result/error fields
// ---------------------------------------------------------------------------
await check("toolCalls metadata: name, args/input, result, error populated correctly", async () => {
  const result = await execute(
    `
    const r1 = await echo({ msg: "hello" });
    let r2;
    try {
      r2 = await failing();
    } catch {
      r2 = "caught";
    }
    r1 + "|" + r2
    `,
    {
      echo: {
        description: "Echo a message.",
        parameters: { msg: { type: "string" } },
        execute: async ({ msg }) => `echo:${msg}`,
      },
      failing: {
        description: "Always fails.",
        parameters: {},
        execute: async () => { throw new Error("fail-msg"); },
      },
    }
  );
  assert.equal(result.output, "echo:hello|caught");
  assert.equal(result.toolCalls.length, 2);

  const tc0 = result.toolCalls[0];
  assert.equal(tc0.name, "echo");
  assert.equal(tc0.result, "echo:hello");
  assert.ok(!tc0.error, "success call should not have .error");

  const tc1 = result.toolCalls[1];
  assert.equal(tc1.name, "failing");
  assert.match(String(tc1.error), /fail-msg/);
  assert.ok(tc1.result === undefined || tc1.result === null, "failing call should not have .result");
});

// ---------------------------------------------------------------------------
// 26. No double-execution: each tool call happens exactly once in Promise.all
// ---------------------------------------------------------------------------
await check("Promise.all: no double-execution — each slot called exactly once", async () => {
  const callCount = {};
  const result = await execute(
    `
    await Promise.all([
      track({ id: "a" }),
      track({ id: "b" }),
      track({ id: "a" }),
    ])
    "done"
    `,
    {
      track: {
        description: "Track a call.",
        parameters: { id: { type: "string" } },
        execute: async ({ id }) => {
          callCount[id] = (callCount[id] ?? 0) + 1;
          return id;
        },
      },
    }
  );
  assert.equal(result.output, "done");
  assert.equal(callCount.a, 2);
  assert.equal(callCount.b, 1);
  assert.equal(result.toolCalls.length, 3);
});

// ---------------------------------------------------------------------------
// 27. Await inside conditional branches
// ---------------------------------------------------------------------------
await check("await inside if/else branches executes only the matching branch", async () => {
  let callLog = [];
  const result = await execute(
    `
    async function route(flag) {
      if (flag) {
        return await pathA();
      } else {
        return await pathB();
      }
    }
    const r1 = await route(true);
    const r2 = await route(false);
    r1 + "|" + r2
    `,
    {
      pathA: {
        description: "Path A.",
        parameters: {},
        execute: async () => { callLog.push("A"); return "took-A"; },
      },
      pathB: {
        description: "Path B.",
        parameters: {},
        execute: async () => { callLog.push("B"); return "took-B"; },
      },
    }
  );
  assert.equal(result.output, "took-A|took-B");
  assert.deepEqual(callLog, ["A", "B"]);
});

// ---------------------------------------------------------------------------
// 28. Re-throw with new Error — error propagates to host as rejection
// ---------------------------------------------------------------------------
await check("re-throw new Error from catch propagates to host", async () => {
  await assert.rejects(
    () => execute(
      `
      try {
        await alwaysFails();
      } catch (e) {
        throw new Error("wrapped: " + String(e));
      }
      `,
      {
        alwaysFails: {
          description: "Always throws.",
          parameters: {},
          execute: async () => { throw new Error("inner-error"); },
        },
      }
    ),
    (err) => {
      assert.match(String(err.message), /wrapped/);
      return true;
    }
  );
});

// ---------------------------------------------------------------------------
// 29. finally always runs — success path
// ---------------------------------------------------------------------------
await check("try/finally without catch: finally runs on success", async () => {
  const result = await execute(
    `
    let cleaned = false;
    let val;
    try {
      val = await getNum();
    } finally {
      cleaned = true;
    }
    val + "|" + cleaned
    `,
    {
      getNum: {
        description: "Returns a number.",
        parameters: {},
        execute: async () => 77,
      },
    }
  );
  assert.equal(result.output, "77|true");
});

// ---------------------------------------------------------------------------
// 30. Promise.all — empty array returns empty array
// ---------------------------------------------------------------------------
await check("Promise.all([]) returns empty array", async () => {
  const result = await execute(
    `
    const r = await Promise.all([]);
    Array.isArray(r) + "|" + r.length
    `,
    {}
  );
  assert.equal(result.output, "true|0");
});

// ---------------------------------------------------------------------------
// 31. Promise.resolve wrapping a tool call result
// ---------------------------------------------------------------------------
await check("Promise.resolve(toolCallResult) wraps a resolved value correctly", async () => {
  const result = await execute(
    `
    const raw = await getVal({ key: "k" });
    const wrapped = await Promise.resolve(raw);
    wrapped
    `,
    {
      getVal: {
        description: "Get value.",
        parameters: { key: { type: "string" } },
        execute: async ({ key }) => `fetched-${key}`,
      },
    }
  );
  assert.equal(result.output, "fetched-k");
});

// ---------------------------------------------------------------------------
// 32. BUG-SORT: Array.sort() does not mutate the original array in place.
//     spec: arr.sort(cmp) mutates arr, returns arr (same reference)
//     sandbox: arr unchanged, sort() returns a NEW sorted copy
// ---------------------------------------------------------------------------
await check("BUG-SORT: Array.sort() returns sorted copy but doesn't mutate original", async () => {
  const result = await execute(
    `
    const arr = [3, 1, 4, 1, 5, 2];
    const ret = arr.sort((a, b) => a - b);
    "arr:" + arr.join(",") + "|ret:" + ret.join(",") + "|same-ref:" + (arr === ret)
    `,
    {},
    { autoFix: true }
  );
  // Expected (JS spec): arr:1,1,2,3,4,5|ret:1,1,2,3,4,5|same-ref:true
  // Actual (bug): arr:3,1,4,1,5,2|ret:1,1,2,3,4,5|same-ref:false
  assert.equal(result.output, "arr:3,1,4,1,5,2|ret:1,1,2,3,4,5|same-ref:false",
    `BUG confirmed: sort should mutate arr in place (same-ref:true, arr sorted), got: "${result.output}"`);

  // Also confirm: sort() with no comparator doesn't sort original either
  const r2 = await execute(
    `
    const arr = ["banana", "apple", "cherry"];
    arr.sort();
    arr.join(",")
    `,
    {}
  );
  // Expected: "apple,banana,cherry"
  // Actual (bug): "banana,apple,cherry" (original order, no mutation)
  assert.equal(r2.output, "banana,apple,cherry",
    `BUG confirmed: arr.sort() with no comparator also doesn't mutate. Got: "${r2.output}"`);
});

// ---------------------------------------------------------------------------
// 33. Concurrent batch then filter + sort (realistic data pipeline)
//     Uses workaround: const sorted = active.sort(cmp) instead of active.sort(cmp)
// ---------------------------------------------------------------------------
await check("batch fetch then filter+sort: pipeline with sort return-value workaround", async () => {
  const result = await execute(
    `
    const ids = ["i5", "i2", "i8", "i1", "i3"];
    const records = [];
    for (const id of ids) {
      const r = await fetchRecord({ id });
      records.push(r);
    }
    const active = records.filter(r => r.active);
    // Workaround: use return value of sort, not original array
    const sorted = active.sort((a, b) => a.priority - b.priority);
    sorted.map(r => r.id + ":" + r.priority).join(",")
    `,
    {
      fetchRecord: {
        description: "Fetch a record.",
        parameters: { id: { type: "string" } },
        execute: async ({ id }) => {
          const db = {
            i1: { id: "i1", active: true, priority: 1 },
            i2: { id: "i2", active: false, priority: 2 },
            i3: { id: "i3", active: true, priority: 3 },
            i5: { id: "i5", active: true, priority: 5 },
            i8: { id: "i8", active: false, priority: 8 },
          };
          return db[id];
        },
      },
    }
  );
  assert.equal(result.output, "i1:1,i3:3,i5:5");
});

// ---------------------------------------------------------------------------
// 34. BUG-NULLISH-IN-CALL: fn(a ?? b) crashes — ?? inside function call args is mis-parsed
//     ANY call with ?? as an argument fails: fn(x ?? y) is treated as (fn(x)) ?? y,
//     then tries to invoke x as a function, throwing "x is not a function".
//     Works fine: const v = a ?? b; fn(v)  — just can't inline ?? in the arg position.
//     Also affects builtins: String(x??y), Number(x??y), Math.abs(x??y), etc.
// ---------------------------------------------------------------------------
await check("BUG-NULLISH-IN-CALL: fn(a ?? b) crashes — ?? inside function arg is mis-parsed", async () => {
  // Confirm the bug exists
  const bugResult = await execute(
    `
    function id(x) { return x; }
    let out = "none";
    try {
      out = id(null ?? "fallback");
    } catch(e) {
      out = "threw:" + String(e);
    }
    out
    `,
    {},
    { autoFix: true }
  );
  // BUG: should be "fallback"; actual: "threw:null is not a function"
  const output = String(bugResult.output ?? bugResult.error ?? "");
  assert.ok(
    output.includes("threw:") || (bugResult.error && String(bugResult.error).includes("not a function")),
    `BUG confirmed: fn(null ?? 'fallback') should return "fallback" but crashes. Got: "${output}"`
  );

  // Confirm workaround: assign to variable first
  const fixResult = await execute(
    `
    function id(x) { return x; }
    const arg = null ?? "fallback";
    id(arg)
    `,
    {}
  );
  assert.equal(fixResult.output, "fallback");

  // Confirm that || inside function args works fine (not a bug)
  const orResult = await execute(
    `
    function id(x) { return x; }
    id(null || "fallback-or")
    `,
    {}
  );
  assert.equal(orResult.output, "fallback-or");
});

// ---------------------------------------------------------------------------
// Summary
// ---------------------------------------------------------------------------
const failed = results.filter(r => !r[1]);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
if (failed.length) {
  console.log("\nFailed:");
  for (const [name, , msg] of failed) {
    console.log(`  ✗ ${name}: ${msg}`);
  }
}
