# Zapcode hardening plan — durable agent workflows

Goal: make Zapcode a solid substrate for **agent-authored workflows that run later**, where the
**entire VM state serializes compactly** and can be passed between functions / Temporal activities.

Each numbered item is one commit. Core (Rust) changes land with `cargo test`; anything user-facing
also gets an **e2e test that actually runs TypeScript** through the napi binding.

## 0. e2e harness (prerequisite)
The zapcode-ai tests currently load a hand-copied `.node` in `node_modules` that drifts from the
local Rust build. Add a `sync-local-binding` step so `test:agent-scenarios` (and a new
`test:e2e`) rebuild the napi addon and link it before running. Without this, "e2e TS tests" don't
actually exercise local changes.

## Serialization / durability (Temporal-facing)
1. **Snapshot version + integrity header.** Prepend `[magic][format_version: u16][sha256: 32B]` to
   `dump()`; `load()`/`resume()` reject version mismatch *and* hash mismatch with actionable errors.
   Ported from Monty `serialization.rs` (`[version u16][sha256][postcard]`, `SERIALIZATION_VERSION=3`):
   they treat snapshot loading as **untrusted-input deserialization**, so the hash is a security
   control, not just corruption detection.
2. **Deterministic bytes.** Capture globals in a stable order so identical state → identical bytes
   (content-addressing, dedup, equality tests).
3. **Compact dump.** Remove the double `postcard` encode on `ZapcodeSnapshot`; add zstd
   compression behind the header. Cuts Temporal payload size.
6. **Cumulative resource accounting.** Carry allocation/state-size budget across `resume` so long
   sessions can't evade limits; enforce a max serialized-state size on `dump`. Adopt Monty's posture:
   resource breach is **host-visible and NOT catchable inside the sandbox** (it's not a normal
   exception), and where a size is predictable from arithmetic on inputs (`x.repeat(n)`, big string
   builds), **pre-check against the limit before allocating** rather than catching OOM after.

## Agent-workflow capability
4. **Error resume path (tri-state resume).** Host can resume a suspended call with a *value* or an
   *error*. An error becomes a **catchable** JS exception at the suspension point so guest
   `try { await tool() } catch {}` works; an uncaught one bubbles to the host unchanged. Ported from
   Monty `ExtFunctionResult { Return | Error | Future }` + PR #424 `resume_with_exception`.
5. **Parallel external calls.** `SuspendedMany { calls: [{ call_id, name, args }] }` + `resume_many`
   so `Promise.all([...])` suspends *once* with N pending calls keyed by `call_id` and the host runs
   them concurrently. Ported from Monty `RunProgress::ResolveFutures` + the scheduler's `call_id` model.
7. **Seedable Math.random.** Replace constant `0.5` with a PRNG seeded from snapshot state —
   deterministic across replay, but varied.

## High-level layer (the part agents actually use)
8. **zapcode-ai durable session bridge.** `createSession()` wrapping `ZapcodeSessionHandle` with
   tool validation + tracing + `dump()/resume()`, so a workflow survives across turns / activities.
9. **undefined/null optional handling.** Strip `undefined` keys (treat `null` as absent for
   optional params) so agent-written `{ metadata: undefined }` isn't rejected.
10. **Python parity.** Named-object args + host-side schema validation + `toolCalls` input shape in
    `zapcode-ai-python`.

## New items from Monty's recent direction (pydantic/monty)
A research pass on Monty (the Python analog that inspired Zapcode; v0.0.18, powers Pydantic AI
`codemode`) surfaced these. Items 1/4/5/6 above already absorbed their mechanisms; the rest are new:

11. **Optional type-check pre-pass.** Monty bundles Astral's `ty` to type-check agent code *before*
    running it. Zapcode analog: type-check the LLM-generated TS against host-provided tool stubs
    (we already emit `declare function` decls in the prompt) before execution, so malformed calls
    fail fast with a compile error the model can self-correct — cheaper than a runtime trap.
12. **(Noted, deferred) In-memory overlay filesystem.** Monty added a `MountTable` with a
    copy-on-write `OverlayMemory` mode (writes never touch the host) plus one-shot, descriptor-free
    host I/O so suspension points stay serializable. Out of scope for this pass; tracked as a future
    capability if agent workflows need scratch files. Their key constraint to copy if we ever do it:
    **no live host handles in a snapshot** — every read/write is a separate one-shot host call.

Monty design principles we're adopting repo-wide (not single commits):
- Treat every unbounded recursion / allocation as a **security bug**, not polish.
- Suspension points must hold **only re-derivable, serializable state** — strip transient caches
  before serializing (their PR #471).
- **Deny-by-default capabilities**: zero ambient FS/net/env; host injects only named functions
  (Zapcode already does this — keep it).

## Verification per commit
- Rust: `cargo test -p zapcode-core`, `cargo clippy -- -D warnings`, `cargo fmt --check`.
- TS e2e: `npm run test:e2e` in `packages/zapcode-ai` (rebuilds + links local binding first).
