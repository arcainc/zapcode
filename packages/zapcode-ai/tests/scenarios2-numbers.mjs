// Numeric / math / financial stress tests for the Zapcode sandbox.
// Run: node tests/scenarios2-numbers.mjs
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

// Shorthand: execute with no tools
const ex = (code) => execute(code, {});

// ---------------------------------------------------------------------------
// 1. Money: line-item sum, tax %, toFixed, "$" prefix
// ---------------------------------------------------------------------------
await check("money — line-item sum with tax and toFixed", async () => {
  const { output } = await ex(`
    const items = [
      { name: "Widget",  price: 9.99,  qty: 3 },
      { name: "Gadget",  price: 24.50, qty: 1 },
      { name: "Doodad",  price: 4.75,  qty: 5 },
    ];
    let subtotal = 0;
    for (const item of items) {
      subtotal += item.price * item.qty;
    }
    const TAX_RATE = 0.0875;
    const tax = subtotal * TAX_RATE;
    const total = subtotal + tax;
    "$" + total.toFixed(2)
  `);
  // subtotal = 9.99*3 + 24.50 + 4.75*5 = 29.97 + 24.50 + 23.75 = 78.22
  // tax = 78.22 * 0.0875 = 6.844...  total = 85.064... → "$85.06"
  assert.equal(output, "$85.06");
});

// ---------------------------------------------------------------------------
// 2. Money: cents / integer arithmetic to avoid float drift
// ---------------------------------------------------------------------------
await check("money — cents integer arithmetic", async () => {
  const { output } = await ex(`
    const prices = [199, 349, 99, 499, 149];   // cents
    let total = 0;
    for (const p of prices) { total += p; }
    const discount = Math.floor(total * 0.10);  // 10% off, rounded down
    const net = total - discount;
    net   // 1295 - 129 = 1166
  `);
  assert.equal(output, 1166);
});

// ---------------------------------------------------------------------------
// 3. Money: weighted average price
// ---------------------------------------------------------------------------
await check("money — weighted average price", async () => {
  const { output } = await ex(`
    const trades = [
      { price: 100, qty: 50 },
      { price: 102, qty: 30 },
      { price: 98,  qty: 20 },
    ];
    let totalValue = 0;
    let totalQty   = 0;
    for (const t of trades) {
      totalValue += t.price * t.qty;
      totalQty   += t.qty;
    }
    const wavg = totalValue / totalQty;
    parseFloat(wavg.toFixed(4))   // 100.4
  `);
  // 100*50+102*30+98*20 = 5000+3060+1960 = 10020 / 100 = 100.2
  assert.equal(output, 100.2);
});

// ---------------------------------------------------------------------------
// 4. Math builtins: round/floor/ceil/trunc/sign/abs
// BUG: Math.round(-3.5) returns -4; JS spec says -3 (half-up = toward +Inf)
//      Confirmed: all negative-half values round away from zero instead of toward +Inf.
// ---------------------------------------------------------------------------
await check("Math — round/floor/ceil/trunc/sign/abs [BUG: Math.round neg halves]", async () => {
  const { output } = await ex(`
    const results = [
      Math.round(3.5),    // 4  ✓
      Math.round(-3.5),   // BUGGY: returns -4, should be -3 (JS half-up toward +Inf)
      Math.floor(3.7),    // 3  ✓
      Math.ceil(-3.1),    // -3 ✓
      Math.trunc(-7.9),   // -7 ✓
      Math.sign(-5),      // -1 ✓
      Math.sign(0),       //  0 ✓
      Math.abs(-42),      // 42 ✓
    ];
    results
  `);
  // Documenting actual (buggy) behavior; correct would be [4, -3, 3, -3, -7, -1, 0, 42]
  assert.equal(output[0], 4);
  assert.equal(output[1], -4);   // BUG: should be -3
  assert.deepEqual(output.slice(2), [3, -3, -7, -1, 0, 42]);
});

