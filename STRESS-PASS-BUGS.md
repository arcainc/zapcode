# Zapcode interpreter — divergences from real JS/TS (stress-pass findings)

## Fix status (in progress)

**Round 9 (PRs #26–#31) — microtask / event-loop ordering: the async model is
now Node's.** Full plan and per-stage detail in `docs/microtask-design.md`
(status: complete). What this closes, beyond what rounds 4–6 left:

- **Eager `.then` ordering — FIXED.** Reactions defer to a serialized FIFO
  microtask queue (drained at top-level completion and at awaits) instead of
  running inline; `.then`/nested-`.then`/`finally` ordering, and the classic
  async-ordering kata, match Node byte-for-byte
  (`conformance_microtask.rs`, `conformance_async_interleave.rs`).
- **`async` returns a Promise; `await` suspends the function — FIXED.**
  `f().then(...)` works; `await` detaches the body into a serializable parked
  `AsyncTask` and always yields a tick (top-level `await` ticks too, via a
  sentinel job — `conformance_await_tick.rs`). Async bodies interleave with
  sync code and each other in Node order; `Promise.all` fan-outs interleave
  their host calls.
- **Throw semantics — FIXED.** A throw escaping an async body rejects the
  call's result promise (callers' try/catch correctly does NOT see it;
  `f().catch(h)` does). The await-rejection boundary rethrows the ORIGINAL
  reason — the wrapped-`Error` divergence documented in
  `conformance_async.rs` is gone, including for `for await` and combinator
  rejections (`Promise.any` now surfaces its real AggregateError reason).
- **Unhandled rejections — FIXED.** A rejection nobody handles fails the run
  deterministically at end-of-drain (Node's unhandled-rejection crash, made
  replay-safe).
- **Durability hardened.** All of the new async state (queue, reaction
  records, parked tasks, unhandled marks) is serialized; replay with a
  dump→load at EVERY suspension is byte- and trace-identical
  (`durable_async_state.rs`). Runaway drains hit resource limits. Wire
  v4 → v8 across the rounds (see `wire.rs`).
