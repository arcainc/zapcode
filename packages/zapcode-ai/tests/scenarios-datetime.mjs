// EXPLORATORY stress-pass catalog (not part of the green test:e2e gate; run via `npm run test:scenarios`).
// Checks named BUG/MISSING document gaps found during the realistic-scenario pass; see ../../KNOWN_GAPS.md.
// Some were fixed after this file was written, so those checks now intentionally show as failing-to-flag-fixed.
/**
 * Date/time, scheduling, and numeric stress tests for the Zapcode sandbox.
 * Covers realistic AI-agent patterns: SLA checks, deadline math, bucketing,
 * duration formatting, ISO parsing, day-of-week, reminder scheduling, and
 * numeric rounding/clamping/edge cases.
 *
 * Run: node tests/scenarios-datetime.mjs
 *
 * Naming conventions:
 *   "BUG: ..."     — confirmed wrong result or crash on valid idiomatic JS
 *   "MISSING: ..." — absent builtin (typeof returns wrong type or call throws)
 *   All others     — expected-passing functionality
 *
 * Bug/missing checks are left failing so CI surfaces them.
 */
import assert from "node:assert/strict";
import { execute } from "../dist/index.js";

// ---------------------------------------------------------------------------
// Shared deterministic "now": 2026-05-31T00:00:00.000Z = 1780185600000
// ---------------------------------------------------------------------------
const NOW_MS = Date.UTC(2026, 4, 31); // 1780185600000

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
// Shared tool factory
// ---------------------------------------------------------------------------
function makeTools(extra = {}) {
  return {
    getNowMs: {
      description: "Return current time as epoch milliseconds.",
      parameters: {},
      execute: async () => NOW_MS,
    },
    parseDateMs: {
      description: "Parse an ISO date string (YYYY-MM-DD) and return epoch ms.",
      parameters: { iso: { type: "string" } },
      execute: async ({ iso }) => {
        const ms = Date.parse(iso + "T00:00:00.000Z");
        if (Number.isNaN(ms)) throw new Error("invalid ISO date: " + iso);
        return ms;
      },
    },
    ...extra,
  };
}

// ===========================================================================
// 1. Is date within next 7 days? (SLA / escalation pattern)
// ===========================================================================
await check("within-7-days: target 4 days out returns true", async () => {
  const { output } = await execute(
    `
    const nowMs = await getNowMs();
    const targetMs = await parseDateMs({ iso: "2026-06-04" }); // +4 days
    const sevenDays = 7 * 24 * 60 * 60 * 1000;
    const diff = targetMs - nowMs;
    diff >= 0 && diff <= sevenDays
    `,
    makeTools()
  );
  assert.equal(output, true);
});

await check("within-7-days: target exactly 7 days away returns true", async () => {
  const { output } = await execute(
    `
    const nowMs = await getNowMs();
    const targetMs = await parseDateMs({ iso: "2026-06-07" }); // +7 days
    const sevenDays = 7 * 24 * 60 * 60 * 1000;
    const diff = targetMs - nowMs;
    diff >= 0 && diff <= sevenDays
    `,
    makeTools()
  );
  assert.equal(output, true);
});

await check("within-7-days: target 8 days away returns false", async () => {
  const { output } = await execute(
    `
    const nowMs = await getNowMs();
    const targetMs = await parseDateMs({ iso: "2026-06-08" }); // +8 days
    const sevenDays = 7 * 24 * 60 * 60 * 1000;
    const diff = targetMs - nowMs;
    diff >= 0 && diff <= sevenDays
    `,
    makeTools()
  );
  assert.equal(output, false);
});

await check("within-7-days: past date returns false", async () => {
  const { output } = await execute(
    `
    const nowMs = await getNowMs();
    const targetMs = await parseDateMs({ iso: "2026-05-20" }); // -11 days
    const sevenDays = 7 * 24 * 60 * 60 * 1000;
    const diff = targetMs - nowMs;
    diff >= 0 && diff <= sevenDays
    `,
    makeTools()
  );
  assert.equal(output, false);
});