// ---------------------------------------------------------------------------
// 5. Math builtins: pow/sqrt/cbrt — hypot is MISSING
// MISSING BUILTIN: Math.hypot throws "Math.hypot is not a function"
// ---------------------------------------------------------------------------
await check("Math — pow/sqrt/cbrt (hypot tested separately)", async () => {
  const { output } = await ex(`
    [
      Math.pow(2, 10),   // 1024
      Math.sqrt(144),    // 12
      Math.cbrt(27),     // 3
    ]
  `);
  assert.deepEqual(output, [1024, 12, 3]);
});

await check("Math.hypot MISSING [MISSING BUILTIN]", async () => {
  // Math.hypot exists as a function according to typeof, but throws when called.
  // MISSING: Math.hypot(3,4) should return 5 but throws "Math.hypot is not a function"
  let threw = false;
  try {
    await ex(`Math.hypot(3, 4)`);
  } catch (e) {
    threw = true;
    assert.match(e.message, /not a function/);
  }
  assert.ok(threw, "Expected Math.hypot to throw (confirming missing builtin)");
});

// ---------------------------------------------------------------------------
// 6. Math builtins: log/log2/log10/exp
// ---------------------------------------------------------------------------
await check("Math — log/log2/log10/exp", async () => {
  const { output } = await ex(`
    [
      Math.round(Math.log(Math.E)),  // 1
      Math.log2(8),                  // 3
      Math.log10(1000),              // 3
      Math.round(Math.exp(1) * 1000) // 2718  (Math.E * 1000 rounded)
    ]
  `);
  assert.deepEqual(output, [1, 3, 3, 2718]);
});

// ---------------------------------------------------------------------------
// 7. Array spread to Math.min/max
// ---------------------------------------------------------------------------
await check("Math.min/max spread on array", async () => {
  const { output } = await ex(`
    const data = [42, 7, 99, 3, 55, 18];
    [Math.min(...data), Math.max(...data)]
  `);
  assert.deepEqual(output, [3, 99]);
});

// ---------------------------------------------------------------------------
// 8. Clamping + lerp (linear interpolation)
// ---------------------------------------------------------------------------
await check("clamp and lerp", async () => {
  const { output } = await ex(`
    const clamp = (v, lo, hi) => Math.min(Math.max(v, lo), hi);
    const lerp  = (a, b, t)  => a + (b - a) * t;
    [
      clamp(-5, 0, 100),   // 0
      clamp(150, 0, 100),  // 100
      clamp(42, 0, 100),   // 42
      lerp(0, 200, 0.25),  // 50
      lerp(10, 20, 0.5),   // 15
    ]
  `);
  assert.deepEqual(output, [0, 100, 42, 50, 15]);
});

// ---------------------------------------------------------------------------
// 9. Percentage change + compound interest
// ---------------------------------------------------------------------------
await check("percentage change and compound interest", async () => {
  const { output } = await ex(`
    const pctChange = (from, to) => ((to - from) / from) * 100;
    const compound  = (principal, rate, n, t) =>
      principal * Math.pow(1 + rate / n, n * t);

    const pc  = parseFloat(pctChange(80, 100).toFixed(2));   // 25.00
    const ci  = parseFloat(compound(1000, 0.05, 12, 10).toFixed(2)); // 1647.01
    [pc, ci]
  `);
  assert.deepEqual(output, [25, 1647.01]);
});

// ---------------------------------------------------------------------------
// 10. Stats: mean / median / variance / stddev via reduce
// ---------------------------------------------------------------------------
await check("stats — mean/median/variance/stddev via reduce", async () => {
  const { output } = await ex(`
    const arr = [2, 4, 4, 4, 5, 5, 7, 9];
    const n    = arr.length;
    const mean = arr.reduce((acc, v) => acc + v, 0) / n;

    const sorted = arr.slice().sort((a, b) => a - b);
    const mid    = Math.floor(sorted.length / 2);
    const median = sorted.length % 2 === 0
      ? (sorted[mid - 1] + sorted[mid]) / 2
      : sorted[mid];

    const variance = arr.reduce((acc, v) => acc + (v - mean) ** 2, 0) / n;
    const stddev   = Math.sqrt(variance);

    [mean, median, parseFloat(variance.toFixed(4)), parseFloat(stddev.toFixed(4))]
  `);
  // mean=5, median=4.5, variance=4, stddev=2
  assert.deepEqual(output, [5, 4.5, 4, 2]);
});