- **Generator fixes flushed out en route:** the generator drive loop now
  settles handler continuations (an `await <then-chain>` inside an async
  generator no longer corrupts the generator's stack), and
  `SuspendedFrame` carries the promoted-cell map — a *latent sync-generator
  bug* where a boxed local written between yields silently reverted on
  resume.

**Round 10 (branch `conformance-promise-constructor`)** closed the round-9
list's promise items:
- **`new Promise(executor)` — FIXED.** The executor runs synchronously with
  serializable resolve/reject capability objects that settle through the
  microtask machinery; thenable adoption, the deferred pattern, the spec'd
  constructor catch (a throw rejects the promise without escaping), tool
  calls inside executors, and combinator interop all work
  (`conformance_promise_constructor.rs`). Pinned: `typeof resolve` inside
  the executor is `"object"` (capabilities are marker objects).
- **`Promise.race`/`any` tick order — FIXED.** With pending-chain elements
  the winner is the first to settle (race) / fulfill (any) as the drain
  progresses; losers' later rejections are absorbed, not unhandled
  (`conformance_combinator_ticks.rs`).
- **`p.then()` identity — FIXED.** `.then`/`.catch`/`.finally` always
  return a NEW dependent promise; non-callable handlers become
  pass-through reactions (`p.then() === p` is now `false`, and
  `Promise.reject(x).then(undefined, undefined).catch(h)` forwards `x`).
- **AggregateError — FIXED.** The `Promise.any` all-reject reason is a real
  branded error: `instanceof AggregateError` AND `instanceof Error`,
  `.name`, `.errors` all match Node.

Remaining known divergences after round 10 (and the batch-methods
follow-up):
- Generator-mainloop Stages 0–1 landed: `.next()` and loop pulls run the
  body in the main loop — tool calls inside generator bodies suspend
  durably (the old "cannot suspend inside a generator" pin is gone), the
  yield-in-try leak is fixed, and re-entrant pulls raise Node's TypeError.
  Stage 3 made async-generator `next()` answer a Promise of the iterator
  result (`it.next().then(...)` works). Remaining pins: body awaits
  between yields run with in-place drains (cross-task tick interleaving
  during a pull can run ahead; results match Node), and
  spread/`Array.from`/destructure still materialize eagerly via the nested
  driver (no tool calls there).
- (FIXED in the batch-methods follow-up) `.then`/`.catch`/`.finally` on a
  *batch* promise now force the batch like the single-call N5 path and run
  the method on the assembled result (`conformance_batch_methods.rs`);
  batches mixing in microtask-pending chain elements keep the pass-through
  (await the combinator instead).
- `Promise.race([])`/`any([])` settle-never pins and `Symbol.toPrimitive`
  non-dispatch are unchanged (documented in `conformance_async.rs` and the
  ToPrimitive notes below).

**Round 11 (branch `parity-differential-harness`) — the differential gate.**
`packages/zapcode-ai/tests/differential.mjs` runs a 130-snippet corpus
through BOTH zapcode and real Node in the same process and deep-compares the
results (normalized for the documented host-marshalling rules). It is wired
into `test:e2e-full`, so parity is now checked mechanically on every run;
documented pins are asserted as pins (and fail loudly if they start agreeing
with Node, forcing promotion into the corpus). Its FIRST run found five real
divergences, all fixed in this round:
- **`arr.map(String)` / conversion globals as callbacks** — `String`/
  `Number`/`Boolean` marker objects are now callable through every internal
  callback path (with the same ToPrimitive shaping as a direct call).
- **`entries()`/`keys()`/`values()`** now return real iterator objects:
  manual `.next()` stepping works alongside spread/for-of (the old
  plain-array pin in `conformance_iterators.rs` is rewritten as a
  capability test).
- **`JSON.parse(text, reviver)` over arrays** — the reviver walk read every
  array element as `undefined` (the holder-read arm only handled objects).
- **Param destructuring field-level defaults** (`({a, b: {c} = {c: 9}})`) —
  the raw argument now rides a hidden temp so the compiler prologue can
  re-destructure the default when the field arrives undefined (mirrors the
  whole-pattern-default mechanism).
- **`Promise.all([p.then(f), …]).then(cb)`** — a batch holding
  microtask-pending chain elements now settles them (one drained job per
  re-dispatch of the method call) before the method forces the batch,
  closing the carve-out from the batch-methods round.

**Round 12 (branch `parity-differential-harness`, second pass) — PR-review
fixes.** Worked the valid findings from the PR #35/#37 review comments (one —
the claimed array-destructure local-slot misalignment — was a false positive:
the hidden-temp push is gated on `has_field_level_default()`, which is
identically false for array patterns in both VM and compiler):
- **`Promise.all([pending]).then(cb)` with an EMPTY microtask queue dropped
  `cb`.** The round-11 fix borrowed a drained job per re-dispatch, so it only
  worked while the queue was non-empty; with nothing queued, the method fell
  through to the legacy pass-through (returning the batch itself — Node 11
  vs zapcode `[1]` on the gate repro). A combinator whose every element is
  internal (settled / plain / microtask-pending — no host call to force) now
  LOWERS to a real pending promise: each pending element gets a `"combine"`
  reaction carrying the batch + index, per-kind progress lives on the batch
  object itself (plain heap data — snapshots for free), and the method runs
  through the ordinary pending-promise path. All four kinds implemented
  (all / race / any incl. AggregateError / allSettled); mixed host+chain
  batches keep the drain-borrowing path. `conformance_batch_lowering.rs`
  (12 tests incl. a snapshot-hop) + 7 differential snippets.
- **Nested destructure defaults evaluated TWICE** (and, worse, field-level
  defaults under a whole-pattern default never applied when the argument was
  supplied: `f({})` against `{a: {b} = {b: 9}} = {}` bound `b` undefined).
  The repair prologue re-destructures the nested subtree — which applies its
  inner defaults — and the inner-defaults pass then descended the same
  subtree again. Fix: the whole-pattern-default path gained a proper
  defined-argument else-branch running the same repair as plain patterns,
  and the inner pass skips fields the repair already covered. Side-effecting
  defaults now run exactly once on every call shape (Node-verified).
- **`gen_iter_triple` allocated without resource accounting** — every
  loop-driven generator pull allocated an untracked 3-slot protocol array;
  now charged against the limits like every other allocation site.
