# Testing zapcode

zapcode is a TypeScript-subset interpreter for AI agents. Because it is a
*language implementation* — not just a library — its test suite is organized like
one: a broad **conformance** layer that pins language semantics against real
Node/JS, an **integration / end-to-end** layer that drives the AI-facing API the
way an orchestrator would, and a **durable-session serialization** layer that
proves VM state survives `dump()`/`loadSession()` across process boundaries.

There are two test tiers, run with two toolchains:

| Tier | Toolchain | Location | What it covers |
|---|---|---|---|
| **Rust conformance + core** | `cargo test` | `crates/zapcode-core/tests/*.rs` | Language semantics against the VM directly |
| **JS end-to-end** | `node` | `packages/zapcode-ai/tests/*.mjs` | The AI-SDK API surface, tool boundary, durable sessions, agent scenarios |

> **Correctness policy.** Every assertion targets *real Node / real-JS* behavior.
> Where the interpreter has a **known, documented divergence** (see
> [`STRESS-PASS-BUGS.md`](./STRESS-PASS-BUGS.md)), the test asserts zapcode's
> **actual** behavior with an inline comment naming the divergence and the JS
> answer — never a value the interpreter does not produce. The suite is green by
> construction.

---

## Tier 1 — Rust conformance (`cargo test -p zapcode-core`)

These run the VM directly. The conformance suites use a tiny harness that runs a
snippet to completion and stringifies the result value against the heap:

```rust
fn run_str(code: &str) -> String {
    let result = ZapcodeRun::new(code.to_string(), vec![], vec![], ResourceLimits::default())
        .unwrap().run(vec![]).unwrap();
    match result.state {
        VmState::Complete(v) => v.to_js_string(&result.heap),
        other => panic!("expected completion, got {other:?}"),
    }
}
```

(Heap-aware: post the heap-with-handles migration, `Value::Array`/`Object` carry
handles into the VM-owned `Heap`, so stringification always threads
`&result.heap`.)

### Conformance areas (`crates/zapcode-core/tests/conformance_*.rs`)

Grouped by language feature, test262-style breadth:

- **Lexical & scoping** — `conformance_lexical.rs`, `conformance_scoping.rs`:
  `let`/`const`, TDZ, hoisting, block scope, shadowing, loop-variable closure
  capture.
- **Expressions & operators** — `conformance_expressions.rs`,
  `conformance_operators.rs`, `conformance_coercion.rs`: precedence/associativity,
  arithmetic/relational/bitwise/logical operators, `==`/`===`, ToPrimitive hooks
  (`valueOf`/`toString`), template literals.
- **Control flow** — `conformance_control_flow.rs`, `conformance_controlflow.rs`:
  `if`/`for`/`for-of`/`for-in`/`while`/`do-while`/`switch`, labeled
  `break`/`continue`, statement completion values.
- **Numbers & math** — `conformance_numbers.rs`, `conformance_math_globals.rs`,
  `conformance_collections_math.rs`: number formatting (`toFixed`/`toPrecision`/
  `toExponential`/radix), `Math.*`, `Number.*`, parsing.
- **Strings & regex** — `conformance_strings.rs`, `conformance_strings_regex.rs`,
  **`conformance_regex.rs`**: query/slice/case/pad/trim, template literals, and a
  dedicated regex sweep — `test`/`exec`/`match`/`matchAll`/`split`/`replace`,
  flags (`g i m s`), char classes, quantifiers (greedy + lazy), anchors,
  boundaries, alternation, grouping, named groups, `lastIndex` stepping.
- **Arrays / objects / collections** — `conformance_arrays.rs`,
  `conformance_arrays_objects.rs`, `conformance_objects.rs`,
  `conformance_mapset.rs`, `conformance_iterators.rs`: array methods, object
  semantics, `Map`/`Set`, the iteration protocol across spread / `for-of` /
  `Array.from` / generator `.next()`.
- **Destructuring** — `conformance_destructuring.rs`: object/array, nested, rest,
  defaults, parameter destructuring.