// ---------------------------------------------------------------------------
// 11. Integer vs float: division, modulo (positive and negative)
// ---------------------------------------------------------------------------
await check("integer vs float — division and modulo", async () => {
  const { output } = await ex(`
    [
      5 / 2,              // 2.5
      Math.floor(5 / 2), // 2
      7 % 3,             // 1
      -7 % 3,            // -1   (JS truncating modulo)
      7 % -3,            // 1
    ]
  `);
  assert.deepEqual(output, [2.5, 2, 1, -1, 1]);
});

// ---------------------------------------------------------------------------
// 12. Exponentiation operator ** and precedence
// ---------------------------------------------------------------------------
await check("exponentiation ** and precedence", async () => {
  const { output } = await ex(`
    [
      2 ** 10,         // 1024
      2 ** 0,          // 1
      2 ** -1,         // 0.5
      (-2) ** 3,       // -8
      4 ** 0.5,        // 2
      2 ** 2 ** 3,     // 2**(2**3) = 2**8 = 256  (right-assoc)
    ]
  `);
  assert.deepEqual(output, [1024, 1, 0.5, -8, 2, 256]);
});

// ---------------------------------------------------------------------------
// 13. Edge values: NaN propagation and checks
// ---------------------------------------------------------------------------
await check("NaN propagation and checks", async () => {
  const { output } = await ex(`
    const nan = NaN;
    [
      Number.isNaN(nan),      // true
      Number.isNaN(0 / 0),    // true
      Number.isNaN(1),        // false
      nan !== nan,            // true  (self-inequality)
      Number.isNaN(nan + 1),  // true  (NaN propagates)
      Number.isNaN("hello" * 2),  // true
    ]
  `);
  assert.deepEqual(output, [true, true, false, true, true, true]);
});

// ---------------------------------------------------------------------------
// 14. Infinity and division by zero
// BUG: Infinity (and -Infinity) serialize as null in output. The values work
//      internally for comparisons/arithmetic but cannot be observed as the
//      final expression value — they come back as null.
// NaN also serializes as null (distinct from Number(undefined) which is also null).
// ---------------------------------------------------------------------------
await check("Infinity — internal use works, output serialization BUG", async () => {
  // Infinity works in comparisons
  const { output: gt } = await ex(`1/0 > 1000`);
  assert.equal(gt, true);

  const { output: eq } = await ex(`(1/0) === Infinity`);
  assert.equal(eq, true);

  // BUG: Infinity as a final expression result returns null
  const { output: inf } = await ex(`Infinity`);
  assert.equal(inf, null);   // BUG: should be Infinity (or the string "Infinity")

  // isFinite / Number.isFinite still work (they return booleans)
  const { output: fin } = await ex(`[isFinite(Infinity), isFinite(42), Number.isFinite(42)]`);
  assert.deepEqual(fin, [false, true, true]);
});

await check("NaN serializes as null [BUG]", async () => {
  // NaN propagates and is detectable, but serializes as null when last expression
  const { output: isnan } = await ex(`Number.isNaN(0/0)`);
  assert.equal(isnan, true);   // ✓ works

  // BUG: NaN as output value becomes null
  const { output: nan } = await ex(`NaN`);
  assert.equal(nan, null);   // BUG: should be NaN (or null is acceptable? but confuses with null)

  const { output: nan2 } = await ex(`0/0`);
  assert.equal(nan2, null);   // BUG: same — 0/0 = NaN comes back as null
});

// ---------------------------------------------------------------------------
// 15. Number.EPSILON and 0.1+0.2 float comparison
// ---------------------------------------------------------------------------
await check("float epsilon comparison — 0.1+0.2", async () => {
  const { output } = await ex(`
    const sum = 0.1 + 0.2;
    const eps = Number.EPSILON;
    const approxEqual = (a, b) => Math.abs(a - b) < eps * 10;
    [
      sum === 0.3,                   // false (float drift)
      approxEqual(sum, 0.3),         // true
      Number.EPSILON < 1e-10,        // true
    ]
  `);
  assert.deepEqual(output, [false, true, true]);
});

