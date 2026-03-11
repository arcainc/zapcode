<p align="center">
  <h1 align="center">Zapcode</h1>
  <p align="center"><strong>"I have a cunning plan"</strong></p>
  <p align="center">A minimal, secure TypeScript interpreter written in Rust for use by AI</p>
</p>

<p align="center">
  <a href="https://github.com/TheUncharted/zapcode/actions"><img src="https://img.shields.io/github/actions/workflow/status/TheUncharted/zapcode/ci.yml?branch=main&label=CI" alt="CI"></a>
  <a href="https://www.npmjs.com/package/@zapcode/core"><img src="https://img.shields.io/npm/v/@zapcode/core" alt="npm"></a>
  <a href="https://pypi.org/project/zapcode/"><img src="https://img.shields.io/pypi/v/zapcode" alt="PyPI"></a>
  <a href="https://github.com/TheUncharted/zapcode/blob/main/LICENSE"><img src="https://img.shields.io/github/license/TheUncharted/zapcode" alt="License"></a>
</p>

---

> **Experimental** — Zapcode is under active development. APIs may change.

Named after Blackadder's eternally optimistic servant — because every AI agent says *"I have a cunning plan"* right before writing code to execute. Zapcode runs that code safely.

When LLMs write TypeScript, you need to run it safely. Containers add hundreds of milliseconds of startup overhead and operational complexity. V8 isolates are fast but bring a 20MB+ runtime and a massive attack surface.

Zapcode takes a different approach: a purpose-built TypeScript interpreter that starts in **under 2 microseconds**, enforces a security sandbox at the language level, and can snapshot execution state to bytes for later resumption — all in a single, embeddable library with zero dependencies on Node.js or V8.

## Benchmarks

All benchmarks run on the full pipeline: parse → compile → execute. No caching, no warm-up.

| Benchmark | Zapcode | Docker + Node.js | V8 Isolate |
|---|---|---|---|
| Simple expression (`1 + 2 * 3`) | **2.1 µs** | ~200-500 ms | ~5-50 ms |
| Variable arithmetic | **2.8 µs** | — | — |
| String concatenation | **2.6 µs** | — | — |
| Template literal | **2.9 µs** | — | — |
| Array creation | **2.4 µs** | — | — |
| Object creation | **5.2 µs** | — | — |
| Function call | **4.6 µs** | — | — |
| Loop (100 iterations) | **77.8 µs** | — | — |
| Fibonacci (n=10, 177 calls) | **138.4 µs** | — | — |
| Snapshot size (typical agent) | **< 2 KB** | N/A | N/A |
| Memory per execution | **~10 KB** | ~50+ MB | ~20+ MB |
| Cold start | **~2 µs** | ~200-500 ms | ~5-50 ms |

No background thread, no GC, no runtime — CPU usage is exactly proportional to the instructions executed.

Run benchmarks: `cargo bench`

## Quick start

### One-line install

```bash
curl -fsSL https://raw.githubusercontent.com/TheUncharted/zapcode/master/install.sh | bash
```

The script auto-detects your project type (TypeScript, Python, Rust, WASM), installs prerequisites, and builds native bindings. Or specify explicitly:

```bash
curl -fsSL ... | bash -s -- --lang ts      # TypeScript / JavaScript
curl -fsSL ... | bash -s -- --lang python   # Python
curl -fsSL ... | bash -s -- --lang rust     # Rust
curl -fsSL ... | bash -s -- --lang wasm     # WebAssembly
```

### Install by language

<details>
<summary><strong>Rust</strong></summary>

```toml
[dependencies]
zapcode-core = { git = "https://github.com/TheUncharted/zapcode.git" }
```
</details>

<details>
<summary><strong>JavaScript / TypeScript (Node.js)</strong></summary>

Once published to npm (coming soon):

```bash
npm install @zapcode/core    # npm
yarn add @zapcode/core       # yarn
pnpm add @zapcode/core       # pnpm
bun add @zapcode/core        # bun
```

Until then, build from source — requires Rust toolchain:

```bash
git clone https://github.com/TheUncharted/zapcode.git
cd zapcode/crates/zapcode-js
npm install && npm run build

# Link into your project
npm link                     # in zapcode-js/
npm link @zapcode/core      # in your project
```
</details>

