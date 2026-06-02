# Zapcode interpreter — divergences from real JS/TS (stress-pass findings)

## Fix status (in progress)

**Round 2 (branch `arca/heap-handles-rewrite`) — Tier A complete + most of Tier B:**
- **J4** nested `for…of`; **D1/D2** function hoisting; **C4** caught runtime
  errors are real `Error` objects; **B1** trailing-block completion values;
  **E1/E2** optional-chaining short-circuit (calls + trailing members).
- **M (Dates)** string/multi-arg construction, `Date.parse`/`Date.UTC`,
  arithmetic & coercion, Invalid Date, `instanceof Date`, `toJSON`/`toString`.
- **C1/C2** `instanceof` ancestor classes + implicit constructor → `super`.
- **N7** `AggregateError` global.
- Deferred as deep follow-ups (architectural, do dedicated): **A** reference
  semantics (the heap-with-handles rewrite — chosen, not yet done); **N1–N5/N8/N9**
  Promise combinator semantics (eager-resolution suspend model + determinism);
  **C3** `super.method()` (needs current-class tracking in frames).

Fixed and verified (cargo tests + full JS scenario suite, native binding rebuilt):

- **Cluster L (Tier 0 crashes):** L1/L2/L3 (BigInt/Infinity/NaN/undefined tool
  returns no longer abort the process — sanitized at the boundary), L5 (`tool({})`
  on a no-arg tool).
- **Coercion/operators:** O1 (string relational), O3 (`+` ToPrimitive), O6
  (array→string null/undefined), O10 (`Number(array)`), F2 (bitwise ToInt32), F5
  (`Number("0x"/"0b"/"0o"/"Infinity")`).
- **Number formatting:** F3 (toFixed rounding), F6 (toPrecision), F7
  (toExponential), F8 (toString radix fraction).
- **Strings:** G2 (template escapes), G5 (split limit + capture groups), G6
  (`$<name>` replacement), G10 (startsWith/endsWith position), G11 (substr,
  codePointAt, String.fromCodePoint).
- **Collections:** H1 (sort in place, guarded), H2 (`[...map]`), H4
  (`new Map(map)`), H5 (`flat(depth)`), H7 (NaN SameValueZero), H8 (fromIndex),
  H9 (`new Set(string)`), H11 (multi-key sort, via O1), H3 (array-rest
  destructuring).
- **JSON:** I1 (drop undefined), I2 (escape control chars), I3 (array replacer),
  M6 (Date→ISO; Map/Set/Error→`{}`).

Not yet addressed (larger / riskier — deferred): A (reference semantics — deep
architectural), B (try/finally completion values & override), C (classes/super/
runtime-error fields), D (function hoisting, call/apply/bind, destructured-param
defaults, arguments), E (optional-chaining call/trailing short-circuit — needs
chain-level compilation), most of M (Date parsing/now/arithmetic/mutators), N
(Promise.race/any/allSettled + deferred-promise semantics), J (iteration/enum
order, nested for-of), O4/O8 (valueOf hooks, Symbol), O5 (`in` string key), G3/G4
(regex lastIndex/exec loop, match groups), I4 (toJSON), L4/L6–L9 (error-as-Error,
Infinity-arg message, etc.), F1/F4 (large-int precision / Infinity output),
H6/H10. The notes below are the original findings (unchanged).

---


Compiled from two subagent stress passes against `packages/zapcode-ai/dist/index.js`
(the built interpreter on branch `arca/hardening-evals`). Every entry was verified by
running the **identical** snippet through both `zapcode.execute(code, {})` and real
Node, and confirming the outputs differ. Repros are the exact code string passed to
`execute`; "Expected" is real Node, "Actual" is zapcode.

These are **pre-existing** interpreter behaviors uncovered by the stress tests — not
regressions introduced by the optional-member/index fix in this PR. (That fix is in a
different code path; see Cluster E for the optional-chaining gaps that remain.)

Severity key: **HIGH** = silently wrong result in ordinary agent code · **MED** = wrong
on edge cases · **LOW** = throws-instead-of-value, or rare.

---

## Cluster A — Reference / identity semantics  *(root cause of several others)*

**A1 [HIGH] Objects/arrays are copied on bind / assign / param-pass / for-of — no shared references.**
`===` identity fails and all aliasing/mutation-through-another-name is lost.
- `const a=[1,2]; a===a` → exp `true`, act `false`. (`{}===` self, `fn===` self also `false`.)
- `const a=[1]; const b=a; b.push(9); JSON.stringify(a)` → exp `[1,9]`, act `[1]`.
- `const a=[1]; const f=(x)=>x.push(2); f(a); JSON.stringify(a)` → exp `[1,2]`, act `[1]`.
- `const a=[{n:1}]; for(const x of a) x.n*=10; JSON.stringify(a)` → exp `[{"n":10}]`, act `[{"n":1}]`.
- `const m=new Map(); m.set('k',[1]); const r=m.get('k'); r.push(2); JSON.stringify(m.get('k'))` → exp `[1,2]`, act `[1]`.
- Mutation through the **original** binding (`const a=[1];a.push(2)`, `o.x=9`, `a[0].n=9`) works — only cross-name sharing is broken. **Breaks the ubiquitous "build a Map of arrays, grab the bucket once, push into it" idiom.**