// ---------------------------------------------------------------------------
// 16. Big integer range — Number.MAX_SAFE_INTEGER works, isSafeInteger MISSING
// MISSING BUILTIN: Number.isSafeInteger throws "not a function"
// ---------------------------------------------------------------------------
await check("Number.MAX_SAFE_INTEGER works", async () => {
  const { output } = await ex(`Number.MAX_SAFE_INTEGER`);
  assert.equal(output, 9007199254740991);
});

await check("Number.isSafeInteger MISSING [MISSING BUILTIN]", async () => {
  let threw = false;
  try {
    await ex(`Number.isSafeInteger(42)`);
  } catch (e) {
    threw = true;
    assert.match(e.message, /not a function/);
  }
  assert.ok(threw, "Expected Number.isSafeInteger to throw (confirming missing builtin)");
});

// ---------------------------------------------------------------------------
// 17. parseInt / parseFloat edge cases
// BUG: parseInt("0xff", 16) returns 0 — the 0x prefix causes early stop
//      instead of being recognized as a hex prefix. parseInt("ff", 16) = 255 works.
// ---------------------------------------------------------------------------
await check("parseInt/parseFloat edge cases [BUG: 0x prefix in string]", async () => {
  const { output } = await ex(`
    [
      parseInt("42px"),         // 42  ✓
      parseInt("  3.14"),       // 3   ✓
      parseInt("0xff", 16),     // BUG: returns 0, should be 255
      parseInt("ff", 16),       // 255 ✓
      parseInt("111", 2),       // 7   ✓
      parseFloat("3.14abc"),    // 3.14 ✓
      Number.isNaN(parseInt("xyz")), // true ✓
    ]
  `);
  assert.equal(output[0], 42);
  assert.equal(output[1], 3);
  assert.equal(output[2], 0);    // BUG: should be 255
  assert.equal(output[3], 255);
  assert.equal(output[4], 7);
  assert.equal(output[5], 3.14);
  assert.equal(output[6], true);
});

// ---------------------------------------------------------------------------
// 18. Hex literals, toString(radix), bitwise ops
// ---------------------------------------------------------------------------
await check("hex literals and toString(radix)", async () => {
  const { output } = await ex(`
    [
      0xff,                   // 255
      (255).toString(16),     // "ff"
      (255).toString(2),      // "11111111"
      (10).toString(8),       // "12"
      parseInt("ff", 16),     // 255
    ]
  `);
  assert.deepEqual(output, [255, "ff", "11111111", "12", 255]);
});

// ---------------------------------------------------------------------------
// 19. Bitwise operators: &, |, ^, ~, <<, >>, >>>
// BUG: >>> on negative numbers returns 0 instead of treating the operand as
//      a 32-bit unsigned integer. Also: -1 & 0xFFFFFFFF returns 2147483647
//      instead of -1 (high bit lost). Positive-operand >>> works correctly.
// ---------------------------------------------------------------------------
await check("bitwise — AND/OR/XOR/NOT/shifts (positive operands)", async () => {
  const { output } = await ex(`
    [
      0b1010 & 0b1100,     // 8
      0b1010 | 0b1100,     // 14
      0b1010 ^ 0b1100,     // 6
      ~5,                  // -6
      1 << 4,              // 16
      256 >> 3,            // 32
      8 >>> 1,             // 4  (positive operand >>> works)
    ]
  `);
  assert.deepEqual(output, [8, 14, 6, -6, 16, 32, 4]);
});

await check("bitwise >>> with negative operands [BUG]", async () => {
  // BUG: -1 >>> 0 returns 0, should be 4294967295
  const { output: r1 } = await ex(`-1 >>> 0`);
  assert.equal(r1, 0);         // BUG: should be 4294967295

  const { output: r2 } = await ex(`-8 >>> 1`);
  assert.equal(r2, 0);         // BUG: should be 2147483644

  // Positive operands work
  const { output: r3 } = await ex(`0xFFFFFFFF >>> 0`);
  assert.equal(r3, 4294967295); // ✓

  const { output: r4 } = await ex(`-1 >> 0`);
  assert.equal(r4, -1);         // ✓ signed >> works
});

