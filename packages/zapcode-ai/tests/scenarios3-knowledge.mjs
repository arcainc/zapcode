/**
 * Realistic knowledge-base / document-retrieval & enrichment agent scenarios.
 *
 * A KB agent fans out across many document sources (Promise.all), merges and
 * dedupes hits, reranks by score, summarizes the top-K, then persists a digest.
 * Exercises async orchestration in diverse expression positions, mixed
 * parallel + sequential phases, partial-failure isolation under concurrency,
 * and agent-program throws.
 *
 * Proven-pattern notes (see tests/scenarios2-async.mjs):
 *   - Promise.all of direct tool calls / .map(async) works and preserves order.
 *   - A tool that throws inside Promise.all propagates to an OUTER try/catch
 *     (NOT to a .catch() on the batch); isolate per-source with a try/catch
 *     INSIDE the .map callback so siblings still resolve.
 *   - Tool errors arrive in guest catch as plain strings -> use String(e).
 *   - Array.sort() returns a sorted copy but does not mutate -> use the return value.
 *   - Concurrent tool calls may resolve in any host order -> sort before asserting.
 *
 * Run from packages/zapcode-ai:
 *   node tests/scenarios3-knowledge.mjs
 */
import assert from "node:assert/strict";
import { execute, zapcode } from "../dist/index.js";

let passed = 0;
async function test(name, fn) {
  try {
    await fn();
    passed++;
    console.log(`  PASS ${name}`);
  } catch (err) {
    console.error(`  FAIL ${name}`);
    throw err;
  }
}

console.log("scenarios3 knowledge-base e2e");

// Fixed, deterministic corpus per source. No Date.now / Math.random anywhere.
const CORPUS = {
  wiki: [
    { docId: "wiki-9", title: "Vector indexes", score: 0.71, snippet: "ann graph" },
    { docId: "shared-1", title: "Embeddings overview", score: 0.62, snippet: "dense vectors" },
  ],
  notion: [
    { docId: "notion-3", title: "Reranking runbook", score: 0.88, snippet: "cross encoder" },
    { docId: "shared-1", title: "Embeddings overview", score: 0.55, snippet: "dense vectors dup" },
  ],
  drive: [
    { docId: "drive-7", title: "Latency budget", score: 0.42, snippet: "p99 tail" },
    { docId: "notion-3", title: "Reranking runbook", score: 0.81, snippet: "cross encoder dup" },
  ],
  github: [
    { docId: "gh-2", title: "Retriever config", score: 0.95, snippet: "top_k tuning" },
  ],
  empty: [],
};

const SUMMARIES = {
  "gh-2": "Configure retriever top_k for recall.",
  "notion-3": "Use a cross-encoder reranker on top hits.",
  "wiki-9": "ANN graph indexes power vector search.",
  "shared-1": "Embeddings map text to dense vectors.",
  "drive-7": "Watch the p99 latency budget.",
};

/**
 * Build the KB tool set. `state` accumulates host-side side effects so tests can
 * assert that validation rejects BEFORE any persistence happens. `failSources`
 * is a Set of source names whose retrieve() deliberately throws.
 */
function createKnowledgeTools(state = {}, failSources = new Set()) {
  state.retrieved = state.retrieved ?? [];
  state.summarized = state.summarized ?? [];
  state.digests = state.digests ?? [];
  return {
    retrieveSource: {
      description: "Search one document source and return scored hits.",
      parameters: {
        source: { type: "string", description: "Source name to query." },
        query: { type: "string", description: "User search query." },
        limit: { type: "number", optional: true, description: "Max hits to return." },
      },
      execute: async ({ source, query, limit }) => {
        state.retrieved.push(source);
        if (failSources.has(source)) {
          throw new Error(`source '${source}' is unavailable`);
        }
        const hits = (CORPUS[source] ?? []).map(hit => ({ ...hit, source }));
        return typeof limit === "number" ? hits.slice(0, limit) : hits;
      },
    },
    summarizeDoc: {
      description: "Produce a one-line summary for a single document.",
      parameters: {
        docId: { type: "string", description: "Document id to summarize." },
      },
      execute: async ({ docId }) => {
        state.summarized.push(docId);
        return SUMMARIES[docId] ?? `No summary for ${docId}.`;
      },
    },
    saveDigest: {
      description: "Persist the final knowledge digest.",
      parameters: {
        sources: { type: "array", description: "Sources that contributed hits." },
        topHits: { type: "array", description: "Reranked top hits." },
        summary: { type: "string", description: "Rolled-up digest summary." },
        metadata: { type: "object", optional: true },
      },
      execute: async input => {
        state.digests.push(input);
        return { saved: true, hitCount: input.topHits.length };
      },
    },
  };
}

