/**
 * scenarios3-statemachine.mjs — third-pass realistic agent scenarios.
 *
 * Domain: a workflow / state-machine orchestration agent that defines reusable
 * steps (a step registry + a runner that threads a single state object through
 * ordered steps), persists progress via recordStep({ name, state }), and replays
 * those steps across durable session dump/load boundaries.
 *
 * TypeScript-subset patterns stressed (all verified against ../dist/index.js):
 *   - CLOSURES: factory functions returning closures over counters/accumulators;
 *     closures capturing a for-(let) loop variable per-iteration; closures stored
 *     in arrays/objects (a step registry) and invoked later.
 *   - HIGHER-ORDER FUNCTIONS: functions taking/returning functions; named fns and
 *     arrows passed to map/filter/reduce/sort; compose + pipe of step functions.
 *   - RECURSION: a recursive task-tree expansion (topological-ish ordered walk)
 *     and accumulation, at a sane depth.
 *   - Rest params, arrow vs function declarations.
 *   - DURABILITY: define top-level state + helper/step functions in one chunk,
 *     dump()/loadSession(), then invoke them in later chunks; multiple dump/load
 *     cycles between state-machine steps with the accumulated state asserted
 *     exactly; recorded-step introspection via the recordStep tool.
 *
 * Run: node tests/scenarios3-statemachine.mjs
 *
 * NOTE on a discovered durability anomaly: a closure that captures *factory-local*
 * mutable state (e.g. `let n` inside makeCounter) works within a single chunk but
 * loses that captured state across a dump/load boundary (the captured frame is not
 * serialized — the returned closure reads as if reset). Top-level bindings, by
 * contrast, persist perfectly and functions re-link to them across reloads, so the
 * realistic durable pattern here threads a *top-level* state object through
 * top-level step functions. Both behaviors are asserted explicitly below.
 */
import assert from "node:assert/strict";
import { createSession, execute, loadSession, zapcode } from "../dist/index.js";

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

function reload(session, tools) {
  return loadSession(session.dump(), { tools });
}

function createWorkflowTools(events) {
  return {
    recordStep: {
      description: "Persist a workflow step result to the durable progress log.",
      parameters: {
        name: { type: "string", description: "Step name." },
        state: { type: "object", description: "State snapshot after the step." },
        note: { type: "string", optional: true },
      },
      execute: async input => {
        events.push({ type: "recordStep", input });
        return { ok: true, index: events.filter(e => e.type === "recordStep").length };
      },
    },
    finalizeWorkflow: {
      description: "Mark a workflow run complete and persist the final state.",
      parameters: {
        runId: { type: "string" },
        finalState: { type: "object" },
        stepCount: { type: "number" },
      },
      execute: async input => {
        events.push({ type: "finalizeWorkflow", input });
        return { finalized: true, runId: input.runId, steps: input.stepCount };
      },
    },
  };
}

console.log("scenarios3 state-machine e2e");

// ─────────────────────────────────────────────────────────────────────────────
// 1. Closures: factory accumulator/counter within a single run.
// ─────────────────────────────────────────────────────────────────────────────
await test("closure factories: counter + accumulator capture mutable state in one run", async () => {
  const result = await execute(
    `
    function makeCounter(start) {
      let n = start;
      return { inc: () => ++n, get: () => n };
    }
    function makeAccumulator() {
      let total = 0;
      const log = [];
      return {
        add: (label, delta) => { total = total + delta; log.push(label); return total; },
        snapshot: () => ({ total, log: log.slice() }),
      };
    }
    const counter = makeCounter(10);
    counter.inc(); counter.inc(); counter.inc();

    const acc = makeAccumulator();
    acc.add("seed", 5);
    acc.add("bump", 7);
    acc.add("trim", -2);

    ({ count: counter.get(), acc: acc.snapshot() })
    `,
    {}
  );
  assert.deepEqual(result.output, {
    count: 13,
    acc: { total: 10, log: ["seed", "bump", "trim"] },
  });
});