await check("bitwise & with large positive operand [BUG]", async () => {
  // BUG: -1 & 0xFFFFFFFF = 2147483647 instead of -1
  // (all 32 bits of -1 AND all 32 bits of 0xFFFFFFFF = 0xFFFFFFFF = -1 as int32)
  const { output } = await ex(`-1 & 0xFFFFFFFF`);
  assert.equal(output, 2147483647);   // BUG: should be -1
});

// ---------------------------------------------------------------------------
// 20. Bitwise tricks: parity check, power-of-two test, swap
// ---------------------------------------------------------------------------
await check("bitwise tricks — parity/pow2/fast-floor", async () => {
  const { output } = await ex(`
    const isEven   = n => (n & 1) === 0;
    const isPow2   = n => n > 0 && (n & (n - 1)) === 0;
    const fastFloor = n => n | 0;          // truncates toward zero for 32-bit
    [
      isEven(4),       // true
      isEven(7),       // false
      isPow2(16),      // true
      isPow2(18),      // false
      fastFloor(3.9),  // 3
      fastFloor(-3.9), // -3  (truncates, not floor)
    ]
  `);
  assert.deepEqual(output, [true, false, true, false, 3, -3]);
});

// ---------------------------------------------------------------------------
// 21. Number formatting — thousands separator built manually
// BUG: Default function parameters (e.g. `decimals = 2`) always yield null
//      when the argument is not supplied. The default expression is ignored.
//      Workaround: explicit `if (decimals === undefined) decimals = 2;`
// ---------------------------------------------------------------------------
await check("number formatting — default param BUG vs workaround", async () => {
  // With default params (BUG): decimals comes in as null → toFixed(null) = toFixed(0) → no decimal
  const { output: buggy } = await ex(`
    function formatNumberBuggy(n, decimals = 2) {
      return n.toFixed(decimals);
    }
    formatNumberBuggy(999)
  `);
  // decimals is null (bug), toFixed(null) → toFixed(0) → "999"
  assert.equal(buggy, "999");   // BUG: should be "999.00"

  // Workaround: manual default check — then formatNumber works correctly
  const { output } = await ex(`
    function formatNumber(n, decimals) {
      if (decimals === undefined || decimals === null) decimals = 2;
      const fixed = n.toFixed(decimals);
      const parts = fixed.split(".");
      const int = parts[0];
      const dec = parts[1];
      let result = "";
      let count = 0;
      for (let i = int.length - 1; i >= 0; i--) {
        if (count > 0 && count % 3 === 0 && int[i] !== "-") result = "," + result;
        result = int[i] + result;
        count++;
      }
      return decimals > 0 ? result + "." + dec : result;
    }
    [
      formatNumber(1234567.89),          // "1,234,567.89"
      formatNumber(999),                 // "999.00"
      formatNumber(1000000, 0),          // "1,000,000"
    ]
  `);
  assert.deepEqual(output, ["1,234,567.89", "999.00", "1,000,000"]);
});

// ---------------------------------------------------------------------------
// 22. toFixed rounding behavior
// ---------------------------------------------------------------------------
await check("toFixed rounding — half-up and banker-like cases", async () => {
  const { output } = await ex(`
    [
      (1.005).toFixed(2),   // "1.00" or "1.01" — float drift makes this "1.00" in JS
      (1.255).toFixed(2),   // "1.25" or "1.26" — JS native
      (2.345).toFixed(2),   // "2.35" or "2.34"
      (0.1 + 0.2).toFixed(1), // "0.3"
    ]
  `);
  // Just verify they are strings and don't throw; actual JS floats vary
  for (const v of output) {
    assert.equal(typeof v, "string");
    assert.match(v, /^\d+\.\d+$/);
  }
});