- **Functions** — `conformance_functions.rs`: declarations/arrows/expressions,
  hoisting, defaults, rest, `call`/`apply`/`bind`, closures.
- **Classes** — `conformance_classes.rs`: `constructor`, methods, `extends`,
  `super`, `static`, `instanceof`, fields (incl. documented field-initializer
  divergence).
- **Generators** — `conformance_generators.rs`: `function*`, `yield`, `yield*`,
  manual `.next()` stepping, for-of consumption.
- **Async** — `conformance_async.rs`: `async`/`await`, Promise combinators
  (`all`/`race`/`any`/`allSettled`), `.then`/`.catch`/`.finally`, await positions,
  `for await…of`.
- **Errors** — `conformance_errors.rs`, `conformance_dates_errors.rs`: `throw`,
  `try`/`catch`/`finally`, `Error` subclasses, runtime-error shape.
- **Dates & JSON** — `conformance_datejson.rs`, `conformance_json.rs`:
  construction/parsing/arithmetic, `JSON.stringify`/`parse` fidelity, replacers,
  `toJSON`.
- **References & identity** — `conformance_references.rs`: shared-reference
  semantics (aliasing, mutate-through-parameter, identity `===`, Map object-keys)
  on the heap-with-handles model.
- **TypeScript subset** — **`conformance_typescript.rs`**: type-syntax *erasure* —
  annotations, `interface`/`type` aliases, generics, `as`/`satisfies` casts,
  optional/rest/`this` params, `declare`, decorators — plus the documented
  unsupported forms (`enum`, `namespace`/`module`, the legacy `<T>expr` cast).

### Core / engine suites (also under `crates/zapcode-core/tests/`)

`basic.rs`, `builtins*.rs`, `closures.rs`, `control_flow*.rs`, `classes.rs`,
`generators.rs`, `async_await.rs`, `operators.rs`, `objects_arrays.rs`,
`data_structures.rs`, `destructuring.rs`, `spread_and_throw.rs`,
`error_handling.rs`/`error_resume.rs`, `regex.rs`, `random.rs`,
`resource_limits.rs`, `sandbox.rs`, `security.rs`, `determinism.rs`, `trace.rs`,
`integration.rs`, plus the **session/serialization** core suites — `session.rs`,
`snapshot.rs`, `wire_format.rs`, `input_heap.rs`, `parallel_calls.rs` — and the
historical **`stress_*.rs`** suites that pin each fixed divergence (references,
classes, coercion, ToPrimitive, match groups, for-await, promise combinators,
call-as-promise, etc.).

### Running it

```bash
cargo test -p zapcode-core            # entire core + conformance suite
cargo test -p zapcode-core --test conformance_regex        # one suite
cargo test -p zapcode-core --test conformance_typescript
cargo test --workspace                # all crates (core, js, wasm, py bindings)
```

---

## Tier 2 — JS end-to-end (`packages/zapcode-ai`)

These import the **built** interpreter (`dist/index.js`, backed by the local
`napi` binding) and exercise the AI-facing API exactly as an application would:

- `execute(code, tools) -> { output, toolCalls }` — one-shot run.
- `createSession({ tools }).runChunk(code, inputs)` + `.dump()` — durable session.
- `loadSession(dump, { tools })` — resume in a fresh handle / process.
- `zapcode({ tools }).{ system, tools, openaiTools, anthropicTools,
  handleToolCall, custom }` — the SDK adapter surface.

Each `.mjs` suite is a standalone script: a green run prints `✓`/`PASS` lines and
a final count, and exits non-zero on the first failed assertion.

### Conformance / integration e2e suites (`tests/e2e-*.mjs`)

- **`e2e-agent-integration.mjs`** — realistic agent code paths: fan-out +
  aggregate via `Promise.all`/`reduce`, try/catch tool retries, catch+fallback.
- **`e2e-tool-contract.mjs`** — the tool-call *boundary* contract: the `toolCalls`
  record shape (`name`/`args`/`input`/`result`/`error`), argument validation &
  coercion, ordering & branch sensitivity, key normalization.
