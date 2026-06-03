/**
 * e2e: `Promise.all([tool(), tool(), ...])` in agent code suspends once with
 * the whole batch and the host runs the calls concurrently. Runs real
 * TypeScript in the sandbox.
 */
import assert from "node:assert/strict";
import { execute } from "../dist/index.js";
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

console.log("parallel e2e");

await test("Promise.all of tool calls runs them concurrently and preserves order", async () => {
  let inFlight = 0;
  let maxInFlight = 0;
  const tools = {
    fetchOne: {
      description: "Fetch one key.",
      parameters: { key: { type: "string" } },
      execute: async ({ key }) => {
        inFlight++;
        maxInFlight = Math.max(maxInFlight, inFlight);
        await new Promise(r => setTimeout(r, 20));
        inFlight--;
        return `v:${key}`;
      },
    },
  };

  const result = await execute(
    `
    const out = await Promise.all([fetchOne("a"), fetchOne("b"), fetchOne("c")]);
    out.join(",")
  `,
    tools
  );

  assert.equal(result.output, "v:a,v:b,v:c");
  assert.equal(result.toolCalls.length, 3);
  // All three were resolved in a single batch → they overlapped on the host.
  assert.ok(maxInFlight >= 2, `expected concurrent execution, max in-flight was ${maxInFlight}`);
});

await test("a failing call in Promise.all is catchable in guest code", async () => {
  const tools = {
    fetchOne: {
      description: "Fetch one key; 'bad' fails.",
      parameters: { key: { type: "string" } },
      execute: async ({ key }) => {
        if (key === "bad") throw new Error("boom");
        return `v:${key}`;
      },
    },
  };

  const result = await execute(
    `
    let out;
    try {
      await Promise.all([fetchOne("a"), fetchOne("bad")]);
      out = "no-error";
    } catch (e) {
      out = "caught:" + e;
    }
    out
  `,
    tools
  );
  assert.equal(result.output, "caught:boom");
});

await test("session batch survives dump/load and resumeMany", () => {
  const session = ZapcodeSessionHandle.create({ externalFunctions: ["fetchOne"] });
  const suspended = session.runChunk(
    `const out = await Promise.all([fetchOne("a"), fetchOne("b")]); out.join("/")`
  );
  assert.equal(suspended.completed, false);
  assert.equal(suspended.kind, "suspended_many");
  assert.equal(suspended.combinator, "all");
  assert.equal(suspended.calls.length, 2);

  // Ship across a boundary, run the calls "in parallel" on the host, resume.
  const resumed = ZapcodeSessionHandle.load(suspended.session).resumeMany(["A", "B"]);
  assert.equal(resumed.completed, true);
  assert.equal(resumed.output, "A/B");
});

// ── N1: Promise.race honors REAL settle timing (fastest tool wins) ─────────

await test("Promise.race resolves with the value of the first tool to settle", async () => {
  // `fast` resolves well before `slow`; race must pick `fast` regardless of
  // array position. The delays make this a genuine timing test.
  const tools = {
    delay: {
      description: "Resolve `label` after `ms` milliseconds.",
      parameters: { label: { type: "string" }, ms: { type: "number" } },
      execute: async ({ label, ms }) => {
        await new Promise(r => setTimeout(r, ms));
        return label;
      },
    },
  };

  const result = await execute(
    `await Promise.race([delay({ label: "slow", ms: 60 }), delay({ label: "fast", ms: 5 })])`,
    tools
  );
  assert.equal(result.output, "fast");
});

await test("Promise.race rejection of the first-to-settle is catchable", async () => {
  const tools = {
    delay: {
      description: "Resolve or reject `label` after `ms` ms.",
      parameters: { label: { type: "string" }, ms: { type: "number" }, fail: { type: "boolean" } },
      execute: async ({ label, ms, fail }) => {
        await new Promise(r => setTimeout(r, ms));
        if (fail) throw new Error("rejected:" + label);
        return label;
      },
    },
  };

  const result = await execute(
    `
    try {
      const r = await Promise.race([
        delay({ label: "slow", ms: 60, fail: false }),
        delay({ label: "boom", ms: 5, fail: true }),
      ]);
      "value:" + r
    } catch (e) {
      "caught:" + e
    }
  `,
    tools
  );
  assert.equal(result.output, "caught:rejected:boom");
});

// ── N2: Promise.any skips rejections, returns first fulfilled ──────────────

await test("Promise.any skips a rejected call and returns the first fulfilled", async () => {
  const tools = {
    delay: {
      description: "Resolve or reject `label` after `ms` ms.",
      parameters: { label: { type: "string" }, ms: { type: "number" }, fail: { type: "boolean" } },
      execute: async ({ label, ms, fail }) => {
        await new Promise(r => setTimeout(r, ms));
        if (fail) throw new Error("rejected:" + label);
        return label;
      },
    },
  };

  // The fast one rejects; `any` must skip it and wait for the fulfilled one.
  const result = await execute(
    `await Promise.any([
      delay({ label: "boom", ms: 5, fail: true }),
      delay({ label: "ok", ms: 30, fail: false }),
    ])`,
    tools
  );
  assert.equal(result.output, "ok");
});

await test("Promise.any rejects (catchable) when every call rejects", async () => {
  const tools = {
    boom: {
      description: "Always reject.",
      parameters: { label: { type: "string" } },
      execute: async ({ label }) => {
        throw new Error("no:" + label);
      },
    },
  };

  const result = await execute(
    `
    try {
      await Promise.any([boom("a"), boom("b")]);
      "unexpected"
    } catch (e) {
      // AggregateError-shaped message from the host.
      String(e).includes("a") && String(e).includes("b") ? "caught-all" : "caught:" + e
    }
  `,
    tools
  );
  assert.equal(result.output, "caught-all");
});

// ── N3: Promise.allSettled reports per-element statuses, never rejects ─────

await test("Promise.allSettled reports fulfilled/rejected per element", async () => {
  const tools = {
    maybe: {
      description: "Resolve unless key is 'bad'.",
      parameters: { key: { type: "string" } },
      execute: async ({ key }) => {
        if (key === "bad") throw new Error("failed:" + key);
        return "ok:" + key;
      },
    },
  };

  const result = await execute(
    `
    const r = await Promise.allSettled([maybe("a"), maybe("bad"), maybe("c")]);
    r.map(x => x.status).join(",") + "|" +
      (r[0].value || "?") + "|" + (r[1].reason || "?")
  `,
    tools
  );
  assert.equal(result.output, "fulfilled,rejected,fulfilled|ok:a|failed:bad");
});

console.log(`\n${passed} parallel checks passed.`);