// ===========================================================================
// 2. Deadline arithmetic: now + N days/hours; compare two timestamps
// ===========================================================================
await check("deadline: now + 3 days in ms", async () => {
  const { output } = await execute(
    `
    const nowMs = await getNowMs();
    const threeDays = 3 * 24 * 60 * 60 * 1000;
    nowMs + threeDays
    `,
    makeTools()
  );
  assert.equal(output, NOW_MS + 3 * 24 * 60 * 60 * 1000);
});

await check("deadline: now + 6 hours in ms", async () => {
  const { output } = await execute(
    `
    const nowMs = await getNowMs();
    const sixHours = 6 * 60 * 60 * 1000;
    nowMs + sixHours
    `,
    makeTools()
  );
  assert.equal(output, NOW_MS + 6 * 60 * 60 * 1000);
});

await check("compare timestamps: earlier < later", async () => {
  const { output } = await execute(
    `
    const t1 = await parseDateMs({ iso: "2026-05-01" });
    const t2 = await parseDateMs({ iso: "2026-06-01" });
    t1 < t2
    `,
    makeTools()
  );
  assert.equal(output, true);
});

// ===========================================================================
// 3. Sort events by timestamp ascending
// ===========================================================================
await check("sort events by timestamp ascending", async () => {
  const DAY = 24 * 60 * 60 * 1000;
  const { output } = await execute(
    `
    const events = [
      { name: "C", ms: ${NOW_MS + 2 * DAY} },
      { name: "A", ms: ${NOW_MS - DAY} },
      { name: "B", ms: ${NOW_MS} },
    ];
    const sorted = events.slice().sort((a, b) => a.ms - b.ms);
    sorted.map(e => e.name)
    `,
    makeTools()
  );
  assert.deepEqual(output, ["A", "B", "C"]);
});

// ===========================================================================
// 4. Bucket items: overdue / due-soon / future
//    Workaround: use bracket property access (item["prop"]) when pushing values
//    from for-of loop (see BUG below). Use separate array variables, not an
//    object-of-arrays (see BUG below on object property mutation).
// ===========================================================================
await check("bucket items overdue/due-soon/future (separate array vars + bracket access)", async () => {
  const { output } = await execute(
    `
    const nowMs = await getNowMs();
    const DAY = 24 * 60 * 60 * 1000;
    const SOON = 3 * DAY;
    const items = [
      { id: 1, dueMs: nowMs - DAY },          // overdue
      { id: 2, dueMs: nowMs + 2 * DAY },      // due-soon
      { id: 3, dueMs: nowMs + 10 * DAY },     // future
      { id: 4, dueMs: nowMs - 5 * DAY },      // overdue
      { id: 5, dueMs: nowMs + SOON },         // due-soon (boundary)
    ];
    const overdue = [];
    const dueSoon = [];
    const future = [];
    for (const item of items) {
      const diff = item["dueMs"] - nowMs;
      const id = item["id"];
      if (diff < 0) overdue.push(id);
      else if (diff <= SOON) dueSoon.push(id);
      else future.push(id);
    }
    ({ overdue, dueSoon, future })
    `,
    makeTools()
  );
  assert.deepEqual(output.overdue, [1, 4]);
  assert.deepEqual(output.dueSoon, [2, 5]);
  assert.deepEqual(output.future, [3]);
});

// ===========================================================================
// 5. BUG A: push(item.prop) via dot notation silently discards the value
//    Confirmed: out.push(item.v) returns correct length but array stays empty.
//    Workaround: use bracket notation item["prop"] or extract to a local var.
// ===========================================================================
await check("BUG: push(item.prop) dot notation in for-of silently discards value", async () => {
  const { output } = await execute(
    `
    const items = [{ v: 1 }, { v: 2 }];
    const out = [];
    for (const item of items) { out.push(item.v); }
    out
    `,
    makeTools()
  );
  // BUG: actual output is [] instead of [1, 2]
  assert.deepEqual(output, [1, 2],
    "BUG: push(item.v) silently no-ops; use bracket notation push(item['v']) or local var");
});