<details>
<summary><strong>Python</strong></summary>

Once published to PyPI (coming soon):

```bash
pip install zapcode         # pip
uv add zapcode              # uv (Astral)
```

Until then, build from source — requires Rust toolchain + [maturin](https://github.com/PyO3/maturin):

```bash
# With uv (recommended)
uv tool install maturin
git clone https://github.com/TheUncharted/zapcode.git
cd zapcode/crates/zapcode-py
maturin develop --release --uv

# With pip
pip install maturin
git clone https://github.com/TheUncharted/zapcode.git
cd zapcode/crates/zapcode-py
maturin develop --release
```
</details>

<details>
<summary><strong>WebAssembly</strong></summary>

Requires [wasm-pack](https://rustwasm.github.io/wasm-pack/):

```bash
git clone https://github.com/TheUncharted/zapcode.git
cd zapcode/crates/zapcode-wasm
wasm-pack build --target web
```

This outputs a `pkg/` directory you can import in any browser or bundler.
</details>

## Usage

### With Vercel AI SDK (`@zapcode/ai`)

The recommended way — one call gives you `{ system, tools }` that plug directly into `generateText` / `streamText`:

```typescript
import { zapcode } from "@zapcode/ai";
import { generateText } from "ai";
import { anthropic } from "@ai-sdk/anthropic";

const { system, tools } = zapcode({
  system: "You are a helpful travel assistant.",
  tools: {
    getWeather: {
      description: "Get current weather for a city",
      parameters: { city: { type: "string", description: "City name" } },
      execute: async ({ city }) => {
        const res = await fetch(`https://api.weather.com/${city}`);
        return res.json();
      },
    },
    searchFlights: {
      description: "Search flights between two cities",
      parameters: {
        from: { type: "string" },
        to: { type: "string" },
        date: { type: "string" },
      },
      execute: async ({ from, to, date }) => {
        return flightAPI.search(from, to, date);
      },
    },
  },
});

// Works with any AI SDK model — Anthropic, OpenAI, Google, etc.
const { text } = await generateText({
  model: anthropic("claude-sonnet-4-20250514"),
  system,
  tools,
  messages: [{ role: "user", content: "Weather in Tokyo and cheapest flight from London?" }],
});
```

Under the hood: the LLM writes TypeScript code that calls your tools → Zapcode executes it in a sandbox → tool calls suspend the VM → your `execute` functions run on the host → results flow back in. All in ~2µs startup + tool execution time.

See [`examples/typescript/ai-agent-zapcode-ai.ts`](examples/typescript/ai-agent-zapcode-ai.ts) for the full working example.

### With Anthropic SDK directly

<details>
<summary><strong>TypeScript</strong></summary>

```typescript
import Anthropic from "@anthropic-ai/sdk";
import { Zapcode, ZapcodeSnapshotHandle } from "@zapcode/core";

const tools = {
  getWeather: async (city: string) => {
    const res = await fetch(`https://api.weather.com/${city}`);
    return res.json();
  },
};

const client = new Anthropic();
const response = await client.messages.create({
  model: "claude-sonnet-4-20250514",
  max_tokens: 1024,
  system: `Write TypeScript to answer the user's question.
Available functions (use await): getWeather(city: string) → { condition, temp }
Last expression = output. No markdown fences.`,
  messages: [{ role: "user", content: "What's the weather in Tokyo?" }],
});

const code = response.content[0].type === "text" ? response.content[0].text : "";

// Execute + resolve tool calls via snapshot/resume
const sandbox = new Zapcode(code, { externalFunctions: ["getWeather"] });
let state = sandbox.start();
while (!state.completed) {
  const result = await tools[state.functionName](...state.args);
  state = ZapcodeSnapshotHandle.load(state.snapshot).resume(result);
}
console.log(state.output);
```

See [`examples/typescript/ai-agent-anthropic.ts`](examples/typescript/ai-agent-anthropic.ts).
</details>

<details>
<summary><strong>Python</strong></summary>

```python
import anthropic
from zapcode import Zapcode

client = anthropic.Anthropic()
response = client.messages.create(
    model="claude-sonnet-4-20250514",
    max_tokens=1024,
    system="""Write TypeScript to answer the user's question.
Available functions (use await): getWeather(city: string) → { condition, temp }
Last expression = output. No markdown fences.""",
    messages=[{"role": "user", "content": "What's the weather in Tokyo?"}],
)
code = response.content[0].text

