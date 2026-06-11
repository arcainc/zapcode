/**
 * Differential parity harness: every corpus snippet runs through BOTH
 * zapcode and real Node (same process), and the results must agree.
 *
 * Contract: each snippet is the BODY of an async function ending in an
 * explicit `return`. zapcode runs `async function main() { <body> } main();`
 * through the binding; Node runs `new AsyncFunction(body)()` natively.
 * Results are normalized for the documented host-marshalling rules
 * (`undefined`/non-finite → null) and deep-compared — so parity is checked
 * mechanically, not test-by-test.
 *
 * Documented divergences live in `pinned` with the zapcode value AND the
 * reason; a pin that starts agreeing with Node fails loudly so it gets
 * promoted into the corpus.
 *
 * Run: npm run build && node tests/differential.mjs
 */
import assert from "node:assert/strict";
import { execute } from "../dist/index.js";

const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;

/** Normalize per the host-boundary marshalling rules (cluster L). */
function normalize(v) {
  if (v === undefined) return null;
  if (typeof v === "number" && !Number.isFinite(v)) return null;
  if (Array.isArray(v)) return v.map(normalize);
  if (v && typeof v === "object") {
    const out = {};
    for (const [k, val] of Object.entries(v)) {
      if (val === undefined) continue; // dropped at the boundary
      out[k] = normalize(val);
    }
    return out;
  }
  return v;
}

async function runZapcode(body) {
  const r = await execute(`async function main() { ${body} } main();`, {});
  return normalize(r.output);
}

async function runNode(body) {
  return normalize(await new AsyncFunction(body)());
}

// ════════════════════════════════════════════════════════════════════════════
//  Corpus — every snippet must agree with Node byte-for-byte (normalized)
// ════════════════════════════════════════════════════════════════════════════

