// EXPLORATORY stress-pass catalog (not part of the green test:e2e gate; run via `npm run test:scenarios`).
// Checks named BUG/MISSING document gaps found during the realistic-scenario pass; see ../../KNOWN_GAPS.md.
// Some were fixed after this file was written, so those checks now intentionally show as failing-to-flag-fixed.
/**
 * ETL / data-processing stress tests for the Zapcode sandbox.
 *
 * Covers: reduce/group-by, dedup, merge, filter+sort+top-N, pagination,
 * flatMap, key rename, JSON round-trip, derived fields, bucketing,
 * parallel fan-out, dataset join, Object.entries, spread, destructuring,
 * Number()/String() coercions, plus probes for interpreter bugs and
 * missing builtins.
 *
 * Run: node tests/scenarios-data.mjs
 *
 * CONFIRMED BUGS (minimal repros in sections 101-106 below):
 *   BUG-A  obj = { arr: [] }; obj.arr.push(x) — object is replaced by push return value
 *   BUG-B  for-of loop body: inline r.prop inside push() returns empty — needs const intermediate
 *   BUG-C  new Map(arrayOfPairs) — constructor ignores pairs, all lookups null/false
 *   BUG-D  Arrow ({ a }) => expr — destructured name a resolves to the whole argument
 *   BUG-E  function({ a, b }) param destructuring — first param = whole obj, rest null
 *   BUG-F  for...of inline destructuring { id, name } — all props null
 *
 * MISSING BUILTINS (sections 201-207):
 *   parseInt, parseFloat, Object.fromEntries, Number.isFinite, Number.isNaN,
 *   structuredClone, Set constructor, Map.size (getter broken — returns "function"),
 *   Map.forEach, Map.entries(), Array.from(map.values()), spread over generator
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
// Shared fixtures
// ---------------------------------------------------------------------------
const SALES = [
  { id: 1, region: "west",  product: "widget", amount: 120, qty: 3 },
  { id: 2, region: "east",  product: "gadget", amount: 200, qty: 2 },
  { id: 3, region: "west",  product: "gadget", amount: 150, qty: 1 },
  { id: 4, region: "east",  product: "widget", amount: 90,  qty: 4 },
  { id: 5, region: "west",  product: "widget", amount: 80,  qty: 2 },
  { id: 6, region: "north", product: "gadget", amount: 310, qty: 5 },
];
function salesTools() {
  return {
    fetchSales: {
      description: "Return all sales records.",
      parameters: {},
      execute: async () => JSON.parse(JSON.stringify(SALES)),
    },
  };
}

// ===========================================================================
// SECTION 1 — ETL scenarios (use workarounds for known bugs so they pass)
// ===========================================================================

// 1. Group by region, sum amounts — basic reduce
await check("group-by reduce: sum amount by region", async () => {
  const r = await execute(
    `
    const sales = await fetchSales();
    const totals = sales.reduce((acc, row) => {
      acc[row.region] = (acc[row.region] || 0) + row.amount;
      return acc;
    }, {});
    totals
    `,
    salesTools()
  );
  assert.deepEqual(r.output, { west: 350, east: 290, north: 310 });
});

// 2. Count + average per product — nested bracket property (workaround: pre-declare then assign)
await check("reduce: count and average qty per product", async () => {
  const r = await execute(
    `
    const sales = await fetchSales();
    const agg = sales.reduce((acc, row) => {
      if (!acc[row.product]) acc[row.product] = { count: 0, totalQty: 0 };
      acc[row.product].count += 1;
      acc[row.product].totalQty += row.qty;
      return acc;
    }, {});
    const result = {};
    for (const k of Object.keys(agg)) {
      result[k] = { count: agg[k].count, avgQty: agg[k].totalQty / agg[k].count };
    }
    result
    `,
    salesTools()
  );
  assert.equal(r.output.widget.count, 3);
  assert.equal(r.output.widget.avgQty, 3);
  assert.equal(r.output.gadget.count, 3);
  assert.ok(Math.abs(r.output.gadget.avgQty - 8 / 3) < 0.001);
});

// 3. Dedup by id — build Map (using m.set), back to array via forEach
// (Workaround: Map constructor from pairs is broken; Array.from(map.values()) broken)
await check("dedup by id using Map.set + Object collect", async () => {
  const r = await execute(
    `
    const raw = [
      { id: 1, val: "a" }, { id: 2, val: "b" },
      { id: 1, val: "a_dup" }, { id: 3, val: "c" },
      { id: 2, val: "b_dup" },
    ];
    const seen = {};
    const deduped = [];
    for (const item of raw) {
      if (!seen[item.id]) {
        seen[item.id] = true;
        deduped.push(item);
      }
    }
    deduped.map(r => r.id)
    `,
    {}
  );
  assert.deepEqual(r.output, [1, 2, 3]);
});

// 4. Merge two datasets by id — workaround: use plain object lookup instead of Map
await check("merge two datasets by id (left join via plain object lookup)", async () => {
  const tools = {
    fetchUsers: {
      description: "Return users.",
      parameters: {},
      execute: async () => [
        { id: 10, name: "Alice" },
        { id: 20, name: "Bob" },
        { id: 30, name: "Carol" },
      ],
    },
    fetchScores: {
      description: "Return scores.",
      parameters: {},
      execute: async () => [
        { userId: 10, score: 95 },
        { userId: 20, score: 80 },
      ],
    },
  };
  const r = await execute(
    `
    const users  = await fetchUsers();
    const scores = await fetchScores();
    const scoreMap = {};
    for (const s of scores) scoreMap[s.userId] = s.score;
    const merged = users.map(u => {
      const score = scoreMap[u.id];
      return { id: u.id, name: u.name, score: score !== undefined ? score : null };
    });
    merged
    `,
    tools
  );
  assert.deepEqual(r.output, [
    { id: 10, name: "Alice", score: 95 },
    { id: 20, name: "Bob",   score: 80 },
    { id: 30, name: "Carol", score: null },
  ]);
});

// 5. Filter + sort + top-N
await check("filter west region, sort by amount desc, top 2", async () => {
  const r = await execute(
    `
    const sales = await fetchSales();
    const top = sales
      .filter(s => s.region === "west")
      .sort((a, b) => b.amount - a.amount)
      .slice(0, 2);
    top.map(s => s.id)
    `,
    salesTools()
  );
  // west: id1=120, id3=150, id5=80 → sorted desc: id3, id1
  assert.deepEqual(r.output, [3, 1]);
});

// 6. Paginated fetch: loop fetchPage(cursor) until no nextCursor
// Workaround: use .map() to extract ids instead of for-of + push(r.id) (BUG-B)
await check("paginated fetch via while loop + .map() workaround", async () => {
  const pages = [
    { records: [{ id: 1 }, { id: 2 }], nextCursor: "c1" },
    { records: [{ id: 3 }, { id: 4 }], nextCursor: "c2" },
    { records: [{ id: 5 }],            nextCursor: null  },
  ];
  let callIdx = 0;
  const tools = {
    fetchPage: {
      description: "Fetch a page of records.",
      parameters: { cursor: { type: "string", optional: true } },
      execute: async () => pages[callIdx++],
    },
  };
  const r = await execute(
    `
    const all = [];
    let cursor = null;
    while (true) {
      const page = await fetchPage({ cursor });
      const ids = page.records.map(r => r.id);
      for (const id of ids) all.push(id);
      if (!page.nextCursor) break;
      cursor = page.nextCursor;
    }
    all
    `,
    tools
  );
  assert.deepEqual(r.output, [1, 2, 3, 4, 5]);
});

// 7. Flatten nested arrays with flatMap + pluck
await check("flatMap to flatten nested arrays and pluck fields", async () => {
  const r = await execute(
    `
    const departments = [
      { name: "eng",     employees: [{ name: "Alice", salary: 120000 }, { name: "Bob", salary: 95000 }] },
      { name: "product", employees: [{ name: "Carol", salary: 110000 }] },
      { name: "sales",   employees: [{ name: "Dave", salary: 80000 }, { name: "Eve", salary: 85000 }] },
    ];
    departments.flatMap(d => d.employees.map(e => e.name))
    `,
    {}
  );
  assert.deepEqual(r.output, ["Alice", "Bob", "Carol", "Dave", "Eve"]);
});

// 8. Rename keys with object spread — workaround: avoid destructuring in callback param (BUG-E/D)
await check("rename keys with object spread (workaround: named param + dot-access)", async () => {
  const r = await execute(
    `
    const rows = [
      { user_id: 1, first_name: "Alice", last_name: "Smith" },
      { user_id: 2, first_name: "Bob",   last_name: "Jones" },
    ];
    const renamed = rows.map(row => ({
      id:   row.user_id,
      name: row.first_name + " " + row.last_name,
    }));
    renamed
    `,
    {}
  );
  assert.deepEqual(r.output, [
    { id: 1, name: "Alice Smith" },
    { id: 2, name: "Bob Jones"  },
  ]);
});

// 9. JSON.parse from tool → transform → JSON.stringify
await check("JSON.parse from tool, transform, JSON.stringify", async () => {
  const tools = {
    fetchRaw: {
      description: "Return a raw JSON string.",
      parameters: {},
      execute: async () =>
        JSON.stringify({ events: [{ ts: 1000, type: "click" }, { ts: 2000, type: "view" }] }),
    },
  };
  const r = await execute(
    `
    const raw  = await fetchRaw();
    const data = JSON.parse(raw);
    const mapped = data.events.map(e => ({ ts: e.ts, type: e.type, tsMs: e.ts * 1000 }));
    JSON.stringify(mapped)
    `,
    tools
  );
  assert.deepEqual(
    JSON.parse(r.output),
    [
      { ts: 1000, type: "click", tsMs: 1000000 },
      { ts: 2000, type: "view",  tsMs: 2000000 },
    ]
  );
});

// 10. Derived fields: normalize email, compute revenue
// Note: floating-point result for 5.49 * 10 = 54.900000000000006 — use delta check
await check("derived fields: normalize email, compute revenue", async () => {
  const r = await execute(
    `
    const records = [
      { email: "  Alice@Example.COM ", price: "19.99", qty: "3" },
      { email: "BOB@test.org ",        price: "5.49",  qty: "10" },
    ];
    records.map(rec => ({
      email:   rec.email.trim().toLowerCase(),
      price:   Number(rec.price),
      qty:     Number(rec.qty),
      revenue: Number(rec.price) * Number(rec.qty),
    }))
    `,
    {}
  );
  assert.equal(r.output[0].email, "alice@example.com");
  assert.equal(r.output[0].revenue, 19.99 * 3);
  assert.equal(r.output[1].email, "bob@test.org");
  assert.ok(Math.abs(r.output[1].revenue - 54.9) < 0.001);
});

// 11. Partition into buckets
// Workaround: use plain object + for-of with explicit const arr = bucket.prop; arr.push()
// OR: use separate arrays and combine at the end
await check("partition records into buckets by score range (workaround: separate arrays)", async () => {
  const r = await execute(
    `
    const scores = [10, 45, 62, 78, 91, 55, 33, 88];
    const low = [], mid = [], high = [];
    for (const s of scores) {
      if (s < 40) low.push(s);
      else if (s < 70) mid.push(s);
      else high.push(s);
    }
    ({ low, mid, high })
    `,
    {}
  );
  assert.deepEqual(r.output, {
    low:  [10, 33],
    mid:  [45, 62, 55],
    high: [78, 91, 88],
  });
});

// 12. Parallel fan-out: Promise.all([fetchA, fetchB, fetchC]) then combine
// Workaround: avoid Map constructor (BUG-C), use for-of + object lookup instead
await check("parallel fan-out: Promise.all then combine (plain object lookup)", async () => {
  const tools = {
    fetchOrders: {
      description: "Return orders.",
      parameters: {},
      execute: async () => [
        { orderId: 1, customerId: 10, total: 200 },
        { orderId: 2, customerId: 20, total: 350 },
      ],
    },
    fetchCustomers: {
      description: "Return customers.",
      parameters: {},
      execute: async () => [
        { customerId: 10, name: "Alice", tier: "gold"   },
        { customerId: 20, name: "Bob",   tier: "silver" },
      ],
    },
    fetchDiscounts: {
      description: "Return discounts per tier.",
      parameters: {},
      execute: async () => ({ gold: 0.2, silver: 0.1, bronze: 0.05 }),
    },
  };
  const r = await execute(
    `
    const orders    = await fetchOrders();
    const customers = await fetchCustomers();
    const discounts = await fetchDiscounts();
    const custMap = {};
    for (const c of customers) custMap[c.customerId] = c;
    orders.map(o => {
      const cust = custMap[o.customerId];
      const disc = discounts[cust.tier] || 0;
      return {
        orderId:    o.orderId,
        customer:   cust.name,
        total:      o.total,
        discounted: o.total * (1 - disc),
      };
    })
    `,
    tools
  );
  assert.deepEqual(r.output, [
    { orderId: 1, customer: "Alice", total: 200, discounted: 160 },
    { orderId: 2, customer: "Bob",   total: 350, discounted: 315 },
  ]);
});

// 13. Join two datasets (lookup table)
await check("join: enrich line-items with product catalog", async () => {
  const tools = {
    fetchLineItems: {
      description: "Return order line-items.",
      parameters: {},
      execute: async () => [
        { lineId: 1, productId: "p1", qty: 2 },
        { lineId: 2, productId: "p2", qty: 5 },
        { lineId: 3, productId: "p1", qty: 1 },
      ],
    },
    fetchCatalog: {
      description: "Return product catalog.",
      parameters: {},
      execute: async () => [
        { productId: "p1", name: "Widget", unitPrice: 10 },
        { productId: "p2", name: "Gadget", unitPrice: 25 },
      ],
    },
  };
  const r = await execute(
    `
    const items   = await fetchLineItems();
    const catalog = await fetchCatalog();
    const catMap  = {};
    for (const p of catalog) catMap[p.productId] = p;
    items.map(li => {
      const prod = catMap[li.productId];
      return { lineId: li.lineId, name: prod.name, qty: li.qty, subtotal: li.qty * prod.unitPrice };
    })
    `,
    tools
  );
  assert.deepEqual(r.output, [
    { lineId: 1, name: "Widget", qty: 2, subtotal: 20  },
    { lineId: 2, name: "Gadget", qty: 5, subtotal: 125 },
    { lineId: 3, name: "Widget", qty: 1, subtotal: 10  },
  ]);
});

// 14. Object.keys / Object.values / Object.entries — manual filter (no fromEntries, missing)
await check("Object.entries + reduce to filter inventory", async () => {
  const r = await execute(
    `
    const inventory = { apples: 50, bananas: 0, cherries: 120, dates: 0, elderberries: 15 };
    const inStock = {};
    for (const entry of Object.entries(inventory)) {
      if (entry[1] > 0) inStock[entry[0]] = entry[1];
    }
    inStock
    `,
    {}
  );
  assert.deepEqual(r.output, { apples: 50, cherries: 120, elderberries: 15 });
});

// 15. String coercion + padStart
await check("String coercion and padStart for zero-padded IDs", async () => {
  const r = await execute(
    `
    const ids = [1, 23, 456, 7890];
    ids.map(id => String(id).padStart(6, "0"))
    `,
    {}
  );
  assert.deepEqual(r.output, ["000001", "000023", "000456", "007890"]);
});

// 16. Sequential enrichment via for-of + tool call
await check("sequential per-record enrichment via for-of + tool call", async () => {
  const tools = {
    listUserIds: {
      description: "Return user IDs.",
      parameters: {},
      execute: async () => [101, 102, 103],
    },
    getUser: {
      description: "Fetch a user by ID.",
      parameters: { id: { type: "number" } },
      execute: async ({ id }) => ({ id, name: `User_${id}`, active: id % 2 === 1 }),
    },
  };
  const r = await execute(
    `
    const ids   = await listUserIds();
    const users = [];
    for (const id of ids) {
      const u = await getUser({ id });
      users.push(u);
    }
    users.filter(u => u.active).map(u => u.id)
    `,
    tools
  );
  assert.deepEqual(r.output, [101, 103]);
});

// 17. CSV-ish parse + transform + aggregate pipeline
await check("CSV-ish parse + transform + aggregate pipeline", async () => {
  const tools = {
    fetchCsvData: {
      description: "Return CSV-like string.",
      parameters: {},
      execute: async () => "region,amount\nwest,100\neast,200\nwest,150\nnorth,300\neast,50",
    },
  };
  const r = await execute(
    `
    const csv     = await fetchCsvData();
    const lines   = csv.trim().split("\\n");
    const headers = lines[0].split(",");
    const rows    = lines.slice(1).map(line => {
      const vals = line.split(",");
      const obj  = {};
      for (let i = 0; i < headers.length; i++) obj[headers[i]] = vals[i];
      return obj;
    });
    rows.reduce((acc, row) => {
      acc[row.region] = (acc[row.region] || 0) + Number(row.amount);
      return acc;
    }, {})
    `,
    tools
  );
  assert.deepEqual(r.output, { west: 250, east: 250, north: 300 });
});

// 18. Rank by score via sort + index
await check("compute ordinal rank via sort + index", async () => {
  const r = await execute(
    `
    const scores = [
      { name: "Alice", score: 88 },
      { name: "Bob",   score: 72 },
      { name: "Carol", score: 95 },
      { name: "Dave",  score: 60 },
    ];
    const sorted = [...scores].sort((a, b) => b.score - a.score);
    sorted.map((s, i) => ({ name: s.name, rank: i + 1 })).map(r => r.name)
    `,
    {}
  );
  assert.deepEqual(r.output, ["Carol", "Alice", "Bob", "Dave"]);
});

// 19. Array spread merge preserves order
await check("spread merge of two arrays preserves order", async () => {
  const r = await execute(
    `
    const a = [1, 2, 3];
    const b = [4, 5, 6];
    [...a, ...b]
    `,
    {}
  );
  assert.deepEqual(r.output, [1, 2, 3, 4, 5, 6]);
});

// 20. Nullish coalescing and optional chaining
await check("nullish coalescing and optional chaining", async () => {
  const r = await execute(
    `
    const rows = [
      { id: 1, meta: { label: "foo" } },
      { id: 2, meta: null },
      { id: 3 },
    ];
    rows.map(r => r.meta?.label ?? "unknown")
    `,
    {}
  );
  assert.deepEqual(r.output, ["foo", "unknown", "unknown"]);
});

// 21. Date constructor and toISOString
await check("Date constructor and toISOString", async () => {
  const r = await execute(`new Date(0).toISOString()`, {});
  assert.equal(r.output, "1970-01-01T00:00:00.000Z");
});

// 22. Array.isArray
await check("Array.isArray is available", async () => {
  const r = await execute(`[Array.isArray([1,2]), Array.isArray({})]`, {});
  assert.deepEqual(r.output, [true, false]);
});

// 23. Generator with for-of (spread over generator is broken; for-of works)
await check("generator function iterated with for-of", async () => {
  const r = await execute(
    `
    function* range(start, end) {
      for (let i = start; i < end; i++) yield i;
    }
    const out = [];
    for (const x of range(1, 6)) out.push(x);
    out
    `,
    {}
  );
  assert.deepEqual(r.output, [1, 2, 3, 4, 5]);
});

// 24. Map.set / Map.get / Map.has (avoid: constructor from pairs, forEach, entries, size)
await check("Map.set / Map.get / Map.has basic operations", async () => {
  const r = await execute(
    `
    const m = new Map();
    m.set("a", 1);
    m.set("b", 2);
    m.set("c", 3);
    [m.get("a"), m.get("b"), m.has("c"), m.has("z")]
    `,
    {}
  );
  assert.deepEqual(r.output, [1, 2, true, false]);
});

// 25. Destructuring assignment in body (workaround for BUG-D/E)
await check("destructuring assignment in function body (not param pattern)", async () => {
  const r = await execute(
    `
    function normalize(rec) {
      const name   = rec.name;
      const active = rec.active !== undefined ? rec.active : true;
      const score  = rec.score  !== undefined ? rec.score  : 0;
      return { name, active, score };
    }
    [normalize({ name: "Alice", score: 90 }), normalize({ name: "Bob", active: false })]
    `,
    {}
  );
  assert.deepEqual(r.output, [
    { name: "Alice", active: true,  score: 90 },
    { name: "Bob",   active: false, score: 0  },
  ]);
});

// ===========================================================================
// SECTION 101-106 — CONFIRMED BUGS (expected to fail; document behavior)
// ===========================================================================

await check("BUG-A: push on array inside object literal mutates object reference", async () => {
  // const o = { a: [] }; o.a.push(1); o — returns [1] instead of { a: [1] }
  // The object identity is corrupted: the object { a: [] } is replaced by
  // the return value of push() (the new length as a number? or an array?).
  const r = await execute(
    `
    const o = { a: [] };
    o.a.push(1);
    o
    `,
    {}
  );
  // BUG: actual output is [1] (array) — should be { a: [1] }
  assert.deepEqual(r.output, { a: [1] });
});

await check("BUG-B: for-of body: inline r.prop arg to push() yields empty result", async () => {
  // for (const r of items) out.push(r.id)  →  out = []
  // Workaround: const id = r.id; out.push(id)  →  works
  const r = await execute(
    `
    const items = [{ id: 1 }, { id: 2 }, { id: 3 }];
    const out   = [];
    for (const r of items) out.push(r.id);
    out
    `,
    {}
  );
  // BUG: actual output is []
  assert.deepEqual(r.output, [1, 2, 3]);
});

await check("BUG-C: new Map(arrayOfPairs) — constructor ignores pairs", async () => {
  // new Map([["k", 99]]).has("k") returns false; .get("k") returns null
  const r = await execute(
    `
    const m = new Map([["alpha", 1], ["beta", 2]]);
    [m.has("alpha"), m.get("alpha"), m.has("beta")]
    `,
    {}
  );
  // BUG: actual output is [false, null, false]
  assert.deepEqual(r.output, [true, 1, true]);
});

await check("BUG-D: arrow ({ a }) => expr — destructured binding resolves to whole arg", async () => {
  // const f = ({ a }) => a; f({ a: 42 }) returns { a: 42 } instead of 42
  const r = await execute(
    `
    const f = ({ a }) => a;
    f({ a: 42 })
    `,
    {}
  );
  // BUG: returns { a: 42 } (whole object)
  assert.equal(r.output, 42);
});

await check("BUG-E: function({ a, b }) param destructuring — first param = whole obj", async () => {
  // function f({ a, b }) { return [a, b]; } f({ a:10, b:20 }) → [{ a:10, b:20 }, null]
  const r = await execute(
    `
    function f({ a, b }) { return [a, b]; }
    f({ a: 10, b: 20 })
    `,
    {}
  );
  // BUG: returns [{ a: 10, b: 20 }, null]
  assert.deepEqual(r.output, [10, 20]);
});

await check("BUG-F: for-of inline destructuring — all pattern-bound vars are null", async () => {
  // for (const { id, name } of rows) — both id and name are null inside loop
  const r = await execute(
    `
    const rows = [{ id: 1, name: "alice" }, { id: 2, name: "bob" }];
    const out  = [];
    for (const { id, name } of rows) out.push({ id, name });
    out
    `,
    {}
  );
  // BUG: returns [{ id: null, name: null }, { id: null, name: null }]
  assert.deepEqual(r.output, [
    { id: 1, name: "alice" },
    { id: 2, name: "bob" },
  ]);
});

// ===========================================================================
// SECTION 201-207 — MISSING BUILTINS
// ===========================================================================

await check("MISSING-BUILTIN: parseInt", async () => {
  // throws "type error: undefined is not a function"
  const r = await execute(`parseInt("42px", 10)`, {});
  assert.equal(r.output, 42);
});

await check("MISSING-BUILTIN: parseFloat", async () => {
  const r = await execute(`parseFloat("3.14abc")`, {});
  assert.equal(r.output, 3.14);
});

await check("MISSING-BUILTIN: Object.fromEntries", async () => {
  // throws "type error: Object.fromEntries is not a function"
  const r = await execute(
    `Object.fromEntries([["a", 1], ["b", 2]])`,
    {}
  );
  assert.deepEqual(r.output, { a: 1, b: 2 });
});

await check("MISSING-BUILTIN: Number.isFinite", async () => {
  // throws "type error: Number.isFinite is not a function"
  const r = await execute(`Number.isFinite(42)`, {});
  assert.equal(r.output, true);
});

await check("MISSING-BUILTIN: Number.isNaN", async () => {
  const r = await execute(`Number.isNaN(NaN)`, {});
  assert.equal(r.output, true);
});

await check("MISSING-BUILTIN: structuredClone", async () => {
  // throws "type error: undefined is not a function"
  const r = await execute(
    `
    const o = { x: { y: 1 } };
    const c = structuredClone(o);
    c.x.y = 99;
    [o.x.y, c.x.y]
    `,
    {}
  );
  assert.deepEqual(r.output, [1, 99]);
});

await check("MISSING-BUILTIN: Set constructor", async () => {
  // throws "type error: undefined is not a constructor"
  const r = await execute(`new Set([1, 2, 2, 3]).size`, {});
  assert.equal(r.output, 3);
});

await check("MISSING-BUILTIN: Map.size is broken (returns 'function' type instead of number)", async () => {
  // m.size should be a number; instead it is a function (getter not working)
  const r = await execute(
    `
    const m = new Map();
    m.set("a", 1);
    m.set("b", 2);
    m.size
    `,
    {}
  );
  assert.equal(r.output, 2);
});

await check("MISSING-BUILTIN: Array.from(map.values())", async () => {
  // throws "type error: Array.values is not a function" (mistakenly calls Array.values)
  const r = await execute(
    `
    const m = new Map();
    m.set("a", 1);
    m.set("b", 2);
    Array.from(m.values())
    `,
    {}
  );
  assert.deepEqual(r.output, [1, 2]);
});

await check("MISSING-BUILTIN: spread over generator is not iterable", async () => {
  // [...gen()] throws "type error: object is not iterable (spread)"
  // for...of gen() works, but spread does not
  const r = await execute(
    `
    function* gen() { yield 1; yield 2; yield 3; }
    [...gen()]
    `,
    {}
  );
  assert.deepEqual(r.output, [1, 2, 3]);
});

// ===========================================================================
// Summary
// ===========================================================================
const failed = results.filter(r => !r[1]);
const bugs   = results.filter(r => !r[1] && r[0].startsWith("BUG-"));
const missing = results.filter(r => !r[1] && r[0].startsWith("MISSING-"));
const etl    = results.filter(r => !r[1] && !r[0].startsWith("BUG-") && !r[0].startsWith("MISSING-"));

console.log(`\n${results.length - failed.length}/${results.length} passed`);
if (etl.length)     console.log(`  ETL scenario failures (unexpected): ${etl.length}`);
if (bugs.length)    console.log(`  Confirmed interpreter bugs (expected to fail): ${bugs.length}`);
if (missing.length) console.log(`  Missing builtins (expected to fail): ${missing.length}`);

if (etl.length) {
  console.log("\nUnexpected ETL failures:");
  for (const [name, , msg] of etl) {
    console.log(`  ✗ ${name}`);
    console.log(`    ${msg}`);
  }
  process.exit(1);
} else if (failed.length) {
  console.log("\n(All failures are known bugs or missing builtins — see comments above)");
  process.exit(1);
}