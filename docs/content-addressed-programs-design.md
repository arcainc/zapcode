# Content-addressed programs — design

> Status: **reviewed, implementing the foundation**. Three independent
> pre-implementation reviews incorporated (see the box under "Design"). The
> `VmSnapshot` foundation + panic-safety is unblocked and being built; the
> session strategy (A/B/C below) needs sign-off. Lever from the optimization
> pass; see `axes-roadmap.md` §2.

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

> **Pre-implementation review (3 independent passes) — incorporated below.**
> Verdict: direction sound (mirror template elision), but four correctness items
> and one scope correction were required. Determinism prerequisite **verified**:
> `compile(source)` is byte-identical across runs (incl. classes/statics), so
> content-addressing is feasible. Key changes from the first draft: the
> fingerprint covers the WHOLE program (incl. external-functions), load is
> panic-safe, the primitive is `&[Arc<CompiledProgram>]` (not a `ProgramStore`
> trait), and the real beneficiary — sessions — has a genuine API fork.

## Design

Mirror template elision, but for `programs`, with the program supplied by the
host instead of a process static.

### Wire (bumps `FORMAT_VERSION` 16 → 17; updates `tests/wire_format.rs`)

`VmSnapshot` gains two **trailing** fields, alongside the existing
`heap_template_elided` / `template_fingerprint` (no `#[serde(default)]` — the
version guard hard-rejects old blobs before postcard runs, matching the v16
convention; `programs` keeps its position, emitted empty when elided):

```rust
/// When true, `programs` is empty on the wire; the loader splices in programs
/// matching `program_fingerprints` positionally (index i ↔ fingerprint i).
programs_elided: bool,
/// fnv1a fingerprint of each elided program, in `programs` order.
program_fingerprints: Vec<u64>,
```

**The fingerprint covers the WHOLE program, not just bytecode.** It is
`fnv1a(postcard::to_allocvec(program))` over the entire serialized
`CompiledProgram` *and* the program's `external_functions` set — because which
calls suspend is compiled into the bytecode at compile time, but
`external_functions` lives beside the program, so a fingerprint over bytecode
alone could match a program compiled with a different external set and desync
resume. Fingerprint = the exact bytes that are elided. **Computed from the actual
program on BOTH capture and load** — the loader never trusts a host-supplied
fingerprint; it hashes the program the host hands it and compares to the
snapshot's recorded fingerprint.

Why `u64` fnv (not sha256): the fingerprint is a **drift guard**, not an
integrity primitive. The frame's sha256 already covers all *stored* bytes
(including these fingerprint fields) against tampering, and the program is
supplied by the host's own store — never by the untrusted blob. So fnv's job is
only "is the program the host handed me the same build that was captured?", for
which u64 over the few-to-thousands distinct programs a host holds is ample
(birthday bound is irrelevant at that scale). Reuse `snapshot::fnv1a`, exactly as
`template_heap_bytes()` does. *If `ProgramStore` ever becomes blob-controlled,
this flips to requiring sha256 — documented so the assumption is explicit.*

`dump()` stays **self-contained by default** (`programs_elided = false`) —
existing callers and existing blobs are unaffected. New `dump_referenced()`
elides the programs and records fingerprints.

### Load — panic-safe by construction

```rust
ZapcodeSnapshot::load(&bytes)                                   // self-contained, unchanged
ZapcodeSnapshot::load_with_programs(&bytes, &[Arc<CompiledProgram>])  // referenced — PRIMITIVE
```

`load_with_programs` is the primitive (a `ProgramStore` convenience can layer on
later — the real callers below already hold the `Arc`s in order, so a fingerprint
→ program map buys nothing). Before any execution it **validates, returning
`SnapshotError` (never `panic!`)**:

1. `programs_elided` ⟹ supplied `programs.len() == program_fingerprints.len()`;
2. each supplied program's recomputed fnv1a == `program_fingerprints[i]`
   (mismatch → `"program N changed since capture"`);
3. every `program_index` / `func_index` reachable from frames, closures, and
   generators is in range for the spliced `programs` (length + per-program
   `functions.len()`).

This is required regardless of this feature — `from_snapshot` currently
`.expect()`s on these indices, so a malformed referenced snapshot would abort the
host process across the napi/PyO3 boundary (a sandbox-invariant-class failure).
Turning the reachable `.expect()`s into validated `SnapshotError`s is part of the
change (and hardens self-contained loads too).

### Sessions — the real beneficiary, and the API fork

Standalone `VmSnapshot` referencing (above) is clean but limited: only raw
`ZapcodeSnapshotHandle` / `forkSnapshot` / a `prepare`d suspension expose a
snapshot, and those callers already hold the program.

The workload that actually re-serializes programs per hop is the **session**:
`makeSession` writes `sessionBytes` after every chunk and every tool round-trip,
and each `runChunk` appends a freshly compiled chunk-program to
`IdleSessionState.programs` (`session.rs`) — a 20-chunk session carries and
re-writes all 20 programs on every dump. `ZapcodeSessionSnapshot` has its **own**
`programs` vec (separate from any `VmSnapshot.programs`), so the snapshot-only
change does NOT cover idle-between-chunks state. Sessions need the same elision on
`IdleSessionState`.

