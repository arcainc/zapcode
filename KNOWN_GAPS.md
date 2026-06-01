# Known gaps & bugs (from the realistic-scenario stress pass)

> **Status: all items below have been fixed** (see GAPS_FIX_PLAN.md and the
> `tests/{receiver_writeback,regex,destructuring,data_structures,spread_and_throw,control_flow_extra,misc_builtins}.rs`
> regression suites). The original prioritized findings are kept below for the record.
> The only intentionally-unsupported regex constructs are lookaround / backreferences
> (the linear-time engine's limitation), which now raise a clear error.


A pass of 5 parallel agents wrote realistic agent-generated TypeScript across
data/ETL, date/time, text/regex, control-flow/errors, and durable-session
domains (~220 checks, in `packages/zapcode-ai/tests/scenarios-*.mjs`). This is
the deduplicated, prioritized result. "Corroborated" = independently hit by
multiple agents.

## ✅ Fixed in this pass
- **Spread** in array/object literals (`[...a, ...b]`, `{...o}`), string spread.
- **Parse**: `throw "literal";` and control-flow blocks ending a program.
- **Thrown-value fidelity**: `catch (e)` binds the original value (string/object), not a stringified error.
- **`String()/Number()/Boolean()`** callable; `Number.MAX_SAFE_INTEGER` etc.
- **Mixed `Promise.all`** unwraps inner promises; `.map(tool)` now errors actionably.
- **Numeric builtins** (this pass): `parseInt`/`parseFloat`/`isNaN`/`isFinite`,
  `Number.isInteger/isNaN/isFinite/parseInt/parseFloat`, `(n).toFixed/toString(radix)/toPrecision/valueOf`,
  `Object.fromEntries`.

## 🔴 P0 — Receiver write-back corruption (architectural, corroborated x4)
The single highest-impact bug. The receiver of a method call is tracked in a
mutable `last_receiver`/`last_receiver_source` slot set when the method is
loaded, but evaluating the call's **arguments** clobbers it before `Call` runs.
Because arrays/objects are value-typed with a "write the mutated receiver back
to its source variable" hack, this silently corrupts the most common patterns:

| Pattern | Actual | Expected |
|---|---|---|
| `const o={a:[]}; o.a.push(1); o` | `[1]` (o replaced!) | `{a:[1]}` |
| `for (const r of items) out.push(r.id)` | `[]` (silent) | `[id, …]` |
| `arr.push([1,2].join(","))` / `arr.push(Math.max(a,b))` / `results.push(await f())` | throws `__array__.push is not a function` | works |
| `url.slice(0, url.indexOf(":"))` (method-chained arg) | throws `__string__.slice is not a function` | works |
| `[1,2].forEach(x => arr.push(x))` | `arr` stays `[]` | mutates `arr` |

Fix requires binding the receiver (and a proper write-back *place*, incl. nested
`obj.a.push`) to the method at load time instead of a clobberable slot — a
focused VM change. **This breaks a large fraction of real agent code; should be next.**

## 🔴 P0 — Regex engine nearly non-functional (corroborated)
- `str.replace(/re/, x)` / `replace(/re/g, x)` is a **no-op**.
- `str.match(/re/)` returns `null` for anything with metacharacters (`\d`, `[a-z]`, `+`, `{2}`, groups, named groups, lookaround); `/g` returns only the first match.
- `RegExp.prototype.test` doesn't exist.
- `str.split(/re/)` doesn't split.
Only literal-substring `.match` works. Needs a real regex engine (large). Breaks
validation, slugs, whitespace-normalize, parsing. (String-literal `.replace`/`.split`/`.replaceAll` DO work.)

## 🟠 P1 — Destructuring in parameters & for-of bindings (corroborated)
Variable-declaration destructuring works (`const {a} = o`), but:
- `({a}) => a` / `function f({a,b})` → first name = whole arg, rest `null`.
- `([k,v]) => …` → `k` = the whole pair, `v` = undefined.
- `for (const {id} of rows)` / `for (const [k,v] of pairs)` → bound names are `null`.
Pervasive in agent code (`.map(([k,v]) => …)`, destructured params). Workaround: destructure in the body.

## 🟠 P1 — Missing data structures & constructors (corroborated)
- **`Set` / `WeakMap`**: absent (`new Set()` → "not a constructor"). The idiomatic dedup primitive.
- **`Map`** half-implemented: `new Map([[k,v]])` ignores entries; `.size` → `null`; no `for...of` / `.entries/.keys/.values/.forEach`. (`.set/.get/.has` work.)
- **`Error`** (and `TypeError`/`RangeError`): `new Error("x")` → "not a constructor"; `e instanceof Error` always false; a thrown host `Error` reaches `catch` as a bare string (`e.message` undefined). Agents write `throw new Error(...)` constantly.

## 🟠 P1 — Call-argument spread (corroborated)
`f(...args)` / `Math.min(...arr)` / `Math.max(...arr)` → `null` (spread not applied at call sites). Common for min/max/variadic. (Array/object *literal* spread is fixed.)

## 🟡 P2 — Control-flow gaps
- **Top-level `switch`** → "allocation limit exceeded" (infinite loop). Works inside a function. Fallthrough semantics need review.
- **Labeled `break`/`continue`** (`break outer`) silently ignored (acts on inner loop only).

## 🟡 P2 — Smaller correctness / missing
- `Number('')` → `null` (should be `0`).
- `str.indexOf(needle, fromIndex)` ignores `fromIndex`.
- `JSON.stringify(obj, null, 2)` ignores the indent arg (no pretty-print).
- Spread over a generator `[...gen()]` throws (for-of over a generator works).
- Missing: `structuredClone`, `String.fromCharCode`, `localeCompare`, `matchAll`, `Array.from(map.values())` (misroutes).
- Date: only `getTime()`/`toISOString()` work; `getDay/getUTCFullYear/…` are phantom (`typeof`=="function" but throw). Decompose via epoch-ms math.

## Notes
The `scenarios-*.mjs` files document behavior *as found during this pass*; checks
named `BUG`/`MISSING` capture gaps above and some are now fixed. They run via
`npm run test:scenarios` (exploratory, not part of the green `test:e2e` gate).
