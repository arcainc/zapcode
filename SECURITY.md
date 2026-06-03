# Zapcode security posture

This document records the threat model for the zapcode interpreter (a TypeScript-subset
interpreter that runs **untrusted, AI-agent-generated code**), why it is structurally immune
to the "Hack Monty 2" unsafe-UAF RCE class, and the DoS / resource-bypass / malicious-snapshot
findings that were confirmed and fixed during the defensive hardening pass.

Context: Pydantic's Hack Monty 2 bounty (https://pydantic.dev/articles/hack-monty-2). Monty's
Round-1 break was an RCE via a use-after-free (`list.sort` + a missing GC root in *unsafe* Rust)
that was used to read a `SECRET` env var out of the host.

## 1. Threat model

The sandbox runs guest code that we treat as fully adversarial. The host is a long-lived Node
process (via the `@unchartedfr/zapcode` napi addon used by `packages/zapcode-ai`), so the bar is:

> **A guest program must never be able to crash, hang, escape, or read host state out of the
> sandbox. Every adverse outcome must be a *catchable* error or a *fired resource limit*, never
> an uncatchable native abort and never a data leak.**

Concrete attacker goals we defend against:

- **(a) DoS** — crash the host (Rust panic or native-stack overflow across the napi boundary
  *aborts the whole Node process*: exit 134/SIGABRT or 139/SIGSEGV, uncatchable) or hang it.
- **(b) Resource-limit bypass** — allocate unbounded memory / CPU despite the configured limits.
- **(c) Sandbox escape** — reach `process`, `globalThis`, `require`, `eval`, `import`, the
  filesystem, the network, or a host `SECRET`/env var.
- **(d) Malicious snapshot** — a forged/tampered durable-session blob that, on load, raises its
  own resource limits or detonates a decompression bomb.

## 2. Why the Monty unsafe-UAF RCE class is structurally impossible here

Monty's RCE depended on properties zapcode does not have:

- **Zero `unsafe`.** `grep -rn unsafe crates/*/src` is empty across *every* crate
  (`zapcode-core`, `-js`, `-py`, `-wasm`). There is no place to corrupt memory.
- **No manual GC, no raw pointers.** The object heap is a *safe flat `Vec`*
  (`crates/zapcode-core/src/heap.rs`): `Value::Array`/`Value::Object` carry a `u32`
  `Handle` index into `Heap.slots`, never a pointer. Reference semantics, aliasing,
  identity, and snapshot round-tripping all fall out of plain integer indices. There is no
  GC root to forget and no freed slot to reuse, so a `list.sort`-style use-after-free has no
  analogue. A "dangling" handle would at worst index a valid-but-wrong safe slot (Rust bounds
  checks make an out-of-range index `None`, returning `undefined`/empty), which is a
  correctness bug, never memory corruption or code execution.
- **`panic = "unwind"` (the default — no `panic = "abort"` in any profile).** An ordinary Rust
  panic that did slip through unwinds and is caught by napi-rs at the FFI boundary, surfacing as
  a JS exception rather than aborting. The *only* uncatchable failure mode for safe Rust is a
  genuine **native-stack overflow (SIGSEGV)** from unbounded recursion — which is precisely the
  DoS class addressed in §3.

The realistic threat to a safe-Rust interpreter is therefore **DoS, resource bypass, escape, and
malicious-snapshot**, not memory-corruption RCE.

## 3. Confirmed-and-fixed findings