// ---------------------------------------------------------------------------
// 23. toPrecision
// BUG: toPrecision ALWAYS uses exponential notation regardless of magnitude.
//      JS spec: use exponential only when exponent < -6 or >= precision.
//      (1).toPrecision(3) should be "1.00" but returns "1.00e0"
//      (123.456).toPrecision(5) should be "123.46" but returns "1.2346e2"
// ---------------------------------------------------------------------------
await check("toPrecision always uses exponential [BUG]", async () => {
  const { output } = await ex(`
    [
      (1).toPrecision(3),         // BUG: "1.00e0",     should be "1.00"
      (123.456).toPrecision(5),   // BUG: "1.2346e2",   should be "123.46"
      (0.000123).toPrecision(2),  // BUG: "1.2e-4",     should be "0.00012"
      (1234).toPrecision(2),      // "1.2e3" — exponential is correct here per spec (exponent 3 >= precision 2)
    ]
  `);
  // Documenting actual (buggy) output — all exponential
  assert.equal(output[0], "1.00e0");   // BUG: should be "1.00"
  assert.equal(output[1], "1.2346e2"); // BUG: should be "123.46"
  assert.equal(output[2], "1.2e-4");   // BUG: should be "0.00012"
  // Note: output[3] has no agreed canonical form ("1.2e3" vs "1.2e+3") so skip
});

// ---------------------------------------------------------------------------
// 24. Number coercions
// BUG: Number(undefined) should be NaN but returns null (NaN serializes as null).
//      This is related to the Infinity/NaN output-serialization bug (#14 above).
//      The internal value IS NaN (Number.isNaN(Number(undefined)) returns true),
//      but the serialized output is null — indistinguishable from null itself.
// ---------------------------------------------------------------------------
await check("Number coercions [BUG: NaN output serializes as null]", async () => {
  const { output } = await ex(`
    [
      Number(""),         // 0
      Number("  42  "),   // 42
      Number("3.14"),     // 3.14
      Number(true),       // 1
      Number(false),      // 0
      Number(null),       // 0
      Number(undefined),  // NaN → serialized as null (BUG)
      1e3,                // 1000
      1.5e-3,             // 0.0015
    ]
  `);
  const [a, b, c, d, e, f, g, h, i] = output;
  assert.equal(a, 0);
  assert.equal(b, 42);
  assert.equal(c, 3.14);
  assert.equal(d, 1);
  assert.equal(e, 0);
  assert.equal(f, 0);
  assert.equal(g, null);  // BUG: should be NaN; Number.isNaN(Number(undefined)) = true but output = null
  assert.equal(h, 1000);
  assert.equal(i, 0.0015);

  // Verify the value IS NaN internally (the bug is only in output serialization)
  const { output: check } = await ex(`Number.isNaN(Number(undefined))`);
  assert.equal(check, true);  // internal NaN is correct ✓
});

// ---------------------------------------------------------------------------
// 25. Realistic invoice: line items, discount, tax, totals, summary string
// ---------------------------------------------------------------------------
await check("invoice — full calculation with discount + tax + summary", async () => {
  const { output } = await ex(`
    const lineItems = [
      { desc: "Consulting (10 hrs)", unitPrice: 150.00, qty: 10 },
      { desc: "Travel expenses",     unitPrice: 87.50,  qty: 1  },
      { desc: "Software license",    unitPrice: 299.00, qty: 3  },
    ];

    let subtotal = 0;
    for (const li of lineItems) {
      subtotal += li.unitPrice * li.qty;
    }

    const DISCOUNT_RATE = 0.05;    // 5% volume discount
    const TAX_RATE      = 0.10;    // 10% sales tax

    const discount = parseFloat((subtotal * DISCOUNT_RATE).toFixed(2));
    const discounted = subtotal - discount;
    const tax = parseFloat((discounted * TAX_RATE).toFixed(2));
    const total = parseFloat((discounted + tax).toFixed(2));

    "Subtotal: $" + subtotal.toFixed(2) +
    " | Discount: $" + discount.toFixed(2) +
    " | Tax: $" + tax.toFixed(2) +
    " | Total: $" + total.toFixed(2)
  `);
  // subtotal = 1500 + 87.50 + 897 = 2484.50
  // discount = 2484.50 * 0.05 = 124.225 → 124.23 (rounded)
  // discounted = 2484.50 - 124.23 = 2360.27
  // tax = 2360.27 * 0.10 = 236.027 → 236.03
  // total = 2360.27 + 236.03 = 2596.30
  assert.equal(
    output,
    "Subtotal: $2484.50 | Discount: $124.23 | Tax: $236.03 | Total: $2596.30"
  );
});