**A2 [MED] `Map` object-key identity broken** (consequence of A1): `const k={}; const m=new Map(); m.set(k,9); m.get(k)` → exp `9`, act `null`.

**A3 [MED] `Object.assign` doesn't mutate its target and returns a new object** (consequence of A1):
`const t={a:1}; const r=Object.assign(t,{b:2},{a:9}); JSON.stringify([t,r===t])` → exp `[{"a":9,"b":2},true]`, act `[{"a":1},false]`.

**A4 [MED] `Object.freeze` doesn't freeze** (returns a non-mutating copy; mutation still succeeds):
`const o=Object.freeze({a:1}); o.a=99; o.a` → exp `1`, act `99`.

---

## Cluster B — `try`/`finally` and statement completion values

**B1 [HIGH] Program's last value is `null` when the final statement is a block (`try`/`if`/`for`/`while`/`switch`).**
Real JS propagates the block's completion value. **Silently nulls the very common pattern of ending a script with a `try/catch`.**
- `try { JSON.parse("{bad"); } catch (e) { e.message }` → exp the SyntaxError message, act `null`.
- `if (true) { 42 }` → exp `42`, act `null`.  ·  `for (let i=0;i<3;i++){ i }` → exp `2`, act `null`.  ·  `switch(1){ case 1: 99; }` → exp `99`, act `null`.
- Side effects inside the block still run; only the completion value is dropped.

**B2 [HIGH] `finally` cannot override the completion of `try`/`catch`.** A `return`/`break`/`throw` in `finally` is ignored (backwards from JS, where `finally` wins).
- `(function(){ try { return "try"; } finally { return "finally"; } })()` → exp `"finally"`, act `"try"`.
- `(function(){ try { throw new Error("x"); } catch(e){ return "catch"; } finally { return "fin"; } })()` → exp `"fin"`, act `"catch"`.
- `try` returning while `finally{ throw }` should surface the finally-throw — it's swallowed instead.
- (`continue` in `finally` *does* correctly swallow a pending exception.)

---

## Cluster C — Classes / errors / prototype chain

**C1 [HIGH] `instanceof` against a parent class is always `false`** (proto chain stops at the subclass).
`class E extends Error{constructor(m){super(m)}} new E("x") instanceof Error` → exp `true`, act `false`. **Breaks `catch(e){ if (e instanceof Error) }` for any custom error.**

**C2 [HIGH] Implicit subclass constructor doesn't forward args to `super`.**
`class A{constructor(x){this.x=x}} class B extends A{} new B(7).x` → exp `7`, act `null`. (`class E extends Error{}` ⇒ `.message` empty.) Works with an explicit `constructor(x){super(x)}`.

**C3 [HIGH] `super.method()` / `super.prop` throws** (`super` is undefined inside methods).
`class A{g(){return 1}} class B extends A{g(){return super.g()+10}} new B().g()` → exp `11`, act throws `Cannot read properties of undefined (reading 'g')`. (Plain inherited methods without `super` work.)

**C4 [HIGH] Runtime-thrown errors carry no `.name`/`.message` and fail `instanceof` in `catch`.**
`try{null.x}catch(e){e.name}` → exp `"TypeError"`, act `null`; `…e.message` → exp non-empty, act `null`; `…e instanceof TypeError` → exp `true`, act `null`.
(Note: user-built `new Error("m")` *does* expose `.message`/`.name` correctly — the gap is specifically host/runtime-thrown errors.)

---

## Cluster D — Functions & parameters

**D1 [HIGH] Function declarations nested inside another function/arrow/IIFE are not bound** → `undefined is not a function`.
`function outer(){function inner(){return 5} return inner()} outer()` → exp `5`, act throws. **Breaks helper-inside-helper code (deep-merge, recursive transforms).** Top-level decls are fine.

**D2 [HIGH] Forward references to top-level function declarations aren't hoisted.**
`f(); function f(){return 1}` → exp runs, act throws `undefined is not a function`.

**D3 [HIGH] `Function.prototype.call` / `apply` / `bind` don't exist.**
`function f(){return this.n} f.call({n:7})` → exp `7`, act throws `Cannot read properties of undefined (reading 'call')`. Same for `apply`/`bind`.

**D4 [MED] Destructured-parameter defaults yield `null`.**
`function f({a=1,b=2}={}){return a+b} f({a:10})` → exp `12`, act `null`.
Also array-destructuring defaults: `const [a=10,b=20]=[1]` ⇒ `b` stays `undefined`.
**NOTE / reconcile:** plain scalar defaults (`function f(a,b=5){return a+b}; f(1)` → `6`) and defaults referencing earlier params (`b=a*2`) were verified **working** by the objects-agent. (Pass-1 statemachine agent reported "default params don't apply" — that appears to have been the destructured/array form, or a stale probe. Worth a definitive recheck when fixing.)