- **`e2e-session-inputs.mjs`** — per-chunk `inputs` as bare globals, conflict
  detection, structured-output preservation, error recovery to the last
  checkpoint.
- **`e2e-session-workflows.mjs`** — durable multi-chunk *workflows* through the
  high-level `createSession` API: state/functions/classes accumulating across many
  `dump()`→`loadSession()` reloads, tool-using async workflows across reloads,
  per-chunk result scoping, replay determinism, idempotent dump, and the
  documented P1 (live-generator) / P2 (re-declared binding) session boundaries.
- **`e2e-durable-serialization.mjs`** — the low-level `ZapcodeSessionHandle`
  suspend/resume *bytes*: mid-tool-call suspension surviving multiple
  serialization hops, `Promise.all` batch suspensions, `resumeError`, and the
  documented nested-closure capture boundary.
- **`e2e-ai-sdk-adapters.mjs`** — the `zapcode({ tools })` integration surface:
  system-prompt generation (declare-function signatures + call shapes), the
  OpenAI / Anthropic / Vercel tool shapes, `handleToolCall`, custom adapters via
  `createAdapter`, and the documented L8 per-parameter-description residual.

### Existing core e2e chain (`test:e2e`)

`agent-scenarios.mjs`, `durable-sessions.mjs`, `tool-errors.mjs`, `parallel.mjs`,
`ai-session.mjs`, `type-check.mjs`, `stress.mjs`, `workflow.mjs`,
`marshalling.mjs` — the original integration chain, incl. the L1–L5 tool
return-value marshalling boundary (BigInt/non-finite/undefined returns).

### Scenario suites

End-to-end *agent scenarios* that read like product workflows:

- **`scenarios3-*.mjs`** (run via `test:scenarios3`): `scheduling`,
  `support-triage`, `etl`, `compliance`, `durable`, `pricing`, `inventory`,
  `analytics`, `knowledge`, `statemachine`.
- **`scenarios-*.mjs`** / **`scenarios2-*.mjs`** (broad language/data/datetime/
  text/errors/sessions/collections/numbers/workflows sweeps).

### Running it

```bash
cd packages/zapcode-ai
npm install                     # first time only

npm run test:e2e               # original integration chain
npm run test:e2e-conformance   # the new e2e-*.mjs conformance/integration suites
npm run test:scenarios3        # the agent scenario suites
npm run test:e2e-full          # everything above, in one gate

# a single suite:
node tests/e2e-session-workflows.mjs
```

`test:e2e`, `test:e2e-conformance`, and `test:scenarios3` each first run
`sync-local-binding` (rebuilds the local `napi` addon from `crates/zapcode-js`)
and `build` (`tsc`), so they always exercise the current Rust VM rather than a
published binary.

---

## The full gate

```bash
# Tier 1
cargo test -p zapcode-core

# Tier 2
cd packages/zapcode-ai && npm run test:e2e-full
```

Both must be fully green. As of this writing the gate is:

- **Rust:** 2029 core + conformance tests pass (0 failed); the conformance layer
  alone is ~1392 `#[test]` functions across the `conformance_*.rs` files.
- **JS:** 237 e2e + scenario checks pass across the `test:e2e-full` chain
  (incl. `test:scenarios3`'s 77 checks).

---

## Adding a test

1. **Verify the expected value against real Node first** (`node -e '…'`). The
   suite asserts real-JS behavior.
2. If the case hits a **documented divergence**
   ([`STRESS-PASS-BUGS.md`](./STRESS-PASS-BUGS.md) — UTF-16 vs code-point string
   indexing, `Symbol.toPrimitive`, the match-result brand, async-generator
   mid-host-call suspension, a deferred-promise nuance), assert zapcode's
   **actual** behavior with an inline comment that names the divergence and the JS
   answer (`// JS: …`). Do **not** author a red test for a divergence; skip the
   case or pin the actual.
3. Group the test by feature with a descriptive `#[test]` name (Rust) or
   `test("…")` block (JS). Keep suites substantial and well-organized.
4. Run the relevant gate and confirm green before committing.