sandbox = Zapcode(code, external_functions=["getWeather"])
state = sandbox.start()
while state.get("suspended"):
    result = get_weather(*state["args"])
    state = state["snapshot"].resume(result)
print(state["output"])
```

See [`examples/python/ai_agent_anthropic.py`](examples/python/ai_agent_anthropic.py).
</details>

### Multi-SDK support

`zapcode()` returns adapters for all major AI SDKs from a single call:

```typescript
const { system, tools, openaiTools, anthropicTools, handleToolCall } = zapcode({
  tools: { getWeather: { ... } },
});

// Vercel AI SDK
await generateText({ model: anthropic("claude-sonnet-4-20250514"), system, tools, messages });

// OpenAI SDK
await openai.chat.completions.create({
  messages: [{ role: "system", content: system }, ...userMessages],
  tools: openaiTools,
});

// Anthropic SDK
await anthropic.messages.create({ system, tools: anthropicTools, messages });

// Any SDK — just extract the `code` from the tool call and pass it to handleToolCall
const result = await handleToolCall(codeFromToolCall);
```

Python:

```python
b = zapcode(tools={...})
b.anthropic_tools  # → Anthropic SDK format
b.openai_tools     # → OpenAI SDK format
b.handle_tool_call(code)  # → Universal handler
```

### Custom adapters

Building a new AI SDK or framework? You can write a custom adapter without forking Zapcode:

<details>
<summary><strong>TypeScript</strong></summary>

```typescript
import { zapcode, createAdapter } from "@zapcode/ai";

// Create a typed adapter for your SDK
const myAdapter = createAdapter("my-sdk", (ctx) => {
  // ctx gives you: system, toolName, toolDescription, toolSchema, handleToolCall
  return {
    systemMessage: ctx.system,
    actions: [{
      id: ctx.toolName,
      schema: ctx.toolSchema,
      run: async (input: { code: string }) => {
        return ctx.handleToolCall(input.code);
      },
    }],
  };
});

const { custom } = zapcode({
  tools: { ... },
  adapters: [myAdapter],
});

const myConfig = custom["my-sdk"];
// { systemMessage: "...", actions: [{ id: "execute_code", ... }] }
```
</details>

<details>
<summary><strong>Python</strong></summary>

```python
from zapcode_ai import zapcode, Adapter, AdapterContext

class LangChainAdapter(Adapter):
    name = "langchain"

    def adapt(self, ctx: AdapterContext):
        from langchain_core.tools import StructuredTool
        return StructuredTool.from_function(
            func=lambda code: ctx.handle_tool_call(code),
            name=ctx.tool_name,
            description=ctx.tool_description,
        )

b = zapcode(
    tools={...},
    adapters=[LangChainAdapter()],
)

langchain_tool = b.custom["langchain"]
```
</details>

The adapter receives an `AdapterContext` with everything needed: system prompt, tool name, tool JSON schema, and a `handleToolCall` function. Return whatever shape your SDK expects.

### Basic usage by language

<details>
<summary><strong>TypeScript / JavaScript</strong></summary>

```typescript
import { Zapcode, ZapcodeSnapshotHandle } from '@zapcode/core';

// Simple expression
const b = new Zapcode('1 + 2 * 3');
console.log(b.run().output);  // 7

// With inputs
const greeter = new Zapcode(
    '`Hello, ${name}! You are ${age} years old.`',
    { inputs: ['name', 'age'] },
);
console.log(greeter.run({ name: 'Zapcode', age: 30 }).output);

// Data processing
const processor = new Zapcode(`
    const items = [
        { name: "Widget", price: 25.99, qty: 3 },
        { name: "Gadget", price: 49.99, qty: 1 },
    ];
    const total = items.reduce((sum, i) => sum + i.price * i.qty, 0);
    { total, names: items.map(i => i.name) }
`);
console.log(processor.run().output);
// { total: 127.96, names: ["Widget", "Gadget"] }