await test("system prompt exposes named-object signatures and call shapes for KB tools", async () => {
  const { system } = zapcode({ tools: createKnowledgeTools() });
  assert.match(
    system,
    /declare function retrieveSource\(input: \{ source: string; query: string; limit\?: number \}\): Promise<unknown>;/
  );
  // Single-parameter tools render as a positional signature, not a named object.
  assert.match(system, /declare function summarizeDoc\(docId: string\): Promise<unknown>;/);
  assert.match(system, /Call shape: await summarizeDoc\(docId: string\)/);
  assert.match(
    system,
    /declare function saveDigest\(input: \{ sources: array; topHits: array; summary: string; metadata\?: object \}\): Promise<unknown>;/
  );
  assert.match(system, /Call shape: await retrieveSource\(\{ source: string, query: string/);
});

await test("fans out retrieval with Promise.all over .map(async), merges + dedupes, reranks, summarizes top-K, saves digest", async () => {
  const state = {};
  const tools = createKnowledgeTools(state);
  const result = await execute(
    `
    const sources = ["wiki", "notion", "drive", "github"];

    // Phase 1 (PARALLEL): fan out one retrieve per source.
    const perSource = await Promise.all(
      sources.map(async source => {
        const hits = await retrieveSource({ source, query: "vector search" });
        return { source, hits };
      })
    );

    // Merge + dedupe by docId, keeping the highest score seen for each doc.
    // NOTE: flatten with flatMap rather than a nested for-of over entry.hits;
    // see BUG-NESTED-FOROF probe in this PR (inner for-of of an outer loop
    // variable's property stops after the first outer iteration).
    const allHits = perSource.flatMap(entry => entry.hits);
    const bestByDoc = new Map();
    for (const hit of allHits) {
      const prev = bestByDoc.get(hit.docId);
      if (!prev || hit.score > prev.score) {
        bestByDoc.set(hit.docId, hit);
      }
    }
    const merged = [];
    for (const hit of bestByDoc.values()) merged.push(hit);

    // Rerank by score desc (sort returns a NEW array in this sandbox; use it).
    const reranked = merged.sort((a, b) => b.score - a.score);
    const topK = reranked.slice(0, 3);

    // Phase 2 (SEQUENTIAL): summarize the top-K one at a time, in rank order.
    const summarized = [];
    for (const hit of topK) {
      const summary = await summarizeDoc({ docId: hit.docId });
      summarized.push({ docId: hit.docId, score: hit.score, summary });
    }

    const digestSummary = summarized.map(s => s.docId + ": " + s.summary).join(" | ");
    const contributing = sources.filter(s => perSource.find(e => e.source === s).hits.length > 0);

    await saveDigest({
      sources: contributing,
      topHits: topK.map(h => ({ docId: h.docId, source: h.source, score: h.score })),
      summary: digestSummary,
      metadata: { merged: merged.length, considered: reranked.length },
    });

    ({ topIds: topK.map(h => h.docId), summarized, contributing })
    `,
    tools
  );

  // shared-1 appears in wiki(0.62)+notion(0.55) -> dedupe keeps 0.62.
  // notion-3 appears in notion(0.88)+drive(0.81) -> dedupe keeps 0.88.
  // Reranked desc: gh-2(0.95), notion-3(0.88), shared-1(0.62), wiki-9(0.71)? -> recheck:
  //   scores: gh-2 0.95, notion-3 0.88, wiki-9 0.71, shared-1 0.62, drive-7 0.42
  //   topK(3) = gh-2, notion-3, wiki-9
  assert.deepEqual(result.output.topIds, ["gh-2", "notion-3", "wiki-9"]);
  assert.deepEqual(result.output.contributing, ["wiki", "notion", "drive", "github"]);
  assert.deepEqual(result.output.summarized, [
    { docId: "gh-2", score: 0.95, summary: "Configure retriever top_k for recall." },
    { docId: "notion-3", score: 0.88, summary: "Use a cross-encoder reranker on top hits." },
    { docId: "wiki-9", score: 0.71, summary: "ANN graph indexes power vector search." },
  ]);

  // 4 sources retrieved (any host order) + 3 summarize calls.
  assert.deepEqual([...state.retrieved].sort(), ["drive", "github", "notion", "wiki"]);
  assert.deepEqual(state.summarized, ["gh-2", "notion-3", "wiki-9"]);

  // Exactly one digest persisted; last toolCall is the saveDigest.
  assert.equal(state.digests.length, 1);
  assert.deepEqual(state.digests[0].topHits, [
    { docId: "gh-2", source: "github", score: 0.95 },
    { docId: "notion-3", source: "notion", score: 0.88 },
    { docId: "wiki-9", source: "wiki", score: 0.71 },
  ]);
  assert.equal(state.digests[0].metadata.merged, 5);
  assert.equal(result.toolCalls.at(-1).name, "saveDigest");
  assert.deepEqual(result.toolCalls.at(-1).input, state.digests[0]);
  assert.equal(result.toolCalls.filter(c => c.name === "retrieveSource").length, 4);
});

await test("STRESS: fan out over a sizable source list, isolate a deliberately-failing source, assert exact merged result", async () => {
  const state = {};
  const tools = createKnowledgeTools(state, new Set(["broken"]));
  const result = await execute(
    `
    // Sizable fan-out; "broken" is configured to throw on the host.
    const sources = ["wiki", "notion", "drive", "github", "empty", "broken"];

    // Per-source try/catch INSIDE the map keeps a failing source from rejecting
    // the whole Promise.all batch; siblings still resolve concurrently.
    const settled = await Promise.all(
      sources.map(async source => {
        try {
          const hits = await retrieveSource({ source, query: "reranking" });
          return { source, ok: true, hits };
        } catch (error) {
          return { source, ok: false, error: String(error), hits: [] };
        }
      })
    );

    const failures = settled.filter(s => !s.ok).map(s => s.source);
    const okSources = settled.filter(s => s.ok && s.hits.length > 0).map(s => s.source);

    // Merge + dedupe surviving hits by docId, keeping the best score.
    // flatMap (not nested for-of over entry.hits) — see BUG-NESTED-FOROF note above.
    const allHits = settled.flatMap(entry => entry.hits);
    const bestByDoc = new Map();
    for (const hit of allHits) {
      const prev = bestByDoc.get(hit.docId);
      if (!prev || hit.score > prev.score) bestByDoc.set(hit.docId, hit);
    }
    const merged = [];
    for (const hit of bestByDoc.values()) merged.push(hit);
    const reranked = merged.sort((a, b) => b.score - a.score);
    const topHits = reranked.slice(0, 2).map(h => ({ docId: h.docId, source: h.source, score: h.score }));

    await saveDigest({
      sources: okSources,
      topHits,
      summary: "partial digest with " + failures.length + " failed source(s)",
      metadata: { failures, recovered: okSources.length },
    });

    ({ failures, okSources, topHits, mergedCount: merged.length })
    `,
    tools
  );

  // Failing source isolated; siblings produced the deterministic merged result.
  assert.deepEqual(result.output.failures, ["broken"]);
  assert.deepEqual(result.output.okSources.sort(), ["drive", "github", "notion", "wiki"]);
  assert.equal(result.output.mergedCount, 5);
  assert.deepEqual(result.output.topHits, [
    { docId: "gh-2", source: "github", score: 0.95 },
    { docId: "notion-3", source: "notion", score: 0.88 },
  ]);

  // All 6 sources were attempted (any host order).
  assert.deepEqual([...state.retrieved].sort(), ["broken", "drive", "empty", "github", "notion", "wiki"]);

  // The failing source's toolCall carries .error; siblings carry results, not errors.
  const retrieveCalls = result.toolCalls.filter(c => c.name === "retrieveSource");
  assert.equal(retrieveCalls.length, 6);
  const failedCall = retrieveCalls.find(c => c.input.source === "broken");
  assert.ok(failedCall, "expected a retrieveSource call for the broken source");
  assert.match(String(failedCall.error), /source 'broken' is unavailable/);
  const okCalls = retrieveCalls.filter(c => c.input.source !== "broken");
  assert.equal(okCalls.length, 5);
  for (const call of okCalls) {
    assert.ok(!call.error, `sibling source '${call.input.source}' should not carry an error`);
  }

  // Despite the failure, exactly one digest was persisted with recovered data.
  assert.equal(state.digests.length, 1);
  assert.deepEqual(state.digests[0].metadata, { failures: ["broken"], recovered: 4 });
});

await test("await in diverse expression positions: array elements, object values, ternary, and nullish-then-await", async () => {
  const state = {};
  const tools = createKnowledgeTools(state);
  const result = await execute(
    `
    // await as object-literal values
    const heads = {
      github: await retrieveSource({ source: "github", query: "q", limit: 1 }),
      drive: await retrieveSource({ source: "drive", query: "q", limit: 1 }),
    };

    // await as array elements
    const firstIds = [heads.github[0].docId, heads.drive[0].docId];

    // await in a ternary branch (only the taken branch runs)
    const useGithub = heads.github[0].score > heads.drive[0].score;
    const winner = useGithub
      ? await summarizeDoc({ docId: heads.github[0].docId })
      : await summarizeDoc({ docId: heads.drive[0].docId });

    // cached ?? await f() — RHS only awaited because cached is null
    const cached = null;
    const backfill = cached ?? await summarizeDoc({ docId: "wiki-9" });

    ({ firstIds, winner, backfill })
    `,
    tools
  );

  assert.deepEqual(result.output.firstIds, ["gh-2", "drive-7"]);
  assert.equal(result.output.winner, "Configure retriever top_k for recall.");
  assert.equal(result.output.backfill, "ANN graph indexes power vector search.");
  // Ternary took the github branch -> drive doc never summarized.
  assert.deepEqual(state.summarized, ["gh-2", "wiki-9"]);
});

await test("try/finally records a cleanup phase around an awaited retrieval on the success path", async () => {
  const state = {};
  const tools = createKnowledgeTools(state);
  const result = await execute(
    `
    let connectionOpen = true;
    let hits;
    try {
      hits = await retrieveSource({ source: "github", query: "config" });
    } finally {
      // always release the source handle, even on success
      connectionOpen = false;
    }
    ({ count: hits.length, topId: hits[0].docId, connectionOpen })
    `,
    tools
  );

  assert.deepEqual(result.output, { count: 1, topId: "gh-2", connectionOpen: false });
  assert.deepEqual(state.retrieved, ["github"]);
});

await test("agent program throws when no usable sources are configured, and the throw propagates to the host", async () => {
  const state = {};
  const tools = createKnowledgeTools(state);
  await assert.rejects(
    () =>
      execute(
        `
        const sources = [];
        const perSource = await Promise.all(
          sources.map(async source => await retrieveSource({ source, query: "q" }))
        );
        const total = perSource.reduce((n, hits) => n + hits.length, 0);
        if (total === 0) {
          throw new Error("no usable knowledge sources configured");
        }
        total
        `,
        tools
      ),
    /no usable knowledge sources configured/
  );
  // Nothing retrieved (empty fan-out) and nothing persisted.
  assert.deepEqual(state.retrieved, []);
  assert.deepEqual(state.digests, []);
});

await test("rejects a missing required argument on retrieveSource before any host retrieval runs", async () => {
  const state = {};
  const tools = createKnowledgeTools(state);
  await assert.rejects(
    () => execute(`await retrieveSource({ source: "wiki" })`, tools),
    /Invalid arguments for tool 'retrieveSource': missing required parameter 'query'/
  );
  assert.deepEqual(state.retrieved, []);
});

await test("rejects wrong-typed arguments on saveDigest before any host persistence runs", async () => {
  const state = {};
  const tools = createKnowledgeTools(state);

  await assert.rejects(
    () => execute(`await saveDigest({ sources: "wiki", topHits: [], summary: "s" })`, tools),
    /Invalid arguments for tool 'saveDigest': parameter 'sources' expected array, got string/
  );
  await assert.rejects(
    () => execute(`await saveDigest({ sources: [], topHits: [], summary: 42 })`, tools),
    /Invalid arguments for tool 'saveDigest': parameter 'summary' expected string, got number/
  );
  await assert.rejects(
    () => execute(`await saveDigest({ sources: [], topHits: [], summary: "s", limit: 5 })`, tools),
    /Invalid arguments for tool 'saveDigest': unexpected parameter 'limit'/
  );
  await assert.rejects(
    () => execute(`await saveDigest([], [], "s")`, tools),
    /Invalid arguments for tool 'saveDigest': expected one named object argument/
  );

  assert.deepEqual(state.digests, []);
});

console.log(`\n${passed} knowledge-base checks passed.`);