- **Guest JSON.stringify could ABORT THE HOST** (pre-existing, surfaced by
  this round's test runs): `Vm::serialize_json_dynamic` uses ~7.5 KB of
  stack per frame unoptimized, so the 256-level `MAX_RENDER_DEPTH` budget
  (~1.9 MB) overran a default 2 MiB thread stack *before the depth guard
  could fire* — a sandbox hole (`security.rs` only passed by margin).
  `MAX_RENDER_DEPTH` is now 128, keeping the fattest walker's worst case
  under ~1 MB.
- Hygiene: the duplicate `make_array_iterator` in `vm/mod.rs` now delegates
  to the `builtins` copy.

**Round 8 (branch `arca/conformance-fixes`) — cluster wrap-up + reflection / RegExp / coercion-builtin edges.**

This round confirms the earlier-landed clusters are GREEN-by-construction (asserting
real Node, not pinned divergences) and closes a batch of remaining reflection /
builtin / string-method divergences. Both gates are green: `cargo test -p
zapcode-core` = **2029 passed, 0 failed**; `cargo build --workspace` clean;
`npm run test:e2e-full` (237 checks) + `npm run test:scenarios3` (77 checks) green.

Confirmed FIXED & asserting correct JS (clusters from the round brief):
- **try/finally completion override (B2)** — `finally`'s `return`/`throw` wins over
  `try`/`catch`; verified in `conformance_errors.rs` and the knowledge/durable e2e.
- **destructuring defaults (D4)** — object- and array-pattern element defaults and
  whole-pattern `={}`/`=[…]` param defaults all apply (`conformance_destructuring.rs`).
- **class fields / static / accessors (cluster C)** — instance & static field
  declarations initialize, getter/setter accessors round-trip (`conformance_classes.rs`).
- **JSON replacers / reviver / toJSON (I3/I4) + function replacer (G1)** — array &
  function `JSON.stringify` replacers, `JSON.parse` reviver, plain-object `toJSON`,
  and function `replace`/`replaceAll` replacers (`conformance_json.rs`, e2e).
- **generators `yield*` / spread** — delegation and `[...gen()]` spread/destructure,
  custom `Symbol.iterator` (`conformance_generators.rs`).
- **class extends Error** — `super(message)` propagates `.message`, `instanceof Error`
  chain established, `this.name` honored (`conformance_dates_errors.rs`/`conformance_errors.rs`).
- **Object.freeze** — actually prevents mutation; `Object.isFrozen` true (`security.rs`).
- **durable nested-closure capture (K1)** — factory-local closure state survives
  `dump()`/`loadSession()` (scenarios3-statemachine, e2e-durable-serialization).

Newly FIXED this round (the "added builtins" / reflection / coercion edges):
- **Class & function reflection.** `typeof Class === "function"`; `Class.name`;
  `new C().constructor === C` (instances carry a `.constructor` back-link to the
  class value); static methods/fields inherit through the `__super__` chain
  (`B.f()` resolves a parent static). Functions expose `.length` (arity = leading
  params before the first default/rest) and `.name` (declared, or inferred from a
  `const f = function(){}` / `=> {}` binding). *Residual:* per-instance method
  identity still differs (`a.m !== b.m`) because heap `Value::Function` has no
  identity — pinned honestly in `conformance_classes.rs`.
- **RegExp constructor (G8) + `instanceof RegExp` (O9).** `new RegExp(pat, flags)` /
  `RegExp(pat, flags)` build a usable regex (`typeof RegExp === "function"`),
  copying source/flags from an existing regex arg; bad patterns throw. Read-only
  accessors `.source`/`.global`/`.ignoreCase`/`.multiline`/`.dotAll`/`.sticky`.
  *Residual:* regex backreferences (`\1`) and lookbehind/lookahead are unsupported
  (the `regex` crate lacks them) — pinned in `conformance_regex.rs`.
- **`exec()` rich result.** `re.exec(s)` now returns the same array-LIKE match
  result `match()` does — `.index`, `.input`, named `.groups`, indexed groups,
  `.length` — instead of a bare positional array. *Trade-off (shared with G4):* it
  is an object, not an `Array`, so use indexed access, not `.slice`/`.map`.
- **`Math.min`/`Math.max` NaN poisoning.** Any NaN argument (in ANY position) makes
  the result `NaN` (was only honored as arg 0); `-0`/`+0` ordering respected.
- **String method edges.** `includes(needle, position)` honors the start position;
  `lastIndexOf(needle, fromIndex)` searches backward from `fromIndex`; `split()`
  with an omitted/undefined separator returns `[wholeString]` (not a char split);
  `at()` returns `undefined` for any out-of-range index (no negative clamping); a
  string-typed numeric subscript (`"hello"["1"]`) reads the char.
- **`String.replace`/`replaceAll` `$`-tokens.** The string-search path expands
  `$$`/`$&`/`` $` ``/`$'` against the match (were inserted literally); a regex
  replacement referencing a non-existent group (`$1` on `/b/`) is left literal,
  matching JS.

*Tuning note:* the ToPrimitive recursion guard was lowered 8 → 5 so the
cyclic-`toString(){ return "" + this }` security case is still caught with a clean
`RuntimeError` rather than overflowing the native stack (the extra `dispatch` match
arms added this round shrank the prior margin). 5 is still comfortably above any
legitimate `valueOf`→`toString` fallback nesting. (`security.rs`.)

Still-pinned residuals (deliberately deferred, asserting actual): UTF-16 vs
code-point string indexing (G9); `[Symbol.toPrimitive]` dispatch (O4 tail); block-
scoped `let`/`const` redeclaration shadowing (compiler uses a flat slot model);
integer-key enumeration order (J1) and one `continue`-outer-label edge (J4 variant);
per-instance method identity (function values have no identity); `#private` fields
(unsupported syntax); named-function-expression internal self-binding; `String(1e21)`
exponential formatting (F9); the match/exec result *brand* (object vs Array, G4);
regex backreferences / lookaround. Plus the tool-boundary / session residuals (L4
errors-as-strings, L8 per-param descriptions, F4 NaN/Infinity→null output, P1 live
generator, P2 chunk scope), which are boundary-marshalling / persistence limits.

**Round 6 (branch `arca/heap-handles-rewrite`) — deferred-five final disposition + O4 security-regression fix:**

This round finishes the "deferred five" (G4 / O4 / G9 / N9 / N5) and, critically,
*repairs a GREEN-suite regression* that Round 5 had left in (and mis-labelled as
"pre-existing"). Final per-item status:

- **G4 — FIXED (with documented architectural residual).** `match()` (non-global)
  and each `matchAll()` element expose `.index`, `.input`, `.groups`, `m[0..n]`,
  and `.length`; named captures populate `.groups`. The non-global result is an
  *array-like heap object* rather than a true `Array` (heap arrays are `Vec`-backed
  and cannot carry the extra named props the spec attaches to a match result), so
  `Array.isArray(nonGlobalMatch) === false` and array methods aren't available on
  it (use indexed access or spread `matchAll`). The global `match(/…/g)` still
  returns a plain array of matched strings, exactly like JS. *Residual is purely
  the value's brand (object vs Array); every property the feature targeted works.*
  Making it a real Array-with-props would require changing the heap's `Array`
  variant to carry a side map of named properties — a core-value-model change that
  ripples through serialization/cloning/equality/all array builtins, i.e. exactly
  the kind of destabilizing rewrite the GREEN guarantee says to avoid. Tests:
  `stress_match_groups.rs`.

- **O4 — FIXED (valueOf/toString fully landed + now cyclic-safe); `Symbol.toPrimitive` documented residual.**
  User `valueOf`/`toString` hooks are honored at every coercion point (`+`, the
  arithmetic/relational operators, string concat / template literals, and
  `String()`/`Number()`), with the correct per-hint method order. **Round 6 fixed a
  real regression this introduced:** a self-referential hook
  (`toString(){ return "" + this }`) recursed *past* the previous 200-deep
  `to_primitive` guard and **overflowed the native Rust stack, aborting the whole
  `security` test binary (SIGABRT)** — i.e. the suite was NOT actually green at the
  Round-5 HEAD, contrary to the note Round 4 left below. Fix:
    1. Lowered the ToPrimitive recursion guard from 200 → 8 (each level nests a full
       guest-call interpreter loop on the native stack; 8 is far above any
       legitimate valueOf→toString fallback yet well below native-stack
       exhaustion), so a cyclic hook now surfaces a clean, catchable
       `RuntimeError("ToPrimitive recursion limit exceeded …")` instead of crashing
       the host.
    2. Fixed `call_closure_internal`'s error path to (a) only catch a `try` block
       that lives *inside* the internal call (`frame_depth > target_frame_depth`)
       and (b) unwind the frames it pushed before propagating — previously a nested
       internal call that errored left orphaned frames and later panicked at
       `frames.last().unwrap()`.
    3. Replaced the obsolete `test_tostring_not_invoked_during_coercion` security
       test (it asserted the *pre-O4* "never invoke toString, always
       `[object Object]`" stance, which O4 deliberately superseded) with three tests
       matching the real contract: a non-cyclic hook IS honored, a cyclic hook is
       *bounded, not fatal*, and the resulting error is *catchable from inside the
       guest*. Invoking the hook is not a sandbox escape (it runs under the same
       limits); the only real security concern — unbounded recursion DoS — is now
       handled cleanly. (`crates/zapcode-core/tests/security.rs`.)
  *Residual (deferred):* `[Symbol.toPrimitive]` is still not dispatched. It depends
  on real well-known-symbol keying (O8): the crate's `Symbol` is a stub whose
  values are `__symbol__`-tagged heap objects, and the four computed-property-key
  sites all coerce keys via `to_js_string`, so a `[Symbol.toPrimitive]` key can't be
  matched without a shared property-key mapping change across the get/set/define
  paths — moderate blast radius for a niche hook, deliberately left out to protect
  the GREEN suite. The common `valueOf`/`toString` path covers the realistic cases.

- **G9 — DOCUMENTED RESIDUAL (intentionally not landed).** Strings remain indexed by
  Unicode *code point* (Rust `chars()`), not UTF-16 code units, so `"😀".length` is
  `1` (JS: `2`) and `"😀".charCodeAt(0)` is `128512` (JS: `55357`, the high
  surrogate). This is *self-consistent* across `length`, indexing `s[i]`, `charAt`,
  `charCodeAt`, `codePointAt`, `at`, `slice`, `substring`, `substr`,
  `indexOf`/`lastIndexOf`, and the regex/match `.index` (all code-point based).
  Converting to UTF-16 is a broad, mutually-coupled change across ~15 call sites in
  `vm/mod.rs` and `vm/builtins.rs` plus the byte↔char-count match machinery; there
  is **no safe partial** — converting only some of those methods would make
  `s.length` disagree with `s[i]`/`slice`, introducing a *new* divergence worse than
  the current consistent model. Only BMP characters are exercised by the suite (and
  the one e2e `charCodeAt` test uses A/B/Z), so no test currently depends on either
  semantics. Per the GREEN guarantee, the full UTF-16 rewrite is left as the
  documented residual rather than shipped half-done. *Practical impact:* correct for
  all BMP text (the overwhelming majority of agent input); diverges only on astral
  (non-BMP) characters — emoji, some CJK-extension and math-script code points.

- **N9 — FIXED (with pre-existing generator residual).** `for await (const x of …)`
  awaits each iterated value (arrays of promises/values, mixed, destructuring,
  break/continue, nesting, rejection-in-loop, and suspend/resume across an external
  call in the body), and async-generator *consumption* via for-await drives the
  iterator and awaits each yielded value. *Residual (unchanged, pre-existing):* an
  `async function*` that suspends on an *external host call mid-iteration* errors
  `cannot suspend inside a generator` — a generator-engine limitation (generators
  run synchronously via `generator_next`, which can't capture/resume a host-call
  suspension across `yield`), not specific to for-await. Pinned by
  `async_generator_external_suspension_is_the_documented_gap` in `stress_for_await.rs`.

- **N5 — FIXED (fully, including stringification).** A bare (un-awaited) tool call is
  a real deferred Promise object (`typeof === "object"`, host call held until
  awaited / `.then`/`.catch`/`.finally`-ed / returned-and-awaited), with `.then`
  chaining, callback-internal tool calls, thenable adoption, and snapshot
  round-trip. **Round 6 closed the last residual:** a `__promise__`-tagged heap
  object now string-coerces to the spec's `[object Promise]` (via a `to_js_string`
  special-case mirroring `__date_ms__`/`__error__`), instead of leaking
  `[object Object]`. So `"" + tool()` → `"[object Promise]"`. (`stress_call_promise.rs`.)

**Net for the deferred five:** G4, O4, N9, N5 are fully landed (each with a small,
honestly-documented residual that is architectural / pre-existing, not a gap in the
targeted behavior); N5's previously-documented stringification residual is now also
closed. **G9 is the one item deliberately left as a documented residual** because no
correctness-preserving partial exists and the full UTF-16 conversion would
destabilize the suite. The suite is now **fully green** (it was *not* at the Round-5
HEAD — the `security` binary was aborting on the cyclic-toString case; that is fixed
here). 634 core tests pass (0 failed), `cargo build --workspace` is clean (0
warnings), and the JS `test:scenarios3` (77 checks) + `test:e2e` (all suites incl.
`stress`/`parallel`/`marshalling`) are green with the native binding rebuilt.

**Round 5 (branch `arca/heap-handles-rewrite`) — N5 bare tool-call as a deferred Promise:**
- **N5 — FIXED (general case).** A bare (un-awaited) tool-call expression now
  evaluates to a real *deferred* Promise object instead of an eagerly-resolved
  value. `const p = tool(); typeof p === "object"` and the host call is **not**
  made until `p` is awaited, `.then`/`.catch`/`.finally`-ed, or returned-and-awaited.
  - Compilation: a non-spread bare external call lowers to
    `CallExternalDeferred(name,argc)` + `MakeCallPromise`, producing a promise
    object `{__promise__:true, status:"pending_call", __call_id__:id}` whose call
    is held in `pending_calls`. The *directly-awaited* form `await tool()` keeps
    the pre-N5 eager-suspend `CallExternal` path (special-cased in the compiler's
    `Expr::Await`), so that hot path is byte-for-byte unchanged.
  - `Await` on a `pending_call` promise suspends once on its host call (an
    ordinary `VmState::Suspended { name, args }` the existing host bridge already
    handles) and resumes with the result.
  - `.then`/`.catch`/`.finally` on a `pending_call` promise force the call via the
    same suspension, recording a `ResumeAction::PromiseMethod`; on resume the
    settled value is wrapped (resolved) or, via `resume_with_error`, rejected, and
    the method runs through the existing N4 promise-callback machinery (so a tool
    call *inside* the callback still suspends/resumes). `.then` chaining works
    (`tool().then(a).then(b)`), and a promise callback that itself *returns* a
    deferred promise (thenable adoption) is forced via `ResumeAction::ChainResult`.
  - State (`resume_action`) is serialized in the snapshot (`#[serde(default)]`), so
    a deferred-promise suspension survives dump/load/resume.
  - `Promise.all`/`race`/`any`/`allSettled` batching is unchanged: direct
    external-call array elements still lower to `CallExternalDeferred` +
    `MakeBatchPromise` and never reach the single-call path. (Verified by
    `stress_promise_combinators.rs`, `parallel_calls.rs`, JS `parallel`/`stress`.)
  - Tests: `stress_call_promise.rs` (13) — typeof/deferral, await of a stored
    promise, `.then` single + chain, `.then` callback making a tool call, `.catch`
    rejection + pass-through, `.finally` pass-through, snapshot round-trip,
    return-then-await, and the unchanged `await tool()` form.
  - **Residual / behavior change (documented):** a bare tool-call promise
    string-coerces to `[object Object]` rather than the spec `[object Promise]`
    (shared with all heap-Object promises here — a stringification detail, not
    N5-specific). Two N4 tests in `stress_then_tools.rs` were updated to the
    now-correct JS semantics: a callback's *returned* deferred promise is adopted
    (so `.finally(() => tool())` drives the call), and a tool error is only
    catchable by an inner `try` when the call is `await`-ed inside it (a bare
    `return tool()` defers past the `try`, matching JS). Pre-N5 snapshot/session/
    wire-format tests that used bare un-awaited calls purely to trigger a
    suspension were switched to `await tool()` (same suspension, correct N5
    semantics).

**Round 4 (branch `arca/heap-handles-rewrite`) — Promise combinators + `super.method` + MED/LOW edges:**
- **C3 `super.method()` / `super.prop` — FIXED.** Class methods now track their
  defining class on the frame, so `super.g()` dispatches to the parent method with
  the subclass instance as `this` (reads `this`-fields set after `super()`), chains
  through three+ levels, and `super.g` (no call) yields the parent function value.
  Coexists with `instanceof` and normal inherited dispatch. (`stress_classes.rs`.)
- **N1 `Promise.race` / N2 `Promise.any` / N3 `Promise.allSettled` — FIXED.** When an
  array element is a direct external call, the combinator lowers to a
  `MakeBatchPromise(kind,n)` deferred batch; awaiting it suspends once with
  `VmState::SuspendedMany { combinator, .. }`. The host bridge runs the *real* JS
  combinator (true settle timing for race/any) and resumes per kind: race →
  first-settled value (or first rejection); any → first fulfilled (else
  AggregateError); allSettled → per-element `{status,value|reason}`, never rejects.
  Inline (no-external-call) combinators settle without a host round-trip. `Promise.all`
  baseline preserved. (`stress_promise_combinators.rs` + e2e `parallel`/`stress`.)
- **N8 `Promise.resolve` adoption — FIXED.** `Promise.resolve(value)` /
  `Promise.resolve(promise)` adopt rather than double-wrap; awaiting a resolved
  promise unwraps to its value.
- **N4 tool calls inside `.then`/`.catch`/`.finally` callbacks — FIXED** (prior commit;
  enables the `primary().catch(() => fallback())` retry pattern). (`stress_then_tools.rs`.)
- **N9 `for await (const x of …)` — FIXED.** The for-of lowering now reads oxc's
  `ForOfStatement::await` flag (previously the loop parsed but the flag was silently
  ignored, so each `x` was the raw Promise object). `Statement::ForOf` carries an
  `await_each: bool`; when set, the compiler emits an `Await` instruction on each
  iterated value just after the loop's done-check and before the binding. This reuses
  the existing `Await` path: a resolved promise unwraps, a rejected promise throws
  (`Unhandled promise rejection: …`), a non-promise passes through, and a pending
  external call suspends/resumes back into the same loop iteration. Verified for arrays
  of promises (`for await ([Promise.resolve(1),Promise.resolve(2)])` sums to 3), plain
  values, mixed, destructuring bindings, break/continue, nesting (no iterator leak),
  rejection-in-loop, and suspend/resume across an external call in the loop body.
  Async-generator *consumption* also works: `for await (… of asyncGen())` drives the
  generator's iterator and awaits each yielded value (yielding promises and internal
  promise-awaits both resolve correctly). (`stress_for_await.rs`.)
  - **Residual gap (deferred):** an `async function*` that suspends on an *external*
    (host) call mid-iteration is unsupported — it errors `cannot suspend inside a
    generator`. This is a pre-existing generator limitation (generators run
    synchronously via `generator_next`, which can't capture/resume a host-call
    suspension across the yield boundary), not specific to for-await; internal
    promise-awaits and yielding promises inside an async generator both work. Pinned by
    `async_generator_external_suspension_is_the_documented_gap` in `stress_for_await.rs`.
