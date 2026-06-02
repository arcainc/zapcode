/**
 * Realistic pricing / invoicing / billing agent scenarios for zapcode-ai.
 *
 * Stresses number-heavy reductions (reduce with object accumulators, Math.round
 * cents rounding, Math.min/max/abs), chained map -> filter -> reduce pipelines,
 * flatMap, sort comparators, Object.entries/keys/values/fromEntries aggregation,
 * nested destructuring + rest/spread + computed keys, and ternary tier chains for
 * volume discounts and per-jurisdiction tax. Also validates that bad tool calls
 * are rejected BEFORE any host side effect.
 *
 * Determinism: no Date.now()/Math.random(); the "billing clock" is injected via a
 * tool returning a fixed epoch value. All numeric assertions are exact.
 *
 * Run from packages/zapcode-ai:
 *   node tests/scenarios3-pricing.mjs
 */
import assert from "node:assert/strict";
import { execute, zapcode } from "../dist/index.js";

const BILLING_NOW_MS = Date.parse("2026-06-01T00:00:00.000Z");

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

console.log("scenarios3 pricing e2e");

function createPricingTools(state) {
  return {
    getBillingClock: {
      description: "Return the deterministic billing-cycle clock as epoch milliseconds.",
      parameters: {},
      execute: async () => BILLING_NOW_MS,
    },
    getTaxTable: {
      description: "Resolve sales-tax rates keyed by jurisdiction code.",
      parameters: {
        jurisdictions: { type: "array", description: "Jurisdiction codes to resolve." },
      },
      execute: async ({ jurisdictions }) => {
        const rates = { CA: 0.0725, NY: 0.08, TX: 0.0625 };
        const out = {};
        for (const code of jurisdictions) out[code] = rates[code] ?? 0;
        return out;
      },
    },
    saveInvoice: {
      description: "Persist a finalized invoice with priced line items and totals.",
      parameters: {
        invoiceId: { type: "string" },
        lineItems: { type: "array" },
        totals: { type: "object" },
        currency: { type: "string", optional: true },
      },
      execute: async input => {
        state.saved.push(input);
        return { ok: true, invoiceId: input.invoiceId, lineCount: input.lineItems.length };
      },
    },
  };
}

