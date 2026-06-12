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
import { runNode, runZapcode } from "./diff-harness-lib.mjs";

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

  // ── combinators over internal chains (.then with empty queue) ─────────
  // PR-review round: lowered batches (combine reactions) replace the legacy
  // pass-through that dropped handlers when the microtask queue was empty.
  `let resolve; const gate = new Promise(r => { resolve = r; }); async function worker() { await gate; return 1; } const chained = Promise.all([worker()]).then(v => v[0] + 10); resolve(5); return await chained;`,
  `let reject; const gate = new Promise((r, j) => { reject = j; }); async function w() { await gate; } const c = Promise.all([w()]).catch(e => 'caught:' + e); reject('bad'); return await c;`,
  `let r1, r2; const g1 = new Promise(r => r1 = r), g2 = new Promise(r => r2 = r); async function a() { return 'A:' + await g1; } async function b() { return 'B:' + await g2; } const c = Promise.race([a(), b()]).then(v => 'won:' + v); r2('two'); r1('one'); return await c;`,
  `let j1, j2; const g1 = new Promise((r, j) => j1 = j), g2 = new Promise((r, j) => j2 = j); async function a() { await g1; } async function b() { await g2; } const c = Promise.any([a(), b()]).catch(e => e.name + ':' + e.errors.join(',')); j1('e1'); j2('e2'); return await c;`,
  `let r1, j2; const g1 = new Promise(r => r1 = r), g2 = new Promise((r, j) => j2 = j); async function a() { return 'va' + await g1; } async function b() { await g2; return 'never'; } const c = Promise.allSettled([a(), b()]).then(rs => rs.map(r => r.status + ':' + (r.value ?? r.reason)).join('|')); r1('1'); j2('e'); return await c;`,
  `let resolve; const gate = new Promise(r => { resolve = r; }); async function w() { await gate; return 7; } const log = []; const c = Promise.all([w()]).finally(() => log.push('fin')).then(v => v[0] + ':' + log.join(',')); resolve(0); return await c;`,
  `let resolve; const gate = new Promise(r => { resolve = r; }); async function w() { await gate; return 3; } const batch = Promise.all([w()]); const c = batch.then(v => v[0] * 2); resolve(0); const direct = await batch; return (await c) + ':' + direct.join(',');`,

  `return await new Promise(r => { const t = typeof r; r(t); });`,
  `let t; new Promise((resolve, reject) => { t = typeof resolve + ',' + typeof reject; resolve(1); }); return t;`,
  `try { await Promise.any([]); return 'settled'; } catch (e) { return e.name + ':' + e.errors.length; }`,
  `const r = await Promise.allSettled([]); return Array.isArray(r) + ':' + r.length;`,

  // ── destructure-default repair (single evaluation, all call shapes) ───
  `function f({a: {b} = {b: 9}} = {}) { return b; } return [f(), f({}), f({a: {b: 1}})].join(',');`,
  `const calls = []; function f({a: {b = (calls.push('hit'), undefined)} = {}} = {}) { return b; } f(); return calls.length;`,
  `const calls = []; function f({a: {b = (calls.push('hit'), undefined)} = {}}) { return b; } f({a: {}}); return calls.length;`,
  `function f({a: {b} = {b: 1}, c = 2, d: {e = 3} = {}}) { return [b, c, e].join(','); } return f({});`,
];


// ════════════════════════════════════════════════════════════════════════════
//  Realistic programs — the shapes agent code actually takes: transform tool
//  results, aggregate, format reports, orchestrate async steps. Not edge
//  cases; whole little programs.
// ════════════════════════════════════════════════════════════════════════════