- **Edges:** **O5** (`"key" in obj` no longer a parse error when the line starts with a
  string literal; array `length`/numeric-index `in` membership, own-keys only),
  **H6** (`Map.set`/`Set.add` return the collection so chaining works; returns the same
  shared handle under reference semantics), **G3** (a `/g` regex maintains `lastIndex`
  so `while((m=re.exec(s))!==null)` terminates, and `/g .test()` advances then resets),
  **O8** (minimal `Symbol`: `typeof Symbol === "function"`, `Symbol()` is a unique
  `typeof === "symbol"` value with `.description`, usable as a computed key) — all FIXED.
  Bonus: a stale `last_global_name` no longer leaks a builtin method into an unrelated
  member access (`String(o.missing)` → `"undefined"`, not `"function"`). (`stress_edges_round1.rs`.)
- **Binding parity:** `zapcode-wasm` and `zapcode-py` now destructure and surface the new
  `combinator` field on `SuspendedMany`, so `cargo build --workspace` is clean.
- **Intentionally deferred (still divergent):** **N5** is now FIXED in Round 5 — a
  bare tool-call expression is a real deferred Promise object with `.then`/`.catch`/
  `.finally`, deferred until awaited/then-ed (see Round 5 above);
  (**N9** `for await…of` is now FIXED — see Round 4; only async-generator
  *external-call* suspension mid-iteration remains a documented gap); **G4**
  `match()`/`matchAll` `.index`/`.input`/named `.groups`; **O4** `valueOf`/`toString`/
  `Symbol.toPrimitive` coercion hooks; **G9** UTF-16 string indexing (strings are indexed
  by code point). **N7** `AggregateError` global landed in Round 2; `Promise.any`'s
  all-reject surfaces an AggregateError-shaped rejection via the host bridge.
