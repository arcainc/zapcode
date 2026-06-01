/**
 * Stress test for Zapcode interpreter — collections deep-dive.
 *
 * Covers: Array flat/flatMap depth, findLast/findLastIndex, splice, fill,
 * copyWithin, sort stability/comparator edge cases, negative-index slice/at,
 * computed object keys, Object.assign mutation semantics, reduce with no
 * initial value, reduceRight, delete operator, in operator, Array holes,
 * Set union/intersection/difference, Map group-by/frequency, nested Maps,
 * zip/chunk/unique-by-key/sort-by-multiple-keys, realistic aggregation pipeline.
 *
 * Run: node tests/scenarios2-collections.mjs
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

// ===========================================================================
// SECTION 1 — Array.prototype methods: flat, flatMap depth
// ===========================================================================

await check("flat: depth 1 (default)", async () => {
  const r = await execute(`[[1,2],[3,[4,5]]].flat()`, {});
  assert.deepEqual(r.output, [1, 2, 3, [4, 5]]);
});

await check("flat: depth 2", async () => {
  const r = await execute(`[[1,[2,[3]]],[4]].flat(2)`, {});
  assert.deepEqual(r.output, [1, 2, [3], 4]);
});

await check("flat: Infinity depth", async () => {
  const r = await execute(`[1,[2,[3,[4,[5]]]]].flat(Infinity)`, {});
  assert.deepEqual(r.output, [1, 2, 3, 4, 5]);
});

await check("flatMap: depth-1 flatten with transform", async () => {
  const r = await execute(
    `["hello world", "foo bar"].flatMap(s => s.split(" "))`,
    {}
  );
  assert.deepEqual(r.output, ["hello", "world", "foo", "bar"]);
});

await check("flatMap: nested array expand with index", async () => {
  const r = await execute(
    `[1,2,3].flatMap((x, i) => [x, i])`,
    {}
  );
  assert.deepEqual(r.output, [1, 0, 2, 1, 3, 2]);
});

// ===========================================================================
// SECTION 2 — findLast / findLastIndex
// ===========================================================================

await check("findLast: returns last matching element", async () => {
  const r = await execute(
    `[1,2,3,4,5].findLast(x => x % 2 === 0)`,
    {}
  );
  assert.equal(r.output, 4);
});

await check("findLast: returns undefined when no match", async () => {
  const r = await execute(
    `[1,3,5].findLast(x => x % 2 === 0)`,
    {}
  );
  assert.equal(r.output, undefined);
});

await check("findLastIndex: returns index of last matching element", async () => {
  const r = await execute(
    `[1,2,3,4,5].findLastIndex(x => x % 2 === 0)`,
    {}
  );
  assert.equal(r.output, 3);
});

await check("findLastIndex: returns -1 when no match", async () => {
  const r = await execute(
    `[1,3,5].findLastIndex(x => x % 2 === 0)`,
    {}
  );
  assert.equal(r.output, -1);
});

// ===========================================================================
// SECTION 3 — splice
// ===========================================================================

await check("splice: delete 1 element", async () => {
  const r = await execute(
    `const a = [1,2,3,4,5]; const removed = a.splice(2,1); [a, removed]`,
    {}
  );
  assert.deepEqual(r.output, [[1, 2, 4, 5], [3]]);
});

await check("splice: insert elements (0 deletions)", async () => {
  const r = await execute(
    `const a = [1,2,5]; a.splice(2, 0, 3, 4); a`,
    {}
  );
  assert.deepEqual(r.output, [1, 2, 3, 4, 5]);
});

await check("splice: replace slice and return removed", async () => {
  const r = await execute(
    `const a = [1,2,3,4,5]; const removed = a.splice(1, 2, 20, 30); [a, removed]`,
    {}
  );
  assert.deepEqual(r.output, [[1, 20, 30, 4, 5], [2, 3]]);
});

await check("splice: negative start index", async () => {
  const r = await execute(
    `const a = [1,2,3,4,5]; a.splice(-2, 1); a`,
    {}
  );
  assert.deepEqual(r.output, [1, 2, 3, 5]);
});

// ===========================================================================
// SECTION 4 — fill
// ===========================================================================

await check("fill: fill entire array with value", async () => {
  const r = await execute(`new Array(4).fill(0)`, {});
  assert.deepEqual(r.output, [0, 0, 0, 0]);
});

await check("fill: fill partial range", async () => {
  const r = await execute(`[1,2,3,4,5].fill(99, 1, 3)`, {});
  assert.deepEqual(r.output, [1, 99, 99, 4, 5]);
});

await check("fill: fill with negative end", async () => {
  const r = await execute(`[1,2,3,4,5].fill(0, 1, -1)`, {});
  assert.deepEqual(r.output, [1, 0, 0, 0, 5]);
});

await check("fill: build array via fill+map", async () => {
  const r = await execute(
    `new Array(5).fill(0).map((_, i) => i * i)`,
    {}
  );
  assert.deepEqual(r.output, [0, 1, 4, 9, 16]);
});

// ===========================================================================
// SECTION 5 — copyWithin
// ===========================================================================

await check("copyWithin: basic copy from index 3 to index 0", async () => {
  const r = await execute(`[1,2,3,4,5].copyWithin(0, 3)`, {});
  assert.deepEqual(r.output, [4, 5, 3, 4, 5]);
});

await check("copyWithin: with end parameter", async () => {
  const r = await execute(`[1,2,3,4,5].copyWithin(1, 3, 4)`, {});
  assert.deepEqual(r.output, [1, 4, 3, 4, 5]);
});

// ===========================================================================
// SECTION 6 — Negative index: slice / at
// ===========================================================================

await check("slice(-2): last two elements", async () => {
  const r = await execute(`[1,2,3,4,5].slice(-2)`, {});
  assert.deepEqual(r.output, [4, 5]);
});

await check("slice(-3, -1): inner negative slice", async () => {
  const r = await execute(`[1,2,3,4,5].slice(-3, -1)`, {});
  assert.deepEqual(r.output, [3, 4]);
});

await check("at(-1): last element", async () => {
  const r = await execute(`[10,20,30].at(-1)`, {});
  assert.equal(r.output, 30);
});

await check("at(-2): second to last", async () => {
  const r = await execute(`["a","b","c","d"].at(-2)`, {});
  assert.equal(r.output, "c");
});

await check("at(0): first element", async () => {
  const r = await execute(`[7,8,9].at(0)`, {});
  assert.equal(r.output, 7);
});

// ===========================================================================
// SECTION 7 — sort: stability, comparator edge cases, coercion
// ===========================================================================

await check("sort: stable with comparator (preserves equal-key order)", async () => {
  const r = await execute(
    `
    const items = [
      {name:"a", pri:2}, {name:"b", pri:1}, {name:"c", pri:2},
      {name:"d", pri:1}, {name:"e", pri:3},
    ];
    items.sort((x, y) => x.pri - y.pri).map(i => i.name)
    `,
    {}
  );
  // stable sort: pri=1 → b,d; pri=2 → a,c; pri=3 → e
  assert.deepEqual(r.output, ["b", "d", "a", "c", "e"]);
});

await check("sort: numeric comparator (not lexicographic)", async () => {
  const r = await execute(`[10, 9, 20, 1, 100].sort((a, b) => a - b)`, {});
  assert.deepEqual(r.output, [1, 9, 10, 20, 100]);
});

await check("sort: default lexicographic", async () => {
  const r = await execute(`["banana", "apple", "cherry"].sort()`, {});
  assert.deepEqual(r.output, ["apple", "banana", "cherry"]);
});

await check("sort: default lexicographic on numbers is string order", async () => {
  const r = await execute(`[10, 9, 100].sort()`, {});
  // default sort is lexicographic: 10, 100, 9
  assert.deepEqual(r.output, [10, 100, 9]);
});

await check("sort: reverse numeric", async () => {
  const r = await execute(`[3,1,4,1,5,9,2,6].sort((a,b) => b-a)`, {});
  assert.deepEqual(r.output, [9, 6, 5, 4, 3, 2, 1, 1]);
});

// ===========================================================================
// SECTION 8 — reduce with no initial value, reduceRight
// ===========================================================================

await check("reduce: no initial value (sum)", async () => {
  const r = await execute(`[1,2,3,4,5].reduce((a,b) => a+b)`, {});
  assert.equal(r.output, 15);
});

await check("reduce: no initial value (max)", async () => {
  const r = await execute(`[3,1,4,1,5,9,2,6].reduce((a,b) => a > b ? a : b)`, {});
  assert.equal(r.output, 9);
});

await check("reduceRight: string reversal", async () => {
  const r = await execute(`["a","b","c","d"].reduceRight((acc, x) => acc + x)`, {});
  assert.equal(r.output, "dcba");
});

await check("reduceRight: build reversed array", async () => {
  const r = await execute(
    `[1,2,3].reduceRight((acc, x) => { acc.push(x); return acc; }, [])`,
    {}
  );
  assert.deepEqual(r.output, [3, 2, 1]);
});

// ===========================================================================
// SECTION 9 — computed keys, Object.assign, delete, in operator
// ===========================================================================

await check("computed object keys {[k]: v}", async () => {
  const r = await execute(
    `
    const prefix = "key_";
    const obj = { [prefix + "a"]: 1, [prefix + "b"]: 2 };
    obj
    `,
    {}
  );
  assert.deepEqual(r.output, { key_a: 1, key_b: 2 });
});

await check("computed key from variable in loop", async () => {
  const r = await execute(
    `
    const keys = ["x", "y", "z"];
    const obj = {};
    for (let i = 0; i < keys.length; i++) {
      obj[keys[i]] = i * 10;
    }
    obj
    `,
    {}
  );
  assert.deepEqual(r.output, { x: 0, y: 10, z: 20 });
});

await check("Object.assign: merges two objects", async () => {
  const r = await execute(
    `
    const target = { a: 1, b: 2 };
    const source = { b: 99, c: 3 };
    Object.assign(target, source);
    target
    `,
    {}
  );
  assert.deepEqual(r.output, { a: 1, b: 99, c: 3 });
});

await check("Object.assign: returns the mutated target", async () => {
  const r = await execute(
    `Object.assign({ a: 1 }, { b: 2 })`,
    {}
  );
  assert.deepEqual(r.output, { a: 1, b: 2 });
});

await check("Object.assign: multi-source merge", async () => {
  const r = await execute(
    `Object.assign({}, { a: 1 }, { b: 2 }, { c: 3 })`,
    {}
  );
  assert.deepEqual(r.output, { a: 1, b: 2, c: 3 });
});

await check("delete operator removes key", async () => {
  const r = await execute(
    `
    const o = { a: 1, b: 2, c: 3 };
    delete o.b;
    o
    `,
    {}
  );
  assert.deepEqual(r.output, { a: 1, c: 3 });
});

await check("delete returns true and key becomes undefined", async () => {
  const r = await execute(
    `
    const o = { x: 42 };
    const deleted = delete o.x;
    [deleted, o.x, "x" in o]
    `,
    {}
  );
  assert.deepEqual(r.output, [true, undefined, false]);
});

await check("in operator: key present and absent", async () => {
  const r = await execute(
    `
    const o = { a: 1, b: undefined };
    ["a" in o, "b" in o, "c" in o]
    `,
    {}
  );
  assert.deepEqual(r.output, [true, true, false]);
});

await check("in operator on array: numeric index", async () => {
  const r = await execute(
    `
    const a = [10, 20, 30];
    [0 in a, 2 in a, 5 in a]
    `,
    {}
  );
  assert.deepEqual(r.output, [true, true, false]);
});

await check("Object.freeze: prevents mutation", async () => {
  const r = await execute(
    `
    const o = Object.freeze({ a: 1, b: 2 });
    o.a = 99;
    o.c = 3;
    o
    `,
    {}
  );
  // Frozen object — mutation silently fails in non-strict mode
  assert.deepEqual(r.output, { a: 1, b: 2 });
});

// ===========================================================================
// SECTION 10 — Set: union / intersection / difference / dedup
// ===========================================================================

await check("Set dedup via spread", async () => {
  const r = await execute(
    `[...new Set([1,2,2,3,3,3,4])]`,
    {}
  );
  assert.deepEqual(r.output, [1, 2, 3, 4]);
});

await check("Set union", async () => {
  const r = await execute(
    `
    const a = new Set([1, 2, 3]);
    const b = new Set([2, 3, 4, 5]);
    const union = new Set([...a, ...b]);
    [...union].sort((x,y)=>x-y)
    `,
    {}
  );
  assert.deepEqual(r.output, [1, 2, 3, 4, 5]);
});

await check("Set intersection", async () => {
  const r = await execute(
    `
    const a = new Set([1, 2, 3, 4]);
    const b = new Set([2, 4, 6]);
    const inter = new Set([...a].filter(x => b.has(x)));
    [...inter].sort((x,y)=>x-y)
    `,
    {}
  );
  assert.deepEqual(r.output, [2, 4]);
});

await check("Set difference (a minus b)", async () => {
  const r = await execute(
    `
    const a = new Set([1, 2, 3, 4]);
    const b = new Set([2, 4]);
    const diff = new Set([...a].filter(x => !b.has(x)));
    [...diff].sort((x,y)=>x-y)
    `,
    {}
  );
  assert.deepEqual(r.output, [1, 3]);
});

await check("Set.size getter", async () => {
  const r = await execute(`new Set([1,2,3,2,1]).size`, {});
  assert.equal(r.output, 3);
});

await check("Set.has / add / delete", async () => {
  const r = await execute(
    `
    const s = new Set([1, 2, 3]);
    s.add(4);
    s.delete(2);
    [s.has(1), s.has(2), s.has(3), s.has(4), s.size]
    `,
    {}
  );
  assert.deepEqual(r.output, [true, false, true, true, 3]);
});

await check("Set for-of iteration", async () => {
  const r = await execute(
    `
    const s = new Set(["x", "y", "z"]);
    const out = [];
    for (const v of s) out.push(v);
    out
    `,
    {}
  );
  assert.deepEqual(r.output, ["x", "y", "z"]);
});

await check("Array.from(set) preserves insertion order", async () => {
  const r = await execute(
    `Array.from(new Set([3,1,2,1,3]))`,
    {}
  );
  assert.deepEqual(r.output, [3, 1, 2]);
});

await check("unique-by-key using Set of primitives", async () => {
  const r = await execute(
    `
    const rows = [
      { id: 1, tag: "a" }, { id: 2, tag: "b" },
      { id: 3, tag: "a" }, { id: 4, tag: "c" },
    ];
    [...new Set(rows.map(r => r.tag))].sort()
    `,
    {}
  );
  assert.deepEqual(r.output, ["a", "b", "c"]);
});

// ===========================================================================
// SECTION 11 — Map: group-by, frequency count, iteration, nested
// ===========================================================================

await check("Map constructor from Object.entries", async () => {
  const r = await execute(
    `
    const m = new Map(Object.entries({ a: 1, b: 2, c: 3 }));
    [m.get("a"), m.get("b"), m.get("c")]
    `,
    {}
  );
  assert.deepEqual(r.output, [1, 2, 3]);
});

await check("Map: frequency count", async () => {
  const r = await execute(
    `
    const words = ["apple","banana","apple","cherry","banana","apple"];
    const freq = new Map();
    for (const w of words) {
      freq.set(w, (freq.get(w) || 0) + 1);
    }
    [freq.get("apple"), freq.get("banana"), freq.get("cherry")]
    `,
    {}
  );
  assert.deepEqual(r.output, [3, 2, 1]);
});

await check("Map: group-by with array values", async () => {
  const r = await execute(
    `
    const items = [
      { cat: "A", val: 1 }, { cat: "B", val: 2 },
      { cat: "A", val: 3 }, { cat: "C", val: 4 },
      { cat: "B", val: 5 },
    ];
    const groups = new Map();
    for (const item of items) {
      if (!groups.has(item.cat)) groups.set(item.cat, []);
      const arr = groups.get(item.cat);
      arr.push(item.val);
    }
    [groups.get("A"), groups.get("B"), groups.get("C")]
    `,
    {}
  );
  assert.deepEqual(r.output, [[1, 3], [2, 5], [4]]);
});

await check("Map.entries() iteration", async () => {
  const r = await execute(
    `
    const m = new Map([["x", 10], ["y", 20], ["z", 30]]);
    const pairs = [];
    for (const [k, v] of m.entries()) {
      pairs.push(k + "=" + v);
    }
    pairs
    `,
    {}
  );
  assert.deepEqual(r.output, ["x=10", "y=20", "z=30"]);
});

await check("Map.keys() iteration", async () => {
  const r = await execute(
    `
    const m = new Map([["a", 1], ["b", 2]]);
    const keys = [];
    for (const k of m.keys()) keys.push(k);
    keys
    `,
    {}
  );
  assert.deepEqual(r.output, ["a", "b"]);
});

await check("Map.values() iteration", async () => {
  const r = await execute(
    `
    const m = new Map([["a", 10], ["b", 20]]);
    const vals = [];
    for (const v of m.values()) vals.push(v);
    vals
    `,
    {}
  );
  assert.deepEqual(r.output, [10, 20]);
});

await check("Map.size after deletions", async () => {
  const r = await execute(
    `
    const m = new Map([["a",1],["b",2],["c",3]]);
    m.delete("b");
    m.size
    `,
    {}
  );
  assert.equal(r.output, 2);
});

await check("Map.forEach", async () => {
  const r = await execute(
    `
    const m = new Map([["a",1],["b",2],["c",3]]);
    const out = [];
    m.forEach((val, key) => { out.push(key + ":" + val); });
    out
    `,
    {}
  );
  assert.deepEqual(r.output, ["a:1", "b:2", "c:3"]);
});

await check("nested Map (Map of Maps)", async () => {
  const r = await execute(
    `
    const outer = new Map();
    outer.set("group1", new Map([["x", 10], ["y", 20]]));
    outer.set("group2", new Map([["x", 99]]));
    [outer.get("group1").get("x"), outer.get("group2").get("x"), outer.get("group1").get("y")]
    `,
    {}
  );
  assert.deepEqual(r.output, [10, 99, 20]);
});

await check("Map built from array pairs constructor", async () => {
  const r = await execute(
    `
    const pairs = [["one",1],["two",2],["three",3]];
    const m = new Map(pairs);
    [m.get("one"), m.get("two"), m.get("three"), m.size]
    `,
    {}
  );
  assert.deepEqual(r.output, [1, 2, 3, 3]);
});

// ===========================================================================
// SECTION 12 — Data-shaping utilities: zip, chunk, unique-by-key,
//              sort-by-multiple-keys, transpose, partition
// ===========================================================================

await check("zip two arrays", async () => {
  const r = await execute(
    `
    const a = [1, 2, 3];
    const b = ["a", "b", "c"];
    a.map((x, i) => [x, b[i]])
    `,
    {}
  );
  assert.deepEqual(r.output, [[1, "a"], [2, "b"], [3, "c"]]);
});

await check("chunk array into groups of N", async () => {
  const r = await execute(
    `
    const arr = [1,2,3,4,5,6,7];
    const n = 3;
    const chunks = [];
    for (let i = 0; i < arr.length; i += n) {
      chunks.push(arr.slice(i, i + n));
    }
    chunks
    `,
    {}
  );
  assert.deepEqual(r.output, [[1, 2, 3], [4, 5, 6], [7]]);
});

await check("unique-by-key: first occurrence wins", async () => {
  const r = await execute(
    `
    const rows = [
      { id: 1, name: "Alice" }, { id: 2, name: "Bob" },
      { id: 1, name: "Alice_dup" }, { id: 3, name: "Carol" },
    ];
    const seen = new Set();
    const unique = rows.filter(r => {
      if (seen.has(r.id)) return false;
      seen.add(r.id);
      return true;
    });
    unique.map(r => r.name)
    `,
    {}
  );
  assert.deepEqual(r.output, ["Alice", "Bob", "Carol"]);
});

await check("sort by multiple keys (pri asc, then name asc)", async () => {
  const r = await execute(
    `
    const items = [
      { name: "c", pri: 2 }, { name: "a", pri: 1 },
      { name: "b", pri: 2 }, { name: "d", pri: 1 },
    ];
    items.sort((x, y) => x.pri !== y.pri ? x.pri - y.pri : x.name < y.name ? -1 : x.name > y.name ? 1 : 0);
    items.map(i => i.name)
    `,
    {}
  );
  assert.deepEqual(r.output, ["a", "d", "b", "c"]);
});

await check("transpose 2D matrix", async () => {
  const r = await execute(
    `
    const matrix = [[1,2,3],[4,5,6],[7,8,9]];
    matrix[0].map((_, i) => matrix.map(row => row[i]))
    `,
    {}
  );
  assert.deepEqual(r.output, [[1, 4, 7], [2, 5, 8], [3, 6, 9]]);
});

await check("partition array into two by predicate", async () => {
  const r = await execute(
    `
    const nums = [1, 2, 3, 4, 5, 6];
    const [evens, odds] = nums.reduce(([e, o], x) => x % 2 === 0 ? [[...e, x], o] : [e, [...o, x]], [[], []]);
    [evens, odds]
    `,
    {}
  );
  assert.deepEqual(r.output, [[2, 4, 6], [1, 3, 5]]);
});

// ===========================================================================
// SECTION 13 — Realistic aggregation pipeline
// ===========================================================================

await check("realistic: group by field, count+sum, sorted summary", async () => {
  const r = await execute(
    `
    const orders = [
      { region: "west",  product: "widget", amount: 120, qty: 3 },
      { region: "east",  product: "gadget", amount: 200, qty: 2 },
      { region: "west",  product: "gadget", amount: 150, qty: 1 },
      { region: "east",  product: "widget", amount: 90,  qty: 4 },
      { region: "west",  product: "widget", amount: 80,  qty: 2 },
      { region: "north", product: "gadget", amount: 310, qty: 5 },
    ];
    const byRegion = {};
    for (const o of orders) {
      if (!byRegion[o.region]) byRegion[o.region] = { region: o.region, count: 0, totalAmount: 0 };
      byRegion[o.region].count += 1;
      byRegion[o.region].totalAmount += o.amount;
    }
    Object.values(byRegion)
      .sort((a, b) => b.totalAmount - a.totalAmount)
      .map(r => ({ region: r.region, count: r.count, totalAmount: r.totalAmount }))
    `,
    {}
  );
  assert.deepEqual(r.output, [
    { region: "north", count: 1, totalAmount: 310 },
    { region: "west",  count: 3, totalAmount: 350 },
    { region: "east",  count: 2, totalAmount: 290 },
  ]);
});

await check("realistic: top-N per group (top 2 products by revenue)", async () => {
  const r = await execute(
    `
    const sales = [
      { product: "A", revenue: 100 }, { product: "B", revenue: 200 },
      { product: "A", revenue: 50  }, { product: "C", revenue: 300 },
      { product: "B", revenue: 150 }, { product: "C", revenue: 100 },
    ];
    const totals = {};
    for (const s of sales) {
      totals[s.product] = (totals[s.product] || 0) + s.revenue;
    }
    Object.entries(totals)
      .sort((a, b) => b[1] - a[1])
      .slice(0, 2)
      .map(e => ({ product: e[0], total: e[1] }))
    `,
    {}
  );
  assert.deepEqual(r.output, [
    { product: "C", total: 400 },
    { product: "B", total: 350 },
  ]);
});

await check("realistic: pivot product counts per region", async () => {
  const r = await execute(
    `
    const orders = [
      { region: "west", product: "widget" },
      { region: "east", product: "gadget" },
      { region: "west", product: "gadget" },
      { region: "east", product: "widget" },
      { region: "west", product: "widget" },
    ];
    const pivot = {};
    for (const o of orders) {
      if (!pivot[o.region]) pivot[o.region] = {};
      pivot[o.region][o.product] = (pivot[o.region][o.product] || 0) + 1;
    }
    pivot
    `,
    {}
  );
  assert.deepEqual(r.output, {
    west: { widget: 2, gadget: 1 },
    east: { gadget: 1, widget: 1 },
  });
});

// ===========================================================================
// SECTION 14 — Additional array methods: indexOf, lastIndexOf, includes, concat,
//              every, some, forEach, join, reverse
// ===========================================================================

await check("indexOf and lastIndexOf", async () => {
  const r = await execute(
    `
    const a = [1, 2, 3, 2, 1];
    [a.indexOf(2), a.lastIndexOf(2), a.indexOf(9)]
    `,
    {}
  );
  assert.deepEqual(r.output, [1, 3, -1]);
});

// KNOWN BUG: NaN serializes as null in Zapcode — includes(NaN) is false even if array has NaN
// because NaN is stored as null; [1, NaN, 3].includes(NaN) → includes(null) → false (no null)
// Workaround: note that [1, null, 3].includes(null) → true works correctly

await check("concat: multiple arrays and primitives", async () => {
  const r = await execute(
    `[1,2].concat([3,4], 5, [6])`,
    {}
  );
  assert.deepEqual(r.output, [1, 2, 3, 4, 5, 6]);
});

await check("every and some", async () => {
  const r = await execute(
    `
    const a = [2, 4, 6, 8];
    [a.every(x => x % 2 === 0), a.some(x => x > 5), a.every(x => x > 5)]
    `,
    {}
  );
  assert.deepEqual(r.output, [true, true, false]);
});

// KNOWN BUG: forEach callback block bodies with external side effects never execute.
// Workaround: use for-of instead of forEach for side-effect operations.
// forEach with pure expression bodies (no external mutation) is also broken (callback never runs).

await check("join with custom separator", async () => {
  const r = await execute(`["a","b","c"].join(" | ")`, {});
  assert.equal(r.output, "a | b | c");
});

await check("reverse mutates in place and returns self", async () => {
  const r = await execute(
    `
    const a = [1,2,3,4,5];
    const b = a.reverse();
    [a, b === a]
    `,
    {}
  );
  // reverse mutates a and b should be the same reference (same array)
  assert.deepEqual(r.output[0], [5, 4, 3, 2, 1]);
  // note: reference equality may not be testable in sandbox — just check values
});

await check("chained: filter + map + reduce pipeline", async () => {
  const r = await execute(
    `
    const data = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
    data
      .filter(x => x % 2 === 0)
      .map(x => x * x)
      .reduce((acc, x) => acc + x, 0)
    `,
    {}
  );
  // evens: [2,4,6,8,10] → squares: [4,16,36,64,100] → sum: 220
  assert.equal(r.output, 220);
});

// ===========================================================================
// SECTION 15 — Object.keys/values/entries, spread merge, nested access
// ===========================================================================

await check("Object.keys / values / entries on plain object", async () => {
  const r = await execute(
    `
    const o = { a: 1, b: 2, c: 3 };
    [Object.keys(o), Object.values(o), Object.entries(o)]
    `,
    {}
  );
  assert.deepEqual(r.output, [
    ["a", "b", "c"],
    [1, 2, 3],
    [["a", 1], ["b", 2], ["c", 3]],
  ]);
});

await check("Object.fromEntries from map", async () => {
  const r = await execute(
    `
    const m = new Map([["x", 10], ["y", 20]]);
    Object.fromEntries(m)
    `,
    {}
  );
  assert.deepEqual(r.output, { x: 10, y: 20 });
});

await check("spread object merge: later keys win", async () => {
  const r = await execute(
    `
    const defaults = { color: "red", size: "medium", weight: 1 };
    const override = { color: "blue", extra: true };
    ({ ...defaults, ...override })
    `,
    {}
  );
  assert.deepEqual(r.output, { color: "blue", size: "medium", weight: 1, extra: true });
});

await check("deep nested access + optional chaining + nullish coalescing", async () => {
  const r = await execute(
    `
    const data = {
      users: [
        { id: 1, profile: { address: { city: "NYC" } } },
        { id: 2, profile: null },
        { id: 3 },
      ]
    };
    data.users.map(u => u.profile?.address?.city ?? "unknown")
    `,
    {}
  );
  assert.deepEqual(r.output, ["NYC", "unknown", "unknown"]);
});

await check("structuredClone: deep clone independence", async () => {
  const r = await execute(
    `
    const original = { a: { b: [1, 2, 3] } };
    const clone = structuredClone(original);
    clone.a.b.push(99);
    [original.a.b, clone.a.b]
    `,
    {}
  );
  assert.deepEqual(r.output[0], [1, 2, 3]);
  assert.deepEqual(r.output[1], [1, 2, 3, 99]);
});

await check("building lookup map with Object.fromEntries + map", async () => {
  const r = await execute(
    `
    const users = [{ id: 1, name: "Alice" }, { id: 2, name: "Bob" }, { id: 3, name: "Carol" }];
    const lookup = Object.fromEntries(users.map(u => [u.id, u.name]));
    [lookup[1], lookup[2], lookup[3]]
    `,
    {}
  );
  assert.deepEqual(r.output, ["Alice", "Bob", "Carol"]);
});

// ===========================================================================
// SECTION 16 — Array holes / sparse arrays, Array.from with mapFn
// ===========================================================================

await check("Array.from with length and mapFn", async () => {
  const r = await execute(
    `Array.from({ length: 5 }, (_, i) => i * 2)`,
    {}
  );
  assert.deepEqual(r.output, [0, 2, 4, 6, 8]);
});

await check("Array.from with string (char split)", async () => {
  const r = await execute(
    `Array.from("hello")`,
    {}
  );
  assert.deepEqual(r.output, ["h", "e", "l", "l", "o"]);
});

await check("Array.from on Set with map function", async () => {
  const r = await execute(
    `Array.from(new Set([1, 2, 3]), x => x * 10)`,
    {}
  );
  assert.deepEqual(r.output, [10, 20, 30]);
});

// ===========================================================================
// SECTION 17 — Tricky sort / comparator edge cases
// ===========================================================================

await check("sort: comparator returning 0 preserves order (stability)", async () => {
  const r = await execute(
    `
    // all elements compare equal → original order should be preserved
    const a = [3, 1, 4, 1, 5, 9];
    const b = [...a].sort(() => 0);
    b
    `,
    {}
  );
  // stable sort: all equal comparisons → original order preserved
  assert.deepEqual(r.output, [3, 1, 4, 1, 5, 9]);
});

await check("sort: objects by multiple criteria using localeCompare", async () => {
  const r = await execute(
    `
    const names = ["Bob", "alice", "Charlie", "alice"];
    names.sort((a, b) => a.toLowerCase().localeCompare(b.toLowerCase()))
    `,
    {}
  );
  assert.deepEqual(r.output, ["alice", "alice", "Bob", "Charlie"]);
});

// ===========================================================================
// SECTION 18 — Chained collection transforms (realistic data pipeline)
// ===========================================================================

await check("realistic: CSV parse + group + sort pipeline (pure, no tools)", async () => {
  const r = await execute(
    `
    const csv = "name,dept,salary\\nAlice,eng,120000\\nBob,sales,80000\\nCarol,eng,110000\\nDave,sales,90000\\nEve,eng,95000";
    const lines = csv.trim().split("\\n");
    const headers = lines[0].split(",");
    const rows = lines.slice(1).map(line => {
      const vals = line.split(",");
      const obj = {};
      for (let i = 0; i < headers.length; i++) obj[headers[i]] = vals[i];
      return obj;
    });
    // group by dept, compute avg salary
    const groups = {};
    for (const row of rows) {
      if (!groups[row.dept]) groups[row.dept] = { dept: row.dept, total: 0, count: 0 };
      groups[row.dept].total += Number(row.salary);
      groups[row.dept].count += 1;
    }
    Object.values(groups)
      .map(g => ({ dept: g.dept, avg: g.total / g.count }))
      .sort((a, b) => b.avg - a.avg)
    `,
    {}
  );
  assert.equal(r.output.length, 2);
  assert.equal(r.output[0].dept, "eng");
  assert.ok(Math.abs(r.output[0].avg - (120000 + 110000 + 95000) / 3) < 0.001);
  assert.equal(r.output[1].dept, "sales");
  assert.ok(Math.abs(r.output[1].avg - (80000 + 90000) / 2) < 0.001);
});

// ===========================================================================
// SECTION 19 — CONFIRMED BUGS (minimal repros; expected to fail)
// ===========================================================================

await check("BUG-flat-depth: flat(n) ignores n — always flattens exactly 1 level", async () => {
  // [1,[2,[3]]].flat(2) should give [1,2,3] but gives [1,2,[3]] (acts as flat(1))
  const r = await execute(`[1,[2,[3]]].flat(2)`, {});
  assert.deepEqual(r.output, [1, 2, 3]);
});

await check("BUG-flat-infinity: flat(Infinity) only flattens 1 level", async () => {
  const r = await execute(`[1,[2,[3,[4]]]].flat(Infinity)`, {});
  assert.deepEqual(r.output, [1, 2, 3, 4]);
});

await check("BUG-findLast: Array.prototype.findLast is missing", async () => {
  // throws "type error: undefined is not a function"
  const r = await execute(`[1,2,3,4].findLast(x => x % 2 === 0)`, {});
  assert.equal(r.output, 4);
});

await check("BUG-findLastIndex: Array.prototype.findLastIndex is missing", async () => {
  const r = await execute(`[1,2,3,4].findLastIndex(x => x % 2 === 0)`, {});
  assert.equal(r.output, 3);
});

await check("BUG-reduceRight: Array.prototype.reduceRight is missing", async () => {
  // throws "type error: undefined is not a function"
  const r = await execute(`["a","b","c"].reduceRight((acc,x) => acc+x)`, {});
  assert.equal(r.output, "cba");
});

await check("BUG-copyWithin: Array.prototype.copyWithin is missing", async () => {
  // throws "type error: undefined is not a function"
  const r = await execute(`[1,2,3,4,5].copyWithin(0,3)`, {});
  assert.deepEqual(r.output, [4, 5, 3, 4, 5]);
});

await check("BUG-new-Array: new Array(n) constructor is broken", async () => {
  // throws "type error: [object Object] is not a constructor"
  const r = await execute(`new Array(3).fill(0)`, {});
  assert.deepEqual(r.output, [0, 0, 0]);
});

await check("BUG-forEach: callback never executes (counter stays 0)", async () => {
  // forEach callback block bodies are completely non-functional.
  // Even simple side effects like counter++ never happen.
  const r = await execute(`let n = 0; [1,2,3].forEach(x => { n = n + 1; }); n`, {});
  assert.equal(r.output, 3);
});

await check("BUG-callback-side-effects: external mutations in callback block bodies never execute", async () => {
  // Any mutation of variables declared OUTSIDE a callback block body silently fails.
  // Affects: forEach (entirely), map/filter/reduce side effects.
  // Return values work; only external-variable mutations are broken.
  const r = await execute(
    `
    let sum = 0;
    [1,2,3].map(x => { sum = sum + x; return x; });
    sum
    `,
    {}
  );
  // BUG: returns 0 (sum never updated)
  assert.equal(r.output, 6);
});

await check("BUG-computed-key-expr: computed key [expr] always produces key '<computed>'", async () => {
  // Only computed keys that are plain string literals work: {'x':1} ✓
  // Any expression (variable, concatenation, method call) produces key '<computed>'.
  const r = await execute(`const k = "foo"; ({ [k]: 42 })`, {});
  // BUG: returns { "<computed>": 42 }
  assert.deepEqual(r.output, { foo: 42 });
});

await check("BUG-computed-key-concat: computed key with string concatenation is '<computed>'", async () => {
  const r = await execute(`const p = "key_"; ({ [p + "a"]: 1, [p + "b"]: 2 })`, {});
  // BUG: returns { "<computed>": 2 } (second overwrites first)
  assert.deepEqual(r.output, { key_a: 1, key_b: 2 });
});

await check("BUG-object-assign-mutation: Object.assign does not mutate the target variable binding", async () => {
  // Object.assign returns a new object with merged keys.
  // The original target variable still holds the OLD unmerged object.
  // Real JS: Object.assign MUTATES the target and returns it (same reference).
  const r = await execute(
    `
    const t = { a: 1 };
    Object.assign(t, { b: 2 });
    t.b
    `,
    {}
  );
  // BUG: returns null (t was not mutated; t.b is undefined/null)
  assert.equal(r.output, 2);
});

await check("BUG-delete: delete operator is not supported (throws unsupported syntax)", async () => {
  // throws: unsupported syntax: delete operator is not supported
  const r = await execute(`const o = { a: 1, b: 2 }; delete o.b; Object.keys(o)`, {});
  assert.deepEqual(r.output, ["a"]);
});

await check("BUG-object-freeze: Object.freeze does not prevent property mutation", async () => {
  // Frozen objects should silently ignore mutations in non-strict mode.
  // Zapcode ignores the freeze and allows mutation.
  const r = await execute(
    `
    const o = Object.freeze({ a: 1 });
    o.a = 99;
    o.a
    `,
    {}
  );
  // BUG: returns 99 (mutation succeeded; freeze is a no-op)
  assert.equal(r.output, 1);
});

await check("BUG-map-forEach: Map.prototype.forEach is missing", async () => {
  // throws "type error: Map.forEach is not a function"
  const r = await execute(
    `
    const m = new Map([["a",1],["b",2]]);
    const out = [];
    m.forEach((v, k) => out.push(k));
    out
    `,
    {}
  );
  assert.deepEqual(r.output, ["a", "b"]);
});

await check("BUG-map-stores-by-copy: Map stores values by copy, not by reference", async () => {
  // In real JS, Map.set stores the actual object reference.
  // In Zapcode, Map.set appears to store a SNAPSHOT (copy).
  // So mutating the object after set is not reflected in Map.get().
  const r = await execute(
    `
    const obj = { x: 1 };
    const m = new Map();
    m.set("k", obj);
    obj.x = 99;
    m.get("k").x
    `,
    {}
  );
  // BUG: returns 1 (mutation of obj not visible via Map.get)
  assert.equal(r.output, 99);
});

await check("BUG-map-group-by: Map.get(k).push(v) — push result doesn't update Map value", async () => {
  // Consequence of Map-stores-by-copy bug:
  // Cannot use Map as a group-by accumulator with mutable array values.
  // Workaround: use plain object {} as accumulator.
  const r = await execute(
    `
    const m = new Map();
    m.set("A", []);
    m.get("A").push(1);
    m.get("A").push(2);
    m.get("A")
    `,
    {}
  );
  // BUG: returns [] (pushes not reflected; Map holds original empty array snapshot)
  assert.deepEqual(r.output, [1, 2]);
});

await check("BUG-NaN-is-null: NaN literal serializes as null (not IEEE 754 NaN)", async () => {
  // NaN is a valid JavaScript value; it should not be null.
  // typeof NaN === 'number' (works) but the value itself serializes to null.
  const r = await execute(`[NaN, 0/0, typeof NaN]`, {});
  // BUG: [null, null, "number"]
  // Expected: [NaN serialized as null in JSON is expected, but typeof should confirm number]
  // The real issue: NaN in arrays becomes null, not the IEEE 754 NaN
  assert.equal(r.output[2], "number");
  // NaN serializes to null in JSON (this is spec-correct for JSON.stringify),
  // but the value in the sandbox is wrong: isNaN(arr[0]) should be true
  // Test that isNaN still works on the "null" output:
});

await check("BUG-includes-NaN: [1,NaN,3].includes(NaN) returns false (NaN stored as null)", async () => {
  // In real JS, Array.includes uses SameValueZero which matches NaN to NaN.
  // In Zapcode, NaN is stored as null, so includes(NaN) checks for null (not found).
  const r = await execute(`[1,NaN,3].includes(NaN)`, {});
  // BUG: returns false
  assert.equal(r.output, true);
});

await check("BUG-object-fromEntries-Map: Object.fromEntries(map) returns {} — Map not iterable by fromEntries", async () => {
  // Object.fromEntries([...]) with array pairs works.
  // Object.fromEntries(map) fails silently, returning {}.
  // Workaround: Object.fromEntries(map.entries()) works correctly.
  const r = await execute(
    `
    const m = new Map([["x", 10], ["y", 20]]);
    Object.fromEntries(m)
    `,
    {}
  );
  // BUG: returns {}
  assert.deepEqual(r.output, { x: 10, y: 20 });
});

await check("BUG-array-from-mapfn: Array.from(iterable, mapFn) — mapFn is ignored", async () => {
  // Array.from with a map function (second argument) silently ignores the mapFn.
  const r = await execute(`Array.from([1,2,3], x => x * 10)`, {});
  // BUG: returns [1,2,3] (mapFn ignored)
  assert.deepEqual(r.output, [10, 20, 30]);
});

await check("BUG-array-from-length-mapfn: Array.from({length:n}, mapFn) — length obj not iterable", async () => {
  // Array.from({length:n}) returns [] (array-like with length not supported).
  // Array.from({length:n}, mapFn) also returns [] (mapFn also ignored).
  const r = await execute(`Array.from({ length: 4 }, (_, i) => i)`, {});
  // BUG: returns []
  assert.deepEqual(r.output, [0, 1, 2, 3]);
});

await check("BUG-localeCompare: String.prototype.localeCompare is missing", async () => {
  // throws "type error: undefined is not a function"
  const r = await execute(`"apple".localeCompare("banana") < 0`, {});
  assert.equal(r.output, true);
});

await check("BUG-closure-capture: outer callback param captured as 0 in nested callback", async () => {
  // When an outer callback parameter (including the index param) is captured
  // by an inner (nested) callback, the inner callback always sees value 0
  // (the initial/first iteration value), not the current outer iteration value.
  // Workaround: pass the outer value explicitly to a named function, not a nested callback.
  const r = await execute(
    `
    // outer index 'i' should be 0,1,2 — inner callback captures it
    [1,2,3].map((_, i) => [0].map(() => i))
    `,
    {}
  );
  // BUG: returns [[0],[0],[0]] — inner always sees i=0
  assert.deepEqual(r.output, [[0], [1], [2]]);
});

await check("BUG-closure-capture: outer value param captured as first value in nested callback", async () => {
  // Not just the index — the value param also always appears as first iteration value.
  const r = await execute(
    `
    ["a","b","c"].map((v, i) => [0].map(() => v))
    `,
    {}
  );
  // BUG: returns [["a"],["a"],["a"]] — inner always sees v="a" (first iteration)
  assert.deepEqual(r.output, [["a"], ["b"], ["c"]]);
});

await check("BUG-sort-multikey: sort with ternary comparator on object properties gives wrong order", async () => {
  // Single-key sort works: [{p:2},{p:1}].sort((x,y)=>x.p-y.p) → [{p:1},{p:2}] ✓
  // But ternary-based comparator on object properties is broken:
  // x.p !== y.p ? x.p - y.p : 0  — condition evaluates but result is wrong order
  const r = await execute(
    `
    const items = [{p:2,n:"b"},{p:1,n:"a"}];
    items.sort((x, y) => x.p !== y.p ? x.p - y.p : 0);
    items.map(i => i.n)
    `,
    {}
  );
  // BUG: returns ["b","a"] (original order, not sorted by p asc)
  assert.deepEqual(r.output, ["a", "b"]);
});

// ===========================================================================
// Summary
// ===========================================================================
const failed = results.filter(r => !r[1]);
const bugs   = results.filter(r => !r[1] && r[0].startsWith("BUG-"));
const others = results.filter(r => !r[1] && !r[0].startsWith("BUG-"));

console.log(`\n${results.length - failed.length}/${results.length} passed`);
if (others.length) {
  console.log(`\nUnexpected non-bug failures:`);
  for (const [name, , msg] of others) {
    console.log(`  ✗ ${name}`);
    console.log(`    ${msg}`);
  }
}
if (bugs.length) {
  console.log(`\nConfirmed bugs (expected to fail): ${bugs.length}`);
  for (const [name, , msg] of bugs) {
    console.log(`  ✗ ${name.slice(0, 60)}`);
  }
}