const realistic = [
  // filter → map → aggregate over records
  `const orders = [
     { id: 1, status: "shipped", total: 41.5, items: 3 },
     { id: 2, status: "pending", total: 12.0, items: 1 },
     { id: 3, status: "shipped", total: 99.99, items: 7 },
     { id: 4, status: "cancelled", total: 5.25, items: 1 },
   ];
   const shipped = orders.filter(o => o.status === "shipped");
   const revenue = shipped.reduce((sum, o) => sum + o.total, 0);
   return shipped.map(o => o.id).join(",") + "|" + revenue.toFixed(2);`,

  // group-by into an object
  `const events = [
     { type: "click", page: "home" }, { type: "view", page: "home" },
     { type: "click", page: "about" }, { type: "click", page: "home" },
   ];
   const byType = {};
   for (const e of events) {
     (byType[e.type] = byType[e.type] || []).push(e.page);
   }
   return Object.entries(byType).map(([k, v]) => k + ":" + v.length).join(",");`,

  // index-by-id, then join two datasets
  `const users = [{ id: "u1", name: "Ana" }, { id: "u2", name: "Bo" }];
   const purchases = [{ user: "u2", sku: "A" }, { user: "u1", sku: "B" }, { user: "u2", sku: "C" }];
   const byId = Object.fromEntries(users.map(u => [u.id, u.name]));
   return purchases.map(p => byId[p.user] + ">" + p.sku).join(",");`,

  // multi-key sort (stable shape agents write)
  `const rows = [
     { team: "b", score: 2 }, { team: "a", score: 2 }, { team: "a", score: 9 },
   ];
   rows.sort((x, y) => y.score - x.score || x.team.localeCompare(y.team));
   return rows.map(r => r.team + r.score).join(",");`,

  // dedup by key, keep first
  `const seen = new Set();
   const items = [{ sku: "x" }, { sku: "y" }, { sku: "x" }, { sku: "z" }, { sku: "y" }];
   const unique = items.filter(i => !seen.has(i.sku) && seen.add(i.sku));
   return unique.map(i => i.sku).join("");`,

  // parse a JSON "API response", transform, re-stringify
  `const body = '{"results":[{"name":"alpha","ok":true},{"name":"beta","ok":false}],"next":null}';
   const data = JSON.parse(body);
   const names = data.results.filter(r => r.ok).map(r => r.name.toUpperCase());
   return JSON.stringify({ names, hasMore: data.next !== null });`,

  // build a small markdown report
  `const stats = { passed: 12, failed: 2, skipped: 1 };
   const lines = ["# Test Report", ""];
   for (const [k, v] of Object.entries(stats)) {
     lines.push("- **" + k + "**: " + v);
   }
   lines.push("", "Total: " + (stats.passed + stats.failed + stats.skipped));
   return lines.join("\\n");`,

  // chunk a list into pages
  `const ids = Array.from({ length: 11 }, (_, i) => i + 1);
   const pages = [];
   for (let i = 0; i < ids.length; i += 4) pages.push(ids.slice(i, i + 4));
   return pages.map(p => p.join("-")).join("|");`,

  // tolerant field access over incomplete data
  `const profiles = [
     { name: "Ana", contact: { email: "a@x.io" } },
     { name: "Bo" },
     { name: "Cy", contact: {} },
   ];
   return profiles.map(p => p.contact?.email ?? "no-email").join(",");`,

  // retry loop with simulated flaky step
  `let attempts = 0;
   async function flaky() {
     attempts += 1;
     if (attempts < 3) throw new Error("transient");
     return "ok";
   }
   let result = null, lastErr = null;
   for (let i = 0; i < 5; i++) {
     try { result = await flaky(); break; }
     catch (e) { lastErr = e.message; }
   }
   return result + ":" + attempts + ":" + lastErr;`,

  // fan-out async steps, merge results
  `async function fetchScore(name) { return { name, score: name.length * 10 }; }
   const names = ["ada", "grace", "alan"];
   const scores = await Promise.all(names.map(fetchScore));
   const best = scores.reduce((a, b) => (b.score > a.score ? b : a));
   return best.name + "@" + best.score;`,

  // sequential pipeline with intermediate state
  `async function step(state, n) { return { ...state, total: state.total + n, steps: state.steps + 1 }; }
   let state = { total: 0, steps: 0 };
   for (const n of [5, 10, 15]) state = await step(state, n);
   return JSON.stringify(state);`,

  // extract structured data from text
  `const log = "ERROR db timeout after 30s\\nINFO retrying\\nERROR api 503\\nINFO done";
   const errors = log.split("\\n").filter(l => l.startsWith("ERROR")).map(l => l.slice(6));
   return errors.length + ":" + errors.join(";");`,

  // regex capture over semi-structured text
  `const text = "Deployed v2.3.1 to prod; previously v2.2.9 on staging";
   const versions = [...text.matchAll(/v(\d+)\.(\d+)\.(\d+)/g)].map(m => m[1] + m[2] + m[3]);
   return versions.join(",");`,

  // query-string building + parsing round trip
  `function toQuery(params) {
     return Object.entries(params).map(([k, v]) => k + "=" + encodeURIComponent(String(v))).join("&");
   }
   const q = toQuery({ page: 2, tag: "a b", active: true });
   const parsed = Object.fromEntries(q.split("&").map(kv => kv.split("=").map(decodeURIComponent)));
   return q + "|" + parsed.tag + "|" + parsed.page;`,

  // basic stats over a series
  `const latencies = [120, 85, 240, 95, 130, 88];
   const sorted = [...latencies].sort((a, b) => a - b);
   const avg = latencies.reduce((a, b) => a + b, 0) / latencies.length;
   const p50 = sorted[Math.floor(sorted.length / 2)];
   return Math.round(avg) + ":" + p50 + ":" + sorted[0] + ":" + sorted[sorted.length - 1];`,

  // flatten a nested config into dotted keys
  `function flatten(obj, prefix = "") {
     const out = {};
     for (const [k, v] of Object.entries(obj)) {
       const key = prefix ? prefix + "." + k : k;
       if (v && typeof v === "object" && !Array.isArray(v)) Object.assign(out, flatten(v, key));
       else out[key] = v;
     }
     return out;
   }
   return JSON.stringify(flatten({ db: { host: "x", pool: { max: 5 } }, debug: false }));`,

  // CSV-ish parsing into records
  `const csv = "name,qty,price\\nwidget,2,9.99\\ngadget,1,19.5";
   const [header, ...rows] = csv.split("\\n").map(l => l.split(","));
   const records = rows.map(r => Object.fromEntries(r.map((v, i) => [header[i], v])));
   const total = records.reduce((s, r) => s + Number(r.qty) * Number(r.price), 0);
   return records.length + ":" + total.toFixed(2);`,

  // counting with a Map
  `const words = "the quick the lazy the quick fox".split(" ");
   const counts = new Map();
   for (const w of words) counts.set(w, (counts.get(w) ?? 0) + 1);
   const top = [...counts.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
   return top.map(([w, n]) => w + ":" + n).join(",");`,

  // small class model
  `class Cart {
     constructor() { this.items = []; }
     add(name, price, qty = 1) { this.items.push({ name, price, qty }); return this; }
     get total() { return this.items.reduce((s, i) => s + i.price * i.qty, 0); }
   }
   const cart = new Cart().add("pen", 2.5, 4).add("pad", 7);
   return cart.items.length + ":" + cart.total.toFixed(2);`,

  // validation returning an error list
  `function validate(user) {
     const errors = [];
     if (!user.name) errors.push("name required");
     if (!/^[^@]+@[^@]+$/.test(user.email ?? "")) errors.push("email invalid");
     if ((user.age ?? -1) < 0) errors.push("age invalid");
     return errors;
   }
   const results = [
     { name: "Ana", email: "a@x.io", age: 33 },
     { email: "nope", age: 5 },
   ].map(u => validate(u).length);
   return results.join(",");`,

  // shape an error report from mixed failures
  `function describe(e) {
     if (e instanceof TypeError) return "type:" + e.message;
     if (e instanceof Error) return "err:" + e.message;
     return "thrown:" + String(e);
   }
   const out = [];
   for (const thrower of [
     () => { throw new TypeError("bad arg"); },
     () => { throw new Error("io"); },
     () => { throw "raw"; },
   ]) {
     try { thrower(); } catch (e) { out.push(describe(e)); }
   }
   return out.join("|");`,

  // date math (deterministic UTC)
  `const start = new Date(Date.UTC(2026, 0, 15));
   const due = new Date(start.getTime() + 14 * 86400000);
   const days = Math.round((due - start) / 86400000);
   return days + ":" + due.toISOString().slice(0, 10);`,

  // exponential backoff schedule
  `const delays = Array.from({ length: 5 }, (_, i) => Math.min(30, 2 ** i));
   return delays.join(",") + "|total:" + delays.reduce((a, b) => a + b, 0);`,

  // build a tree from a flat parent/child list
  `const flat = [
     { id: 1, parent: null }, { id: 2, parent: 1 }, { id: 3, parent: 1 }, { id: 4, parent: 2 },
   ];
   const children = {};
   for (const n of flat) {
     if (n.parent !== null) (children[n.parent] = children[n.parent] || []).push(n.id);
   }
   function render(id) {
     const kids = children[id] || [];
     return kids.length ? id + "(" + kids.map(render).join(",") + ")" : String(id);
   }
   return render(1);`,

  // diff two config objects
  `const before = { region: "us", retries: 3, debug: true };
   const after = { region: "eu", retries: 3, timeout: 30 };
   const keys = new Set([...Object.keys(before), ...Object.keys(after)]);
   const changes = [];
   for (const k of [...keys].sort()) {
     if (!(k in after)) changes.push("-" + k);
     else if (!(k in before)) changes.push("+" + k);
     else if (before[k] !== after[k]) changes.push("~" + k);
   }
   return changes.join(",");`,

  // paginate via an async generator (a common tool-cursor shape)
  `async function* pages() {
     const all = [["a", "b"], ["c", "d"], ["e"]];
     for (const page of all) yield page;
   }
   const collected = [];
   for await (const page of pages()) collected.push(page.join(""));
   return collected.join("|");`,

  // slugify + truncate (formatting helpers agents write constantly)
  `function slug(s, max = 20) {
     const out = s.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
     return out.length > max ? out.slice(0, max).replace(/-$/, "") : out;
   }
   return [slug("Hello, World!"), slug("  Multi   Space  "), slug("A Very Long Title That Keeps Going", 12)].join("|");`,

  // accumulate a summary across mixed-outcome async tasks
  `async function task(n) {
     if (n % 3 === 0) throw new Error("fail-" + n);
     return n * 2;
   }
   const outcomes = await Promise.allSettled([1, 2, 3, 4, 5, 6].map(task));
   const ok = outcomes.filter(o => o.status === "fulfilled").map(o => o.value);
   const failed = outcomes.filter(o => o.status === "rejected").map(o => o.reason.message);
   return ok.join(",") + "|" + failed.join(",");`,

  // state machine over events
  `const transitions = { idle: { start: "running" }, running: { pause: "paused", stop: "idle" }, paused: { start: "running" } };
   let state = "idle";
   const trace = [state];
   for (const ev of ["start", "pause", "start", "stop", "pause"]) {
     state = transitions[state]?.[ev] ?? state;
     trace.push(state);
   }
   return trace.join(">");`,


  // ── evaluation counts: side effects run exactly as often as in JS ─────
  // (double evaluation is invisible to value-only assertions — these thread
  // call counters through the result)
  `let calls = 0; const o = { a: { x: 10 } }; function f() { calls++; return o; } f().a.x = 1; f().a.x += 5; f().a.x++; return calls + ':' + o.a.x;`,
  `let kc = 0, oc = 0; const o = { k: 5 }; const key = () => (kc++, 'k'); const obj = () => (oc++, o); obj()[key()] += 1; obj()[key()]++; return oc + ',' + kc + ':' + o.k;`,
  `let calls = 0; const o = { v: 0, w: 1, m: null }; function f() { calls++; return o; } f().v ||= 9; f().w &&= 7; f().m ??= 3; return calls + ':' + [o.v, o.w, o.m].join(',');`,
  `let calls = 0; const o = { x: 1, y: 2 }; function f() { calls++; return o; } const d1 = delete f().x; delete f()['y']; return calls + ':' + ('x' in o) + ',' + ('y' in o) + ':' + d1;`,
  // ── evaluation ORDER: target reference (object, then key) before value ──
  `const log = []; const t = (x) => (log.push(x), x); const o = {}; o[t('key')] = t('val'); return log.join(',');`,
  `const log = []; const t = (x) => (log.push(x), x); const o = {}; o[t('k1')] = t('v1'); o[t('k2')] = t('v2'); return log.join(',');`,
  // ── lazy iterator: early break pulls exactly k, not to exhaustion ──────
  `let n = 0; const it = { [Symbol.iterator]() { return { next() { n++; return n < 100 ? { value: n, done: false } : { done: true }; } }; } }; for (const x of it) { if (x >= 3) break; } return n;`,
  `let calls = 0; const it = { [Symbol.iterator]() { calls++; let i = 0; return { next() { return i < 2 ? { value: i++, done: false } : { done: true }; } }; } }; const o = []; for (const a of it) for (const b of it) o.push(a + ':' + b); return o.join(',') + '|' + calls;`,
  // ── getter invoked once per read site; compound = one get + one set ────
  `let reads = 0, writes = 0; const o = { _v: 5, get v() { reads++; return this._v; }, set v(x) { writes++; this._v = x; } }; const sum = o.v + o.v; o.v += 10; return reads + ',' + writes + ',' + sum + ',' + o._v;`,

  // ── round 2: orchestration patterns ───────────────────────────────────
  // concurrency-limited batches, order preserved
  `async function work(n) { return n * n; }
   const ids = [1, 2, 3, 4, 5, 6, 7];
   const out = [];
   for (let i = 0; i < ids.length; i += 3) {
     out.push(...await Promise.all(ids.slice(i, i + 3).map(work)));
   }
   return out.join(',');`,

  // memoized async lookup
  `const cache = new Map();
   let misses = 0;
   async function lookup(key) {
     if (cache.has(key)) return cache.get(key);
     misses += 1;
     const value = key.toUpperCase();
     cache.set(key, value);
     return value;
   }
   const got = [];
   for (const k of ['a', 'b', 'a', 'c', 'b', 'a']) got.push(await lookup(k));
   return got.join('') + '|misses:' + misses;`,

  // middleware composition
  `const middleware = [
     next => async req => next({ ...req, auth: true }),
     next => async req => next({ ...req, traced: req.auth ? 'yes' : 'no' }),
   ];
   const handler = middleware.reduceRight((next, mw) => mw(next), async req => JSON.stringify(req));
   return await handler({ path: '/x' });`,

  // sync event emitter
  `class Emitter {
     constructor() { this.handlers = {}; }
     on(ev, fn) { (this.handlers[ev] = this.handlers[ev] || []).push(fn); return this; }
     emit(ev, data) { for (const fn of this.handlers[ev] || []) fn(data); }
   }
   const log = [];
   const bus = new Emitter().on('job', d => log.push('a:' + d)).on('job', d => log.push('b:' + d));
   bus.emit('job', 1); bus.emit('other', 9); bus.emit('job', 2);
   return log.join(',');`,

  // retry with recorded backoff schedule
  `let calls = 0;
   async function flaky() { calls += 1; if (calls < 4) throw new Error('nope'); return 'ok'; }
   const waits = [];
   let result;
   for (let attempt = 0; attempt < 6; attempt++) {
     try { result = await flaky(); break; }
     catch { waits.push(2 ** attempt * 100); }
   }
   return result + '|' + waits.join(',');`,

  // circuit-breaker counter
  `let failures = 0, state = 'closed';
   const trace = [];
   async function call(shouldFail) {
     if (state === 'open') { trace.push('skipped'); return; }
     if (shouldFail) { failures += 1; trace.push('fail' + failures); if (failures >= 3) state = 'open'; }
     else { failures = 0; trace.push('ok'); }
   }
   for (const f of [true, false, true, true, true, false]) await call(f);
   return state + '|' + trace.join(',');`,

  // queue drain with requeue
  `const queue = [['t1', 0], ['t2', 1], ['t3', 0]];
   const done = [];
   while (queue.length) {
     const [name, fails] = queue.shift();
     if (fails > 0) queue.push([name, fails - 1]);
     else done.push(name);
   }
   return done.join(',');`,

  // ── round 2: data munging ─────────────────────────────────────────────
  // pivot rows into columns
  `const rows = [
     { metric: 'cpu', host: 'a', v: 10 }, { metric: 'cpu', host: 'b', v: 20 },
     { metric: 'mem', host: 'a', v: 30 }, { metric: 'mem', host: 'b', v: 40 },
   ];
   const pivot = {};
   for (const r of rows) (pivot[r.metric] = pivot[r.metric] || {})[r.host] = r.v;
   return JSON.stringify(pivot);`,

  // merge records by id (last write wins per field)
  `const a = [{ id: 1, name: 'Ana' }, { id: 2, name: 'Bo', city: 'Oslo' }];
   const b = [{ id: 2, city: 'Bergen' }, { id: 3, name: 'Cy' }];
   const byId = new Map();
   for (const rec of [...a, ...b]) byId.set(rec.id, { ...byId.get(rec.id), ...rec });
   return JSON.stringify([...byId.values()]);`,

  // interval merging
  `const spans = [[1, 3], [2, 6], [8, 10], [9, 12], [15, 16]];
   spans.sort((x, y) => x[0] - y[0]);
   const merged = [];
   for (const [lo, hi] of spans) {
     const last = merged[merged.length - 1];
     if (last && lo <= last[1]) last[1] = Math.max(last[1], hi);
     else merged.push([lo, hi]);
   }
   return merged.map(s => s.join('-')).join(',');`,

  // partition + summarize
  `const txns = [12.5, -3, 40, -7.25, 0, 18];
   const credits = txns.filter(t => t > 0), debits = txns.filter(t => t < 0);
   const sum = arr => arr.reduce((a, b) => a + b, 0);
   return 'in:' + sum(credits).toFixed(2) + ' out:' + sum(debits).toFixed(2) + ' net:' + sum(txns).toFixed(2);`,

  // deep defaults merge
  `function mergeDefaults(cfg, defaults) {
     const out = { ...defaults, ...cfg };
     for (const k of Object.keys(defaults)) {
       if (cfg[k] && typeof cfg[k] === 'object' && !Array.isArray(cfg[k])) {
         out[k] = mergeDefaults(cfg[k], defaults[k] || {});
       }
     }
     return out;
   }
   return JSON.stringify(mergeDefaults({ db: { port: 6432 } }, { db: { host: 'x', port: 5432 }, debug: false }));`,

  // pick / omit helpers
  `const pick = (o, keys) => Object.fromEntries(Object.entries(o).filter(([k]) => keys.includes(k)));
   const omit = (o, keys) => Object.fromEntries(Object.entries(o).filter(([k]) => !keys.includes(k)));
   const user = { id: 7, name: 'Ana', password: 'hunter2', role: 'admin' };
   return JSON.stringify(pick(user, ['id', 'name'])) + '|' + JSON.stringify(omit(user, ['password']));`,

  // top-N with ties broken alphabetically
  `const scores = { ana: 9, bo: 7, cy: 9, di: 3, ed: 7 };
   const top3 = Object.entries(scores)
     .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
     .slice(0, 3);
   return top3.map(([n, s]) => n + '=' + s).join(',');`,

  // frequency histogram into buckets
  `const values = [3, 12, 7, 25, 18, 4, 9, 31, 15];
   const buckets = { '0-9': 0, '10-19': 0, '20+': 0 };
   for (const v of values) {
     if (v < 10) buckets['0-9'] += 1;
     else if (v < 20) buckets['10-19'] += 1;
     else buckets['20+'] += 1;
   }
   return Object.entries(buckets).map(([k, n]) => k + ':' + n).join(' ');`,

  // moving average
  `const series = [10, 12, 11, 15, 20, 18];
   const window = 3;
   const avgs = [];
   for (let i = window - 1; i < series.length; i++) {
     const slice = series.slice(i - window + 1, i + 1);
     avgs.push((slice.reduce((a, b) => a + b, 0) / window).toFixed(1));
   }
   return avgs.join(',');`,

  // running totals
  `const deltas = [5, -2, 7, -1, 3];
   let total = 0;
   return deltas.map(d => (total += d)).join(',');`,

  // cents-safe money math
  `const prices = [19.99, 4.5, 0.1, 0.2];
   const cents = prices.map(p => Math.round(p * 100));
   const totalCents = cents.reduce((a, b) => a + b, 0);
   return (totalCents / 100).toFixed(2);`,

  // safe JSON parse with fallback
  `function safeParse(text, fallback) {
     try { return JSON.parse(text); } catch { return fallback; }
   }
   return JSON.stringify([safeParse('{"a":1}', {}), safeParse('{bad', { error: true })]);`,

  // binary search over sorted ids
  `const sorted = [2, 5, 8, 12, 16, 23, 38, 56, 72, 91];
   function bsearch(arr, target) {
     let lo = 0, hi = arr.length - 1;
     while (lo <= hi) {
       const mid = (lo + hi) >> 1;
       if (arr[mid] === target) return mid;
       if (arr[mid] < target) lo = mid + 1; else hi = mid - 1;
     }
     return -1;
   }
   return [bsearch(sorted, 23), bsearch(sorted, 2), bsearch(sorted, 91), bsearch(sorted, 7)].join(',');`,

  // diff lists: added / removed
  `const before = ['a', 'b', 'c', 'd'];
   const after = ['b', 'c', 'e'];
   const added = after.filter(x => !before.includes(x));
   const removed = before.filter(x => !after.includes(x));
   return '+' + added.join('+') + ' -' + removed.join('-');`,

  // dependency-order resolution
  `const deps = { app: ['db', 'cache'], db: ['net'], cache: ['net'], net: [] };
   const ordered = [];
   const visit = (name) => {
     if (ordered.includes(name)) return;
     for (const d of deps[name]) visit(d);
     ordered.push(name);
   };
   for (const name of Object.keys(deps)) visit(name);
   return ordered.join(',');`,

  // LRU-ish cache with Map insertion order
  `const lru = new Map();
   const MAX = 3;
   function put(k, v) {
     if (lru.has(k)) lru.delete(k);
     lru.set(k, v);
     if (lru.size > MAX) lru.delete(lru.keys().next().value);
   }
   for (const [k, v] of [['a', 1], ['b', 2], ['c', 3], ['a', 9], ['d', 4]]) put(k, v);
   return [...lru.entries()].map(([k, v]) => k + v).join(',');`,

  // tree node count + max depth
  `const tree = { v: 1, kids: [{ v: 2, kids: [{ v: 4, kids: [] }] }, { v: 3, kids: [] }] };
   function walk(node, depth) {
     return node.kids.reduce(
       (acc, k) => { const r = walk(k, depth + 1); return { n: acc.n + r.n, d: Math.max(acc.d, r.d) }; },
       { n: 1, d: depth }
     );
   }
   return JSON.stringify(walk(tree, 1));`,

  // ── round 2: text & formatting ────────────────────────────────────────
  // aligned text table
  `const rows = [['name', 'qty'], ['widget', '2'], ['gizmo', '110']];
   const w0 = Math.max(...rows.map(r => r[0].length));
   return rows.map(r => r[0].padEnd(w0 + 2) + r[1].padStart(4)).join(';');`,

  // pluralize + title case
  `const titleCase = s => s.split(' ').map(w => w[0].toUpperCase() + w.slice(1)).join(' ');
   const plural = (n, word) => n + ' ' + word + (n === 1 ? '' : 's');
   return titleCase('weekly status report') + ': ' + plural(1, 'error') + ', ' + plural(4, 'warning');`,

  // extract mentions and tags
  `const post = 'Thanks @ana and @bo for #launch help! cc @ana #launch #q3';
   const mentions = [...new Set(post.match(/@\\w+/g))];
   const tags = [...new Set(post.match(/#\\w+/g))];
   return mentions.join(',') + '|' + tags.join(',');`,

  // markdown link extraction with named groups
  `const md = 'See [docs](https://d.io/a) and [api](https://d.io/b).';
   const links = [...md.matchAll(/\\[(?<label>[^\\]]+)\\]\\((?<url>[^)]+)\\)/g)]
     .map(m => m.groups.label + '=>' + m.groups.url);
   return links.join(',');`,

  // word wrap
  `function wrap(text, width) {
     const words = text.split(' ');
     const lines = [''];
     for (const w of words) {
       const cur = lines[lines.length - 1];
       if (cur && (cur + ' ' + w).length > width) lines.push(w);
       else lines[lines.length - 1] = cur ? cur + ' ' + w : w;
     }
     return lines;
   }
   return wrap('the quick brown fox jumps over the lazy dog', 12).join('/');`,

  // truncate with ellipsis, replaceAll cleanup
  `const raw = '  Spaced   out   draft\\ttitle  ';
   const clean = raw.replaceAll('\\t', ' ').trim().replace(/\\s+/g, ' ');
   const truncate = (s, n) => s.length <= n ? s : s.slice(0, n - 1) + '…';
   return clean + '|' + truncate(clean, 12);`,

  // semver compare
  `function cmp(a, b) {
     const pa = a.split('.').map(Number), pb = b.split('.').map(Number);
     for (let i = 0; i < 3; i++) { if (pa[i] !== pb[i]) return pa[i] < pb[i] ? -1 : 1; }
     return 0;
   }
   const vs = ['1.2.10', '1.10.0', '1.2.9', '0.9.9'];
   return vs.sort(cmp).join(' < ');`,

  // key=value config parsing with comments
  `const conf = ['# main', 'host=db.local', 'port=5432', '', '# tuning', 'pool = 8'];
   const parsed = {};
   for (const line of conf) {
     const t = line.trim();
     if (!t || t.startsWith('#')) continue;
     const [k, v] = t.split('=').map(x => x.trim());
     parsed[k] = /^\\d+$/.test(v) ? Number(v) : v;
   }
   return JSON.stringify(parsed);`,

  // ── round 2: dates & ids ──────────────────────────────────────────────
  // day-of-week + range iteration (UTC, deterministic)
  `const start = new Date(Date.UTC(2026, 5, 8));
   const names = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
   const days = [];
   for (let i = 0; i < 5; i++) {
     const d = new Date(start.getTime() + i * 86400000);
     days.push(names[d.getUTCDay()] + d.getUTCDate());
   }
   return days.join(',');`,

  // duration formatting
  `function fmt(ms) {
     const s = Math.floor(ms / 1000), m = Math.floor(s / 60), h = Math.floor(m / 60);
     return h + 'h' + String(m % 60).padStart(2, '0') + 'm' + String(s % 60).padStart(2, '0') + 's';
   }
   return [fmt(45000), fmt(3725000), fmt(86400000)].join(' ');`,

  // month bucketing by date string
  `const events = ['2026-03-02', '2026-03-05', '2026-04-11', '2026-04-12'];
   const byMonth = {};
   for (const e of events) {
     const key = e.slice(0, 7);
     byMonth[key] = (byMonth[key] ?? 0) + 1;
   }
   return JSON.stringify(byMonth);`,

  // sequential id factory via closure + generator
  `function* idGen(prefix) { let n = 0; while (true) yield prefix + '-' + (++n); }
   const ids = idGen('task');
   const made = [ids.next().value, ids.next().value, ids.next().value];
   return made.join(',');`,

  // ── round 2: language-surface probes agents rely on ──────────────────
  `return [[1, [2, [3, [4]]]].flat(Infinity).join(''), [[1, 2], [3]].flatMap(x => x).join('')].join('|');`,
  `const a = [5, 12, 8, 130, 44]; return [a.at(-1), a.findLast(x => x < 50), a.findLastIndex(x => x < 50)].join(',');`,
  `return ['9'.padStart(3, '0'), 'ab'.padEnd(5, '.'), 'abc'.at(-1)].join('|');`,
  `const o = structuredClone({ a: [1, { b: 2 }], s: 'x' }); o.a[1].b = 99; return JSON.stringify(o);`,
  `const grouped = Object.groupBy([6.1, 4.2, 6.3], n => Math.floor(n)); return JSON.stringify(grouped);`,
  `const m = Map.groupBy(['apple', 'avocado', 'beet'], w => w[0]); return [...m.keys()].join(',') + '|' + m.get('a').length;`,
  `return btoa('hello world') + '|' + atob('aGk=');`,
  `await new Promise(resolve => setTimeout(resolve, 1)); return 'slept';`,
  `const order = []; setTimeout(() => order.push('timeout'), 0); await Promise.resolve().then(() => order.push('micro')); await new Promise(r => setTimeout(r, 1)); return order.join(',');`,
  `class Resource { #open = true; close() { this.#open = false; } get open() { return this.#open; } } const r = new Resource(); try { throw new Error('boom'); } catch {} finally { r.close(); } return r.open;`,
  `const obj = { _n: 5, get double() { return this._n * 2; }, set n(v) { this._n = v; } }; obj.n = 21; return obj.double;`,
  `const range = { from: 1, to: 4, [Symbol.iterator]() { let c = this.from; const end = this.to; return { next: () => c <= end ? { value: c++, done: false } : { value: undefined, done: true } }; } }; return [...range].join(',') + '|' + Math.max(...range);`,
  `const wm = new WeakMap(); const k1 = {}, k2 = {}; wm.set(k1, 'one'); return [wm.has(k1), wm.has(k2), wm.get(k1)].join(',');`,
  `let i = 0; const out = []; do { out.push(i); i += 2; } while (i < 7); return out.join(',');`,
  `outer: for (const x of [1, 2, 3]) { for (const y of [10, 20]) { if (x * y === 40) break outer; } } return 'done';`,
  `try { null.x; } catch (e) { const wrapped = new Error('ctx', { cause: e }); return wrapped.message + '|' + (wrapped.cause instanceof TypeError); }`,
  `return [Number.parseFloat('3.14abc'), parseInt('1f', 16), Number.isNaN(Number('42px')), (255).toString(16)].join('|');`,
  `return [(1234.5678).toFixed(2), (0.000001234).toPrecision(2)].join('|');`,
  `return [[NaN].includes(NaN), [NaN].indexOf(NaN), Array.from(new Set([0, -0])).length].join(',');`,
  `const stable = [{k:'b',i:1},{k:'a',i:2},{k:'b',i:3},{k:'a',i:4}]; stable.sort((x,y)=>x.k.localeCompare(y.k)); return stable.map(e=>e.k+e.i).join(',');`,
  `const fns = { greet: n => 'hi ' + n }; return (fns.greet?.('ana') ?? 'none') + '|' + (fns.missing?.('x') ?? 'none');`,
  `const s = new Set('mississippi'); return [...s].join('') + '|' + s.size;`,
  `const m1 = new Map([['a', 1], ['b', 2]]); const m2 = new Map([...m1].map(([k, v]) => [k, v * 10])); return JSON.stringify(Object.fromEntries(m2));`,
  `function clamp(n, lo, hi) { return Math.min(hi, Math.max(lo, n)); } return [clamp(5, 0, 10), clamp(-3, 0, 10), clamp(99, 0, 10)].join(',');`,
  `const seen = {}; const path = 'a.b.c'; let cur = seen; for (const part of path.split('.')) cur = cur[part] = cur[part] ?? {}; cur.value = 42; return JSON.stringify(seen);`,
  `function get(obj, path, dflt) { return path.split('.').reduce((o, k) => (o ?? {})[k], obj) ?? dflt; } const cfg = { a: { b: { c: 7 } } }; return [get(cfg, 'a.b.c', 0), get(cfg, 'a.x.c', -1)].join(',');`,

  // ── round 2: async generators over "tool pages" ───────────────────────
  `async function* fetchPages() {
     const pages = [{ items: ['a', 'b'], next: true }, { items: ['c'], next: true }, { items: [], next: false }];
     for (const p of pages) { yield p.items; if (!p.next) return; }
   }
   const all = [];
   for await (const items of fetchPages()) all.push(...items);
   return all.join(',');`,

  `async function* numbers() { for (let i = 1; i <= 7; i++) yield i; }
   async function take(gen, n) { const out = []; for await (const v of gen) { out.push(v); if (out.length === n) break; } return out; }
   return (await take(numbers(), 4)).join(',');`,

  `async function* source() { for (const v of [5, 3, 8, 1, 9, 2, 7]) yield v; }
   const batches = [];
   let cur = [];
   for await (const v of source()) {
     cur.push(v);
     if (cur.length === 3) { batches.push(cur); cur = []; }
   }
   if (cur.length) batches.push(cur);
   return batches.map(b => Math.max(...b)).join(',');`,

  // ── round 3: modern array & object surface ───────────────────────────
  `const base = [3, 1, 2]; const sorted = base.toSorted(); const rev = base.toReversed(); const w = base.with(1, 9); return JSON.stringify({ base, sorted, rev, w });`,
  `const log = [5, 2, 9, 1]; return [log.some(x => x > 8), log.every(x => x > 0), log.reduceRight((a, b) => a + '-' + b)].join('|');`,
  `const buf = new Array(6).fill(0).map((_, i) => i); buf.copyWithin(0, 3); return buf.join(',');`,
  `const o = Object.assign({}, { a: 1, b: 2 }, { b: 3, c: 4 }, null, { d: 5 }); return JSON.stringify(o);`,
  `const mixed = { b: 1, 2: 'two', a: 3, 1: 'one' }; return Object.keys(mixed).join(',');`,
  `const user = { name: 'ana' }; return [('name' in user), user.hasOwnProperty('name'), ('toString' in user), user.hasOwnProperty('toString')].join(',');`,
  `const defaults = { retries: 3, debug: false }; const override = { debug: true }; return JSON.stringify({ ...defaults, ...override, source: 'env' });`,
  `return [Number.isInteger(5.0), Number.isSafeInteger(2 ** 53), Number.MAX_SAFE_INTEGER === 2 ** 53 - 1, Math.abs(0.1 + 0.2 - 0.3) < Number.EPSILON].join(',');`,
  `return [Math.hypot(3, 4), Math.cbrt(27), Math.sign(-5), Math.trunc(-4.7), Math.log2(1024), Math.log10(1000)].join(',');`,
  `return [(12345.6789).toExponential(2), (0.5).toFixed(0), (1.005).toFixed(2)].join('|');`,

  // ── round 3: string surface agents actually hit ──────────────────────
  `const s = 'Hello, World'; return [s.startsWith('Hello'), s.endsWith('World'), s.startsWith('World', 7), s.includes('lo, W')].join(',');`,
  `return ['  pad  '.trimStart() + '|', '|' + '  pad  '.trimEnd(), 'ab'.repeat(3), 'a-b-c-d'.split('-', 2).join('+')].join(' ');`,
  `const path = '/api/v2/users/42/orders'; const parts = path.split('/').filter(Boolean); return parts.at(-2) + ':' + parts.lastIndexOf('users');`,
  `return ['café'.normalize('NFC').length, 'cafe\\u0301'.normalize('NFC').length, 'x'.codePointAt(0)].join(',');`,
  `const csvField = 'said "hi", left'; const quoted = '"' + csvField.replaceAll('"', '""') + '"'; return quoted;`,

  // ── round 3: regex replacement patterns ────────────────────────────────
  `return 'john smith'.replace(/(\\w+) (\\w+)/, '$2, $1');`,
  `return '2026-06-11'.replace(/(?<y>\\d{4})-(?<m>\\d{2})-(?<d>\\d{2})/, '$<d>/$<m>/$<y>');`,
  `return 'price: 42'.replace(/\\d+/, m => String(Number(m) * 2)) + '|' + 'aaa'.replace(/a/g, '$&$&');`,
  `const re = /t(e)(st(\\d?))/g; const out = []; let m; while ((m = re.exec('test1test2')) !== null) { out.push(m[0] + ':' + m[3] + '@' + m.index); } return out.join(',');`,
  `const r = /ab+c/gi; return [r.source, r.flags, r.global, r.ignoreCase].join('|');`,

  // ── round 3: classes & closures in agent shapes ───────────────────────
  `class Tool {
     static registry = [];
     static register(name) { Tool.registry.push(name); return new Tool(name); }
     constructor(name) { this.name = name; }
     describe() { return 'tool:' + this.name; }
   }
   const t1 = Tool.register('search'); Tool.register('fetch');
   return t1.describe() + '|' + Tool.registry.join(',');`,
  `class Base { greet() { return 'base'; } } class Mid extends Base { greet() { return 'mid>' + super.greet(); } } class Leaf extends Mid { greet() { return 'leaf>' + super.greet(); } } const l = new Leaf(); return l.greet() + '|' + (l instanceof Base) + ',' + (l instanceof Mid);`,
  `class Money { constructor(cents) { this.cents = cents; } toString() { return '$' + (this.cents / 100).toFixed(2); } } const m = new Money(1999); return \`total: \${m}\`;`,
  `const counters = []; for (let i = 0; i < 3; i++) { counters.push(() => i * 10); } return counters.map(f => f()).join(',');`,
  `function makeCounter() { let n = 0; return { inc: () => ++n, get: () => n }; } const c1 = makeCounter(), c2 = makeCounter(); c1.inc(); c1.inc(); c2.inc(); return c1.get() + ',' + c2.get();`,
  `const config = (() => { const secret = 'abc'; return { masked: () => secret.replace(/./g, '*') }; })(); return config.masked();`,

  // ── round 3: collections depth ────────────────────────────────────────
  `const m = new Map(); const kObj = { id: 1 }; m.set(kObj, 'by-ref'); m.set(NaN, 'nan-key'); m.set(1, 'int'); m.set('1', 'str'); return [m.get(kObj), m.get(NaN), m.get(1), m.get('1'), m.has({ id: 1 })].join('|');`,
  `const a = new Set([1, 2, 3, 4]); const b = new Set([3, 4, 5]); const inter = [...a].filter(x => b.has(x)); const union = new Set([...a, ...b]); return inter.join(',') + '|' + union.size;`,
  `const m = new Map([['b', 2], ['a', 1]]); const out = []; m.forEach((v, k) => out.push(k + '=' + v)); return out.join(',') + '|' + [...m.entries()].flat().join('');`,
  `const visits = new Map(); for (const page of ['a', 'b', 'a', 'c', 'a']) { visits.set(page, (visits.get(page) ?? 0) + 1); } const top = [...visits].sort((x, y) => y[1] - x[1])[0]; return top.join(':');`,

  // ── round 3: generators & iterator protocol ───────────────────────────
  `function* chunks(arr, size) { for (let i = 0; i < arr.length; i += size) yield arr.slice(i, i + size); } return [...chunks([1,2,3,4,5], 2)].map(c => c.join('')).join('|');`,
  `function* infinite() { let i = 0; while (true) yield i++; } const it = infinite(); const first = [it.next().value, it.next().value]; const r = it.return('stop'); return first.join(',') + '|' + r.done + ',' + it.next().done;`,
  `function* gen() { yield 1; yield 2; yield 3; } const [first, ...rest] = gen(); return first + '|' + rest.join(',');`,
  `class Range { constructor(a, b) { this.a = a; this.b = b; } *[Symbol.iterator]() { for (let i = this.a; i <= this.b; i++) yield i; } } return [...new Range(2, 5)].join(',');`,

  // ── round 3: async patterns, one more pass ────────────────────────────
  `const results = []; for await (const v of [Promise.resolve('a'), 'plain', Promise.resolve('c')]) { results.push(v); } return results.join(',');`,
  `const fetchUser = async id => ({ id, name: 'u' + id }); const fetchOrders = async user => [user.id * 10, user.id * 10 + 1]; const user = await fetchUser(7); const orders = await fetchOrders(user); return user.name + ':' + orders.join(',');`,
  `async function risky(n) { if (n > 2) throw new RangeError('too big: ' + n); return n; } const settled = await Promise.allSettled([1, 2, 3, 4].map(risky)); return settled.map(s => s.status === 'fulfilled' ? s.value : s.reason.name).join(',');`,
  `let timeline = []; async function step(name) { timeline.push('start:' + name); await Promise.resolve(); timeline.push('end:' + name); } await Promise.all([step('a'), step('b')]); return timeline.join(',');`,
  `const cache = {}; async function once(key, fn) { if (!(key in cache)) cache[key] = await fn(); return cache[key]; } let calls = 0; const f = async () => { calls++; return 'val'; }; await once('k', f); await once('k', f); return calls;`,

  // ── round 3: JSON depth ───────────────────────────────────────────────
  `const payload = { user: { id: 7 }, tags: ['a', 'b'] }; return JSON.stringify(payload, null, 2).split('\\n').length;`,
  `return JSON.stringify({ a: 1, b: 2, c: 3 }, ['a', 'c']);`,
  `const inv = { date: new Date(Date.UTC(2026, 0, 5)), total: 99 }; const s = JSON.stringify(inv); return s.includes('2026-01-05') + '|' + JSON.parse(s).total;`,
  `class Temp { constructor(c) { this.c = c; } toJSON() { return { celsius: this.c, f: this.c * 9 / 5 + 32 }; } } return JSON.stringify({ reading: new Temp(20) });`,
  `return JSON.stringify(JSON.parse('{ "a" : [ 1 , 2 ] , "b" : null }'));`,

  // ── round 3: control flow & operators in real shapes ──────────────────
  `function category(code) { switch (true) { case code < 200: return 'info'; case code < 300: return 'ok'; case code < 400: return 'redirect'; default: return 'error'; } } return [101, 204, 301, 503].map(category).join(',');`,
  `function level(n) { switch (n) { case 0: case 1: return 'low'; case 2: return 'mid'; default: return 'high'; } } return [0, 1, 2, 5].map(level).join(',');`,
  `let attempts = 0, result = null; while (result === null && attempts < 5) { attempts += 1; if (attempts === 3) result = 'found'; } return result + '@' + attempts;`,
  `const flags = 0b1010; return [(flags & 0b10) !== 0, (flags | 0b1) === 11, flags >> 1, (flags ^ 0b1111)].join(',');`,
  `const score = 77; const grade = score >= 90 ? 'A' : score >= 80 ? 'B' : score >= 70 ? 'C' : 'F'; return grade;`,
  `let a = 5; a += 2; a **= 2; a %= 30; a ||= 99; let b = null; b ??= 'filled'; let c = 'keep'; c ??= 'no'; return [a, b, c].join(',');`,
  `const ids = []; for (const [i, ch] of [...'abc'].entries()) ids.push(i + ch); return ids.join(',');`,

  // ── round 3: tagged templates & misc surface ──────────────────────────
  `const esc = (strings, ...vals) => strings.reduce((acc, s, i) => acc + s + (i < vals.length ? String(vals[i]).replaceAll("'", "''") : ''), ''); const name = "O'Brien"; return esc\`SELECT * WHERE name = '\${name}'\`;`,
  `return [void 0 === undefined, (1, 2, 3), typeof void 'x'].join('|');`,
  `const cloned = structuredClone(new Map([['a', [1, 2]]])); return cloned instanceof Map ? cloned.get('a').join(',') : 'not-map';`,
  `const big = 9007199254740993n; return (big + 1n).toString() + '|' + (typeof big);`,
  `const d = new Date(Date.UTC(2026, 11, 31, 23, 59)); d.setUTCDate(d.getUTCDate() + 1); return d.toISOString().slice(0, 16);`,

  // ════════════════════════════════════════════════════════════════════════
  //  stdlib surface sweep — every commonly-used method, probed by name so a
  //  missing one fails loudly here rather than deep inside an agent program.
  // ════════════════════════════════════════════════════════════════════════

  // Array.prototype (mutators)
  `const a = [1, 2, 3]; a.push(4, 5); const p = a.pop(); a.unshift(0); const sh = a.shift(); a.splice(1, 2, 'x'); return JSON.stringify({ a, p, sh });`,
  `const a = [3, 1, 2]; a.sort(); a.reverse(); a.fill(0, 2); a.copyWithin(0, 1); return a.join(',');`,
  // Array.prototype (readers)
  `const a = [1, 2, 3, 4]; return [a.slice(1, 3).join(''), a.concat([5]).length, a.indexOf(3), a.lastIndexOf(2), a.includes(9), a.at(-2)].join('|');`,
  `const a = [5, 12, 8, 130]; return [a.find(x => x > 10), a.findIndex(x => x > 10), a.findLast(x => x > 10), a.findLastIndex(x => x > 10)].join(',');`,
  `const a = [1, 2, 3]; return [a.some(x => x > 2), a.every(x => x > 0), a.filter(x => x % 2).join(''), a.map(x => x * 2).join(''), a.reduce((s, x) => s + x, 0), a.reduceRight((s, x) => s + x)].join('|');`,
  `return [[1, [2, [3]]].flat().length, [1, 2].flatMap(x => [x, x]).join(''), [...[1, 2].keys()].join(''), [...[1, 2].values()].join(''), [...['a'].entries()][0].join(':')].join('|');`,
  `const a = [3, 1]; return [a.toSorted().join(''), a.toReversed().join(''), a.with(0, 9).join(''), a.toSpliced(0, 1).join(''), a.join('-')].join('|');`,
  `return [Array.isArray([]), Array.from('ab').join(''), Array.from({ length: 3 }, (_, i) => i).join(''), Array.of(1, 2).join('')].join('|');`,
  // String.prototype
  `const s = 'Hello World'; return [s.charAt(1), s.charCodeAt(0), s.codePointAt(0), s.at(-1), s.indexOf('o'), s.lastIndexOf('o'), s.includes('World')].join('|');`,
  `const s = 'Hello'; return [s.startsWith('He'), s.endsWith('lo'), s.slice(1, 3), s.substring(1, 3), s.toUpperCase(), s.toLowerCase()].join('|');`,
  `return ['  x  '.trim(), ' x '.trimStart(), ' x '.trimEnd(), '5'.padStart(3, '0'), '5'.padEnd(3, '!'), 'ab'.repeat(2), 'a,b,c'.split(',').length].join('|');`,
  `return ['a-b'.replace('-', '+'), 'a-b-c'.replaceAll('-', '+'), 'x1y2'.match(/\\d/g).join(''), 'abc'.search(/b/), 'a'.concat('b'), 'a'.localeCompare('b') < 0, 'caf\\u00e9'.normalize().length, String.fromCharCode(72, 105)].join('|');`,
  `return [[...'hi'].join('-'), 'abc'[1], \`\${1}\${'x'}\`, String.raw\`a\\nb\`.length].join('|');`,
  // Object statics
  `const o = { b: 2, a: 1 }; return [Object.keys(o).join(''), Object.values(o).join(''), Object.entries(o).flat().join(''), Object.assign({}, o, { c: 3 }).c, Object.fromEntries([['x', 1]]).x].join('|');`,
  `const o = Object.freeze({ a: 1 }); return [Object.isFrozen(o), Object.hasOwn({ a: 1 }, 'a'), Object.hasOwn({}, 'a'), Object.getOwnPropertyNames({ a: 1, b: 2 }).join('')].join('|');`,
  `return JSON.stringify(Object.groupBy([1.1, 2.2, 1.9], Math.floor));`,
  // Number + Math
  `return [Number.parseFloat('1.5x'), Number.parseInt('ff', 16), Number.isNaN(NaN), Number.isFinite(1 / 0), Number.isInteger(2.0), Number.isSafeInteger(2 ** 53)].join('|');`,
  `return [(3.14159).toFixed(2), (1234.5).toPrecision(3), (0.00001).toExponential(1), (255).toString(2).length, Number.MAX_SAFE_INTEGER > 0, Number.EPSILON > 0].join('|');`,
  `return [Math.abs(-2), Math.ceil(1.1), Math.floor(1.9), Math.round(2.5), Math.trunc(-1.7), Math.sign(-9), Math.sqrt(16), Math.cbrt(8), Math.pow(2, 5), 2 ** 6].join(',');`,
  `return [Math.min(3, 1, 2), Math.max(3, 1, 2), Math.hypot(3, 4), Math.exp(0), Math.log(1), Math.log2(8), Math.log10(100), Math.atan2(0, 1), typeof Math.random()].join(',');`,
  // JSON
  `return [JSON.stringify({ a: [1, null], b: 'x' }), JSON.parse('[1, 2]').length, JSON.stringify({ a: 1, b: 2 }, ['a']), JSON.stringify({ a: 1 }, null, 2).includes('  '), JSON.parse('"s"')].join('|');`,
  // Map / Set
  `const m = new Map([['a', 1]]); m.set('b', 2); const had = m.has('a'); m.delete('a'); return [had, m.size, m.get('b'), [...m.keys()].join(''), [...m.values()].join(''), [...m.entries()].flat().join('')].join('|');`,
  `const s = new Set([1, 2, 2]); s.add(3); const had = s.has(1); s.delete(1); return [had, s.size, [...s].join(''), [...s.keys()].join('')].join('|');`,
  `const m = new Map([['k', 1]]); m.clear(); const s = new Set([1]); s.clear(); return m.size + ',' + s.size;`,
  `const m = Map.groupBy([1, 2, 3], n => n % 2 ? 'odd' : 'even'); return [...m.keys()].sort().join(',') + '|' + m.get('odd').join('');`,
  // Promise statics + instance
  `const r = await Promise.all([Promise.resolve(1), 2]); const s = await Promise.allSettled([Promise.reject('e')]); const rc = await Promise.race([Promise.resolve('w')]); const an = await Promise.any([Promise.reject('x'), Promise.resolve('y')]); return [r.join(''), s[0].status, rc, an].join('|');`,
  `let out = []; await Promise.resolve(1).then(v => out.push('t' + v)).finally(() => out.push('f')); await Promise.reject('e').catch(e => out.push('c' + e)); return out.join(',');`,
  // Date (UTC surface)
  `const d = new Date(Date.UTC(2026, 2, 15, 10, 30, 45, 500)); return [d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate(), d.getUTCDay(), d.getUTCHours(), d.getUTCMinutes(), d.getUTCSeconds(), d.getUTCMilliseconds()].join(',');`,
  `const d = new Date(0); d.setUTCFullYear(2030, 5, 20); d.setUTCHours(8, 15, 30, 250); return d.toISOString();`,
  `const d = new Date('2026-04-01T12:00:00Z'); d.setTime(d.getTime() + 86400000); return [d.toISOString().slice(0, 10), d.getTime() > 0, typeof Date.now(), Date.parse('2026-01-01T00:00:00Z') > 0].join('|');`,
  // RegExp surface
  `const re = /(\\w+)@(\\w+)/; const m = 'user@host'.match(re); return [re.test('a@b'), m[1], m[2], m.index, re.exec('x@y')[0], re.source.length > 0, re.flags].join('|');`,
  // globals
  `return [parseInt('42px'), parseFloat('3.5kg'), isNaN('x'), isFinite('5'), encodeURIComponent('a b'), decodeURIComponent('a%20b'), btoa('ok'), atob('b2s=')].join('|');`,
  `return [typeof structuredClone({}), typeof queueMicrotask, typeof setTimeout, typeof clearTimeout, typeof Symbol(), typeof Symbol.iterator].join('|');`,
];

// ════════════════════════════════════════════════════════════════════════════
//  Pinned divergences — assert zapcode's ACTUAL value, with the reason.
//  If zapcode starts agreeing with Node, the pin fails so it gets promoted.
// ════════════════════════════════════════════════════════════════════════════

const pinned = [
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
const bothThrew = [];

for (const body of [...corpus, ...realistic]) {
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
      // A both-throw "agreement" usually means the SNIPPET is broken (Node
      // is ground truth for validity) — surface it so it gets fixed rather
      // than silently counting as parity.
      bothThrew.push({ body, node: nodeErr });
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

if (bothThrew.length > 0) {
  console.log(`note: ${bothThrew.length} snippet(s) threw in BOTH engines (counted as parity, but check the snippets):`);
  for (const t of bothThrew) {
    console.log("  node error:", t.node, "\n  snippet:", t.body.slice(0, 100).replace(/\n/g, " "));
  }
}
console.log(`differential parity: ${passed} snippets agree with Node (${pinned.length} documented pins held)`);