// External function (snapshot/resume)
const app = new Zapcode(`const data = await fetch(url); data`, {
    inputs: ['url'],
    externalFunctions: ['fetch'],
});
const state = app.start({ url: 'https://api.example.com' });
if (!state.completed) {
    console.log(state.functionName);  // "fetch"
    const snapshot = ZapcodeSnapshotHandle.load(state.snapshot);
    const final_ = snapshot.resume({ status: 'ok' });
    console.log(final_.output);  // { status: "ok" }
}

// Classes
const counter = new Zapcode(`
    class Counter {
        count: number;
        constructor(start: number) { this.count = start; }
        increment() { return ++this.count; }
    }
    const c = new Counter(10);
    [c.increment(), c.increment(), c.increment()]
`);
console.log(counter.run().output);  // [11, 12, 13]
```

See [`examples/typescript/basic.ts`](examples/typescript/basic.ts) for the full example.
</details>

<details>
<summary><strong>Rust</strong></summary>

```rust
use zapcode_core::{ZapcodeRun, Value, ResourceLimits, VmState};

// Simple expression
let runner = ZapcodeRun::new(
    "1 + 2 * 3".to_string(), vec![], vec![],
    ResourceLimits::default(),
)?;
assert_eq!(runner.run_simple()?, Value::Int(7));

// With inputs and external functions (snapshot/resume)
let runner = ZapcodeRun::new(
    r#"const weather = await getWeather(city);
       `${city}: ${weather.condition}, ${weather.temp}°C`"#.to_string(),
    vec!["city".to_string()],
    vec!["getWeather".to_string()],
    ResourceLimits::default(),
)?;

let state = runner.start(vec![
    ("city".to_string(), Value::String("London".into())),
])?;

if let VmState::Suspended { snapshot, .. } = state {
    let weather = Value::Object(indexmap::indexmap! {
        "condition".into() => Value::String("Cloudy".into()),
        "temp".into() => Value::Int(12),
    });
    let final_state = snapshot.resume(weather)?;
    // VmState::Complete("London: Cloudy, 12°C")
}
```

See [`examples/rust/basic.rs`](examples/rust/basic.rs) for the full example.
</details>

<details>
<summary><strong>Python</strong></summary>

```python
from zapcode import Zapcode, ZapcodeSnapshot

# Simple expression
b = Zapcode("1 + 2 * 3")
print(b.run()["output"])  # 7

# With inputs
b = Zapcode(
    '`Hello, ${name}!`',
    inputs=["name"],
)
print(b.run({"name": "Zapcode"})["output"])  # "Hello, Zapcode!"

# External function (snapshot/resume)
b = Zapcode(
    "const w = await getWeather(city); `${city}: ${w.temp}°C`",
    inputs=["city"],
    external_functions=["getWeather"],
)
state = b.start({"city": "London"})
if state.get("suspended"):
    result = state["snapshot"].resume({"condition": "Cloudy", "temp": 12})
    print(result["output"])  # "London: 12°C"

# Snapshot persistence
state = b.start({"city": "Tokyo"})
if state.get("suspended"):
    bytes_ = state["snapshot"].dump()          # serialize to bytes
    restored = ZapcodeSnapshot.load(bytes_)   # load from bytes
    result = restored.resume({"condition": "Clear", "temp": 26})
```

See [`examples/python/basic.py`](examples/python/basic.py) for the full example.
</details>

<details>
<summary><strong>WebAssembly (browser)</strong></summary>

```html
<script type="module">
import init, { Zapcode } from './zapcode-wasm/zapcode_wasm.js';

await init();