- **Verified:** 579 core tests (0 failed) + `cargo build --workspace` clean (0 warnings) +
  full JS `test:scenarios3` (10 scenarios) and `test:e2e` (all suites incl. `parallel`/
  `stress`/`marshalling`) green.
- **`security` test issue — RESOLVED in Round 6 (it was an O4 regression, not "pre-existing").**
  As of the Round-5 HEAD, the `security` binary aborted (SIGABRT) on
  `test_tostring_not_invoked_during_coercion`: the O4 hook invoked a self-referential
  `toString(){ return "" + this }`, which recursed past the (then 200-deep)
  ToPrimitive guard and overflowed the native stack — taking the whole binary down
  (so `test_weakref_escape`, run earlier in the same process, also appeared to
  "fail"). This was a genuine O4-introduced regression, *not* a pre-existing /
  platform issue. Round 6 fixes it (recursion guard 200→8 so it errors cleanly,
  `call_closure_internal` frame-unwind on the error path, and the obsolete test
  replaced by three tests asserting the real bounded/catchable/honored contract).
  The full suite — including `security`, `stress_for_await.rs`, `async_await`,
  `generators`, and all `stress_*` — is now green.

**Round 3 (branch `arca/heap-handles-rewrite`) — the heap-with-handles rewrite:**
- **A (reference semantics) — FIXED.** `Value::Array`/`Object` now carry a `Handle`
  into a VM-owned `Heap`; cloning a value shares the slot, so aliasing,
  mutate-through-parameter, Map-of-arrays bucket pushes, identity `===`, and
  identity Map object-keys all behave like JS. `structuredClone` deep-copies.
  The old place/write-back machinery is retired in favour of in-place heap
  mutation. The heap serializes with the snapshot (handles preserve sharing; a
  cycle-safe visited-set was added to the serializability walk). Verified: 537
  core tests + 8 new `stress_references.rs` tests + full JS scenario3/e2e suites,
  all green. (54 files, +2762/-1614.)



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

