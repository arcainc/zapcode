# Content-addressed programs — design

> Status: **proposal** (needs a decision on the durable-artifact contract before
> implementation). Lever identified in the cycle-N optimization pass; see
> `axes-roadmap.md` §2.

## Problem

Every snapshot serializes the program's bytecode. `VmSnapshot.programs` is a
`Vec<Arc<CompiledProgram>>`, and `Arc<T>` serializes as `T`, so the full
bytecode lands in the bytes on every `dump()`.

Measured (apple-silicon release, via the snapshot-size probe):

- snapshot size grows ~**15 B/statement** of source, on top of a ~527 B floor;
- a 300-statement program suspends to ~5.5 KB, of which the great majority is
  bytecode, not live state.

This is paid on *every* hop of a durable workflow. A session that suspends 20
times re-serializes the same program 20 times into 20 stored blobs. For the
"park 1000 agents of the same workflow" scenario it is 1000 copies of one
program on disk/in Redis.

In memory we already fixed this: a prepare-once fleet shares ONE
`Arc<CompiledProgram>` (cycle 2), so live RSS/VM is ~0 KB of program. **On the
wire we still pay per-snapshot.**

## What we already have

- **Arc-shared programs in memory** — `programs: Vec<Arc<CompiledProgram>>`;
  capture clones the refcount, not the bytecode (cycle 2).
- **Template elision** — the builtin-globals heap prefix is elided from the
  snapshot and spliced back from a per-process `OnceLock` on load, guarded by a
  `template_fingerprint`. This is the exact pattern to mirror: *something the
  loader can reconstruct need not be in the bytes.*
- **DEFLATE** — repeated keys/strings already dedupe well, so the residual
  growth term really is the (less-repetitive) bytecode.

## The contract tension (the decision)

Today a snapshot is a **self-contained durable artifact**: `dump()` → store the
bytes anywhere → `load(bytes)` → `resume()`, in any process, with nothing else.
The program is *in* the bytes, which is what makes this work.

Content-addressing breaks self-containment: the program travels/stores
**separately**, keyed by hash; the snapshot only references it. This is strictly
better for size and for the many-snapshots-one-program case, but the loader now
needs access to the program. That is the trade to accept or reject.

The builtin template gets away with a process `OnceLock` because it is *one set
per build*. User programs are arbitrary and per-code, so there is no global
singleton — **the host must supply the program (or a store) at load time.**

## Design

Mirror template elision, but for `programs`, with the program supplied by the
host instead of a process static.

### Wire (bumps `FORMAT_VERSION`; updates `tests/wire_format.rs`)

`VmSnapshot` gains, alongside the existing `heap_template_elided` /
`template_fingerprint`:

```rust
/// When true, `programs` is empty on the wire and the loader must splice in
/// programs matching `program_fingerprints` (same order).
programs_elided: bool,
/// FNV/sha fingerprint of each elided program, in `programs` order. Guards
/// against resuming against a different build of the same source.
program_fingerprints: Vec<u64>,
```

`dump()` stays **self-contained by default** (programs embedded, `programs_elided
= false`) — existing callers and existing blobs are unaffected. A new
`dump_referenced()` (or a `DumpMode` arg) elides the programs and records their
fingerprints.

### Load

```rust
// Self-contained (unchanged):
ZapcodeSnapshot::load(&bytes) -> Result<…>

// Referenced — caller supplies the programs (or a store) to splice back:
ZapcodeSnapshot::load_with_programs(&bytes, &[Arc<CompiledProgram>]) -> Result<…>
ZapcodeSnapshot::load_with_store(&bytes, &dyn ProgramStore) -> Result<…>
```

`ProgramStore` is a host-provided `fn get(fingerprint: u64) -> Option<Arc<CompiledProgram>>`.
On load of a referenced snapshot, each fingerprint is resolved through the store;
a miss is a clear `SnapshotError("program <hash> not available")`, a fingerprint
mismatch is `SnapshotError("program changed since capture")` — never a panic,
never silent corruption (the bytecode indexes the heap/handles, so a wrong
program would mis-resume everything).

### Bindings / zapcode-ai

- `ZapcodeProgramHandle` / `prepare()` already hold the `Arc<CompiledProgram>`,
  so they ARE the natural store: a `prepare`d program can `dump_referenced()` its
  suspensions and `load_with_programs()` them back for free.
- A `ProgramStore` impl over a `Map<u64, Arc<CompiledProgram>>` covers the
  "host caches its known workflows by hash" case (the real durable-fleet shape).
- The in-process **driver** (PR for lever #1) never serializes between hops, so
  it doesn't need this at all — content-addressing is specifically for the
  *persisted* / cross-process artifacts.

## Hazards

1. **Program unavailable on load.** A referenced snapshot resumed where the
   program isn't known → hard error, by design. The host opts into referencing
   precisely when it controls the program store; everyone else keeps
   self-contained `dump()`.
2. **Program drift.** Recompiling the same source on a new build can change
   bytecode; the per-program fingerprint catches it (same guard class as
   `template_fingerprint`).
3. **Program GC.** The store now owns program lifetime — a content-addressed
   store can refcount/evict by hash; document that evicting a program strands
   its referenced snapshots (same as losing the bytecode would).
4. **Mixed batches.** `programs` is a `Vec` (nested function programs); elide
   all-or-nothing per snapshot to keep the fingerprint vector simple.

## Expected payoff

- Referenced snapshots drop to ~the floor + live state (the ~527 B class)
  regardless of program size — e.g. the 300-stmt / 5.5 KB case → ~1 KB.
- N parked snapshots of one workflow store the program **once**, not N times.
- Zero change for self-contained callers (default `dump()` untouched).

## Plan

1. Wire fields + `FORMAT_VERSION` bump + `wire_format.rs` round-trip test
   (both self-contained and referenced).
2. Core: `dump_referenced()`, `load_with_programs()` / `load_with_store()`,
   fingerprinting (reuse the template-fingerprint hasher).
3. Bindings: `ZapcodeProgramHandle.dumpReferenced()` /
   `loadReferencedSnapshot(bytes, programHandle)`; a `ProgramStore` shim.
4. Bench: extend `profile_density.rs` with a referenced-mode column (per-VM
   stored bytes with the program shared) to quantify the win.
5. Gate: full e2e + differential + the 104-binary suite stay green.

## Open question for sign-off

Do we want referencing as an **opt-in** (default `dump()` stays self-contained
forever — recommended; zero risk to existing durability) or eventually the
**default** for sessions/prepare (smaller blobs, but the host must always have
the program)? The proposal above ships opt-in; flipping the default is a later,
separate decision once the store ergonomics are proven.