await test("system prompt renders pricing tool signatures", async () => {
  const { system } = zapcode({ tools: createPricingTools({ saved: [] }) });
  assert.match(
    system,
    /declare function saveInvoice\(input: \{ invoiceId: string; lineItems: array; totals: object; currency\?: string \}\): Promise<unknown>;/
  );
  assert.match(
    system,
    /declare function getTaxTable\(jurisdictions: array\): Promise<unknown>;/
  );
  assert.match(system, /declare function getBillingClock\(\): Promise<unknown>;/);
  assert.match(system, /Call shape: await saveInvoice\(\{ invoiceId: string, lineItems: array, totals: object/);
});

// ---------------------------------------------------------------------------
// Stress case: sizable line-item array, tiered volume discounts, per-jurisdiction
// tax, cents rounding, object-accumulator reductions, computed-key tax-by-region.
// ---------------------------------------------------------------------------
const INVOICE_PROGRAM = `
const lineItems = [
  { sku: "SEAT-STD",  qty: 120, unitPrice: 12.50, jurisdiction: "CA" },
  { sku: "SEAT-PRO",  qty: 45,  unitPrice: 30.00, jurisdiction: "CA" },
  { sku: "ADDON-SMS", qty: 8,   unitPrice: 4.99,  jurisdiction: "NY" },
  { sku: "ADDON-API", qty: 1,   unitPrice: 199.0, jurisdiction: "NY" },
  { sku: "SUP-GOLD",  qty: 12,  unitPrice: 0.10,  jurisdiction: "TX" },
];

const round2 = n => Math.round(n * 100) / 100;
const volumeDiscount = q => q >= 100 ? 0.20 : q >= 50 ? 0.10 : q >= 10 ? 0.05 : 0;

const taxTable = await getTaxTable({
  jurisdictions: [...new Set(lineItems.map(li => li.jurisdiction))],
});

const priced = lineItems.map(li => {
  const gross = li.unitPrice * li.qty;
  const discountRate = volumeDiscount(li.qty);
  const discount = round2(gross * discountRate);
  const net = round2(gross - discount);
  const taxRate = taxTable[li.jurisdiction] ?? 0;
  const tax = round2(net * taxRate);
  return {
    sku: li.sku,
    jurisdiction: li.jurisdiction,
    gross: round2(gross),
    discountRate,
    discount,
    net,
    taxRate,
    tax,
    total: round2(net + tax),
  };
});

const totals = priced.reduce(
  (acc, p) => {
    acc.gross = round2(acc.gross + p.gross);
    acc.discount = round2(acc.discount + p.discount);
    acc.net = round2(acc.net + p.net);
    acc.tax = round2(acc.tax + p.tax);
    acc.total = round2(acc.total + p.total);
    return acc;
  },
  { gross: 0, discount: 0, net: 0, tax: 0, total: 0 }
);

const taxByJurisdiction = priced.reduce((acc, p) => {
  acc[p.jurisdiction] = round2((acc[p.jurisdiction] ?? 0) + p.tax);
  return acc;
}, {});

const clock = await getBillingClock();
const invoiceId = "inv_" + clock;

await saveInvoice({ invoiceId, lineItems: priced, totals, currency: "USD" });
({ invoiceId, priced, totals, taxByJurisdiction })
`;

await test("prices a multi-jurisdiction invoice with tiered discounts and exact cents", async () => {
  const state = { saved: [] };
  const result = await execute(INVOICE_PROGRAM, createPricingTools(state));

  assert.equal(result.output.invoiceId, `inv_${BILLING_NOW_MS}`);
  assert.deepEqual(result.output.priced, [
    { sku: "SEAT-STD", jurisdiction: "CA", gross: 1500, discountRate: 0.2, discount: 300, net: 1200, taxRate: 0.0725, tax: 87, total: 1287 },
    { sku: "SEAT-PRO", jurisdiction: "CA", gross: 1350, discountRate: 0.05, discount: 67.5, net: 1282.5, taxRate: 0.0725, tax: 92.98, total: 1375.48 },
    { sku: "ADDON-SMS", jurisdiction: "NY", gross: 39.92, discountRate: 0, discount: 0, net: 39.92, taxRate: 0.08, tax: 3.19, total: 43.11 },
    { sku: "ADDON-API", jurisdiction: "NY", gross: 199, discountRate: 0, discount: 0, net: 199, taxRate: 0.08, tax: 15.92, total: 214.92 },
    { sku: "SUP-GOLD", jurisdiction: "TX", gross: 1.2, discountRate: 0.05, discount: 0.06, net: 1.14, taxRate: 0.0625, tax: 0.07, total: 1.21 },
  ]);
  assert.deepEqual(result.output.totals, {
    gross: 3090.12,
    discount: 367.56,
    net: 2722.56,
    tax: 199.16,
    total: 2921.72,
  });
  assert.deepEqual(result.output.taxByJurisdiction, { CA: 179.98, NY: 19.11, TX: 0.07 });
});

await test("invoice is persisted exactly once and host input matches the last tool call", async () => {
  const state = { saved: [] };
  const result = await execute(INVOICE_PROGRAM, createPricingTools(state));

  assert.equal(state.saved.length, 1);
  assert.equal(state.saved[0].invoiceId, `inv_${BILLING_NOW_MS}`);
  assert.equal(state.saved[0].currency, "USD");
  assert.equal(state.saved[0].lineItems.length, 5);
  assert.deepEqual(state.saved[0].totals, result.output.totals);
  // toolCalls introspection: last call is saveInvoice and its input == host-received input.
  const lastCall = result.toolCalls.at(-1);
  assert.equal(lastCall.name, "saveInvoice");
  assert.deepEqual(lastCall.input, state.saved[0]);
  assert.deepEqual(lastCall.result, { ok: true, invoiceId: `inv_${BILLING_NOW_MS}`, lineCount: 5 });
});

await test("float-drift cent fees aggregate to an exact rounded total", async () => {
  const state = { saved: [] };
  const result = await execute(
    `
    const round2 = n => Math.round(n * 100) / 100;
    // Classic 0.1 + 0.2 style drift: raw sum is 0.45000000000000007.
    const microFees = [0.10, 0.20, 0.05, 0.07, 0.03];
    const rawSum = microFees.reduce((a, b) => a + b, 0);
    const rounded = round2(rawSum);
    const driftDetected = rawSum !== 0.45;
    ({ rawSum, rounded, driftDetected, count: microFees.length })
    `,
    createPricingTools(state)
  );

  assert.equal(result.output.rounded, 0.45);
  assert.equal(result.output.driftDetected, true);
  assert.equal(result.output.count, 5);
  assert.deepEqual(state.saved, []);
});

await test("prorates annual plans by usage window with Math + chained pipeline", async () => {
  const state = { saved: [] };
  const result = await execute(
    `
    const round2 = n => Math.round(n * 100) / 100;
    const plans = [
      { id: "p1", annual: 1200,   daysUsed: 10, daysInPeriod: 30 },
      { id: "p2", annual: 999.99, daysUsed: 15, daysInPeriod: 31 },
      { id: "p3", annual: 480,    daysUsed: 7,  daysInPeriod: 28 },
      { id: "p4", annual: 600,    daysUsed: 0,  daysInPeriod: 30 },
    ];
    const prorated = plans
      .map(p => ({ id: p.id, amount: round2(p.annual * (p.daysUsed / p.daysInPeriod)) }))
      .filter(p => p.amount > 0)
      .sort((a, b) => b.amount - a.amount);
    const billed = round2(prorated.reduce((sum, p) => sum + p.amount, 0));
    const largest = Math.max(...prorated.map(p => p.amount));
    const smallest = Math.min(...prorated.map(p => p.amount));
    ({
      ids: prorated.map(p => p.id),
      amounts: prorated.map(p => p.amount),
      billed,
      largest,
      smallest,
      lookup: Object.fromEntries(prorated.map(p => [p.id, p.amount])),
    })
    `,
    createPricingTools(state)
  );

  assert.deepEqual(result.output.ids, ["p2", "p1", "p3"]);
  assert.deepEqual(result.output.amounts, [483.87, 400, 120]);
  assert.equal(result.output.billed, 1003.87);
  assert.equal(result.output.largest, 483.87);
  assert.equal(result.output.smallest, 120);
  assert.deepEqual(result.output.lookup, { p2: 483.87, p1: 400, p3: 120 });
});

await test("flatMap expands bundles, then ternary tiers and Object.entries aggregate revenue", async () => {
  const state = { saved: [] };
  const result = await execute(
    `
    const round2 = n => Math.round(n * 100) / 100;
    const orders = [
      { region: "west", bundle: [{ sku: "A", amount: 120 }, { sku: "B", amount: 80 }] },
      { region: "east", bundle: [{ sku: "A", amount: 200 }] },
      { region: "west", bundle: [{ sku: "C", amount: 50 }, { sku: "A", amount: 40 }] },
      { region: "east", bundle: [{ sku: "B", amount: 310 }, { sku: "C", amount: 90 }] },
    ];
    // flatMap each order's bundle into flat priced rows tagged with region.
    const rows = orders.flatMap(o => o.bundle.map(item => ({ ...item, region: o.region })));
    // Tiered loyalty rebate via ternary chain on amount.
    const rebated = rows.map(r => {
      const rebateRate = r.amount >= 300 ? 0.15 : r.amount >= 100 ? 0.08 : 0.02;
      const rebate = round2(r.amount * rebateRate);
      return { ...r, rebate, net: round2(r.amount - rebate) };
    });
    // Aggregate net by region with an object accumulator + computed key.
    const byRegion = rebated.reduce((acc, r) => {
      acc[r.region] = round2((acc[r.region] ?? 0) + r.net);
      return acc;
    }, {});
    const ranked = Object.entries(byRegion)
      .map(([region, net]) => ({ region, net }))
      .sort((a, b) => b.net - a.net);
    ({ rowCount: rows.length, byRegion, ranked, totalRebate: round2(rebated.reduce((s, r) => s + r.rebate, 0)) })
    `,
    createPricingTools(state)
  );

  // west: A120(rebate 9.6 ->110.4) B80(1.6->78.4) C50(1->49) A40(0.8->39.2) = 277.0
  // east: A200(16->184) B310(46.5->263.5) C90(1.8->88.2) = 535.7
  assert.equal(result.output.rowCount, 7);
  assert.deepEqual(result.output.byRegion, { west: 277, east: 535.7 });
  assert.deepEqual(result.output.ranked, [
    { region: "east", net: 535.7 },
    { region: "west", net: 277 },
  ]);
  assert.equal(result.output.totalRebate, 77.3);
});

await test("rejects missing required invoice argument before any host persistence", async () => {
  const state = { saved: [] };
  await assert.rejects(
    () =>
      execute(
        `
        await saveInvoice({
          lineItems: [{ sku: "X", total: 10 }],
          totals: { total: 10 },
        })
        `,
        createPricingTools(state)
      ),
    /Invalid arguments for tool 'saveInvoice': missing required parameter 'invoiceId'/
  );
  assert.deepEqual(state.saved, []);
});

await test("rejects wrong-typed and unexpected invoice arguments before host persistence", async () => {
  const state = { saved: [] };

  await assert.rejects(
    () =>
      execute(
        `await saveInvoice({ invoiceId: "inv_1", lineItems: { a: 1 }, totals: { total: 0 } })`,
        createPricingTools(state)
      ),
    /Invalid arguments for tool 'saveInvoice': parameter 'lineItems' expected array, got object/
  );
  await assert.rejects(
    () =>
      execute(
        `await saveInvoice({ invoiceId: "inv_1", lineItems: [], totals: [] })`,
        createPricingTools(state)
      ),
    /Invalid arguments for tool 'saveInvoice': parameter 'totals' expected object, got array/
  );
  await assert.rejects(
    () =>
      execute(
        `await saveInvoice({ invoiceId: "inv_1", lineItems: [], totals: {}, draft: true })`,
        createPricingTools(state)
      ),
    /Invalid arguments for tool 'saveInvoice': unexpected parameter 'draft'/
  );
  await assert.rejects(
    () => execute(`await saveInvoice("inv_1", [], {})`, createPricingTools(state)),
    /Invalid arguments for tool 'saveInvoice': expected one named object argument/
  );

  assert.deepEqual(state.saved, []);
});

await test("getTaxTable validation rejects bad argument type before resolution", async () => {
  const state = { saved: [] };
  await assert.rejects(
    () => execute(`await getTaxTable({ jurisdictions: "CA" })`, createPricingTools(state)),
    /Invalid arguments for tool 'getTaxTable': parameter 'jurisdictions' expected array, got string/
  );
  assert.deepEqual(state.saved, []);
});

console.log(`\n${passed} pricing checks passed.`);