**G4 [RESOLVED] `match()`/`matchAll` results lack `.index` / `.input` / named `.groups`.**
~~`"xxabc".match(/abc/).index` → exp `2`, act `null`; `"12-34".match(/(?<a>\d+)-(?<b>\d+)/).groups` → exp `{a,b}`, act `undefined`.~~
Non-global `match(re)` and each `matchAll(re)` element now return an **array-like heap object**: keys `"0".."n"` for the capture groups, plus `length`, `index` (match start in *chars*), `input` (the subject), and `groups` (an object of named captures, or `undefined` if the pattern declares none). So `m[0]`, `m[1]`, `m.index`, `m.input`, `m.groups.name`, and `m.length` all work, and `matchAll` is iterable/spreadable. The **global** `match(re)/g` still returns a plain array of matched strings (JS does too), so `.join()` etc. work on it.
**Trade-off (acceptable):** because heap arrays are `Vec`-backed and cannot carry extra named properties, the non-global result is an *object* rather than an *array* — `Array.isArray("a1".match(/(.)/))` is `false` (and array methods like `.join()`/`.map()` are not available on the non-global result; use indexed access or spread `matchAll`). Tests: `crates/zapcode-core/tests/stress_match_groups.rs`.

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

**N5 [MED] A tool-call expression is an eagerly-resolved value, not a Promise** — `const p = delay(…); typeof p` → exp `"object"`, act `"string"` (resolved value); `p.then` is `undefined`; the op runs to completion at the assignment. **Root cause of N1–N3, N6.** (`Promise.resolve(5)` *is* a real promise — specific to tool calls.) **N7 [MED]** `AggregateError` is undefined and `Promise.any`'s rejection carries no `.errors`/`.name`. **N8 [MED]** `Promise.resolve(thenable)` doesn't adopt the thenable. **N9 [FIXED — Round 4]** `for await…of` now awaits each iterated value (arrays of promises/values, mixed, destructuring, break/continue, nesting, rejection, suspend/resume across an external call in the body). Async-generator *consumption* via for-await also works (yielding promises + internal awaits resolve). Residual gap: an async generator that suspends on an *external* host call mid-iteration errors `cannot suspend inside a generator` (a pre-existing generator limitation). See `stress_for_await.rs`.
*Good:* `Promise.all` parallelizes and preserves index order; `.then`/`.catch`/`.finally` chaining + value/promise unwrap; `await` non-promise; `allSettled` element shape `{status,value/reason}`.