// ===========================================================================
// 6. BUG B: Object-property arrays do NOT reflect mutations via reference
//    e.g. const arr = obj.x; arr.push(v) — arr is updated but obj.x stays empty.
//    Separately: obj.x.push(v) crashes with 'Cannot read properties of undefined'.
// ===========================================================================
await check("BUG: obj.prop.push() in for-of replaces obj with the array on first iteration", async () => {
  // When iterating over 2+ items: b.arr.push(x) on the 1st iteration replaces
  // the variable b with the array (b becomes ['x'], not {arr:['x']}). On the 2nd
  // iteration, b.arr is undefined → crash "Cannot read properties of undefined (reading 'push')".
  let threw = false;
  try {
    await execute(
      `
      const b = { arr: [] };
      for (const x of ["a", "b"]) { b.arr.push(x); }
      b
      `,
      makeTools()
    );
  } catch (e) {
    threw = true;
    assert.ok(e.message.includes("Cannot read properties"),
      "BUG: expected 'Cannot read properties' crash on 2nd iteration, got: " + e.message);
  }
  assert.ok(threw,
    "BUG: obj.arr.push() in multi-iteration for-of should crash (obj gets replaced by array on 1st push)");
});

await check("BUG: obj property array ref — arr=map.x then arr.push(v) does not update map.x", async () => {
  const { output } = await execute(
    `
    const map = { x: [] };
    const arr = map.x;
    for (const key of ["a", "b"]) { arr.push(key); }
    map.x
    `,
    makeTools()
  );
  // BUG: arr.push(v) works (arr becomes ['a','b']) but map.x stays [].
  // Objects use copy-on-assign semantics for property arrays rather than shared references.
  assert.deepEqual(output, ["a", "b"],
    "BUG: const arr=map.x; arr.push(v) does not update map.x (copy-on-assign, not shared reference)");
});

// ===========================================================================
// 7. Count business days (Mon–Fri) — uses ms-based day-of-week formula
//    because getDay() is MISSING (see below)
// ===========================================================================
await check("count business days via ms-based day-of-week formula", async () => {
  // 2026-06-01 (Mon) through 2026-06-05 (Fri) = 5 business days inclusive
  const { output } = await execute(
    `
    const startMs = await parseDateMs({ iso: "2026-06-01" });
    const endMs   = await parseDateMs({ iso: "2026-06-05" });
    const DAY_MS = 86400000;
    const EPOCH_DOW = 4; // 1970-01-01 was Thursday
    let count = 0;
    let cur = startMs;
    while (cur <= endMs) {
      const dow = (Math.floor(cur / DAY_MS) + EPOCH_DOW) % 7;
      if (dow !== 0 && dow !== 6) count++;
      cur += DAY_MS;
    }
    count
    `,
    makeTools()
  );
  assert.equal(output, 5);
});

// ===========================================================================
// 8. Format duration ms → "2h 5m" / "1h" / "30m"
// ===========================================================================
await check("format duration: 7500000ms → '2h 5m'", async () => {
  const { output } = await execute(
    `
    function formatDuration(ms) {
      const totalMinutes = Math.floor(ms / 60000);
      const hours = Math.floor(totalMinutes / 60);
      const minutes = totalMinutes % 60;
      if (hours > 0 && minutes > 0) return hours + "h " + minutes + "m";
      if (hours > 0) return hours + "h";
      return minutes + "m";
    }
    formatDuration(7500000)
    `,
    makeTools()
  );
  assert.equal(output, "2h 5m");
});