**The fork (needs sign-off):** unlike `prepare()` (host holds the program),
`loadSession(bytes, { tools })` is given *tools, not chunk source* — the chunk
programs only exist in the session bytes. So sessions cannot reference
transparently; the host must get the programs from somewhere on reload. Options:

- **(A) In-process program registry.** A bounded, refcounted process-level
  `Map<fnv1a, Arc<CompiledProgram>>` that `runChunk` populates as it compiles and
  `loadSession`/resume consult. Referencing becomes transparent *within a
  process* (the dominant case: one worker compiles and resumes), no API change.
  Cross-process still needs (B). Cost: a registry with a lifetime policy (LRU /
  capacity bound) so it can't leak programs forever.
- **(B) Explicit program persistence.** `session.dump()` returns/streams the
  referenced programs by hash; the host stores them and supplies them to
  `loadSession(bytes, { tools, programs })`. Fully durable cross-process, but a
  new API surface and host responsibility.
- **(C) Snapshots only for now.** Ship `VmSnapshot` referencing (prepare /
  forkSnapshot), leave sessions self-contained. Smaller win, zero session risk.

Recommendation: **(A) for the in-process fleet** (captures most of the win with
no API change), with **(B) as the opt-in cross-process escape hatch**. (C) is the
safe-but-small fallback if we want to ship incrementally.

### Bindings / zapcode-ai

- `ZapcodeProgramHandle` / `prepare()` already hold the `Arc<CompiledProgram>`,
  so a prepared suspension can `dumpReferenced()` and `loadWithPrograms()` for
  free. The referenced suspension result must also surface the program
  fingerprint(s) to JS so the host knows which program to keep.
- `index.d.ts` AND `index.js` are hand-maintained — every new `#[napi]` method
  needs a `.d.ts` line, every new class needs a `module.exports.X` line (the
  driver work hit exactly this).
- The in-process **driver** (lever #1) never serializes between hops, so it
  doesn't need this — content-addressing is for the *persisted* artifacts.

## Hazards

1. **Program unavailable on load** → hard `SnapshotError`, by design (no
   per-program process singleton to fall back to, unlike the template `OnceLock`).
2. **Program drift** → caught by the whole-program fnv1a (same guard class as
   `template_fingerprint`); relies on the verified compile determinism.
3. **Index out-of-range on a malformed referenced snapshot** → validated to a
   `SnapshotError` before execution, never a host-process panic (see Load).
4. **Program GC / lifetime.** Referencing shifts program lifetime to the store /
   registry; evicting a program strands its referenced snapshots (same as losing
   the bytecode). Option (A)'s registry needs a bounded eviction policy.
5. **All-or-nothing per snapshot.** `programs` is a `Vec` (a snapshot can
   reference >1 distinct program via `CallFrame.program_index`; nested *functions*
   are entries in one program's `functions` vec, not separate programs). Elide the
   whole vec; restore rebuilds it **positionally** from `program_fingerprints`.

## Expected payoff

- **Headline (post-DEFLATE-proof): dedup.** N parked snapshots / N session dumps
  of one workflow store the program **once**, not N times — the unassailable win.
- Per-snapshot shrink toward the floor + live state (~527 B class) for large
  programs — real, but **must be measured compressed** (current ~15 B/stmt /
  5.5 KB figures are pre-DEFLATE; bytecode is higher-entropy than repeated keys,
  so the post-DEFLATE delta is the honest number to quote). Lead with dedup.
- Zero change for self-contained callers (default `dump()` untouched).

## Plan

1. **Determinism guard** — a test asserting `compile(src)` is byte-identical
   across two compiles (locks the content-addressing premise; verified manually).
2. **Panic-safety** — validate program count + all reachable program/func indices
   in `from_snapshot`, returning `SnapshotError` (helps self-contained loads too).
3. **Wire** — `programs_elided` + `program_fingerprints` trailing fields,
   `FORMAT_VERSION` → 17 (+ changelog line in `wire.rs`), bump the hardcoded
   `16u16` in `wire_format.rs::forge_frame`.
4. **Core** — `dump_referenced()` + `load_with_programs()`; whole-program fnv1a
   (incl. `external_functions`); the same on `IdleSessionState` per the session
   decision (A/B/C).
5. **Bindings + zapcode-ai** — `dumpReferenced` / `loadWithPrograms` (+ surface
   fingerprints to JS); session integration per the decision; `.d.ts`/`.js` lines.
6. **Bench** — extend `profile_density.rs` with a referenced column measuring
   **compressed** per-VM stored bytes with the program shared.
7. **Tests** (`wire_format.rs`): referenced round-trip (+ assert `programs` empty
   on the wire), referenced < self-contained (compressed), fingerprint-mismatch →
   error, missing/length-mismatch → error, self-contained default unchanged.
8. **Gate** — full e2e + differential + the binary suite stay green.

## Open question for sign-off

**Which session strategy — (A) in-process registry, (B) explicit program
persistence, or (C) snapshots-only first?** This determines whether sessions (the
main win) get referencing now and how. The `VmSnapshot` foundation (prepare /
forkSnapshot, all the safety fixes) is unblocked and being built regardless;
referencing stays **opt-in** (`dump()` self-contained forever).