const b = new Zapcode(`
    const items = [10, 20, 30];
    items.map(x => x * 2).reduce((a, b) => a + b, 0)
`);
const result = b.run();
console.log(result.output);  // 120
</script>
```

See [`examples/wasm/index.html`](examples/wasm/index.html) for a full playground.
</details>

## What Zapcode can and cannot do

### Can do

- **Execute a useful subset of TypeScript** — variables, functions, classes, generators, async/await, closures, destructuring, spread/rest, optional chaining, nullish coalescing, template literals, try/catch
- **Strip TypeScript types** at parse time via [oxc](https://oxc.rs) — no `tsc` needed
- **Snapshot execution to bytes** and resume later, even in a different process or machine
- **Call from Rust, Node.js, Python, or WebAssembly**
- **Track and limit resources** — memory, allocations, stack depth, and wall-clock time
- **30+ string methods, 25+ array methods**, plus Math, JSON, Object, and Promise builtins

### Cannot do

- Run arbitrary npm packages or the full Node.js standard library
- Execute regular expressions (parsing supported, execution is a no-op)
- Provide full `Promise` semantics (`.then()` chains, `Promise.race`, etc.)
- Run code that requires `this` in non-class contexts

These are intentional constraints, not bugs. Zapcode targets one use case: **running code written by AI agents** inside a secure, embeddable sandbox.

<details>
<summary><strong>Full supported syntax table</strong></summary>

| Feature | Status |
|---|---|
| Variables (`const`, `let`) | Supported |
| Functions (declarations, arrows, expressions) | Supported |
| Classes (`constructor`, methods, `extends`, `super`, `static`) | Supported |
| Generators (`function*`, `yield`, `.next()`) | Supported |
| Async/await | Supported |
| Control flow (`if`, `for`, `while`, `do-while`, `switch`, `for-of`) | Supported |
| Try/catch/finally, `throw` | Supported |
| Closures with mutable capture | Supported |
| Destructuring (object and array) | Supported |
| Spread/rest operators | Supported |
| Optional chaining (`?.`) | Supported |
| Nullish coalescing (`??`) | Supported |
| Template literals | Supported |
| Type annotations, interfaces, type aliases | Stripped at parse time |
| String methods (30+) | Supported |
| Array methods (25+, including `map`, `filter`, `reduce`) | Supported |
| Math, JSON, Object, Promise | Supported |
| `import` / `require` / `eval` | Blocked (sandbox) |
| Regular expressions | Parsed, not executed |
| `var` declarations | Not supported (use `let`/`const`) |
| Decorators | Not supported |
| `Symbol`, `WeakMap`, `WeakSet` | Not supported |
</details>

## Alternatives

| | Language completeness | Security | Startup | Snapshots | Setup |
|---|---|---|---|---|---|
| **Zapcode** | TypeScript subset | Language-level sandbox | **~2 µs** | Built-in, < 2 KB | `cargo add` / `npm install` |
| Docker + Node.js | Full Node.js | Container isolation | ~200-500 ms | No | Container runtime |
| V8 Isolates | Full JS/TS | Isolate boundary | ~5-50 ms | No | V8 (~20 MB) |
| Deno Deploy | Full TS | Isolate + permissions | ~10-50 ms | No | Cloud service |
| QuickJS | Full ES2023 | Process isolation | ~1-5 ms | No | C library |
| WASI/Wasmer | Depends on guest | Wasm sandbox | ~1-10 ms | Possible | Wasm runtime |

### Why not Docker?

Docker provides strong isolation but adds hundreds of milliseconds of cold-start latency, requires a container runtime, and doesn't support snapshotting execution state mid-function. For AI agent loops that execute thousands of small code snippets, the overhead dominates.

### Why not V8?

V8 is the gold standard for JavaScript execution. But it brings ~20 MB of binary size, millisecond startup times, and a vast API surface that must be carefully restricted for sandboxing. If you need full ECMAScript compliance, use V8. If you need microsecond startup, byte-sized snapshots, and a security model where "blocked by default" is the foundation rather than an afterthought, use Zapcode.

## Security

Running AI-generated code is inherently dangerous. Unlike Docker, which isolates at the OS level, Zapcode isolates at the **language level** — no container, no process boundary, no syscall filter. The sandbox must be correct by construction, not by configuration.

### Deny-by-default sandbox

Guest code runs inside a bytecode VM with no access to the host:

| Blocked | How |
|---|---|
| Filesystem (`fs`, `path`) | No `std::fs` in the core crate |
| Network (`net`, `http`, `fetch`) | No `std::net` in the core crate |
| Environment (`process.env`, `os`) | No `std::env` in the core crate |
| `eval`, `Function()`, dynamic import | Blocked at parse time |
| `import`, `require` | Blocked at parse time |
| `globalThis`, `global` | Blocked at parse time |
| Prototype pollution | Not applicable — objects are plain `IndexMap` values |

The **only** escape hatch is external functions that you explicitly register. When guest code calls one, the VM suspends and returns a snapshot — your code resolves the call, not the guest.

### Resource limits

| Limit | Default | Configurable |
|---|---|---|
| Memory | 32 MB | `memory_limit_bytes` |
| Execution time | 5 seconds | `time_limit_ms` |
| Call stack depth | 512 frames | `max_stack_depth` |
| Heap allocations | 100,000 | `max_allocations` |

Limits are checked during execution, so infinite loops, deep recursion, and allocation bombs are all caught.

### Zero `unsafe` code

The `zapcode-core` crate contains **zero `unsafe` blocks**. Memory safety is guaranteed by the Rust compiler. No FFI calls, no raw pointers, no transmutes.

<details>
<summary><strong>Adversarial test suite — 65 tests across 19 attack categories</strong></summary>

The sandbox is validated by **65 adversarial security tests** (`tests/security.rs`) that simulate real attack scenarios:

| Attack category | Tests | Result |
|---|---|---|
| Prototype pollution (`Object.prototype`, `__proto__`) | 4 | Blocked |
| Constructor chain escapes (`({}).constructor.constructor(...)`) | 3 | Blocked |
| `eval`, `Function()`, indirect eval, dynamic import | 5 | Blocked at parse time |
| `globalThis`, `process`, `require`, `import` | 6 | Blocked at parse time |
| Stack overflow (direct + mutual recursion) | 2 | Caught by stack depth limit |
| Memory exhaustion (huge arrays, string doubling) | 4 | Caught by allocation limit |
| Infinite loops (`while(true)`, `for(;;)`) | 2 | Caught by time/allocation limit |
| JSON bombs (deep nesting, huge payloads) | 2 | Depth-limited (max 64) |
| Sparse array attacks (`arr[1e9]`, `arr[MAX_SAFE_INTEGER]`) | 3 | Capped growth (max +1024) |
| toString/valueOf hijacking during coercion | 3 | Not invoked (by design) |
| Unicode escapes for blocked keywords | 2 | Blocked |
| Computed property access tricks | 2 | Returns undefined |
| Timing side channels (`performance.now`) | 1 | Blocked |
| Error message information leakage | 3 | No host paths/env exposed |
| Type confusion attacks | 4 | Proper TypeError |
| Promise/Generator internal abuse | 4 | No escape |
| Negative array indices | 2 | Returns undefined |
| `setTimeout`, `setInterval`, `Proxy`, `Reflect` | 6 | Blocked |
| `with` statement, `arguments.callee` | 3 | Blocked |

Run the security tests: `cargo test -p zapcode-core --test security`

**Known limitations:**
- `Object.freeze()` is not yet implemented — frozen objects can still be mutated (correctness gap, not a sandbox escape)
- User-defined `toString()`/`valueOf()` are not called during implicit type coercion (intentional — prevents injection)
</details>

## Architecture

```
TypeScript source
    │
    ▼