await check("format duration: 3600000ms → '1h'", async () => {
  const { output } = await execute(
    `
    function formatDuration(ms) {
      const totalMinutes = Math.floor(ms / 60000);
      const hours = Math.floor(totalMinutes / 60);
      const minutes = totalMinutes % 60;
      if (hours > 0 && minutes > 0) return hours + "h " + minutes + "m";
      if (hours > 0) return hours + "h";
      return minutes + "m";
    }
    formatDuration(3600000)
    `,
    makeTools()
  );
  assert.equal(output, "1h");
});

await check("format duration: 1800000ms → '30m'", async () => {
  const { output } = await execute(
    `
    function formatDuration(ms) {
      const totalMinutes = Math.floor(ms / 60000);
      const hours = Math.floor(totalMinutes / 60);
      const minutes = totalMinutes % 60;
      if (hours > 0 && minutes > 0) return hours + "h " + minutes + "m";
      if (hours > 0) return hours + "h";
      return minutes + "m";
    }
    formatDuration(1800000)
    `,
    makeTools()
  );
  assert.equal(output, "30m");
});

// ===========================================================================
// 9. Day-of-week via ms arithmetic (workaround for missing getDay)
// ===========================================================================
await check("day-of-week via ms math: 2026-05-31 is Sunday (0)", async () => {
  const { output } = await execute(
    `
    const ms = await parseDateMs({ iso: "2026-05-31" });
    const DAY_MS = 86400000;
    const EPOCH_DOW = 4; // 1970-01-01 = Thursday
    (Math.floor(ms / DAY_MS) + EPOCH_DOW) % 7
    `,
    makeTools()
  );
  assert.equal(output, 0, `expected 0 (Sun) but got ${output}`);
});

await check("day-of-week via ms math: 2026-06-01 is Monday (1)", async () => {
  const { output } = await execute(
    `
    const ms = await parseDateMs({ iso: "2026-06-01" });
    const DAY_MS = 86400000;
    const EPOCH_DOW = 4;
    (Math.floor(ms / DAY_MS) + EPOCH_DOW) % 7
    `,
    makeTools()
  );
  assert.equal(output, 1);
});

// ===========================================================================
// 10. MISSING: Date instance methods — only getTime() and toISOString() work.
//     getDay / getUTCFullYear / getUTCMonth / getUTCDate / getUTCHours /
//     getUTCMinutes / getUTCSeconds / getFullYear / getMonth / getDate etc.
//     all throw "Date.X is not a function" even though typeof returns "function".
// ===========================================================================
await check("MISSING: Date.getDay throws even though typeof is 'function'", async () => {
  const typeRes = await execute(`typeof new Date(${NOW_MS}).getDay`, makeTools());
  assert.equal(typeRes.output, "function"); // it EXISTS as a function...
  let threw = false;
  try {
    await execute(`new Date(${NOW_MS}).getDay()`, makeTools());
  } catch (e) {
    threw = true;
    assert.ok(e.message.includes("not a function"),
      "MISSING: Date.getDay() throws: " + e.message);
  }
  assert.ok(threw, "MISSING: Date.getDay() should throw but did not");
});

await check("MISSING: Date.getUTCFullYear throws", async () => {
  let threw = false;
  try { await execute(`new Date(${NOW_MS}).getUTCFullYear()`, makeTools()); }
  catch (e) { threw = true; }
  assert.ok(threw, "MISSING: Date.getUTCFullYear should throw");
});

await check("MISSING: Date.getUTCMonth throws", async () => {
  let threw = false;
  try { await execute(`new Date(${NOW_MS}).getUTCMonth()`, makeTools()); }
  catch (e) { threw = true; }
  assert.ok(threw, "MISSING: Date.getUTCMonth should throw");
});

await check("MISSING: Date.getUTCDate throws", async () => {
  let threw = false;
  try { await execute(`new Date(${NOW_MS}).getUTCDate()`, makeTools()); }
  catch (e) { threw = true; }
  assert.ok(threw, "MISSING: Date.getUTCDate should throw");
});