const corpus = [
  // ── operators & coercion ──────────────────────────────────────────────
  `return 1 + '2' + 3;`,
  `return '5' * '4' + 2 ** 3;`,
  `return [1 == '1', 1 === '1', null == undefined, null === undefined].join(',');`,
  `return [0 || 'a', '' ?? 'b', null ?? 'c', 0 ?? 'd'].join('|');`,
  `return [NaN === NaN, Object.is(NaN, NaN), Object.is(-0, 0)].join(',');`,
  `return [7 % 3, -7 % 3, 7.5 % 2].join(',');`,
  `return [5 & 3, 5 | 3, 5 ^ 3, ~5, 1 << 4, -16 >> 2, -16 >>> 28].join(',');`,
  `return ['b' > 'a', '10' < '9', 10 < 9, '10' < 9].join(',');`,
  `return [+true, +null, +undefined ? 'n' : 'isNaN', +'', +'3px' ? 'n' : 'isNaN'].join(',');`,
  `return [typeof 1, typeof 'x', typeof true, typeof undefined, typeof null, typeof {}, typeof [], typeof (() => 1)].join(',');`,
  `return ['' + [1,2], '' + {}, '' + [[]], '' + [null], '' + [undefined]].join('|');`,
  `let x = 5; x += 2; x *= 3; x -= 1; x /= 4; return x;`,
  `return [1 < 2 === true, 'abc' + false, null + 1, true + true].join(',');`,
  `return [(() => {})() === undefined, void 0 === undefined].join(',');`,

  // ── strings ───────────────────────────────────────────────────────────
  `return 'Hello World'.toLowerCase().split(' ').map(w => w[0].toUpperCase() + w.slice(1)).join('-');`,
  `return ['abc'.padStart(6, '*'), 'abc'.padEnd(5, '!'), ' x '.trim(), 'x'.repeat(3)].join('|');`,
  `return ['abcdef'.slice(1, -1), 'abcdef'.substring(4, 2), 'abcdef'.at(-2)].join(',');`,
  `return ['a,b,,c'.split(','), 'abc'.split('')].flat().join('|');`,
  `return ['xyz'.includes('y'), 'xyz'.startsWith('x'), 'xyz'.endsWith('y'), 'xyzxyz'.indexOf('z', 3), 'xyzxyz'.lastIndexOf('x')].join(',');`,
  `return 'a-b-c'.replace('-', '+') + '|' + 'a-b-c'.replaceAll('-', '+');`,
  `return ['Ab'.localeCompare('ab') === 0, 'abc'.charCodeAt(1), String.fromCharCode(72, 105)].join(',');`,
  `return [\`\${1 + 1}x\${'y'.toUpperCase()}\`, \`a\\nb\`.split('\\n').length].join('|');`,
  `return JSON.stringify('he said "hi"\\n');`,

  // ── numbers & Math ────────────────────────────────────────────────────
  `return [Math.max(1, 5, 3), Math.min(...[4, 2, 8]), Math.abs(-7), Math.sign(-3)].join(',');`,
  `return [Math.floor(2.7), Math.ceil(2.1), Math.round(2.5), Math.round(3.5), Math.trunc(-2.7)].join(',');`,
  `return [(1234.5678).toFixed(2), (0.000001234).toExponential(2), (255).toString(16), (8).toString(2)].join('|');`,
  `return [parseInt('42px'), parseFloat('3.14abc'), Number('42'), Number(''), Number('  7  ')].join(',');`,
  `return [Number.isInteger(5.0), Number.isInteger(5.5), Number.isSafeInteger(2 ** 53), Number.MAX_SAFE_INTEGER].join(',');`,
  `return [0.1 + 0.2 === 0.3, Math.abs(0.1 + 0.2 - 0.3) < Number.EPSILON].join(',');`,
  `return [10n + 32n, 2n ** 10n, 7n / 2n, (-7n) / 2n, 5n % 3n].map(String).join(',');`,
  `return [typeof 10n, 10n === 10n, 10n == 10].join(',');`,

  // ── arrays ────────────────────────────────────────────────────────────
  `return [1,2,3,4,5].filter(x => x % 2).map(x => x * 10).reduce((a, b) => a + b, 0);`,
  `const a = [3,1,2]; const b = a.toSorted(); return a.join('') + '|' + b.join('') + '|' + a.toReversed().join('') + '|' + a.with(1, 9).join('');`,
  `return [[1,[2,[3,[4]]]].flat(2).join(','), [1,2,3].flatMap(x => [x, x * 2]).join(',')].join('|');`,
  `return [[1,2,3].includes(2), [1,2,3].indexOf(9), [5,12,8].find(x => x > 6), [5,12,8].findIndex(x => x > 6), [5,12,8].findLast(x => x > 6)].join(',');`,
  `return [Array.from({length: 4}, (_, i) => i * i).join(','), Array.of(7, 8).join(','), [...'abc'].join('-')].join('|');`,
  `const a = [1,2,3,4,5]; return [a.slice(1, 3).join(''), a.slice(-2).join(''), a.join('')].join('|');`,
  `const a = [1,2,3]; a.splice(1, 1, 'x', 'y'); return a.join(',');`,
  `return [[2,1,10].sort().join(','), [2,1,10].sort((x, y) => x - y).join(','), ['b','a','C'].sort().join(',')].join('|');`,
  `return [[1,2,3].every(x => x > 0), [1,2,3].some(x => x > 2), [].every(x => false)].join(',');`,
  `return [[1,2,3].at(-1), [1,2,3].at(0), [1,2,3].at(5) === undefined ? 'u' : 'v'].join(',');`,
  `const [first, ...rest] = [1, 2, 3, 4]; return first + '|' + rest.join(',');`,
  `return [...[1,2], ...[3], 4].join(',') + '|' + Math.max(...[5, 9, 2]);`,
  `return [1,2,3].reduceRight((acc, x) => acc + x, '');`,
  `return Array.isArray([]) + ',' + Array.isArray('ab') + ',' + [3].concat([4, [5]]).length;`,
  `const a = new Array(3).fill('x'); return a.join(',') + '|' + [0,0,0,0].fill(7, 1, 3).join(',');`,
  `return [1,2,3,4].entries().next().value.join(':') + '|' + [...[9, 8].keys()].join(',');`,

  // ── objects ───────────────────────────────────────────────────────────
  `const o = {a: 1, b: 2, c: 3}; return Object.keys(o).join(',') + '|' + Object.values(o).join(',') + '|' + Object.entries(o).map(([k, v]) => k + v).join(',');`,
  `const o = {x: 1}; const m = {...o, y: 2}; return JSON.stringify(m) + '|' + JSON.stringify(Object.assign({}, m, {x: 9}));`,
  `const {a, b = 10, ...rest} = {a: 1, c: 3, d: 4}; return a + ',' + b + ',' + JSON.stringify(rest);`,
  `const o = Object.fromEntries([['k1', 1], ['k2', 2]]); return JSON.stringify(o);`,
  `const o = {a: 1}; return ['a' in o, 'b' in o, o.hasOwnProperty('a'), delete o.a, 'a' in o].join(',');`,
  `const key = 'dyn'; const o = {[key + '1']: 'v'}; return JSON.stringify(o);`,
  `const o = {n: 1, get double() { return this.n * 2; }, set double(v) { this.n = v / 2; }}; o.double = 10; return o.n + ',' + o.double;`,
  `const o = Object.freeze({a: 1}); try { o.a = 2; } catch {} return o.a + ',' + Object.isFrozen(o);`,
  `const a = {v: 1}; const b = a; b.v = 2; return a.v;`,
  `const rows = [{id: 1, tags: ['x']}, {id: 2, tags: []}]; rows[0].tags.push('y'); return JSON.stringify(rows);`,
  `return [({}).constructor === Object, [].constructor === Array, (5).constructor === Number].join(',');`,
  `const o = {a: {b: {c: 42}}}; return [o?.a?.b?.c, o?.x?.y, o.a?.['b']?.c, o.missing?.()].join(',');`,

  // ── classes ───────────────────────────────────────────────────────────
  `class P { constructor(n) { this.n = n; } greet() { return 'p:' + this.n; } } class C extends P { greet() { return super.greet() + '/c'; } } return new C(5).greet();`,
  `class A { static count = 10; static bump() { return ++A.count; } #secret = 7; reveal() { return this.#secret; } } return A.bump() + ',' + new A().reveal();`,
  `class T { items = []; add(x) { this.items.push(x); return this; } } return new T().add(1).add(2).items.join(',');`,
  `class E extends Error { constructor(m) { super(m); this.name = 'E'; } } try { throw new E('boom'); } catch (e) { return [e instanceof E ? 'E' : '-', e instanceof Error, e.message, e.name].join(','); }`,
  `class V { constructor(x) { this.x = x; } valueOf() { return this.x; } } return new V(20) + 5;`,
  `class S { toString() { return 'custom'; } } return \`\${new S()}\` + '|' + ('' + new S());`,
  `return [new Map().constructor === Map, new Set([1,1,2]).size].join(',');`,

  // ── Map / Set ─────────────────────────────────────────────────────────
  `const m = new Map([['a', 1]]); m.set('b', 2).set('a', 9); return [m.get('a'), m.size, m.has('b'), m.delete('b'), m.size].join(',');`,
  `const s = new Set([1, 2, 2, 3]); s.add(2).add(4); return [...s].join(',') + '|' + s.has(3);`,
  `const m = new Map([['x', 1], ['y', 2]]); return [...m.keys()].join(',') + '|' + [...m.values()].join(',') + '|' + [...m.entries()].map(e => e.join(':')).join(',');`,
  `const k = {id: 1}; const m = new Map([[k, 'obj']]); return m.get(k) + ',' + (m.get({id: 1}) === undefined ? 'miss' : 'hit');`,

  // ── control flow & errors ─────────────────────────────────────────────
  `let s = ''; for (let i = 0; i < 5; i++) { if (i === 2) continue; if (i === 4) break; s += i; } return s;`,
  `let s = ''; const o = {a: 1, b: 2}; for (const k in o) s += k; for (const v of [3, 4]) s += v; return s;`,
  `let i = 0; let s = ''; while (i < 3) s += i++; do { s += 'd'; } while (false); return s;`,
  `const f = n => { switch (n) { case 1: return 'one'; case 2: case 3: return 'few'; default: return 'many'; } }; return [f(1), f(2), f(3), f(9)].join(',');`,
  `try { try { throw new TypeError('inner'); } finally { } } catch (e) { return e.name + ':' + e.message; }`,
  `const f = () => { try { return 'try'; } finally { } }; const g = () => { try { return 'try'; } finally { return 'finally'; } }; return f() + ',' + g();`,
  `let log = []; try { throw 'plain'; } catch (e) { log.push(typeof e, e); } return log.join(',');`,
  `outer: for (const i of [0, 1]) { for (const j of [0, 1]) { if (j === 1) continue outer; } } return 'labels-ok';`,
  `try { null.x; } catch (e) { return e instanceof TypeError; }`,
  `try { JSON.parse('{bad'); } catch (e) { return e instanceof Error; }`,

  // ── functions & closures ──────────────────────────────────────────────
  `function counter() { let n = 0; return () => ++n; } const c = counter(); c(); c(); return c();`,
  `const make = (mult) => (x) => x * mult; return [make(2)(5), make(3)(5)].join(',');`,
  `function f(a, b = a * 2, ...rest) { return a + ',' + b + ',' + rest.join('+'); } return f(1) + '|' + f(1, 9, 3, 4);`,
  `const fns = []; for (let i = 0; i < 3; i++) fns.push(() => i); return fns.map(f => f()).join(',');`,
  `var fns2 = []; for (var i = 0; i < 3; i++) fns2.push(() => i); return fns2.map(f => f()).join(',');`,
  `function fact(n) { return n <= 1 ? 1 : n * fact(n - 1); } return fact(6);`,
  `const compose = (...fs) => x => fs.reduceRight((v, f) => f(v), x); return compose(x => x + 1, x => x * 2)(5);`,
  `function args() { return arguments.length + ':' + arguments[1]; } return args('a', 'b', 'c');`,
  `return hoisted(); function hoisted() { return 'hoisted-ok'; }`,
  `const obj = { n: 7, read() { return this.n; }, arrow: function() { return (() => this.n)(); } }; return obj.read() + ',' + obj.arrow();`,

  // ── destructuring in params / loops ───────────────────────────────────
  `const pairs = [['a', 1], ['b', 2]]; return pairs.map(([k, v]) => k + v).join(',');`,
  `const rows = [{id: 1, name: 'x'}, {id: 2, name: 'y'}]; let s = ''; for (const {id, name} of rows) s += id + name; return s;`,
  `const f = ({a, b: {c} = {c: 9}}) => a + c; return f({a: 1, b: {c: 2}}) + ',' + f({a: 1});`,

  // ── JSON ──────────────────────────────────────────────────────────────
  `return JSON.stringify({b: 2, a: [1, null, 'x'], c: {d: true}});`,
  `return JSON.stringify({a: 1, skip: undefined, f: () => 1});`,
  `const o = JSON.parse('{"a":[1,2],"b":{"c":null}}'); return o.a[1] + ',' + (o.b.c === null);`,
  `return JSON.stringify([1, 'a'], null, 1).split('\\n').length;`,
  `return JSON.stringify({t: new Date(0).toJSON ? 'has-toJSON' : 'no'});`,
  `return JSON.parse('[1,2,3]', (k, v) => typeof v === 'number' ? v * 10 : v).join(',');`,
  `return JSON.stringify({x: 5, y: 6}, ['x']);`,

  // ── async / promises ──────────────────────────────────────────────────
  `const log = []; const p = Promise.resolve('A').then(v => { log.push(v); }); log.push('B'); await p; return log.join(',');`,
  `async function f() { return 5; } return await f().then(x => x * 2);`,
  `async function f() { throw new Error('x'); } return await f().catch(e => 'caught:' + e.message);`,
  `const log = []; async function a() { log.push('a1'); await null; log.push('a2'); } const pa = a(); log.push('sync'); await pa; return log.join(',');`,
  `return await Promise.all([Promise.resolve(1).then(x => x + 1), 5, Promise.resolve(3)]).then(arr => arr.join(','));`,
  `return await Promise.race([Promise.resolve(1).then(x => x).then(() => 'slow'), Promise.resolve(2).then(() => 'fast')]);`,
  `return await Promise.any([Promise.reject('r'), Promise.resolve('win')]);`,
  `const r = await Promise.allSettled([Promise.resolve(1), Promise.reject('bad')]); return r.map(x => x.status).join(',');`,
  `let res; const gate = new Promise(r => { res = r; }); const chain = gate.then(v => v + '!'); res('open'); return await chain;`,
  `return await new Promise((_, reject) => reject('r')).catch(e => 'c:' + e);`,
  `return await new Promise(r => r(Promise.resolve(9)));`,
  `try { await Promise.reject(new TypeError('t')); } catch (e) { return e.name + ':' + e.message; }`,
  `const out = []; await Promise.resolve(7).finally(() => out.push('fin')).then(v => out.push(v)); return out.join(',');`,
  `const p = Promise.resolve(1); return (p.then() === p) + ',' + await p.then();`,
  `try { await Promise.any([Promise.reject('a'), Promise.reject('b')]); } catch (e) { return [e instanceof AggregateError, e.errors.join(',')].join('|'); }`,

  // ── generators ────────────────────────────────────────────────────────
  `function* g(a) { const b = yield a + 1; yield b * 2; return 'end'; } const it = g(10); return [it.next().value, it.next(5).value, it.next().done].join(',');`,
  `function* nat() { let i = 0; while (true) yield i++; } const out = []; for (const x of nat()) { if (x >= 3) break; out.push(x); } return out.join(',');`,
  `function* inner() { yield 1; yield 2; } function* outer() { yield 0; yield* inner(); yield 3; } return [...outer()].join(',');`,
  `function* g() { try { yield 1; null.x; } catch (e) { yield 'caught'; } } const out = []; for (const x of g()) out.push(x); return out.join(',');`,
  `async function* g() { yield 1; return 'end'; } const it = g(); const r1 = await it.next(); const r2 = await it.next(); return r1.value + ',' + r2.value + ',' + r2.done;`,
  `async function* g() { const v = await Promise.resolve(1).then(x => x + 1); yield v; yield v + 10; } const out = []; for await (const x of g()) out.push(x); return out.join(',');`,
  `function* g() { yield 1; yield 2; } const [a, b] = g(); return a + ',' + b;`,

  // ── dates (deterministic operations only) ─────────────────────────────
  `const d = new Date(Date.UTC(2024, 0, 15, 12, 30)); return [d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate(), d.getUTCHours()].join(',');`,
  `return new Date(0).toISOString() + '|' + new Date('2024-06-15T00:00:00Z').getTime();`,
  `return [new Date('invalid').getTime() ? 'n' : 'isNaN', isNaN(new Date('nope'))].join(',');`,
  `const a = new Date(Date.UTC(2024, 0, 1)); const b = new Date(Date.UTC(2024, 0, 2)); return (b - a) / 3600000;`,

  // ── regex (supported subset) ──────────────────────────────────────────
  `return 'a1b22c333'.replace(/\\d+/g, n => '<' + n + '>');`,
  `return ['x-1', 'y-2'].map(s => s.match(/([a-z])-(\\d)/)).map(m => m[1] + m[2]).join(',');`,
  `return /^[\\w.]+@[\\w.]+$/.test('a.b@c.io') + ',' + /^\\d+$/.test('12a');`,
  `return 'one  two\\tthree'.split(/\\s+/).join('|');`,
];