**D5 [LOW] `arguments` object unsupported.** `function f(){return arguments.length} f(1,2,3)` → exp `3`, act throws.

---

## Cluster E — Optional chaining on a nullish receiver  *(adjacent to this PR's fix)*

Plain property/index short-circuit correctly; **every call/method link throws**, and a
non-optional member *after* a short-circuited optional also throws instead of
short-circuiting the whole chain.

| expression (head nullish) | expected | actual | ok? |
|---|---|---|---|
| `null?.b`, `null?.[k]`, `null?.b?.c` | `undefined` | `undefined` | ✅ |
| `null?.f()` / `undefined?.f()` | `undefined` | throws `undefined is not a function` | ❌ |
| `null?.()` | `undefined` | throws `null is not a function` | ❌ |
| `null?.at(0)` / `undefined?.at(-1)` | `undefined` | throws `undefined is not a function` | ❌ |
| `({})?.f?.()`, `({a:1})?.miss?.()` | `undefined` | throws `undefined is not a function` | ❌ |
| `({a:null})?.a?.b()` | `undefined` | throws `undefined is not a function` | ❌ |
| **`null?.b.c`** (non-opt member after opt) | `undefined` | throws `Cannot read properties of undefined (reading 'c')` | ❌ |

**E1 [HIGH] Optional call/method on nullish throws** (e.g. the natural `rec?.geo?.at(-1)?.region ?? "x"` hard-throws instead of falling back).
**E2 [HIGH] `a?.b.c` doesn't short-circuit the trailing non-optional member.**

---

## Cluster F — Numbers & math

**F1 [HIGH] Large integers silently use BigInt internally** → numerically wrong vs IEEE-754 and unserializable.
- `9007199254740991 + 2` → exp `9007199254740992`, act `9007199254740993`.
- `Number.MAX_SAFE_INTEGER + 1` and factorials return a `bigint` whose `JSON.stringify` throws `Do not know how to serialize a BigInt`. `typeof` still reports `"number"`, hiding it.

**F2 [HIGH] Bitwise ops saturate at INT32_MAX instead of ToInt32 (mod-2³²) wraparound.**
`4294967296 | 0` → exp `0`, act `2147483647`; `4294967295 | 0` → exp `-1`, act `2147483647`; `0xFFFFFFFF ^ 0` → exp `-1`, act `2147483647`. (Operands < 2³¹ are correct.) **Breaks checksums/hashing/flag math.**

**F3 [HIGH] `toFixed` uses banker's rounding instead of half-away-from-zero** (financial).
`(2.5).toFixed(0)` → exp `"3"`, act `"2"`; `(0.5).toFixed(0)` → `"1"` vs `"0"`; `(0.125).toFixed(2)` → `"0.13"` vs `"0.12"`; `(-2.5).toFixed(0)` → `"-3"` vs `"-2"`.

**F4 [HIGH] `Infinity` / `-Infinity` / `NaN` marshalled to `null` when they are the output value.**
`1/0` → exp `Infinity`, act `null`; `0/0` → exp `NaN`, act `null`; `[1, 1/0, 3]` → exp `[1,Infinity,3]`, act `[1,null,3]`; `Math.max()` → exp `-Infinity`, act `null`. (Correct *inside* the sandbox — output-marshalling defect.)

**F5 [HIGH] `Number(string)` rejects forms Node accepts.** `Number("0x1F")` → exp `31`, act NaN/`null`; same for `"0b101"`(5), `"0o17"`(15), `"Infinity"`. `Number([5])` → exp `5`, act not-coerced.

**F6 [MED] `toPrecision` always outputs exponential form + wrong rounding.** `(123.456).toPrecision(4)` → exp `"123.5"`, act `"1.235e2"`; `(3).toPrecision(1)` → exp `"3"`, act `"3e0"`.

**F7 [LOW] `toExponential` not implemented** (throws).

**F8 [MED] `Number.prototype.toString(radix)` drops the fractional part.** `(3.5).toString(2)` → exp `"11.1"`, act `"11"`.

**F9 [MED] Value→string never switches to exponential at the boundaries.** `String(1e21)` → exp `"1e+21"`, act `"1000000000000000000000"`; `String(1e-7)` → exp `"1e-7"`, act `"0.0000001"`. (A bare `1e21` literal *does* print `"1e+21"` — inconsistent formatters.)

**F10 [MED] Loose `==` doesn't coerce arrays.** `[1] == 1` → exp `true`, act `false`; `[] == 0` → exp `true`, act `false`. (Primitive `==` coercions are correct.)