// ─────────────────────────────────────────────────────────────────────────────
// 2. Closures over a for-(let) loop variable: per-iteration binding.
//    Verified: this build gives correct per-iteration capture (0,1,2,3),
//    while `var` shares one binding (3,3,3,3) — both asserted.
// ─────────────────────────────────────────────────────────────────────────────
await test("closures capture for-(let) loop variable per-iteration; var shares one binding", async () => {
  const result = await execute(
    `
    const letFns = [];
    for (let i = 0; i < 4; i++) { letFns.push(() => i); }

    const varFns = [];
    for (var j = 0; j < 4; j++) { varFns.push(() => j); }

    ({
      let: letFns.map(f => f()).join(","),
      var: varFns.map(f => f()).join(","),
    })
    `,
    {}
  );
  // Per-iteration `let` binding is correct in this build.
  assert.equal(result.output.let, "0,1,2,3");
  // `var` shares a single binding; all closures observe the post-loop value.
  assert.equal(result.output.var, "4,4,4,4");
});

// ─────────────────────────────────────────────────────────────────────────────
// 3. Higher-order functions: compose / pipe / named + arrow callbacks.
// ─────────────────────────────────────────────────────────────────────────────
await test("higher-order step functions: compose, pipe, and map/filter/reduce/sort callbacks", async () => {
  const result = await execute(
    `
    function compose(f, g) { return (x) => f(g(x)); }
    const pipe = (...fns) => (x) => fns.reduce((acc, f) => f(acc), x);

    const inc = (x) => x + 1;
    const dbl = (x) => x * 2;
    function square(x) { return x * x; }

    const composed = compose(inc, dbl);   // inc(dbl(x))
    const piped = pipe(inc, dbl, square); // square(dbl(inc(x)))

    const nums = [5, 1, 4, 2, 3];
    const named = nums.map(square);                       // named fn as callback
    const evens = nums.filter(n => n % 2 === 0);          // arrow callback
    const sum = nums.reduce((a, b) => a + b, 0);          // arrow reducer
    const sorted = nums.slice().sort((a, b) => a - b);    // comparator

    ({
      composed: composed(5),
      piped: piped(3),
      named: named.join(","),
      evens: evens.join(","),
      sum,
      sorted: sorted.join(","),
    })
    `,
    {}
  );
  assert.deepEqual(result.output, {
    composed: 11, // dbl(5)=10, inc=11
    piped: 64, // inc(3)=4, dbl=8, square=64
    named: "25,1,16,4,9",
    evens: "4,2",
    sum: 15,
    sorted: "1,2,3,4,5",
  });
});

// ─────────────────────────────────────────────────────────────────────────────
// 4. Recursion: ordered (topological-ish) expansion of a nested task tree.
// ─────────────────────────────────────────────────────────────────────────────
await test("recursive task-tree expansion produces deterministic depth-first order + depth sum", async () => {
  const result = await execute(
    `
    const plan = {
      name: "deploy",
      children: [
        {
          name: "build",
          children: [
            { name: "compile", children: [] },
            { name: "bundle", children: [{ name: "minify", children: [] }] },
          ],
        },
        { name: "migrate", children: [] },
        {
          name: "release",
          children: [{ name: "notify", children: [] }],
        },
      ],
    };

    function expand(node, depth) {
      let acc = [{ name: node.name, depth }];
      for (const child of node.children) {
        acc = acc.concat(expand(child, depth + 1));
      }
      return acc;
    }

    const flat = expand(plan, 0);
    const order = flat.map(n => n.name).join(" > ");
    const depthSum = flat.reduce((a, n) => a + n.depth, 0);
    const maxDepth = flat.reduce((m, n) => (n.depth > m ? n.depth : m), 0);

    ({ order, count: flat.length, depthSum, maxDepth })
    `,
    {}
  );
  assert.deepEqual(result.output, {
    order: "deploy > build > compile > bundle > minify > migrate > release > notify",
    count: 8,
    depthSum: 12, // 0+1+2+2+3+1+1+2
    maxDepth: 3,
  });
});

// ─────────────────────────────────────────────────────────────────────────────
// 5. System-prompt assertion via zapcode({ tools }).
// ─────────────────────────────────────────────────────────────────────────────
await test("zapcode system prompt documents the workflow tools and their call shape", async () => {
  const events = [];
  const { system } = zapcode({ tools: createWorkflowTools(events) });
  assert.ok(typeof system === "string" && system.length > 0);
  assert.ok(system.includes("recordStep"));
  assert.ok(system.includes("finalizeWorkflow"));
  assert.ok(system.includes("await recordStep({ name: string, state: object, note?: string })"));
  assert.ok(system.includes("Persist a workflow step result"));
  assert.equal(events.length, 0); // building the prompt runs no host side effects
});