All fixes turn a would-be host abort / bypass into a **catchable error or a fired limit**. Fixed
in commit `79c417c` ("security: harden recursive value walkers, parse-depth, builtin allocs,
snapshot load") plus the earlier `f605aba` (bounded ToPrimitive). Tests live in
`crates/zapcode-core/tests/security.rs` (91 tests), `tests/wire_format.rs`, and
`packages/zapcode-ai/tests/marshalling.mjs`.

### (a) Native-stack-overflow / panic DoS — FIXED

Reference semantics let a guest build cycles (`const a=[]; a.push(a)`) and arbitrarily deep
structures. The recursive value walkers had no cycle guard and would overflow the native stack
(uncatchable SIGSEGV) when stringifying / cloning / marshalling such a value.

- `value.rs::to_js_string` — depth-bounded at `MAX_RENDER_DEPTH = 256`; renders a past-cap /
  cyclic node as a placeholder. `String()`, template literals, `Array.join`, and `console.log`
  of a cycle are now bounded, not fatal.
- `vm/builtins.rs::serialize_json` — now fallible with a visited-handle set + depth cap.
  `JSON.stringify` of a cycle throws the JS-faithful `TypeError: Converting circular structure
  to JSON`; over-deep acyclic input is a catchable `RuntimeError`.
- `heap.rs::deep_clone` — cycle-preserving (`HashMap<src,clone>` registered *before* descent,
  matching `structuredClone`) + depth cap. A cyclic value round-trips; a too-deep one is catchable.
- `crates/zapcode-js/src/lib.rs::value_to_json` (and the py/wasm equivalents) — the napi outbound
  marshaller is fallible with its own visited-set + `MAX_MARSHAL_DEPTH = 256`, threaded through
  every run/resume/run_chunk and tool-arg path. Returning, logging, or passing a cyclic/over-deep
  value to a tool is a catchable `napi::Error`, never a SIGSEGV.
- `parser/mod.rs::check_nesting_depth` — a string/comment/regex-aware bracket pre-scan rejects
  nesting beyond `MAX_NESTING_DEPTH = 64` with a `ParseError` *before* oxc's recursive descent
  can overflow the stack at parse time. Brackets inside string literals do not count.
- ToPrimitive recursion (`f605aba`) — a self-referential coercion hook
  (`toString(){ return ""+this }`) recursed past the old 200-deep guard and aborted the test
  binary; the guard is now 8 (each level nests a full guest-call loop on the native stack) and
  surfaces a catchable `RuntimeError("ToPrimitive recursion limit exceeded")`.
- Existing-and-verified loop/recursion limits: infinite recursion → `StackOverflow`, infinite
  loop → `TimeLimitExceeded`/`AllocationLimitExceeded`, infinite async generator → caught.

### (b) Resource-limit bypass via untracked builtin allocations — FIXED

Several builtins materialized large buffers *before* charging the resource tracker, so they could
exceed `memory_limit_bytes` regardless of the configured cap.

- `Array.from({length:n})` / `Array.of` — charge `track_array_capacity` (size computed without
  allocating) before materializing.
- `String.padStart` / `padEnd` / `concat` and `Array.join` — reject the projected size against the
  memory limit before building, via a shared `check_string_alloc` (mirrors the existing
  `String.repeat` guard).
- Sparse-array writes at huge / `MAX_SAFE_INTEGER` indices and `new Array(1e9)` hit a limit rather
  than allocating.

### (c) Sandbox escape — held (no escape found)

`process`, `globalThis`/`global`, `require`, `eval`, dynamic `import()`, `Function`/
`new Function`, `Reflect.construct(Function,…)`, `setTimeout`/`setInterval`, `performance.now`,
`import.meta`, `with`, indirect eval `(0,eval)(…)`, unicode-escaped/computed `eval`/`Function`/
`constructor` access, `Proxy`, and the `({}).constructor.constructor` chain are all blocked
(`crates/zapcode-core/src/sandbox.rs` + the interpreter). There is no host `Value` for env/fs/net,
so there is nothing to reach even if a guard were bypassed. Error messages are scrubbed of host
paths, crate names, and env vars (info-leak tests in `security.rs`).

### (d) Malicious snapshot — FIXED

The wire frame's SHA-256 is a *keyless integrity* check (detects corruption, not forgery), so an
attacker who controls stored bytes can recompute a valid hash. Defense does **not** rely on the
hash being unforgeable:

- `sandbox.rs::ResourceLimits::clamp_to_default` — on snapshot/session load
  (`snapshot.rs:185`, `session.rs:193-194`) every limit is clamped *down* to the build defaults,
  so a blob can never raise its own limits. A blob produced under *tighter* limits keeps them.
- `wire.rs::decode_frame` — DEFLATE inflation is bounded with `decompress_to_vec_with_limit` and
  the uncompressed payload is capped, so a decompression bomb is rejected before a multi-GB
  allocation. `JSON.parse` has its own `JSON_MAX_DEPTH = 64` guard.

## 4. Verification performed in this pass

- **`cargo test -p zapcode-core`** — green, **660 passed / 0 failed**.
- **`cargo build --workspace`** — clean, zero warnings (all four crates compiled).
- **`npm run test:e2e`** (production napi boundary) — green, including the 15 host-boundary
  marshalling checks (cycle/deep/limit) that previously SIGSEGV'd the host. Exit 0.
- **`npm run test:scenarios3`** — green (all 10 agent scenarios). Exit 0.
- **Independent adversarial replay** — a standalone harness ran 21 Monty-class attacks, each in a
  *fresh* node subprocess with a planted `SECRET=TOP_SECRET_FLAG` env var, and recorded the exit
  code/signal. **Result: 21/21 BOUNDED, 0 host-aborts, 0 SECRET leaks.** Every attack — cyclic
  return / stringify / tool-arg, 50k-deep return / string-coerce / parse, self-referential
  toString, infinite recursion/loop, `Array.from`/`new Array`/`padStart`/`repeat`/`join`
  over-allocation, `process`/`globalThis`/`require`/`eval`/`import`/constructor-chain escape, and
  a tampered session blob — surfaced a catchable JS error (e.g. `Converting circular structure to
  JSON`, `parse error: expression nesting depth exceeds the maximum of 64`, `memory limit
  exceeded`, `stack overflow (depth 513)`, `sandbox violation: process is forbidden`, `snapshot
  integrity check failed`). No probe produced exit 134/139 or printed the secret.

## 5. Residual risk (honest)

- **Correctness divergences from real JS, not security holes** (tracked in `STRESS-PASS-BUGS.md`):
  strings are indexed by Unicode code point not UTF-16 code unit (astral chars diverge);
  `[Symbol.toPrimitive]` is not dispatched (the common `valueOf`/`toString` path is); non-global
  regex match results are array-like objects rather than true `Array`s; `Object.freeze` is a
  no-op. None of these is a sandbox escape or a host-abort vector.
- **`~22 unwrap/expect/unreachable` in `vm/mod.rs`/`builtins.rs`** are interpreter-internal
  invariants (active-frame present, group-0 of an already-matched capture, validated radix in
  `2..=36`, validated callback). They are not reachable from guest input via any path exercised
  by the 91-test adversarial suite or the 21-probe replay. Because the build uses
  `panic = "unwind"`, even a hypothetical trip would be caught by napi and surfaced as a JS
  exception rather than aborting — only a native-stack overflow is uncatchable, and those paths
  are now all depth-capped.
- **Keyless snapshot integrity.** The wire SHA detects corruption, not forgery; authenticity of a
  persisted blob is the caller's responsibility (store it in trusted storage). The
  `clamp_to_default` defense means even a fully-forged blob cannot raise its limits, so the
  residual is "a forged blob could carry adversarial *but limit-bounded* guest state," not an
  escape.

## 6. Verdict

For the Hack Monty 2 threat model, **zapcode stands up well.** The specific Monty-1 RCE class
(unsafe-Rust use-after-free → arbitrary read of a host `SECRET`) is *structurally impossible*:
there is no `unsafe`, no manual GC, no raw pointer, and no host secret reachable as a guest value.
The realistic safe-Rust attack surface — native-stack-overflow DoS via the recursive value
walkers, untracked builtin over-allocation, and malicious-snapshot limit-raising — was the live
risk, and each confirmed finding is now bounded by a catchable error or a fired resource limit,
proven both by the in-tree suites and by an independent fresh-subprocess adversarial replay that
recorded zero host aborts and zero secret leaks. The remaining items are JS-correctness
divergences and internal-invariant `unwrap`s that are unreachable from guest input and, under
`panic = "unwind"`, would be catchable rather than fatal anyway.