**F11 [LOW] Negative zero lost/printed as `0`** (`-1*0` → `0`); **`void 0` → `null`** instead of `undefined`; **BigInt literals `10n` rejected** by the parser (inconsistent, since ints already use BigInt internally per F1).

---

## Cluster G — Strings, regex, template literals

**G1 [HIGH] Function replacer in `replace`/`replaceAll` is never invoked — inserts literal `"function"`.**
`"hello world".replace(/\b\w/g, c=>c.toUpperCase())` → exp `"Hello World"`, act `"functionello functionorld"`. **Breaks title-case, camel↔snake, redaction — almost every cleanup routine.**

**G2 [HIGH] Template literals don't process backslash escapes** (`\n`, `\t`, `\uXXXX`, `\\` kept literal; double-quoted strings are fine).
`` `a\nb`.length `` → exp `3`, act `4`; `` `aAb` `` → exp `"aAb"`, act `"aAb"`.

**G3 [HIGH] Global-regex `lastIndex` not maintained → `while((m=re.exec(s)))` loops forever** (`allocation limit exceeded`).
`const r=/a/g; r.test("aaa"); r.lastIndex` → exp `1`, act `null`.

**G4 [HIGH] `match()`/`matchAll` results lack `.index` / `.input` / named `.groups`.**
`"xxabc".match(/abc/).index` → exp `2`, act `null`; `"12-34".match(/(?<a>\d+)-(?<b>\d+)/).groups` → exp `{a,b}`, act `undefined`.

**G5 [MED] `split` ignores the `limit` arg and drops regex capture groups.**
`"a,b,c".split(",",2)` → exp `["a","b"]`, act `["a","b","c"]`; `"a1b2c".split(/(\d)/)` → exp `["a","1","b","2","c"]`, act `["a","b","c"]`.

**G6 [MED] `$<name>`, `` $` ``, `$'` replacement patterns emitted literally** (only `$1`/`$&`/`$$` work).
`"2020-01".replace(/(?<y>\d+)-(?<m>\d+)/,"$<m>/$<y>")` → exp `"01/2020"`, act `"$<m>/$<y>"`.

**G7 [MED] Regex backreferences (`\1`) rejected as a parse error.** `/(a)\1/.test("aa")` → exp `true`, act throws.

**G8 [LOW] `RegExp` constructor unavailable** (`typeof RegExp === "undefined"`) → no dynamic/variable-built patterns.

**G9 [MED] Strings indexed by code point, not UTF-16.** `"😀".length` → exp `2`, act `1`; `"😀".charCodeAt(0)` → exp `55357`, act `128512`. **G10 [MED]** `startsWith`/`endsWith` ignore the position arg.

**G11 [LOW] Missing/throwing string methods:** `substr`, `normalize`, `codePointAt`, `String.fromCodePoint` all throw (some despite `typeof === "function"`).

---

## Cluster H — Arrays, Map, Set

**H1 [HIGH] `Array.prototype.sort()` doesn't mutate in place** (returns sorted copy; original unchanged — unlike `reverse`/`splice`/`fill`/`copyWithin`, which mutate correctly).
`const a=[3,1,2]; a.sort((x,y)=>x-y); JSON.stringify(a)` → exp `[1,2,3]`, act `[3,1,2]`.

**H2 [HIGH] Spreading a Map (`[...map]`) throws `object is not iterable`** (Set spreads fine; `[...map.entries()]`/`Array.from(map)`/`for…of map` work). **Breaks `[...map].sort()` group-by/histogram/top-N.**

**H3 [HIGH] Array rest in destructuring yields `undefined`.** `const [a,...rest]=[1,2,3]; JSON.stringify([a,rest])` → exp `[1,[2,3]]`, act `[1,null]`. (Object rest `{x,...rest}` works.)

**H4 [HIGH] `new Map(existingMap)` produces an empty Map.** `new Map(new Map([['a',1]])).get('a')` → exp `1`, act `null`.

**H5 [HIGH] `flat(depth)` ignores depth > 1.** `[1,[2,[3]]].flat(2)` → exp `[1,2,3]`, act `[1,2,[3]]`; `flat(Infinity)` likewise. (`flat()`/`flat(0)` fine.)

**H6 [MED] `Map.set` / `Set.add` don't return the collection** (chaining broken). `m.set('a',1).set('b',2); m.size` → exp `2`, act `1`.

**H7 [MED] `Set`/`Map`/`includes` mishandle `NaN` (SameValueZero):** `new Set([NaN,NaN]).size` → exp `1`, act `2`; `new Map().set(NaN,9).get(NaN)` → exp `9`, act `null`; `[NaN].includes(NaN)` → exp `true`, act `false`.

**H8 [MED] `includes`/`indexOf` ignore `fromIndex`.** `[1,2,3].includes(1,1)` → exp `false`, act `true`; `[1,2,3,1].indexOf(1,-2)` → exp `3`, act `0`.

