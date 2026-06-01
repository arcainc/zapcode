# Gap-fix plan (from KNOWN_GAPS.md)

Each item is a commit (or a small group), landing with `cargo test -p zapcode-core`,
`clippy -D warnings`, `fmt`, `cargo check` for py/wasm, and the TS e2e gate green.
As gaps close, flip the matching `scenarios-*.mjs` probes to assert correct behavior.

## P0 — Receiver write-back corruption  [commit 1]
Root cause: the method receiver lives in a clobberable `last_receiver`/
`last_receiver_source` slot, and mutating methods copy-back to a single source
variable (no nested paths).
- Carry the receiver **and a write-back place** on the method value itself
  (`Value::BuiltinMethod { …, recv, place }`), set at `GetProperty`/`GetIndex`,
  so argument evaluation can't clobber it.
- Generalize the place to a root var + property/index path (`obj.a.push`,
  `rows[i].items.push`), navigating + writing the root back on mutation.
- Fixes: `o.a.push(x)`, `for-of` + `out.push(r.id)`, `arr.push(f())`,
  `str.slice(0, str.indexOf())`, and array-method args that are calls.

## P0 — Regex engine  [commit 2]
Back regex with the pure-Rust `regex` crate (wasm-safe). Represent a RegExp as a
serializable `{source, flags}` (compile on use, cache).
- `str.match` (incl. `/g` → all), `str.matchAll`, `str.replace(re, repl)` with
  `$1`/`$&`, `str.split(re)`, `re.test`, `re.exec`. Flags g/i/m/s.
- Document unsupported (lookaround/backrefs — `regex` crate limitation) with a
  clear error rather than silent wrong results.

## P1 — Destructuring in params & for-of bindings  [commit 3]
`({a,b}) => …`, `function f({a},[x])`, `for (const {id} of …)`, `for (const [k,v] of …)`.
Reuse the working var-declaration destructuring path for parameter and for-of
binding patterns.

## P1 — Set, Map completeness, Error  [commits 4–5]
- `Set`/`WeakSet`: `new Set(iterable)`, `add/has/delete/clear/size`, `for-of`,
  spread, `Array.from`.
- `Map`: fix `new Map(iterable)` ctor, `.size`, `for-of`, `entries/keys/values/forEach`.
- `Error`/`TypeError`/`RangeError`/…: `new Error(msg)` → `{name,message}`,
  `e instanceof Error`, `e.message`; host-thrown `Error` reaches `catch` with `.message`.

## P1 — Call-argument spread  [commit 6]
`f(...xs)`, `Math.min(...arr)`, `fn(a, ...rest)`. Build the arg list dynamically
when a call has spread args; dispatch with the expanded args.

## P2 — Control flow  [commit 7]
- Top-level `switch` no longer infinite-loops (allocation error); verify fallthrough.
- Labeled `break`/`continue` (`break outer`) target the labeled loop.

## P2 — Smaller correctness / missing  [commit 8]
- `Number('')` → 0; `str.indexOf(needle, fromIndex)`; `JSON.stringify(x, null, n)` indent;
  spread over a generator; `String.fromCharCode`, `structuredClone`,
  `Array.from(map.values())`; Date decomposition (`getUTCFullYear/getDay/…`).

## Verification per commit
`cargo test -p zapcode-core && cargo clippy --all-targets -- -D warnings && cargo fmt --check`,
`cargo check -p zapcode-py -p zapcode-wasm`, and `cd packages/zapcode-ai && npm run test:e2e`.