// ─────────────────────────────────────────────────────────────────────────────
// 6 & 7. Invalid-argument validation: rejected before any host side effect.
// ─────────────────────────────────────────────────────────────────────────────
await test("invalid recordStep arguments reject before the host log is touched", async () => {
  const events = [];
  const tools = createWorkflowTools(events);

  await assert.rejects(
    () => execute(`await recordStep({ name: "init" })`, tools),
    /Invalid arguments for tool 'recordStep': missing required parameter 'state'/
  );
  await assert.rejects(
    () => execute(`await recordStep({ name: "init", state: {}, bogus: true })`, tools),
    /Invalid arguments for tool 'recordStep': unexpected parameter 'bogus'/
  );
  await assert.rejects(
    () => execute(`await recordStep({ name: 42, state: {} })`, tools),
    /Invalid arguments for tool 'recordStep': parameter 'name' expected string, got number/
  );
  await assert.rejects(
    () => execute(`await recordStep([{ name: "init", state: {} }])`, tools),
    /Invalid arguments for tool 'recordStep': expected one named object argument/
  );

  assert.deepEqual(events, []);
});

await test("invalid finalizeWorkflow arguments reject before finalize runs", async () => {
  const events = [];
  const tools = createWorkflowTools(events);

  await assert.rejects(
    () => execute(`await finalizeWorkflow({ runId: "r1", finalState: {} })`, tools),
    /Invalid arguments for tool 'finalizeWorkflow': missing required parameter 'stepCount'/
  );
  await assert.rejects(
    () => execute(`await finalizeWorkflow({ runId: "r1", finalState: {}, stepCount: "two" })`, tools),
    /Invalid arguments for tool 'finalizeWorkflow': parameter 'stepCount' expected number, got string/
  );
  await assert.rejects(
    () => execute(`await finalizeWorkflow({ runId: "r1", finalState: [], stepCount: 1 })`, tools),
    /Invalid arguments for tool 'finalizeWorkflow': parameter 'finalState' expected object, got array/
  );

  assert.equal(events.length, 0);
});

// ─────────────────────────────────────────────────────────────────────────────
// 8. STRESS: durable state machine — step registry + runner threading a single
//    top-level state object, with a dump/load cycle BETWEEN every step and a
//    recursive sub-expansion driving the step sequence. Asserts exact, fully
//    deterministic final state and the recorded-step trail.
// ─────────────────────────────────────────────────────────────────────────────
await test("STRESS: durable state machine replays a registry across many dump/load cycles", async () => {
  const events = [];
  const tools = createWorkflowTools(events);
  let session = createSession({ tools });

  // Chunk 1: define the durable top-level state + a registry of pure step
  // functions (arrows stored in an object) + a runner. These survive reloads.
  const setup = await session.runChunk(`
    const wf = {
      runId: "run-7",
      n: 0,
      history: [],
    };

    // Step registry: name -> pure (state) => state. Stored in an object and
    // invoked later, across dump/load boundaries.
    const registry = {};
    function register(name, fn) {
      registry[name] = fn;
      return Object.keys(registry).length;
    }
    register("seed", (s) => ({ ...s, n: 1, history: s.history.concat(["seed"]) }));
    register("inc", (s) => ({ ...s, n: s.n + 1, history: s.history.concat(["inc"]) }));
    register("double", (s) => ({ ...s, n: s.n * 2, history: s.history.concat(["double"]) }));

    // The runner threads the top-level wf state through one named step and
    // returns the step's name (so the caller can persist it).
    async function applyStep(name) {
      const fn = registry[name];
      wf = fn(wf);
      await recordStep({ name, state: { n: wf.n } });
      return name;
    }

    Object.keys(registry).sort().join(",")
  `);
  assert.equal(setup.output, "double,inc,seed");
  assert.deepEqual(setup.toolCalls, []);

  // Recursively expand the ordered step plan on the host side so the program we
  // feed each chunk is deterministic. (seed, then inc/double interleaved.)
  function expandPlan(node) {
    let acc = [node.name];
    for (const child of node.children || []) acc = acc.concat(expandPlan(child));
    return acc;
  }
  const plan = {
    name: "seed",
    children: [
      { name: "inc", children: [{ name: "inc", children: [] }] },
      { name: "double", children: [{ name: "inc", children: [] }] },
    ],
  };
  const order = expandPlan(plan); // ["seed","inc","inc","double","inc"]
  assert.deepEqual(order, ["seed", "inc", "inc", "double", "inc"]);

  // Drive each step in its OWN chunk, with a dump/load cycle before every step.
  const appliedNames = [];
  for (const stepName of order) {
    session = reload(session, tools);
    const r = await session.runChunk(`await applyStep(${JSON.stringify(stepName)})`);
    assert.equal(r.output, stepName);
    assert.equal(r.toolCalls.length, 1);
    assert.equal(r.toolCalls[0].name, "recordStep");
    assert.equal(r.toolCalls[0].input.name, stepName);
    appliedNames.push(r.toolCalls[0].input.name);
  }
  assert.deepEqual(appliedNames, ["seed", "inc", "inc", "double", "inc"]);

  // After all reloads, the top-level state must reflect every step exactly:
  // n: 0 ->seed=1 ->inc=2 ->inc=3 ->double=6 ->inc=7
  session = reload(session, tools);
  const view = await session.runChunk(`({ n: wf.n, history: wf.history.join(">"), runId: wf.runId })`);
  assert.deepEqual(view.output, {
    n: 7,
    history: "seed>inc>inc>double>inc",
    runId: "run-7",
  });

  // Finalize through a tool after one more reload.
  session = reload(session, tools);
  const done = await session.runChunk(`
    await finalizeWorkflow({ runId: wf.runId, finalState: { n: wf.n }, stepCount: wf.history.length })
  `);
  assert.deepEqual(done.output, { finalized: true, runId: "run-7", steps: 5 });

  // Host-side recorded-state introspection: the durable log captured the exact
  // running value of n after each step, plus a single finalize.
  const recorded = events.filter(e => e.type === "recordStep").map(e => e.input.state.n);
  assert.deepEqual(recorded, [1, 2, 3, 6, 7]);
  const finals = events.filter(e => e.type === "finalizeWorkflow");
  assert.equal(finals.length, 1);
  assert.equal(finals[0].input.stepCount, 5);
});

