# In-run heap compaction — design

> Status: design → implementation. Closes the standing memory lead in
> `docs/axes-roadmap.md` ("the arena never frees *during* a run").

## Problem

The heap is an append-only arena (`Heap.slots: Vec<HeapSlot>`). Nothing
shrinks it during a run — `alloc_array`/`alloc_object` only push. Dead slots
(temporaries from a loop, intermediate arrays in a `map`/`filter`/`reduce`
chain) are reclaimed **only** at snapshot capture, which mark-compacts
(`VmSnapshot::compact_heap`). So a long *synchronous* stretch that churns
temporaries holds every dead slot until it either suspends (snapshot
compacts) or finishes.

Two consequences:
1. **Peak RSS** for such a stretch is the *total* allocated, not the *live*
   set — bad for hosts running many concurrent VMs.
2. **`memory_bytes` is cumulative**, never decremented (`track_memory` only
   `saturating_add`s). So `memory_limit_bytes` (default 32 MiB) is today an
   *allocation-volume* limit, not a *live-memory* limit: a churn loop trips
   it on total bytes ever allocated, even if live memory stays tiny.

Agent data-processing (group/aggregate/transform over collections, building
and discarding intermediates) is exactly the pattern that churns. This is
the workload the feature unblocks.

## What we already have

`VmSnapshot::compact_heap` + `Heap::compact_retaining(keep_prefix, roots)`:
an order-preserving mark-compact that (a) marks from a root set, retaining
the builtin-template prefix, (b) compacts survivors into a dense arena, (c)
rewrites every handle via the remap. `VmSnapshot::for_each_handle_mut`
enumerates the snapshot's root set (stack, frames, cells, globals,
continuations, last_receiver, pending_calls, resolved, pending_batch,
resume_action, microtasks, unhandled_rejections, async_tasks, timers).

In-run compaction is **the same rewrite, triggered during a run instead of
only at capture** — so it reuses `compact_retaining` and a root walk
structurally identical to the snapshot's.

## The two hazards, and why the design is safe

### Hazard 1 — handles in Rust locals (the dangerous one)

A compaction relocates slots and rewrites handles. Any live handle the
compactor *doesn't* know about becomes dangling → silent corruption. The
root walk covers all handle-bearing **VM fields**, but a handle sitting in a
**Rust local** across the compaction point would be missed.

Where do handles live in Rust locals? Inside the **nested interpreter
loops** — `call_method_internal` / `call_function_internal` / `to_primitive`
hold `Value`s (`hook`, `value`, args) in Rust locals across the nested
dispatch they drive. Compacting *there* would corrupt them.

**Resolution: only the top-level `execute()` loop compacts.** The nested
loops never call the trigger. At `execute()`'s loop top — before any frame
borrow, before the instruction fetch — every live handle is in a VM field:
the expression stack (`self.stack`), the call frames, or the continuation
records. Crucially, main-loop-driven array callbacks (`.map`/`.filter`/…)
keep their in-flight state in a `Continuation` (`ArrayMap.source`/`results`,
etc.) — which **is** a root — so churn inside those is safe to compact.
Only the short, bounded nested-Rust-call cases (a `valueOf` hook, a sort
comparator, a JSON reviver) are excluded, and they cannot churn unboundedly.

### Hazard 2 — determinism

Compaction reorders handles. This is **unobservable to guest code** (handles
are internal arena indices, never exposed) and the heap *contents* are
identical, only relocated. Two further guarantees:
- The trigger is **deterministic**: an instruction counter + `heap.len()`,
  never the wall clock. (The existing amortized `check_time` reads the clock,
  but its *counter* is deterministic; we gate GC on a counter, not time.)
- Snapshot bytes are **unaffected regardless of GC timing**, because
  `VmSnapshot::capture` re-compacts to a canonical order at capture time. So
  whether or not in-run GC ran, a captured snapshot is byte-identical.

## `memory_bytes` semantics: cumulative → live

On compaction we **recompute `memory_bytes` from the surviving slots**
(sum of element counts × `size_of::<Value>()`, consistent with how
`track_array_capacity` adds it). This turns `memory_limit_bytes` into a
*live-memory* ceiling — the intuitive, correct meaning — while:
- **DoS/total-work stays bounded** by `max_allocations` (count, kept
  cumulative — never reset by GC) and `time_limit_ms`.
- **The limit is never under-enforced**: between compactions `memory_bytes`
  still over-counts (garbage included), so a program can never exceed the
  true live limit undetected; after a compaction it is exact.

A single instruction that allocates a huge amount in one shot
(`new Array(huge)`) is still caught by the per-allocation `track_memory`
check — GC can't help a single oversized object, and shouldn't.

This is a deliberate behavior change: some programs that today trip the
cumulative limit will now run (their live set fits). That is the point, and
it matches what "memory limit" means everywhere else. Any test asserting the
old cumulative behavior is updated intentionally.

## Trigger heuristic

State (transient, not serialized — resets on resume, which is fine):
`gc_instr_counter: u32`, `heap_slots_at_last_gc: usize`.

At the `execute()` loop top, every `GC_CHECK_INTERVAL` (1024) instructions:
- O(1) check: if `heap.len() > max(MIN_GC_SLOTS, heap_slots_at_last_gc *
  GROWTH)` → run `compact_live_heap()`, then set `heap_slots_at_last_gc` to
  the post-GC live count.
- `MIN_GC_SLOTS = 4096` (don't scan small heaps), `GROWTH = 2` (compact when
  the heap has doubled since the last GC).

**Cost:** the per-tick check is O(1); a GC is O(live + edges) and fires at
most once per doubling, so amortized O(1) per allocation (geometric).
**Backoff is inherent:** setting `heap_slots_at_last_gc` to the *post-GC*
count means a mostly-live heap (little reclaimed) needs another full doubling
before the next attempt — no thrashing on incompressible heaps.

## Plan

1. `Vm::for_each_root_handle_mut(&mut dyn FnMut(&mut Handle))` — mirror of the
   snapshot walker over live VM fields (comment ties the two together).
2. `Vm::compact_live_heap()` — collect roots, `compact_retaining`, rewrite,
   recompute `memory_bytes`.
3. `Vm::maybe_compact_heap()` — the gated heuristic; called only at the
   `execute()` loop top.
4. Validation:
   - A GC-stress hook (a process-global `AtomicBool`, set via a test-only
     `enable_gc_stress_for_tests()` — no env access, respecting the sandbox
     invariant) that forces a compaction at **every** tick.
   - `gc_stress.rs`: run a batch of representative + churn-heavy programs
     under stress mode and assert outputs match non-stress. Maximally
     aggressive in-run rewriting that stays green is the proof the root walk
     is complete.
   - `examples/profile_inrun.rs`: a churn loop showing peak `slots`/
     `memory_bytes` bounded with GC vs unbounded without.
5. No wire-format change (purely in-memory; the transient counters aren't
   serialized).
