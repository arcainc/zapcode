/**
 * Realistic warehouse inventory / order-fulfillment / allocation agent scenarios
 * for zapcode-ai.
 *
 * Stresses agent-authored control flow that survives the interpreter subset:
 *   - switch statements (with intentional fallthrough) for SKU handling classes
 *   - classic indexed for-loops nested 3 deep (orders -> items -> warehouses)
 *   - break / continue inside nested loops with a running "remaining" counter
 *   - while / do-while loops for replenishment draw-down
 *   - early return inside helper functions
 *   - Map as the primary stock ledger (string composite keys, get/set/has/delete,
 *     .values()/.entries() iteration, Map(entries[]) construction)
 *   - Set for reserved/backordered SKU tracking (.add/.has/.delete/.size)
 *   - guard-heavy accumulation with deterministic, exact output
 *
 * Notes on the proven subset (discovered while authoring this file):
 *   - Nested `for...of` loops are unreliable: an outer `for...of` only runs its
 *     first iteration when it contains an inner `for...of`. Multi-level iteration
 *     therefore uses classic `for (let i...)` loops, which work at any depth.
 *   - Multi-key ternary sort comparators mis-order; a single composite numeric
 *     comparator (priority*100 + seq) sorts correctly, so priority-first ordering
 *     is expressed that way.
 *
 * Run from packages/zapcode-ai:
 *   node tests/scenarios3-inventory.mjs
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

console.log("scenarios3 inventory/allocation e2e");

const WAREHOUSES = [
  { warehouseId: "wh_near", lines: [{ sku: "SKU-A", qty: 5 }, { sku: "SKU-B", qty: 2 }] },
  { warehouseId: "wh_mid", lines: [{ sku: "SKU-A", qty: 4 }, { sku: "SKU-C", qty: 10 }] },
  { warehouseId: "wh_far", lines: [{ sku: "SKU-B", qty: 1 }] },
];

const ORDERS = [
  { orderId: "ord_2", priority: 3, seq: 2, items: [{ sku: "SKU-A", qty: 3 }] },
  { orderId: "ord_1", priority: 1, seq: 0, items: [{ sku: "SKU-A", qty: 7 }, { sku: "SKU-B", qty: 4 }] },
  { orderId: "ord_3", priority: 1, seq: 1, items: [{ sku: "SKU-C", qty: 2 }] },
];

function clone(value) {
  return JSON.parse(JSON.stringify(value));
}

function createInventoryTools(state, { warehouses = WAREHOUSES, orders = ORDERS } = {}) {
  return {
    getWarehouseStock: {
      description: "Fetch per-warehouse on-hand stock lines, nearest warehouse first.",
      parameters: {
        region: { type: "string", description: "Region whose warehouses to fetch." },
      },
      execute: async ({ region }) => {
        state.stockFetches.push(region);
        return clone(warehouses);
      },
    },
    getOpenOrders: {
      description: "Fetch open orders awaiting fulfillment for a region.",
      parameters: {
        region: { type: "string" },
      },
      execute: async ({ region }) => {
        state.orderFetches.push(region);
        return clone(orders);
      },
    },
    commitAllocations: {
      description: "Persist the planned allocations and any backorders in one batch.",
      parameters: {
        allocations: { type: "array", description: "Allocation lines to reserve." },
        backorders: { type: "array", description: "Unfulfillable shortfalls." },
        dryRun: { type: "boolean", optional: true },
      },
      execute: async input => {
        state.commits.push(input);
        return { committed: input.allocations.length, backordered: input.backorders.length };
      },
    },
  };
}

// Priority-first across orders (composite numeric key avoids the multi-key
// ternary-comparator bug), then nearest-first across warehouses per item.
const ALLOCATION_PROGRAM = `
const warehouses = await getWarehouseStock({ region: "na" });
const orders = await getOpenOrders({ region: "na" });

const stock = new Map();
for (let w = 0; w < warehouses.length; w++) {
  const wh = warehouses[w];
  for (let l = 0; l < wh.lines.length; l++) {
    const line = wh.lines[l];
    stock.set(line.sku + "@" + wh.warehouseId, line.qty);
  }
}

const warehouseOrder = warehouses.map(w => w.warehouseId);

const sortedOrders = orders
  .slice()
  .sort((a, b) => (a.priority * 100 + a.seq) - (b.priority * 100 + b.seq));

const allocations = [];
const backorders = [];
const reservedSkus = new Set();
const backorderedSkus = new Set();

for (let o = 0; o < sortedOrders.length; o++) {
  const order = sortedOrders[o];
  for (let i = 0; i < order.items.length; i++) {
    const item = order.items[i];
    let remaining = item.qty;
    for (let w = 0; w < warehouseOrder.length; w++) {
      if (remaining <= 0) break;
      const wh = warehouseOrder[w];
      const key = item.sku + "@" + wh;
      const avail = stock.get(key) || 0;
      if (avail <= 0) continue;
      const take = avail < remaining ? avail : remaining;
      stock.set(key, avail - take);
      remaining -= take;
      allocations.push({ orderId: order.orderId, sku: item.sku, warehouseId: wh, qty: take });
      reservedSkus.add(item.sku);
    }
    if (remaining > 0) {
      backorders.push({ orderId: order.orderId, sku: item.sku, shortfall: remaining });
      backorderedSkus.add(item.sku);
    }
  }
}

const reserved = [];
for (const sku of reservedSkus) reserved.push(sku);
reserved.sort();

const result = await commitAllocations({ allocations, backorders });
({
  allocations,
  backorders,
  reserved,
  fulfilledOrder: sortedOrders.map(o => o.orderId),
  committed: result.committed,
  backordered: result.backordered,
})
`;

await test("system prompt exposes named-object inventory tool signatures", async () => {
  const state = { stockFetches: [], orderFetches: [], commits: [] };
  const { system } = zapcode({ tools: createInventoryTools(state) });
  assert.match(
    system,
    /declare function commitAllocations\(input: \{ allocations: array; backorders: array; dryRun\?: boolean \}\): Promise<unknown>;/
  );
  assert.match(system, /declare function getOpenOrders\(region: string\): Promise<unknown>;/);
  assert.match(system, /Call shape: await commitAllocations\(\{ allocations: array, backorders: array, dryRun\?: boolean \}\)/);
});

await test("STRESS: priority-first orders allocated nearest-first across warehouses with break/continue + Map ledger", async () => {
  const state = { stockFetches: [], orderFetches: [], commits: [] };
  const result = await execute(ALLOCATION_PROGRAM, createInventoryTools(state));

  assert.deepEqual(state.stockFetches, ["na"]);
  assert.deepEqual(state.orderFetches, ["na"]);
  assert.equal(state.commits.length, 1);

  // ord_1 (p1,seq0) -> ord_3 (p1,seq1) -> ord_2 (p3,seq2)
  assert.deepEqual(result.output.fulfilledOrder, ["ord_1", "ord_3", "ord_2"]);

  // ord_1 SKU-A qty7: 5 from wh_near + 2 from wh_mid (mid drops to 2 left).
  // ord_1 SKU-B qty4: 2 from wh_near + 1 from wh_far -> 1 short (backorder).
  // ord_3 SKU-C qty2: 2 from wh_mid.
  // ord_2 SKU-A qty3: wh_near empty, 2 left in wh_mid -> 1 short (backorder).
  assert.deepEqual(result.output.allocations, [
    { orderId: "ord_1", sku: "SKU-A", warehouseId: "wh_near", qty: 5 },
    { orderId: "ord_1", sku: "SKU-A", warehouseId: "wh_mid", qty: 2 },
    { orderId: "ord_1", sku: "SKU-B", warehouseId: "wh_near", qty: 2 },
    { orderId: "ord_1", sku: "SKU-B", warehouseId: "wh_far", qty: 1 },
    { orderId: "ord_3", sku: "SKU-C", warehouseId: "wh_mid", qty: 2 },
    { orderId: "ord_2", sku: "SKU-A", warehouseId: "wh_mid", qty: 2 },
  ]);
  assert.deepEqual(result.output.backorders, [
    { orderId: "ord_1", sku: "SKU-B", shortfall: 1 },
    { orderId: "ord_2", sku: "SKU-A", shortfall: 1 },
  ]);
  assert.deepEqual(result.output.reserved, ["SKU-A", "SKU-B", "SKU-C"]);
  assert.equal(result.output.committed, 6);
  assert.equal(result.output.backordered, 2);
});

await test("toolCalls introspection: ordered host calls and the committed payload matches host state", async () => {
  const state = { stockFetches: [], orderFetches: [], commits: [] };
  const result = await execute(ALLOCATION_PROGRAM, createInventoryTools(state));

  assert.deepEqual(result.toolCalls.map(call => call.name), [
    "getWarehouseStock",
    "getOpenOrders",
    "commitAllocations",
  ]);
  const commitCall = result.toolCalls.at(-1);
  assert.deepEqual(commitCall.input, state.commits[0]);
  assert.deepEqual(commitCall.input.allocations, result.output.allocations);
  assert.deepEqual(commitCall.result, { committed: 6, backordered: 2 });
  assert.equal(commitCall.input.backorders.length, 2);
});

await test("switch with intentional fallthrough classifies SKU handling and routes allocation lanes", async () => {
  const state = { stockFetches: [], orderFetches: [], commits: [] };
  const result = await execute(
    `
    function laneFor(sku) {
      const out = [];
      switch (sku.charAt(sku.length - 1)) {
        case "A":
          out.push("priority");
          // intentional fallthrough: priority SKUs also use the express lane
        case "B":
          out.push("express");
          break;
        case "C":
        case "D":
          out.push("bulk");
          break;
        default:
          out.push("standard");
      }
      return out.join("+");
    }

    const skus = ["SKU-A", "SKU-B", "SKU-C", "SKU-D", "SKU-Z"];
    const lanes = [];
    for (const sku of skus) {
      lanes.push(sku + ":" + laneFor(sku));
    }
    lanes
    `,
    createInventoryTools(state)
  );

  assert.deepEqual(result.output, [
    "SKU-A:priority+express",
    "SKU-B:express",
    "SKU-C:bulk",
    "SKU-D:bulk",
    "SKU-Z:standard",
  ]);
  // pure planning logic — no host mutations
  assert.deepEqual(state.commits, []);
});

await test("while/do-while replenishment draws down a Map ledger until reorder point", async () => {
  const state = { stockFetches: [], orderFetches: [], commits: [] };
  const result = await execute(
    `
    // ledger Map seeded from entries[] then drawn down by a demand queue.
    const ledger = new Map([["SKU-A", 12], ["SKU-B", 3]]);
    const demand = [4, 5, 6];

    // do-while: always attempt at least the first demand pull.
    let idx = 0;
    const events = [];
    do {
      const want = demand[idx];
      let onHand = ledger.get("SKU-A");
      const give = onHand >= want ? want : onHand;
      ledger.set("SKU-A", onHand - give);
      events.push("pull:" + give);
      idx++;
    } while (idx < demand.length && ledger.get("SKU-A") > 0);

    // while loop: top SKU-B back up to its reorder point of 8.
    let reorders = 0;
    while (ledger.get("SKU-B") < 8) {
      ledger.set("SKU-B", ledger.get("SKU-B") + 2);
      reorders++;
    }

    ({
      events,
      remainingA: ledger.get("SKU-A"),
      remainingB: ledger.get("SKU-B"),
      reorders,
    })
    `,
    createInventoryTools(state)
  );

  // pulls 4 (->8), 5 (->3), then 6 wanted but only 3 -> give 3 (->0); loop stops.
  assert.deepEqual(result.output.events, ["pull:4", "pull:5", "pull:3"]);
  assert.equal(result.output.remainingA, 0);
  // SKU-B 3 -> 5 -> 7 -> 9 (>=8), 3 reorders.
  assert.equal(result.output.remainingB, 9);
  assert.equal(result.output.reorders, 3);
});

await test("Set tracks reserved vs backordered SKUs with has/delete/size during a single pass", async () => {
  const state = { stockFetches: [], orderFetches: [], commits: [] };
  const result = await execute(
    `
    const reserved = new Set();
    const backordered = new Set();
    const lines = [
      { sku: "SKU-A", filled: true },
      { sku: "SKU-B", filled: false },
      { sku: "SKU-A", filled: true },
      { sku: "SKU-B", filled: true },
      { sku: "SKU-C", filled: false },
    ];
    for (const line of lines) {
      if (line.filled) {
        reserved.add(line.sku);
        // a later fill clears an earlier backorder for the same SKU.
        if (backordered.has(line.sku)) backordered.delete(line.sku);
      } else if (!reserved.has(line.sku)) {
        backordered.add(line.sku);
      }
    }
    const reservedList = [];
    for (const sku of reserved) reservedList.push(sku);
    reservedList.sort();
    const backorderedList = [];
    for (const sku of backordered) backorderedList.push(sku);
    backorderedList.sort();
    ({ reservedList, backorderedList, reservedSize: reserved.size, backorderedSize: backordered.size })
    `,
    createInventoryTools(state)
  );

  assert.deepEqual(result.output, {
    reservedList: ["SKU-A", "SKU-B"],
    backorderedList: ["SKU-C"],
    reservedSize: 2,
    backorderedSize: 1,
  });
});

await test("rejects missing required allocation argument before any host commit", async () => {
  const state = { stockFetches: [], orderFetches: [], commits: [] };
  await assert.rejects(
    () =>
      execute(
        `await commitAllocations({ allocations: [{ orderId: "ord_1", sku: "SKU-A", warehouseId: "wh_near", qty: 1 }] })`,
        createInventoryTools(state)
      ),
    /Invalid arguments for tool 'commitAllocations': missing required parameter 'backorders'/
  );
  assert.deepEqual(state.commits, []);
});

await test("rejects wrong-typed allocation argument before any host commit", async () => {
  const state = { stockFetches: [], orderFetches: [], commits: [] };
  await assert.rejects(
    () => execute(`await commitAllocations({ allocations: {}, backorders: [] })`, createInventoryTools(state)),
    /Invalid arguments for tool 'commitAllocations': parameter 'allocations' expected array, got object/
  );
  assert.deepEqual(state.commits, []);
});

await test("rejects optional dryRun of the wrong type and positional calls before host commit", async () => {
  const state = { stockFetches: [], orderFetches: [], commits: [] };
  await assert.rejects(
    () => execute(`await commitAllocations({ allocations: [], backorders: [], dryRun: "yes" })`, createInventoryTools(state)),
    /Invalid arguments for tool 'commitAllocations': parameter 'dryRun' expected boolean, got string/
  );
  await assert.rejects(
    () => execute(`await commitAllocations([], [])`, createInventoryTools(state)),
    /Invalid arguments for tool 'commitAllocations': expected one named object argument/
  );
  assert.deepEqual(state.commits, []);
});

await test("early-return helper short-circuits allocation when an item has no demand", async () => {
  const state = { stockFetches: [], orderFetches: [], commits: [] };
  const result = await execute(
    `
    const stock = new Map([["SKU-A", 10], ["SKU-B", 0]]);
    function allocate(sku, want) {
      if (want <= 0) return { sku, status: "skipped", qty: 0 };
      const onHand = stock.get(sku) || 0;
      if (onHand <= 0) return { sku, status: "backorder", qty: 0, shortfall: want };
      const take = onHand < want ? onHand : want;
      stock.set(sku, onHand - take);
      const status = take < want ? "partial" : "full";
      return { sku, status, qty: take };
    }
    const plan = [
      allocate("SKU-A", 0),
      allocate("SKU-A", 6),
      allocate("SKU-A", 6),
      allocate("SKU-B", 3),
    ];
    plan
    `,
    createInventoryTools(state)
  );

  assert.deepEqual(result.output, [
    { sku: "SKU-A", status: "skipped", qty: 0 },
    { sku: "SKU-A", status: "full", qty: 6 },
    { sku: "SKU-A", status: "partial", qty: 4 },
    { sku: "SKU-B", status: "backorder", qty: 0, shortfall: 3 },
  ]);
  assert.deepEqual(state.commits, []);
});

console.log(`\n${passed} inventory/allocation checks passed.`);