**H9 [MED] `new Set(string)` doesn't iterate chars.** `new Set('aab').size` → exp `2`, act `0`.

**H10 [MED] Array holes materialized as `undefined` instead of skipped.** `[1,,3].forEach(()=>c++)` visits 3 not 2; `[1,,3].join('-')` → exp `"1--3"`, act `"1-undefined-3"`; `[1,,3].indexOf(undefined)` → exp `-1`, act `1`.

**H11 [HIGH] (pass-1) Multi-key ternary `sort` comparator mis-orders.** `(a,b)=>a.p!==b.p ? a.p-b.p : (a.id<b.id?-1:1)` produces wrong order; a single composite-numeric comparator works. *(May share a root cause with H1.)*

---

## Cluster I — JSON serialization fidelity

**I1 [HIGH] `JSON.stringify` doesn't drop `undefined` — emits the bare token `undefined` → invalid JSON.**
`JSON.stringify({a:1,b:undefined,c:3})` → exp `{"a":1,"c":3}`, act `{"a":1,"b":undefined,"c":3}`; `[1,undefined,3]` → exp `[1,null,3]`, act `[1,undefined,3]`.

**I2 [HIGH] `JSON.stringify` doesn't escape control characters → invalid JSON.** `JSON.stringify("a\nb")` emits a literal newline instead of `\n`.

**I3 [MED] Replacer (array whitelist / function) ignored.** `JSON.stringify({a:1,b:2,c:3},["a","c"])` → exp `{"a":1,"c":3}`, act unchanged.

**I4 [MED] `toJSON()` hook ignored.** `JSON.stringify({toJSON(){return {x:1}}})` → exp `{"x":1}`, act `{"toJSON":undefined}`.

**I5 [MED] Circular references don't throw** (silently truncated). `const o={}; o.self=o; JSON.stringify(o)` → exp throws, act `{"self":{}}`.

---

## Cluster J — Enumeration & iteration order

**J1 [MED] Integer-like keys not ordered ascending-first in `Object.keys` / `for…in`.** `{2:"a",1:"b",10:"c",x:"d"}` keys → exp `1,2,10,x`, act `2,1,10,x`.

**J2 [MED] `for…in` over a sparse array visits holes.** `for(let i in [1,,3])` → exp `0,2`, act `0,1,2`.