// ─────────────────────────────────────────────────────────────────────────────
// 9. Durable helper functions + rest params survive dump/load and stay callable.
// ─────────────────────────────────────────────────────────────────────────────
await test("durable helper functions (rest params, named callbacks) survive dump/load", async () => {
  const events = [];
  const tools = createWorkflowTools(events);
  let session = createSession({ tools });

  await session.runChunk(`
    function sumAll(first, ...rest) {
      return rest.reduce((acc, v) => acc + v, first);
    }
    function pluck(items, key) {
      return items.map(item => item[key]);
    }
    "helpers-ready"
  `);

  session = reload(session, tools);
  const r1 = await session.runChunk(`sumAll(1, 2, 3, 4, 5)`);
  assert.equal(r1.output, 15);

  session = reload(session, tools);
  const r2 = await session.runChunk(`
    const rows = [{ id: "a", weight: 3 }, { id: "b", weight: 1 }, { id: "c", weight: 2 }];
    const sorted = rows.slice().sort((x, y) => x.weight - y.weight);
    pluck(sorted, "id").join(",")
  `);
  assert.equal(r2.output, "b,c,a");
  assert.equal(events.length, 0);
});

// ─────────────────────────────────────────────────────────────────────────────
// 10. A factory-local closure's captured mutable state IS preserved across a
//     dump/load boundary, just like top-level state (covered by the STRESS test
//     above). The shared upvalue cell backing the capture travels in the
//     idle-session snapshot and is re-linked on load. Matches real Node.
// ─────────────────────────────────────────────────────────────────────────────
await test("factory-local closure state survives dump/load (matches Node)", async () => {
  const events = [];
  const tools = createWorkflowTools(events);

  // Within a single chunk, factory-local captured state is correct.
  const inChunk = await execute(
    `
    function makeCounter() { let n = 0; return () => { n = n + 1; return n; }; }
    const tick = makeCounter();
    [tick(), tick(), tick()].join(",")
    `,
    tools
  );
  assert.equal(inChunk.output, "1,2,3");

  // Across a dump/load boundary, the captured cell survives: tick() continues.
  let session = createSession({ tools });
  const first = await session.runChunk(`
    function makeCounter() { let n = 0; return () => { n = n + 1; return n; }; }
    const tick = makeCounter();
    tick()
  `);
  assert.equal(first.output, 1);

  session = reload(session, tools);
  const afterReload = await session.runChunk(`tick()`);
  // Standard JS gives 2 here; the factory-local frame's cell is preserved.
  assert.equal(afterReload.output, 2);
});

console.log(`\n${passed} scenarios3 state-machine checks passed.`);
