# Workflow patterns (half-deterministic, half-agentic)

Runnable demos of the orchestration shapes you'd actually build with
[`@unchartedfr/zapcode-ai`](../../../packages/zapcode-ai): real control flow
(loops, conditionals, retries, compensation) with tool calls woven through it,
run deterministically in the Zapcode sandbox.

This example is **offline** — it uses mock tools and asserts every result, so it
runs with no API key. The same code is what `zapcode({ tools })` feeds to a
model; to watch a model generate it, see [`../ai-agent`](../ai-agent).

## Run

```bash
npm install
npm start
```

## What it shows

1. **Parallel fan-out** — `Promise.all` over tools suspends once; the host runs the batch concurrently, then deterministic aggregation runs in-sandbox.
2. **Retry with fallback** — a thrown tool is a real `Error` in the guest, so idiomatic `try/catch` retry + fallback works; console output is captured across the suspensions.
3. **Saga / compensation** — a failed step triggers compensating tool calls, asserted against real host-side effects.
4. **Durable session** — define a workflow, `dump()` the whole VM to bytes, and `loadSession()` to resume it (simulating another process / a Temporal activity).
5. **`dryRun` pre-flight** — typecheck + run against side-effect-free stubs before any real tool fires.