**J3 [MED] `for…of` over an array snapshots length** (doesn't see live appends during iteration).

**J4 [HIGH] (pass-1) Nested `for…of` runs only the FIRST outer iteration** when the inner loop is also `for…of`. Reproduces without async. Indexed `for` nests fine. **Distinct from J3.**

---

## Cluster K — Durable sessions

**K1 [HIGH] (pass-1) Factory-local closure state is lost across `dump()`/`loadSession()`.**
A closure capturing `let n` inside a factory counts correctly in-chunk (`1,2,3`) but returns `null` after a dump/load boundary — the captured call-frame environment isn't serialized. **Top-level state bindings *do* persist**, so durable workflows must thread top-level state rather than closure-captured state.

---

## Verified-correct (no divergence found)

Probed and matched Node exactly — useful to avoid wasted effort:
- Strings: `slice`/`substring`/`at`/`padStart`/`padEnd`/`repeat`/`trim*`/`indexOf`/`includes`/`charAt`/`charCodeAt`(BMP)/`toUpperCase`/`toLowerCase`/`localeCompare`; `$1`/`$&`/`$$` patterns; `match` array *contents*; `test`; non-global `exec`; multiline/dotall/`\d\w\s`/alternation/anchors; template **expression** interpolation/nesting.
- Numbers: modulo (neg/float); `**` precedence/assoc; `++`/`--`; compound assignment incl. `&&=`/`||=`/`??=`; `Math.floor/ceil/round/trunc/sign/abs/sqrt/cbrt/hypot/log2/log10/exp/pow`; `parseInt`(radix)/`parseFloat`; `Number.isInteger/isNaN/isFinite`; `0.1+0.2` and its `.toFixed(2)`.
- Collections: array/object/Set/string spread; object destructuring (rename/nested/rest/default/param/skip); `reverse`/`splice`/`fill`/`copyWithin`/`push`/`pop`/`shift`/`unshift`/`reduceRight`/`findLast*`/`at`/`Array.from`(mapFn/Set/string/Map)/`Array.of`/`flatMap`/default `sort()`; Map construct-from-entries + `get/set/has/delete/clear/size` + iteration order; Set dedupe/order/`-0` normalization.
- Control flow: labeled `break`/`continue`; `switch` fallthrough/mid-default/strict-case/string discriminant; short-circuit side-effect skipping (`&&`/`||`/`??`/ternary/`if`); async error flow (throwing tool in `try/catch`; `Promise.all` member rejection catchable via inner `try/catch`; sequential awaits after a caught error); `do…while`; `while(true)+break`.
- Objects: spread override order; shorthand/computed/method-shorthand props; `Object.keys/values/entries/fromEntries` (string keys); `Object.hasOwn`/`hasOwnProperty`/`delete`; rest params; too-few/too-many args; closures; IIFE; plain & earlier-param-referencing defaults; method `this` & lexical-arrow `this`; `JSON` key order (string keys)/`null` retention/indentation/`parse` roundtrip.

---

## Suggested fix ordering (highest leverage first)

1. **A1 reference/identity semantics** — root cause of A2–A4 and contributes to H-cluster mutation bugs; biggest blast radius.
2. **B1 statement-completion-value as program output** — silently nulls scripts ending in `try/catch`/`if`/loop (extremely common agent shape).
3. **G1 function replacer** + **G2 template-literal escapes** — break the majority of text-processing agents.
4. **F4 Infinity/NaN→null** + **F1 BigInt-for-large-ints** + **F3 toFixed rounding** — numeric correctness/serialization.
5. **E1/E2 optional-chaining call & trailing-member short-circuit** — directly continues this PR's hardening theme.
6. **D1/D2 function hoisting** (nested + forward) — breaks multi-helper programs.
7. **C1–C4 class/error/proto chain** — breaks custom-error handling.
8. **H2 `[...map]`**, **H1 `sort` in place**, **H3 array rest**, **H5 `flat(depth)`** — common collection idioms.
9. **I1/I2 JSON `undefined`/control-char** — invalid-JSON output.
10. **J4 nested `for…of`** + **J1 key order** — iteration correctness.

---
---

# Pass 3 findings — Date/time, async combinators, durable sessions, coercion/operators, tool boundary

Same method (verified node-vs-zapcode, or reload-path-vs-single-shot, diffs). New clusters below.
**Several of these are more severe than anything in passes 1–2** — including two that crash or
abort the host process, and "always-false" / "always-null" operators that silently corrupt
extremely common code.

## Cluster L — Tool boundary: return-value marshalling can ABORT the host  *(most severe)*

**L1 [CRITICAL] A tool returning a `BigInt` panics Rust and kills the node process (SIGABRT, exit 134).**
Tool `execute: async () => 10n` ⇒ `thread '<unnamed>' panicked … serde.rs:45 not yet implemented; fatal runtime error … aborting`. Unrecoverable — not a catchable JS error; one misbehaving tool return takes down the whole host. (Note F1 makes large integer *computations* into BigInt, so a tool that returns `someBigIntResult` trips this without the author ever typing `n`.)

**L2 [HIGH] A tool returning `Infinity`/`NaN` (or one nested in an object/array) throws an *uncatchable* marshalling error that aborts execution.**
`execute: async () => Infinity` (or `({ok:true, score: Infinity})`) ⇒ `Failed to convert js number to serde_json::Number`, **not** caught by guest `try/catch`; the tool already ran (side effects happened) but the result can never reach the sandbox. Asymmetric with args (L6, where Infinity→null).

**L3 [HIGH] A tool returning `undefined` throws an uncatchable marshalling error.**
`execute: async () => undefined` ⇒ `undefined cannot be represented as a serde_json::Value`, aborts. **Void/side-effect-only tools ("save", "notify") cannot be `await`ed** unless they return something. An array containing `undefined` hits the same error.

**L4 [HIGH] All thrown tool errors are flattened to a string; `e instanceof Error` is always `false` in the guest `catch`.**
Even `throw new Error("x")` arrives with `typeof e === "string"`, `e.message === null`, `e instanceof Error === false`. `throw {code:"X", message:"m"}` ⇒ guest sees only `"m"` (`.code` lost); an object with no `message` ⇒ `toolCalls.error === "[object Object]"` (payload destroyed); `throw 42`→`"42"`, `throw null`→`"null"`. **Breaks the ubiquitous `error instanceof Error ? error.message : String(error)` pattern** (used in the support-triage scenario itself — the `instanceof` branch is never taken).

**L5 [MED] Param-less tool can't be called as `tool({})`** — rejected `received 1 positional arguments but expected 0`. LLMs frequently call no-arg tools with `{}`; `tool()` works. Likely false-positive validation failure.

**L6 [MED] `Infinity`/`NaN` args silently become `null`, then are rejected with a misleading "got null".**
`await echo({ n: Infinity })` ⇒ `parameter 'n' expected number, got null` for an arg the agent clearly wrote as a number. Large ints arrive as `bigint` ⇒ `expected number, got bigint`. (Asymmetric with returns, L2.)

**L7 [MED] Single-`object`-param tools misclassify the call object.** `await echo({ payload: 1, other: 2 })` ⇒ `unexpected parameter 'other'`: the heuristic flips between "this object IS the payload" and "this object holds named args" based on whether keys match the param name, so a payload containing a key named after the param is rejected.

**L8 [LOW] Per-parameter `description` is silently dropped from the generated system prompt** (only tool-level description + param type render). All per-arg guidance the integrator wrote (e.g. scheduling's "Priority from 1 high to 5 low") is invisible to the model. **L9 [LOW]** prompt shows `Promise<unknown>` while the type-check stub uses `Promise<any>` — minor mismatch.

*Observed `toolCalls` record shape (good):* `{ name, args:unknown[], input:Record, result?, error?:string }` — `input` is post-validation with optional params stripped, **key order preserved**, `result` absent on failure, `error` is a string, calls recorded in execution order, skipped-branch calls absent. (Validation messages, array-vs-object distinction, string/emoji/control-char arg round-trip all verified correct. No nested element/shape validation exists — `type:"array"`/`"object"` guarantees nothing about contents, by design.)

## Cluster M — Dates & time  *(only `new Date(integerMs)` + UTC getters + `toISOString` work)*

**M1 [HIGH] `new Date(isoString)` ignores the string — always yields epoch 0.** `new Date("2023-11-14T22:13:20Z").getTime()` → exp `1700000000000`, act `0`. Date-only and offset strings too. **Agents parse API/DB timestamps constantly.**

**M2 [HIGH] `new Date(y, m, d, …)` multi-arg constructor uses the first arg as ms and ignores the rest.** `new Date(2024,0,15).getFullYear()` → exp `2024`, act `1970`.

**M3 [HIGH] `new Date()` returns epoch 0, not wall-clock** (`(new Date()).toISOString()` → `"1970-01-01T00:00:00.000Z"`). Every "now"/overdue/duration calc silently uses 1970.

**M4 [HIGH] Date↔number coercion returns `null`** — `date2 - date1`, `+date`, `Number(date)`, and `<`/`>` comparisons all fail. `new Date(b) - new Date(a)` → exp ms diff, act `null`; `dA < dB` → `false`. (Workaround: `.getTime()` comparisons work.)

**M5 [HIGH] `Date.now`/`Date.parse`/`Date.UTC` and all `setX` mutators are uncallable, yet `typeof` reports `"function"`** — defeats feature-detect guards: `typeof Date.now === "function"` is `true`, then `Date.now()` throws. Same for `toJSON`/`toString`/`toDateString`/`getTimezoneOffset` (M8).

**M6 [HIGH] `JSON.stringify(date)` leaks the internal repr** `{"__date_ms__":N}` instead of an ISO string. Corrupts any serialized result.

**M7 [MED] Invalid Date silently becomes epoch 0, not `NaN`/Invalid** — `isNaN(new Date("garbage").getTime())` → exp `true`, act `false`; bad input treated as 1970. **M9 [MED]** `String(date)`/`""+date` → `"[object Object]"`. **M10 [MED]** `new Date(…) instanceof Date` → `false`. **M11 [LOW]** local accessors (`getHours` etc.) return UTC components (sandbox ≈ UTC).
*Good:* `new Date(ms)` + all `getUTC*` (incl. 0-indexed month, `getUTCDay`), `new Date(d.getTime()+86400000)`, `toISOString` full-precision, Dates inside arrays/objects accessed by method.

## Cluster N — Promise combinators  *(root cause: tool calls are eager values, not deferred Promises)*

**N1 [HIGH] `Promise.race` returns the first ARRAY ELEMENT, not the first to settle** (and runs serially). `Promise.race([delay(slow), delay(fast)])` → exp `"fast"`, act `"slow"`.

**N2 [HIGH] `Promise.any` returns element 0, doesn't skip rejections, and a rejecting element surfaces/short-circuits.** `Promise.any([fail(), delay(ok)])` → exp `"ok"`, act throws/`caught:undefined`.

**N3 [HIGH] `Promise.allSettled` throws (rejects) when an element rejects — violates its core never-rejects guarantee.** `Promise.allSettled([record(ok), fail(bad)])` → exp `[{fulfilled},{rejected}]`, act throws uncaught `external function error: bad`. **Breaks partial-failure aggregation.** It also runs serially (N6), unlike `Promise.all`.

**N4 [HIGH] Tool calls inside `.then`/`.catch`/`.finally` callbacks throw** `runtime error: cannot call an external function inside an array-callback method` (misleading message — it's a promise callback). **Blocks the idiomatic `primary().catch(() => fallback())` retry pattern.**

**N5 [MED] A tool-call expression is an eagerly-resolved value, not a Promise** — `const p = delay(…); typeof p` → exp `"object"`, act `"string"` (resolved value); `p.then` is `undefined`; the op runs to completion at the assignment. **Root cause of N1–N3, N6.** (`Promise.resolve(5)` *is* a real promise — specific to tool calls.) **N7 [MED]** `AggregateError` is undefined and `Promise.any`'s rejection carries no `.errors`/`.name`. **N8 [MED]** `Promise.resolve(thenable)` doesn't adopt the thenable. **N9 [LOW]** `for await…of` / async-generator *consumption* fails to parse (plain `function*` + `.next()` work).
*Good:* `Promise.all` parallelizes and preserves index order; `.then`/`.catch`/`.finally` chaining + value/promise unwrap; `await` non-promise; `allSettled` element shape `{status,value/reason}`.

## Cluster O — Coercion / operators  *(silent corruption of common code)*

**O1 [HIGH] String-vs-string relational comparison always returns `false`.** `"apple" < "banana"` → `false`; `"car" <= "car"` → `false`. **Every** string `<`/`>`/`<=`/`>=` is broken — silently corrupts sorting/filtering/branching on strings. (Number-vs-number, number-vs-string, and default `sort()` comparator all work — isolated to both-string operands.)

**O2 [HIGH] `+` with an `undefined` numeric operand returns `null` instead of `NaN`.** `5 + undefined` → exp `NaN`, act `null`. (`"x"+undefined` correctly gives `"xundefined"` — only the numeric branch.)

**O3 [HIGH] `+` with any object/array operand returns `null`** (no object→primitive). `[1,2]+[3]` → exp `"1,23"`, act `null`; `1+[2]`, `[]+{}`, `+[5]`, `+{}` all `null`.

**O4 [HIGH] `valueOf`/`toString`/`Symbol.toPrimitive` hooks ignored in coercion.** `({valueOf(){return 42}})+1` → exp `43`, act `null`; `({toString(){return "hi"}})+""` → exp `"hi"`, act `"[object Object]"`; `Symbol.toPrimitive` ⇒ error (Symbol global missing, O8).

**O5 [HIGH] `in` operator: string-literal left operand is a PARSE ERROR; inherited/`length` keys not seen.** `"a" in {a:1}` → exp `true`, act parse error `Unexpected token`; `"length" in [1,2]` → `false`.

**O6 [MED] `Array.join`/array→string doesn't coerce `null`/`undefined`/holes to `""`.** `[1,null,2,undefined,3].join(",")` → exp `"1,,2,,3"`, act `"1,null,2,undefined,3"`. **O7 [MED]** `.toString()` called explicitly on primitives/arrays/objects throws `undefined is not a function` (implicit template coercion works). **O8 [MED]** `Symbol` global missing (`typeof Symbol` → `"undefined"`). **O9 [MED]** `instanceof RegExp` false for a regex literal (other `instanceof` on builtins work). **O10 [MED]** `Number([5])`→`null` (array→number path).
*Good:* `==`/`===` across null/undefined/0/false/"0"/NaN; number & mixed relational; truthiness of `[]`/`{}`/`"0"`; `typeof` for all types except Symbol/bigint; comma operator; ternary/exponent assoc; `??` precedence; template interpolation of array/object/null.

## Cluster P — Durable sessions (additional)

**P1 [HIGH] A live generator object held in a top-level binding bricks the session.** A chunk like `function* g(){…} const it=g(); it.next().value` throws `snapshot error: cannot persist session global 'it': generators cannot be persisted` — and because the wrapper persists VM state at every chunk end, the session is then unusable (`dump()` and the next `runChunk` both throw). The same code runs fine under one-shot `execute`. (A generator *function* with no live instance dumps fine.)

**P2 [LOW] Re-declaring a `let`/`const` across chunks throws `already been declared in this session`,** while a single-shot `execute` of the concatenation (`let z=1; let z=2`) accepts it and yields `2`. A chunking-model decision (consistent with/without reload), but agents that regenerate a full program across chunks will hit it — worth documenting or making later chunks shadow.
*Good (verified round-trips correctly across dump/load):* Map/Set/Date/nested objects/large arrays(10k)/large strings(50k); closures over **top-level** state (read current mutated value after reload); class instances (fields+methods); stored Promises; `undefined`/`null`/`NaN`/`±Infinity`; per-chunk `toolCalls` (not cumulative); snapshot-at-tool-call threading; error-recovery to last good checkpoint; `dump()` stability/idempotent reload; per-chunk `inputs` as bare globals with conflict detection.

---

## Updated fix-priority (incorporating pass 3)

**Tier 0 — crashes / uncatchable (do first):** L1 (BigInt return → process abort), L2/L3 (Infinity/NaN/undefined return → uncatchable abort; blocks void tools), P1 (generator bricks session).
**Tier 1 — silent corruption of ubiquitous code:** A1 (reference semantics), O1 (string relational always-false), O2–O4 (`+`/coercion → null), B1 (block-stmt → null output), G1 (replace fn-replacer), G2 (template escapes), L4 (`instanceof Error` always false in catch).
**Tier 2 — major feature gaps:** M1–M6 (Date parsing/now/arith/serialize), N1–N5 (race/any/allSettled + tool-call-as-Promise + `.then` tool calls), F1/F3/F4 (BigInt ints / toFixed / Infinity→null), E1/E2 (optional-chaining calls).
**Tier 3:** D/C (function hoisting, call/apply/bind, class/super/error), H (sort/`[...map]`/rest/flat), I (JSON), J (iteration order), L5–L9/O5–O10 (validation & coercion edges).