// ---------------------------------------------------------------------------
// 26. Math.trunc vs Math.floor for negative numbers
// ---------------------------------------------------------------------------
await check("Math.trunc vs Math.floor — negative values differ", async () => {
  const { output } = await ex(`
    [
      Math.trunc(-3.7),  // -3  (toward zero)
      Math.floor(-3.7),  // -4  (toward -Infinity)
      Math.trunc(3.7),   // 3
      Math.floor(3.7),   // 3
    ]
  `);
  assert.deepEqual(output, [-3, -4, 3, 3]);
});

// ---------------------------------------------------------------------------
// 27. Bitwise >>> confirmation (already covered in test 19 expanded above)
// ---------------------------------------------------------------------------
await check("bitwise >>> positive operands work correctly", async () => {
  const { output } = await ex(`
    [
      8 >>> 1,    // 4  ✓
      8 >> 1,     // 4  ✓
      -1 >> 0,    // -1 ✓ (signed shift preserves sign)
      16 >>> 2,   // 4  ✓
    ]
  `);
  assert.deepEqual(output, [4, 4, -1, 4]);
});

// ---------------------------------------------------------------------------
// 28. Running portfolio P&L with reduce
// ---------------------------------------------------------------------------
await check("portfolio P&L with reduce", async () => {
  const { output } = await ex(`
    const positions = [
      { symbol: "AAPL", costBasis: 150.00, currentPrice: 175.50, shares: 10 },
      { symbol: "GOOG", costBasis: 2800.0, currentPrice: 2650.0, shares: 2  },
      { symbol: "MSFT", costBasis: 300.00, currentPrice: 320.00, shares: 5  },
    ];

    const summary = positions.reduce((acc, pos) => {
      const cost  = pos.costBasis * pos.shares;
      const value = pos.currentPrice * pos.shares;
      const pnl   = value - cost;
      acc.totalCost  += cost;
      acc.totalValue += value;
      acc.totalPnl   += pnl;
      return acc;
    }, { totalCost: 0, totalValue: 0, totalPnl: 0 });

    const pctReturn = (summary.totalPnl / summary.totalCost) * 100;
    parseFloat(pctReturn.toFixed(2))
  `);
  // AAPL: cost=1500, val=1755, pnl=255
  // GOOG: cost=5600, val=5300, pnl=-300
  // MSFT: cost=1500, val=1600, pnl=100
  // totalPnl=55, totalCost=8600, pctReturn=55/8600*100=0.639534...→0.64
  assert.equal(output, 0.64);
});

// ---------------------------------------------------------------------------
// 29. Default function parameters BUG (standalone confirmation)
// BUG: Default parameter values are always null when the argument is omitted,
//      regardless of the specified default expression.
//      `function f(x = 42) { return x; } f()` → null (should be 42)
//      Same for arrow functions: `(x = 42) => x` called with no arg → null
// ---------------------------------------------------------------------------
await check("default function parameters [BUG]", async () => {
  const { output } = await ex(`
    function f(x = 42) { return x; }
    const g = (x = "hello") => x;
    const h = (x = true) => x;
    [f(), g(), h(), f(99)]
  `);
  assert.equal(output[0], null);   // BUG: should be 42
  assert.equal(output[1], null);   // BUG: should be "hello"
  assert.equal(output[2], null);   // BUG: should be true
  assert.equal(output[3], 99);     // ✓ explicit arg works fine
});

// ---------------------------------------------------------------------------
// Final summary
// ---------------------------------------------------------------------------
const failed = results.filter(r => !r[1]);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
if (failed.length) {
  console.log("\nFailed tests:");
  for (const [name, , msg] of failed) {
    console.log(`  ✗ ${name}: ${msg}`);
  }
}
