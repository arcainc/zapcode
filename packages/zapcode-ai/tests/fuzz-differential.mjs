/**
 * Property-based differential fuzzer: generates seeded random programs from
 * the KNOWN-SUPPORTED grammar (see AGENTS.md subset table + differential.mjs
 * corpus) and runs each through BOTH zapcode and real Node via the shared
 * diff-harness-lib. Any divergence is auto-minimized (greedy statement drop,
 * then return-field shrink) and printed as a minimal repro.
 *
 * Two bug classes value-only diffing misses are fuzzed here:
 *   1. EFFECT ORDER + LAZINESS. Every program carries a deterministic preamble
 *      `const __log = []; const __t = (v) => (__log.push(String(v)), v);` and
 *      generated subexpressions are randomly wrapped in `__t(<expr>)`. `__t`
 *      is value-identity-preserving, so wrapping never changes a value — it
 *      only records that the expression was evaluated, and WHEN. `__log` is
 *      then part of the program's returned object, so evaluation order,
 *      short-circuit laziness (`&&`/`||`/`??`/`?:`), and call counts are
 *      mechanically diffed against Node (Node is ground truth).
 *   2. RICHER GRAMMAR: classes (ctor/methods/extends/super/get/set), Map/Set
 *      construction + iteration, switch (with fallthrough), optional chaining +
 *      nullish, object getters/setters, for-of over custom [Symbol.iterator]
 *      iterables, labeled break/continue, try/catch/finally — all with their
 *      own effect logs so order/laziness bugs surface inside them too.
 *
 * Deterministic: program i is generated from mulberry32(FUZZ_SEED ^ mix(i)),
 * so a failure reproduces from (FUZZ_SEED, index) alone.
 *
 * Deliberately NOT generated: Date.now, Math.random, setTimeout/host races,
 * regex, unbounded loops, Symbol-as-value, var. (Symbol.iterator is used only
 * as a computed method key on a custom iterable, never as a first-class value.)
 *
 * Run: npm run test:fuzz            (300 programs, seed 1)
 *      FUZZ_SEED=7 FUZZ_COUNT=1000 node tests/fuzz-differential.mjs
 */
import { runBoth, isDivergent } from "./diff-harness-lib.mjs";

const SEED = Number(process.env.FUZZ_SEED ?? 1);
const COUNT = Number(process.env.FUZZ_COUNT ?? 300);
const MAX_MINIMIZED = Number(process.env.FUZZ_MAX_MINIMIZED ?? 10);

// ── deterministic PRNG ──────────────────────────────────────────────────────