## Cluster O — Coercion / operators  *(silent corruption of common code)*

**O1 [HIGH] String-vs-string relational comparison always returns `false`.** `"apple" < "banana"` → `false`; `"car" <= "car"` → `false`. **Every** string `<`/`>`/`<=`/`>=` is broken — silently corrupts sorting/filtering/branching on strings. (Number-vs-number, number-vs-string, and default `sort()` comparator all work — isolated to both-string operands.)

**O2 [HIGH] `+` with an `undefined` numeric operand returns `null` instead of `NaN`.** `5 + undefined` → exp `NaN`, act `null`. (`"x"+undefined` correctly gives `"xundefined"` — only the numeric branch.)

**O3 [HIGH] `+` with any object/array operand returns `null`** (no object→primitive). `[1,2]+[3]` → exp `"1,23"`, act `null`; `1+[2]`, `[]+{}`, `+[5]`, `+{}` all `null`.

**O4 [FIXED — valueOf/toString; Symbol.toPrimitive deferred] User ToPrimitive hooks now honored in coercion.** A VM-level `Vm::to_primitive(value, hint)` (crates/zapcode-core/src/vm/mod.rs) invokes a heap object's callable `valueOf`/`toString` field via the guest-call path (`call_method_internal`, binding `this`) and uses a primitive result. Applied at: `Add` (default hint), `Sub`/`Mul`/`Div`/`Rem`/`Pow`/`Neg` (number hint), relational `Lt`/`Lte`/`Gt`/`Gte` (number hint), `ConcatStrings` / template literals (string hint), and the `String()`/`Number()` globals. Method order per spec (number/default → valueOf-then-toString; string → toString-then-valueOf); a hook returning an object is skipped; re-entrancy is bounded (`to_primitive_depth`, cap 200) so a cyclic/suspending hook can't loop. Now: `({valueOf(){return 42}})+1` → `43`, `({toString(){return "hi"}})+""` → `"hi"`, `Number({valueOf(){return 3}})` → `3`, `({valueOf(){return 100}})<200` → `true`. Regression tests: crates/zapcode-core/tests/stress_toprimitive.rs.
  - **Residual gap (deferred):** `[Symbol.toPrimitive]` is NOT dispatched. The crate's `Symbol` is a stub (no well-known symbols; object property keys are plain `Arc<str>`, and a computed `[Symbol.toPrimitive]` key stringifies to `"[object Object]"`), so the well-known-symbol key can't be matched reliably. Wiring it requires real well-known-symbol keying first (depends on O8). A hook that itself makes a tool call (would need suspension mid-instruction) is also out of scope — it surfaces as a RuntimeError from the internal call path rather than suspending.

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