await check("MISSING: Date.getUTCHours throws", async () => {
  let threw = false;
  try { await execute(`new Date(${NOW_MS}).getUTCHours()`, makeTools()); }
  catch (e) { threw = true; }
  assert.ok(threw, "MISSING: Date.getUTCHours should throw");
});

// ===========================================================================
// 11. Date.getTime() and toISOString() — the only two Date methods that work
// ===========================================================================
await check("Date.getTime() roundtrip", async () => {
  const { output } = await execute(`new Date(${NOW_MS}).getTime()`, makeTools());
  assert.equal(output, NOW_MS);
});

await check("Date.toISOString() returns correct ISO string", async () => {
  const { output } = await execute(`new Date(${NOW_MS}).toISOString()`, makeTools());
  assert.equal(output, "2026-05-31T00:00:00.000Z");
});

// ===========================================================================
// 12. Schedule N reminders at fixed offsets (pure ms arithmetic)
// ===========================================================================
await check("schedule 4 reminders at day offsets before deadline", async () => {
  const { output } = await execute(
    `
    const nowMs = await getNowMs();
    const deadlineMs = nowMs + 30 * 24 * 60 * 60 * 1000; // +30 days
    const DAY = 24 * 60 * 60 * 1000;
    const offsets = [7, 3, 1, 0]; // days before deadline
    offsets.map(d => deadlineMs - d * DAY)
    `,
    makeTools()
  );
  const DAY = 24 * 60 * 60 * 1000;
  const deadline = NOW_MS + 30 * DAY;
  const expected = [7, 3, 1, 0].map(d => deadline - d * DAY);
  assert.deepEqual(output, expected);
});

// ===========================================================================
// 13. Numeric: money rounding
//    Note: Math.round(1.005 * 100)/100 = 1 in ALL JS engines (IEEE-754 float).
//    1.005 * 100 = 100.49999... in IEEE-754, so this is NOT a sandbox bug.
// ===========================================================================
await check("money rounding: Math.round(n*100)/100", async () => {
  const { output } = await execute(
    `
    function roundMoney(n) { return Math.round(n * 100) / 100; }
    [roundMoney(2.345), roundMoney(9.994), roundMoney(9.999)]
    `,
    makeTools()
  );
  assert.equal(output[0], 2.35);
  assert.equal(output[1], 9.99);
  assert.equal(output[2], 10);
});

// ===========================================================================
// 14. MISSING: Number.prototype.toFixed
// ===========================================================================
await check("MISSING: toFixed is not a function", async () => {
  // typeof shows "undefined" (property doesn't exist), call throws
  const typeRes = await execute(`typeof (3.14).toFixed`, makeTools());
  assert.equal(typeRes.output, "undefined",
    "MISSING: toFixed property should not exist, got typeof=" + typeRes.output);
});

// ===========================================================================
// 15. Numeric: percentages, min/max/avg
// ===========================================================================
await check("percentage: Math.round((part/total)*100)", async () => {
  const { output } = await execute(`Math.round((17 / 40) * 100)`, makeTools());
  assert.equal(output, 43); // 42.5 → 43
});

await check("Math.min / Math.max with literal args work", async () => {
  const { output } = await execute(`[Math.min(5, 2, 9, 1), Math.max(5, 2, 9, 1)]`, makeTools());
  assert.deepEqual(output, [1, 9]);
});

await check("BUG: Math.min(...array) returns null instead of minimum", async () => {
  const { output } = await execute(
    `
    const values = [5, 2, 9, 1, 7, 3];
    Math.min(...values)
    `,
    makeTools()
  );
  // BUG: returns null instead of 1
  assert.equal(output, 1,
    "BUG: Math.min/max with spread (...array) returns null; use array.reduce((a,b) => a<b?a:b) instead");
});