function mulberry32(seed) {
  let a = seed >>> 0;
  return function () {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

// ── program generator ───────────────────────────────────────────────────────

const WORDS = ["fox", "bar", "zap", "Quu", "k9", "wide net", "Ada", "omega"];
const KEYS = ["a", "b", "c", "d", "e"];

class Gen {
  constructor(rng) {
    this.rng = rng;
    this.n = 0;
    this.vars = []; // { name, type, mutable } — types: num str bool arrnum arrstr obj
    this.fns = []; // { name, sig } — sig: 'num' | 'str' (unary, same in/out type)
    this.stmts = [];
  }
  pick(arr) {
    return arr[Math.floor(this.rng() * arr.length)];
  }
  chance(p) {
    return this.rng() < p;
  }
  int(lo, hi) {
    return lo + Math.floor(this.rng() * (hi - lo + 1));
  }
  fresh(prefix = "v") {
    return `${prefix}${this.n++}`;
  }
  varsOf(...types) {
    return this.vars.filter((v) => types.includes(v.type));
  }

  // ── expressions, type-directed, depth-bounded ─────────────────────────────

  numExpr(d) {
    const opts = [() => String(this.int(-20, 20)), () => String(this.int(-81, 81) / 4)];
    const nv = this.varsOf("num");
    if (nv.length) opts.push(() => this.pick(nv).name, () => this.pick(nv).name);
    if (d > 0) {
      const op = () => this.pick(["+", "-", "*", "%", "/"]);
      opts.push(() => `(${this.numExpr(d - 1)} ${op()} ${this.numExpr(d - 1)})`);
      opts.push(
        () =>
          `Math.${this.pick(["floor", "ceil", "round", "trunc", "abs", "sign"])}(${this.numExpr(d - 1)})`
      );
      opts.push(
        () => `Math.${this.pick(["max", "min"])}(${this.numExpr(d - 1)}, ${this.numExpr(d - 1)})`
      );
      opts.push(() => `(${this.boolExpr(d - 1)} ? ${this.numExpr(d - 1)} : ${this.numExpr(d - 1)})`);
      const anyArr = this.varsOf("arrnum", "arrstr");
      if (anyArr.length) opts.push(() => `${this.pick(anyArr).name}.length`);
      const an = this.varsOf("arrnum");
      if (an.length) opts.push(() => `${this.pick(an).name}.reduce((t, x) => t + x, 0)`);
      const sv = this.varsOf("str");
      if (sv.length) opts.push(() => `${this.pick(sv).name}.length`);
      const fs = this.fns.filter((f) => f.sig === "num");
      if (fs.length) opts.push(() => `${this.pick(fs).name}(${this.numExpr(d - 1)})`);
    }
    return this.pick(opts)();
  }

  strLit() {
    return JSON.stringify(this.pick(WORDS));
  }

  strExpr(d) {
    const opts = [() => this.strLit()];
    const sv = this.varsOf("str");
    if (sv.length) opts.push(() => this.pick(sv).name, () => this.pick(sv).name);
    if (d > 0) {
      opts.push(() => `(${this.strExpr(d - 1)} + ${this.chance(0.5) ? this.strExpr(d - 1) : this.numExpr(d - 1)})`);
      opts.push(() => `\`\${${this.numExpr(d - 1)}}-\${${this.strExpr(d - 1)}}\``);
      opts.push(
        () => `(${this.strExpr(d - 1)}).${this.pick(["toUpperCase()", "toLowerCase()", "trim()"])}`
      );
      opts.push(() => `(${this.strExpr(d - 1)}).slice(${this.int(0, 3)}, ${this.int(-2, 6)})`);
      opts.push(() => `(${this.strExpr(d - 1)}).repeat(${this.int(0, 3)})`);
      opts.push(() => `(${this.strExpr(d - 1)}).padStart(${this.int(0, 8)}, "*")`);
      opts.push(() => `(${this.strExpr(d - 1)}).replaceAll("a", "_")`);
      opts.push(() => `(${this.strExpr(d - 1)}).split("").reverse().join("")`);
      opts.push(() => `String(${this.numExpr(d - 1)})`);
      opts.push(() => `(${this.numExpr(d - 1)}).toFixed(${this.int(0, 3)})`);
      const av = this.varsOf("arrnum", "arrstr");
      if (av.length) opts.push(() => `${this.pick(av).name}.join(${this.strLit()})`);
      const ov = this.varsOf("obj");
      if (ov.length) {
        opts.push(
          () => `Object.entries(${this.pick(ov).name}).map(([k, v]) => k + "=" + v).join("&")`
        );
        opts.push(() => `Object.values(${this.pick(ov).name}).join(",")`);
        opts.push(() => `JSON.stringify(${this.pick(ov).name})`);
      }
      const fs = this.fns.filter((f) => f.sig === "str");
      if (fs.length) opts.push(() => `${this.pick(fs).name}(${this.strExpr(d - 1)})`);
    }
    return this.pick(opts)();
  }

  boolExpr(d) {
    const opts = [() => this.pick(["true", "false"])];
    const bv = this.varsOf("bool");
    if (bv.length) opts.push(() => this.pick(bv).name);
    if (d > 0) {
      const cmp = () => this.pick(["<", "<=", ">", ">=", "===", "!=="]);
      opts.push(() => `(${this.numExpr(d - 1)} ${cmp()} ${this.numExpr(d - 1)})`);
      opts.push(() => `(${this.strExpr(d - 1)} ${this.pick(["===", "!==", "<", ">"])} ${this.strExpr(d - 1)})`);
      opts.push(() => `(${this.strExpr(d - 1)}).includes(${this.strLit()})`);
      opts.push(() => `(${this.boolExpr(d - 1)} ${this.pick(["&&", "||"])} ${this.boolExpr(d - 1)})`);
      opts.push(() => `!(${this.boolExpr(d - 1)})`);
      opts.push(() => `Number.isInteger(${this.numExpr(d - 1)})`);
      const an = this.varsOf("arrnum");
      if (an.length) opts.push(() => `${this.pick(an).name}.includes(${this.int(-5, 5)})`);
    }
    return this.pick(opts)();
  }

  scalarExpr(d) {
    const t = this.pick(["num", "num", "str", "str", "bool"]);
    return { type: t, src: this.expr(t, d) };
  }

  arrLit(elemType, d) {
    const len = this.int(0, 4);
    const elems = [];
    for (let i = 0; i < len; i++)
      elems.push(elemType === "num" ? this.numExpr(d) : this.strLit());
    return `[${elems.join(", ")}]`;
  }

  arrExpr(elemType, d) {
    // elemType: 'num' | 'str'; returns arr-typed expression source
    const vtype = elemType === "num" ? "arrnum" : "arrstr";
    const opts = [() => this.arrLit(elemType, Math.max(0, d - 1))];
    const av = this.varsOf(vtype);
    if (av.length) opts.push(() => this.pick(av).name, () => this.pick(av).name);
    if (d > 0) {
      const base = () => `(${this.arrExpr(elemType, d - 1)})`;
      if (elemType === "num") {
        opts.push(() => `${base()}.map((x) => ${this.pick(["x * 2", "x + 1", "Math.abs(x)", "x % 3", "x - 4"])})`);
        opts.push(() => `${base()}.filter((x) => x ${this.pick(["<", ">", "<=", "%2 ==="])} ${this.int(-3, 3)})`);
        opts.push(() => `[...${base()}].sort((p, q) => p - q)`);
        opts.push(() => `[...${base()}].sort()`);
        opts.push(() => `[...${base()}, ${this.numExpr(d - 1)}]`);
        opts.push(() => `Array.from({ length: ${this.int(0, 4)} }, (_, i) => i ${this.pick(["*", "+", "-"])} ${this.int(1, 3)})`);
      } else {
        opts.push(() => `${base()}.map((s) => s + ${this.strLit()})`);
        opts.push(() => `${base()}.filter((s) => s.length ${this.pick(["<", ">", "==="])} ${this.int(1, 5)})`);
        opts.push(() => `[...${base()}].sort()`);
        const ov = this.varsOf("obj");
        if (ov.length) opts.push(() => `Object.keys(${this.pick(ov).name})`);
      }
      opts.push(() => `${base()}.slice(${this.int(0, 2)}, ${this.int(-1, 4)})`);
      opts.push(() => `${base()}.concat(${this.arrLit(elemType, 0)})`);
    }
    return this.pick(opts)();
  }

  objExpr(d) {
    // returns { src, keys: {key: scalarType} }
    const ov = this.varsOf("obj");
    if (ov.length && d > 0 && this.chance(0.3)) {
      const base = this.pick(ov);
      if (this.chance(0.4)) {
        return { src: `JSON.parse(JSON.stringify(${base.name}))`, keys: { ...base.keys } };
      }
      const extra = this.pick(KEYS);
      const { type, src } = this.scalarExpr(d - 1);
      return {
        src: `{ ...${base.name}, ${extra}: ${src} }`,
        keys: { ...base.keys, [extra]: type },
      };
    }
    const nKeys = this.int(1, 3);
    const keys = {};
    const parts = [];
    const pool = [...KEYS];
    for (let i = 0; i < nKeys; i++) {
      const k = pool.splice(Math.floor(this.rng() * pool.length), 1)[0];
      const { type, src } = this.scalarExpr(Math.max(0, d - 1));
      keys[k] = type;
      parts.push(`${k}: ${src}`);
    }
    return { src: `{ ${parts.join(", ")} }`, keys };
  }

  expr(type, d) {
    if (type === "num") return this.numExpr(d);
    if (type === "str") return this.strExpr(d);
    if (type === "bool") return this.boolExpr(d);
    throw new Error(`expr: ${type}`);
  }

  // ── statements ────────────────────────────────────────────────────────────

  declare(type, src, extra = {}) {
    const name = this.fresh();
    const mutable = this.chance(0.5);
    this.stmts.push(`${mutable ? "let" : "const"} ${name} = ${src};`);
    this.vars.push({ name, type, mutable, ...extra });
    return name;
  }

  stmtDecl() {
    const kind = this.pick(["num", "num", "str", "str", "bool", "arrnum", "arrstr", "obj"]);
    if (kind === "arrnum") return this.declare("arrnum", this.arrExpr("num", 2));
    if (kind === "arrstr") return this.declare("arrstr", this.arrExpr("str", 2));
    if (kind === "obj") {
      const { src, keys } = this.objExpr(2);
      return this.declare("obj", src, { keys });
    }
    return this.declare(kind, this.expr(kind, 2));
  }

  mutableScalar() {
    return this.varsOf("num", "str").filter((v) => v.mutable);
  }

  stmtReassign() {
    const v = this.pick(this.mutableScalar());
    const op = this.chance(0.5) ? "+=" : "=";
    this.stmts.push(`${v.name} ${op} ${this.expr(v.type, 2)};`);
  }

  stmtIf() {
    const v = this.pick(this.mutableScalar());
    const thenS = `${v.name} += ${this.expr(v.type, 1)};`;
    const elseS = `${v.name} = ${this.expr(v.type, 1)};`;
    this.stmts.push(
      this.chance(0.4)
        ? `if (${this.boolExpr(2)}) { ${thenS} }`
        : `if (${this.boolExpr(2)}) { ${thenS} } else { ${elseS} }`
    );
  }

  stmtFor() {
    const v = this.pick(this.mutableScalar());
    const i = this.fresh("i");
    this.vars.push({ name: i, type: "num", mutable: false });
    const body = `${v.name} += ${this.expr(v.type, 1)};`;
    this.vars.pop();
    this.stmts.push(`for (let ${i} = 0; ${i} < ${this.int(1, 4)}; ${i}++) { ${body} }`);
  }

  stmtForOf() {
    const arr = this.pick(this.varsOf("arrnum", "arrstr"));
    const acc = this.pick(this.mutableScalar());
    const it = this.fresh("it");
    this.vars.push({ name: it, type: arr.type === "arrnum" ? "num" : "str", mutable: false });
    const body = `${acc.name} += ${this.expr(acc.type, 1)};`;
    this.vars.pop();
    this.stmts.push(`for (const ${it} of ${arr.name}) { ${body} }`);
  }

  stmtWhile() {
    const acc = this.pick(this.mutableScalar());
    const c = this.fresh("c");
    this.stmts.push(
      `let ${c} = 0; while (${c} < ${this.int(1, 4)}) { ${acc.name} += ${this.expr(acc.type, 1)}; ${c} += 1; }`
    );
  }

  stmtPush() {
    const arr = this.pick(this.varsOf("arrnum", "arrstr"));
    const elem = arr.type === "arrnum" ? this.numExpr(1) : this.strExpr(1);
    this.stmts.push(`${arr.name}.push(${elem});`);
  }

  stmtFn() {
    const name = this.fresh("f");
    if (this.chance(0.3)) {
      // closure via maker
      const mk = this.fresh("mk");
      this.stmts.push(
        `const ${mk} = (m) => (x) => x * m + ${this.int(-3, 3)}; const ${name} = ${mk}(${this.int(-2, 4)});`
      );
      this.fns.push({ name, sig: "num" });
      return;
    }
    const sig = this.chance(0.6) ? "num" : "str";
    this.vars.push({ name: "x", type: sig, mutable: false });
    const body = this.expr(sig, 2);
    this.vars.pop();
    this.stmts.push(
      this.chance(0.5)
        ? `const ${name} = (x) => ${body.startsWith("{") ? `(${body})` : body};`
        : `function ${name}(x) { return ${body}; }`
    );
    this.fns.push({ name, sig });
  }

  stmtTryCatch() {
    const name = this.fresh("t");
    this.stmts.push(
      `let ${name}; try { if (${this.boolExpr(2)}) { throw new Error(${this.strLit()}); } ${name} = "ok:" + ${this.strExpr(1)}; } catch (e) { ${name} = "err:" + e.message; }`
    );
    this.vars.push({ name, type: "str", mutable: true });
  }

  stmtDestructureArr() {
    const arr = this.pick(this.varsOf("arrnum", "arrstr"));
    const t = arr.type === "arrnum" ? "num" : "str";
    const a = this.fresh("d");
    const b = this.fresh("d");
    const r = this.fresh("r");
    const dflt = () => (t === "num" ? String(this.int(-9, 9)) : this.strLit());
    this.stmts.push(`const [${a} = ${dflt()}, ${b} = ${dflt()}, ...${r}] = ${arr.name};`);
    this.vars.push({ name: a, type: t, mutable: false });
    this.vars.push({ name: b, type: t, mutable: false });
    this.vars.push({ name: r, type: arr.type, mutable: false });
  }

  stmtDestructureObj() {
    const obj = this.pick(this.varsOf("obj"));
    const known = Object.keys(obj.keys);
    const k = this.chance(0.7) ? this.pick(known) : this.pick(KEYS);
    const t = obj.keys[k] ?? "num";
    const name = this.fresh("o");
    const dflt = t === "num" ? String(this.int(-9, 9)) : t === "str" ? this.strLit() : "false";
    this.stmts.push(`const { ${k}: ${name} = ${dflt} } = ${obj.name};`);
    this.vars.push({ name, type: t, mutable: false });
  }

  stmtJsonRoundTrip() {
    const src = this.pick(this.varsOf("obj", "arrnum", "arrstr"));
    const name = this.fresh("j");
    this.stmts.push(`const ${name} = JSON.parse(JSON.stringify(${src.name}));`);
    this.vars.push({ name, type: src.type, mutable: false, keys: src.keys && { ...src.keys } });
  }

  stmtAwaitResolve() {
    const { type, src } = this.scalarExpr(2);
    const name = this.fresh("p");
    this.stmts.push(`const ${name} = await Promise.resolve(${src});`);
    this.vars.push({ name, type, mutable: false });
  }

  stmtPromiseAll() {
    const name = this.fresh("q");
    const mk = () =>
      this.pick([
        () => `Promise.resolve(${this.numExpr(1)})`,
        () => `(async () => ${this.numExpr(1)})()`,
        () => this.numExpr(1), // plain value mixed in
      ])();
    const n = this.int(1, 3);
    const items = Array.from({ length: n }, mk);
    this.stmts.push(`const ${name} = await Promise.all([${items.join(", ")}]);`);
    this.vars.push({ name, type: "arrnum", mutable: false });
  }

  stmtAllSettled() {
    const name = this.fresh("s");
    const items = [
      `Promise.resolve(${this.numExpr(1)})`,
      this.chance(0.6)
        ? `Promise.reject(new Error(${this.strLit()}))`
        : `Promise.resolve(${this.strExpr(1)})`,
    ];
    this.stmts.push(
      `const ${name} = (await Promise.allSettled([${items.join(", ")}])).map((r) => r.status + ":" + (r.value ?? r.reason?.message ?? "")).join("|");`
    );
    this.vars.push({ name, type: "str", mutable: false });
  }

  stmtAsyncFn() {
    const f = this.fresh("af");
    const name = this.fresh("r");
    this.vars.push({ name: "x", type: "num", mutable: false });
    const body = this.numExpr(2);
    this.vars.pop();
    this.stmts.push(
      `async function ${f}(x) { await null; return ${body}; } const ${name} = await ${f}(${this.numExpr(1)});`
    );
    this.vars.push({ name, type: "num", mutable: false });
  }

  // ── program assembly ──────────────────────────────────────────────────────

  generate() {
    const total = this.int(6, 14);
    this.stmtDecl();
    this.stmtDecl();
    while (this.stmts.length < total) {
      const opts = [() => this.stmtDecl()];
      if (this.mutableScalar().length) {
        opts.push(
          () => this.stmtReassign(),
          () => this.stmtIf(),
          () => this.stmtFor(),
          () => this.stmtWhile()
        );
        if (this.varsOf("arrnum", "arrstr").length) opts.push(() => this.stmtForOf());
      }
      if (this.varsOf("arrnum", "arrstr").length) {
        opts.push(() => this.stmtPush(), () => this.stmtDestructureArr());
      }
      if (this.varsOf("obj").length) opts.push(() => this.stmtDestructureObj());
      if (this.varsOf("obj", "arrnum", "arrstr").length) opts.push(() => this.stmtJsonRoundTrip());
      opts.push(
        () => this.stmtFn(),
        () => this.stmtTryCatch(),
        () => this.stmtAwaitResolve(),
        () => this.stmtPromiseAll(),
        () => this.stmtAllSettled(),
        () => this.stmtAsyncFn()
      );
      this.pick(opts)();
    }
    // summarize state: return an object of (up to 8) declared variables
    const candidates = this.vars.map((v) => v.name);
    const retVars = candidates.slice(-8);
    return { stmts: this.stmts, retVars };
  }
}

function renderBody(stmts, retVars) {
  const ret = retVars.length ? `return { ${retVars.join(", ")} };` : "return 0;";
  return [...stmts, ret].join("\n");
}

function programFor(seed, index) {
  const rng = mulberry32((seed ^ Math.imul(index + 1, 0x9e3779b9)) >>> 0);
  return new Gen(rng).generate();
}

// ── minimizer: greedy statement drop, then return-field shrink ──────────────

async function minimize(prog) {
  let { stmts, retVars } = prog;
  let changed = true;
  while (changed) {
    changed = false;
    for (let i = stmts.length - 1; i >= 0; i--) {
      const cand = stmts.slice(0, i).concat(stmts.slice(i + 1));
      if (isDivergent(await runBoth(renderBody(cand, retVars)))) {
        stmts = cand;
        changed = true;
      }
    }
    for (let i = retVars.length - 1; i >= 0 && retVars.length > 1; i--) {
      const cand = retVars.slice(0, i).concat(retVars.slice(i + 1));
      if (isDivergent(await runBoth(renderBody(stmts, cand)))) {
        retVars = cand;
        changed = true;
      }
    }
  }
  return renderBody(stmts, retVars);
}

// ── main loop ───────────────────────────────────────────────────────────────

const show = (r) =>
  r.nodeErr !== undefined || r.zapErr !== undefined
    ? `node=${JSON.stringify(r.nodeErr ?? r.node)} zapcode=${JSON.stringify(r.zapErr ?? r.zapcode)}`
    : `node=${JSON.stringify(r.node)} zapcode=${JSON.stringify(r.zapcode)}`;

let passed = 0;
const failures = [];

for (let i = 0; i < COUNT; i++) {
  const prog = programFor(SEED, i);
  const body = renderBody(prog.stmts, prog.retVars);
  const res = await runBoth(body);
  if (isDivergent(res)) failures.push({ i, prog, body, res });
  else passed++;
}

console.log(`fuzz-differential: seed=${SEED} programs=${COUNT} pass=${passed} fail=${failures.length}`);

for (const f of failures) {
  console.error(`\n━━━ DIVERGENCE program #${f.i} (FUZZ_SEED=${SEED}) ━━━`);
  console.error(f.body);
  console.error(`>> ${show(f.res)}`);
}

if (failures.length) {
  console.error(`\n━━━ minimized repros (first ${Math.min(failures.length, MAX_MINIMIZED)}) ━━━`);
  for (const f of failures.slice(0, MAX_MINIMIZED)) {
    const min = await minimize(f.prog);
    const res = await runBoth(min);
    console.error(`\n# program #${f.i} minimized:`);
    console.error(min);
    console.error(`>> ${show(res)}`);
  }
  process.exit(1);
}