┌─────────┐   oxc_parser (fastest TS parser in Rust)
│  Parse  │──────────────────────────────────────────►  Strip types
└────┬────┘
     ▼
┌─────────┐
│   IR    │   ZapcodeIR (statements, expressions, operators)
└────┬────┘
     ▼
┌─────────┐
│ Compile │   Stack-based bytecode (~50 instructions)
└────┬────┘
     ▼
┌─────────┐
│   VM    │   Execute, snapshot at external calls, resume later
└────┬────┘
     ▼
  Result / Suspended { snapshot }
```

## Contributing

```bash
git clone https://github.com/TheUncharted/zapcode.git
cd zapcode

# Run all tests (214 tests)
cargo test

# Run benchmarks
cargo bench

# Check all crates (including bindings)
cargo check --workspace
```

## Why AI agents should write code

For motivation on why you might want LLMs to write and execute code instead of chaining tool calls:

- [CodeMode](https://blog.cloudflare.com/codemode-ai-agent-coding) from Cloudflare
- [Programmatic Tool Calling](https://docs.anthropic.com/en/docs/build-with-claude/tool-use/tool-use-examples#programmatic-tool-calling) from Anthropic
- [Code Execution with MCP](https://www.anthropic.com/engineering/code-execution-mcp) from Anthropic
- [Smol Agents](https://huggingface.co/docs/smolagents/en/index) from Hugging Face

Zapcode is inspired by [Monty](https://github.com/pydantic/monty), Pydantic's Python subset interpreter that takes the same approach for Python.

## License

MIT