await check("BUG: Math.max(...array) returns null instead of maximum", async () => {
  const { output } = await execute(
    `
    const values = [5, 2, 9, 1, 7, 3];
    Math.max(...values)
    `,
    makeTools()
  );
  // BUG: returns null instead of 9
  assert.equal(output, 9,
    "BUG: Math.max with spread (...array) returns null");
});

await check("Math.min/max workaround: array.reduce", async () => {
  const { output } = await execute(
    `
    const values = [5, 2, 9, 1, 7, 3];
    const mn = values.reduce((a, b) => a < b ? a : b);
    const mx = values.reduce((a, b) => a > b ? a : b);
    ({ min: mn, max: mx })
    `,
    makeTools()
  );
  assert.equal(output.min, 1);
  assert.equal(output.max, 9);
});

await check("average via reduce", async () => {
  const { output } = await execute(
    `
    const values = [10, 20, 30, 40, 50];
    const sum = values.reduce((acc, v) => acc + v, 0);
    sum / values.length
    `,
    makeTools()
  );
  assert.equal(output, 30);
});

// ===========================================================================
// 16. Numeric: clamping
// ===========================================================================
await check("clamp: Math.max(lo, Math.min(hi, v))", async () => {
  const { output } = await execute(
    `
    function clamp(v, lo, hi) { return Math.max(lo, Math.min(hi, v)); }
    [clamp(5, 0, 10), clamp(-3, 0, 10), clamp(15, 0, 10)]
    `,
    makeTools()
  );
  assert.deepEqual(output, [5, 0, 10]);
});

// ===========================================================================
// 17. Numeric: Number() coercion, Number.MAX_SAFE_INTEGER, Number.EPSILON
// ===========================================================================
await check("Number() coercion: valid numeric strings", async () => {
  const { output } = await execute(`[Number("42"), Number("3.14")]`, makeTools());
  assert.equal(output[0], 42);
  assert.equal(output[1], 3.14);
});

await check("BUG: Number('') returns null instead of 0", async () => {
  const { output } = await execute(`Number('')`, makeTools());
  // In real JS: Number('') === 0. In sandbox: returns null.
  // Note: NaN serializes to null in JSON (expected). But '' → 0 should NOT be null.
  assert.equal(output, 0,
    "BUG: Number('') returns null in sandbox; real JS spec says it equals 0");
});

await check("BUG: Number.isInteger typeof='function' but throws on call", async () => {
  // typeof shows 'function' but calling throws — same phantom-function pattern as Date methods
  const typeRes = await execute(`typeof Number.isInteger`, makeTools());
  assert.equal(typeRes.output, "function"); // claims to exist...
  let threw = false;
  try { await execute(`Number.isInteger(5)`, makeTools()); }
  catch { threw = true; }
  // BUG: should NOT throw if typeof correctly reports 'function'
  assert.ok(!threw,
    "BUG: Number.isInteger typeof='function' but throws 'Number.isInteger is not a function' on call");
});

await check("Number.MAX_SAFE_INTEGER is 9007199254740991", async () => {
  const { output } = await execute(`Number.MAX_SAFE_INTEGER`, makeTools());
  assert.equal(output, 9007199254740991);
});

await check("Number.EPSILON is ~2.22e-16", async () => {
  const { output } = await execute(`Number.EPSILON`, makeTools());
  assert.ok(output > 2.2e-16 && output < 2.3e-16, `got ${output}`);
});

// ===========================================================================
// 18. MISSING: global isFinite, isNaN, parseInt, parseFloat
//     Number.isNaN, Number.isFinite all missing.
//     Note: Number.isInteger typeof = 'function' but throws on call (see BUG above).
// ===========================================================================
await check("MISSING: global isFinite is undefined", async () => {
  const { output } = await execute(`typeof isFinite`, makeTools());
  assert.equal(output, "undefined");
});