// ════════════════════════════════════════════════════════════════════════════
//  Pinned divergences — assert zapcode's ACTUAL value, with the reason.
//  If zapcode starts agreeing with Node, the pin fails so it gets promoted.
// ════════════════════════════════════════════════════════════════════════════

const pinned = [
  {
    reason:
      "Promise-executor capabilities are marker objects (a Value-enum variant would ripple through every binding crate)",
    body: `return await new Promise(r => { const t = typeof r; r(t); });`,
    zapcode: "object", // Node: "function"
  },
  {
    reason:
      "Symbol.toPrimitive is not dispatched (Symbol support is a stub; see STRESS-PASS-BUGS.md ToPrimitive notes)",
    body: `const o = { [Symbol.toPrimitive]() { return 42; }, valueOf() { return 7; } }; return o + 1;`,
    zapcode: 8, // Node: 43
  },
];

// ════════════════════════════════════════════════════════════════════════════

let passed = 0;
let failed = 0;
const failures = [];

for (const body of corpus) {
  let nodeResult, nodeErr, zapResult, zapErr;
  try {
    nodeResult = await runNode(body);
  } catch (e) {
    nodeErr = String(e);
  }
  try {
    zapResult = await runZapcode(body);
  } catch (e) {
    zapErr = String(e);
  }
  if (nodeErr !== undefined || zapErr !== undefined) {
    // Both sides must agree on *whether* it fails; the message text may
    // differ between engines.
    if ((nodeErr === undefined) === (zapErr === undefined)) {
      passed++;
    } else {
      failed++;
      failures.push({ body, node: nodeErr ?? nodeResult, zapcode: zapErr ?? zapResult });
    }
    continue;
  }
  try {
    assert.deepStrictEqual(zapResult, nodeResult);
    passed++;
  } catch {
    failed++;
    failures.push({ body, node: nodeResult, zapcode: zapResult });
  }
}

for (const pin of pinned) {
  const zap = await runZapcode(pin.body);
  const node = await runNode(pin.body).catch((e) => `ERR:${e}`);
  assert.deepStrictEqual(
    zap,
    pin.zapcode,
    `pinned divergence changed (${pin.reason}):\n${pin.body}\n zapcode now: ${JSON.stringify(zap)}`
  );
  assert.notDeepStrictEqual(
    zap,
    node,
    `pin agrees with Node now — promote to the corpus (${pin.reason}):\n${pin.body}`
  );
  passed++;
}

if (failures.length > 0) {
  console.error(`\n${failures.length} DIVERGENCES:`);
  for (const f of failures) {
    console.error(`\n  snippet: ${f.body}`);
    console.error(`     node: ${JSON.stringify(f.node)}`);
    console.error(`  zapcode: ${JSON.stringify(f.zapcode)}`);
  }
  process.exit(1);
}

console.log(`differential parity: ${passed} snippets agree with Node (${pinned.length} documented pins held)`);