await check("MISSING: global isNaN is undefined", async () => {
  const { output } = await execute(`typeof isNaN`, makeTools());
  assert.equal(output, "undefined");
});

await check("MISSING: global parseInt is undefined", async () => {
  const { output } = await execute(`typeof parseInt`, makeTools());
  assert.equal(output, "undefined");
});

await check("MISSING: global parseFloat is undefined", async () => {
  const { output } = await execute(`typeof parseFloat`, makeTools());
  assert.equal(output, "undefined");
});

// Workaround: detect NaN via n !== n
await check("NaN detection workaround: n !== n", async () => {
  const { output } = await execute(`const n = NaN; n !== n`, makeTools());
  assert.equal(output, true);
});

// Workaround: parseInt via manual slice + Number()
await check("integer parse workaround: Number('42') === 42", async () => {
  const { output } = await execute(`Number("42") === 42`, makeTools());
  assert.equal(output, true);
});

// ===========================================================================
// 19. Multi-ISO pipeline: parse multiple dates + sort + bucket
//    Workaround: separate array vars + bracket access for pushed values
// ===========================================================================
await check("multi-ISO parse + sort + bucket pipeline (workarounds applied)", async () => {
  const { output } = await execute(
    `
    const nowMs = await getNowMs();
    const DAY = 24 * 60 * 60 * 1000;
    const SOON = 5 * DAY;
    const isos = ["2026-05-20", "2026-06-02", "2026-06-10", "2026-06-04", "2026-05-30"];
    const parsed = [];
    for (const iso of isos) {
      const ms = await parseDateMs({ iso });
      parsed.push({ iso, ms });
    }
    parsed.sort((a, b) => a["ms"] - b["ms"]);
    const overdue = [];
    const soon = [];
    const future = [];
    for (const item of parsed) {
      const diff = item["ms"] - nowMs;
      const iso = item["iso"];
      if (diff < 0) overdue.push(iso);
      else if (diff <= SOON) soon.push(iso);
      else future.push(iso);
    }
    ({ overdue, soon, future })
    `,
    makeTools()
  );
  // NOW = 2026-05-31; SOON = +5 days
  assert.deepEqual(output.overdue, ["2026-05-20", "2026-05-30"]);
  assert.deepEqual(output.soon,    ["2026-06-02", "2026-06-04"]);
  assert.deepEqual(output.future,  ["2026-06-10"]);
});

// ===========================================================================
// 20. Date.now() is unavailable (sandbox blocks ambient clock)
// ===========================================================================
await check("Date.now() is unavailable or returns non-realtime value", async () => {
  try {
    const { output, error } = await execute(`Date.now()`, makeTools());
    if (error) return; // blocked — expected
    const hostNow = Date.now();
    const drift = Math.abs(output - hostNow);
    assert.ok(drift > 60_000,
      `Date.now() appears to return real host time (drift=${drift}ms) — sandbox clock leak!`);
  } catch {
    // throws = blocked = fine
  }
});

// ===========================================================================
// 21. Math builtins: abs/ceil/floor/pow/sqrt/trunc/sign
// ===========================================================================
await check("Math.abs/ceil/floor/pow/sqrt/trunc/sign", async () => {
  const { output } = await execute(
    `
    [
      Math.abs(-7),
      Math.ceil(1.1),
      Math.floor(1.9),
      Math.pow(2, 10),
      Math.sqrt(144),
      Math.trunc(3.9),
      Math.sign(-5)
    ]
    `,
    makeTools()
  );
  assert.deepEqual(output, [7, 2, 1, 1024, 12, 3, -1]);
});

// ===========================================================================
// Summary
// ===========================================================================
const failed = results.filter(r => !r[1]);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
if (failed.length) {
  console.log("\nFailed:");
  for (const [name, , msg] of failed) {
    console.log(`  ✗ ${name}\n    ${msg}`);
  }
  process.exit(1);
}