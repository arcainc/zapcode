<p align="center">
  <h1 align="center">Baldrick</h1>
  <p align="center"><strong>"I have a cunning plan"</strong></p>
  <p align="center">A minimal, secure TypeScript interpreter written in Rust for use by AI</p>
</p>

<p align="center">
  <a href="https://github.com/TheUncharted/baldrick/actions"><img src="https://img.shields.io/github/actions/workflow/status/TheUncharted/baldrick/ci.yml?branch=main&label=CI" alt="CI"></a>
  <a href="https://www.npmjs.com/package/@baldrick/core"><img src="https://img.shields.io/npm/v/@baldrick/core" alt="npm"></a>
  <a href="https://pypi.org/project/baldrick/"><img src="https://img.shields.io/pypi/v/baldrick" alt="PyPI"></a>
  <a href="https://github.com/TheUncharted/baldrick/blob/main/LICENSE"><img src="https://img.shields.io/github/license/TheUncharted/baldrick" alt="License"></a>
</p>

---

> **Experimental** — Baldrick is under active development. APIs may change.

Named after Blackadder's eternally optimistic servant — because every AI agent says *"I have a cunning plan"* right before writing code to execute. Baldrick runs that code safely.

When LLMs write TypeScript, you need to run it safely. Containers add hundreds of milliseconds of startup overhead and operational complexity. V8 isolates are fast but bring a 20MB+ runtime and a massive attack surface.

Baldrick takes a different approach: a purpose-built TypeScript interpreter that starts in **under 2 microseconds**, enforces a security sandbox at the language level, and can snapshot execution state to bytes for later resumption — all in a single, embeddable library with zero dependencies on Node.js or V8.

## What Baldrick can do

- **Execute a useful subset of TypeScript** with startup times measured in single-digit microseconds, not hundreds of milliseconds
- **Block all access to the host** — no filesystem, network, environment variables, `eval`, `import`, or `require`. The only way to interact with the outside world is through registered external functions that you control
- **Strip TypeScript types** at parse time — type annotations, interfaces, and type aliases are handled natively via [oxc](https://oxc.rs), no `tsc` needed
- **Snapshot execution to bytes** and resume later, potentially in a different process or on a different machine — enabling durable, interruptible agent workflows
- **Call from Rust, JavaScript/Node.js, Python, or WebAssembly** — thin binding layers over the same core engine
- **Track and limit resource usage** — memory, allocations, stack depth, and wall-clock time, all configurable per execution
- **Run async code** with `async`/`await` — external function calls suspend execution and return snapshots that the host resolves
- **Support classes, generators, closures, try/catch**, and the full set of built-in methods on strings, arrays, objects, Math, JSON, and Promise

## What Baldrick cannot do

- Run arbitrary npm packages or the full Node.js standard library
- Execute regular expressions (parsing supported, execution is a no-op)
- Provide full `Promise` semantics (`.then()` chains, `Promise.race`, etc.)
- Run code that requires `this` in non-class contexts (global `this` is blocked)

These are intentional constraints, not bugs. Baldrick targets one use case: **running code written by AI agents** inside a secure, embeddable sandbox.

## Security

Running AI-generated code is inherently dangerous. Unlike Docker, which isolates at the OS level with containers, Baldrick isolates at the **language level** — there is no container, no process boundary, and no syscall filter between guest code and your application. This means the sandbox must be correct by construction, not by configuration.

### How the sandbox works

Baldrick uses a **deny-by-default** architecture. Guest code runs inside a purpose-built bytecode VM that has no access to the host:

| Blocked | How |
|---|---|
| Filesystem (`fs`, `path`) | No `std::fs` in the core crate. Not importable, not reachable. |
| Network (`net`, `http`, `fetch`) | No `std::net` in the core crate. `fetch` is only available if you register it as an external function. |
| Environment (`process.env`, `os`) | No `std::env` in the core crate. `process` is a parse-time error. |
| Dynamic code execution (`eval`, `Function()`) | Blocked at parse time. There is no mechanism to compile new code at runtime inside the VM. |
| Module system (`import`, `require`) | Blocked at parse time. No module resolution, no dynamic imports. |
| Global escape hatches (`globalThis`, `global`) | Blocked at parse time. |
| Prototype pollution | Not applicable — objects are plain `IndexMap` values, not prototype-chained. |

### The only way out: external functions

The **only** way guest code can interact with the outside world is through external functions that you explicitly register:

```typescript
const b = new Baldrick(`const data = await fetch(url); data`, {
    inputs: ['url'],
    externalFunctions: ['fetch'],  // Only 'fetch' is callable
});
```

When guest code calls `fetch()`, the VM **suspends** and returns a snapshot. Your code runs the actual fetch, then resumes the VM with the result. The guest never touches the network — you do.

Calling an unregistered function produces `BaldrickError::UnknownExternalFunction`, not a silent no-op.

### Resource limits

Every execution is bounded. Guest code cannot exhaust host resources:

| Limit | Default | Configurable |
|---|---|---|
| Memory | 32 MB | `memory_limit_bytes` |
| Execution time | 5 seconds | `time_limit_ms` |
| Call stack depth | 512 frames | `max_stack_depth` |
| Heap allocations | 100,000 | `max_allocations` |

Limits are checked during execution (not just at boundaries), so infinite loops, deep recursion, and allocation bombs are all caught.

### What this means in practice

- **No container runtime needed.** No Docker, no Firecracker, no gVisor.
- **No V8 sandbox to configure.** No `--no-network`, no permission flags to forget.
- **No syscall filter to maintain.** No seccomp profiles, no AppArmor policies.
- **Microsecond startup.** No cold-start penalty means you can run thousands of snippets per second.
- **Trade-off: limited language surface.** You get a safe subset of TypeScript, not the full language. This is the price of the security guarantee.

### No `unsafe` code

The `baldrick-core` crate contains **zero `unsafe` blocks**. Memory safety is guaranteed by the Rust compiler. There are no FFI calls, no raw pointers, no transmutes.

## Performance

All benchmarks run on the full pipeline: parse → compile → execute. No caching, no warm-up.

| Benchmark | Median | What it does |
|---|---|---|
| Simple expression | **2.1 µs** | `1 + 2 * 3` |
| Variable arithmetic | **2.8 µs** | `const x = 10; const y = 20; x * y + 5` |
| String concatenation | **2.6 µs** | `"hello" + " " + "world"` |
| Template literal | **2.9 µs** | `` `hello ${name}` `` |
| Array creation | **2.4 µs** | `[1, 2, 3, 4, 5]` |
| Object creation | **5.2 µs** | `{ name: "test", value: 42, nested: { a: 1 } }` |
| Function call | **4.6 µs** | Define and call a function |
| Loop (100 iterations) | **77.8 µs** | `for` loop with arithmetic |
| Fibonacci (n=10) | **138.4 µs** | Recursive function, 177 calls |

Snapshot size for typical agent code with external calls: **< 2 KB**.

## Usage

### Rust

```rust
use baldrick_core::{BaldrickRun, Value, ResourceLimits};
use baldrick_core::vm::VmState;

// Compile once
let runner = BaldrickRun::new(
    r#"
        const response = await fetch(url);
        response + " processed"
    "#.to_string(),
    vec!["url".to_string()],
    vec!["fetch".to_string()],
    ResourceLimits::default(),
)?;

// Start execution — suspends at fetch()
let state = runner.start(vec![
    ("url".to_string(), Value::String("https://api.example.com".into())),
])?;

match state {
    VmState::Suspended { function_name, args, snapshot } => {
        assert_eq!(function_name, "fetch");

        // Serialize snapshot to bytes (store in DB, send over network, etc.)
        let bytes = snapshot.dump()?;

        // Later: restore and resume with the external function's result
        let restored = baldrick_core::BaldrickSnapshot::load(&bytes)?;
        let final_state = restored.resume(Value::String("response data".into()))?;

        match final_state {
            VmState::Complete(value) => {
                assert_eq!(value, Value::String("response data processed".into()));
            }
            _ => unreachable!(),
        }
    }
    VmState::Complete(value) => println!("Result: {}", value),
}
```

### JavaScript / Node.js

```bash
npm install @baldrick/core
```

```typescript
import { Baldrick, BaldrickSnapshotHandle } from '@baldrick/core';

const b = new Baldrick(`
    const data = await fetch(url);
    JSON.parse(data)
`, {
    inputs: ['url'],
    externalFunctions: ['fetch'],
    timeLimitMs: 5000,
});

// Sync run (no external calls)
const result = b.run({ url: '"hello"' });
if (result.completed) {
    console.log(result.output);  // "hello"
}

// Async run with snapshot/resume
const state = b.start({ url: '"https://api.example.com"' });
if (!state.completed) {
    console.log(state.functionName);  // "fetch"
    console.log(state.args);          // ["https://api.example.com"]

    // Resolve externally, then resume
    const snapshot = BaldrickSnapshotHandle.load(state.snapshot);
    const final = snapshot.resume('{"status": "ok"}');
    console.log(final.output);
}
```

### Python

```bash
pip install baldrick
```

```python
from baldrick import Baldrick, BaldrickSnapshot

b = Baldrick("""
    const result = await fetch(url);
    result + " done"
""", external_functions=["fetch"])

state = b.start({"url": "https://api.example.com"})

if state["suspended"]:
    print(state["function_name"])  # "fetch"
    print(state["args"])           # ["https://api.example.com"]

    snapshot = state["snapshot"]
    final = snapshot.resume("response data")
    print(final["output"])  # "response data done"
```

### WebAssembly

```javascript
import init, { Baldrick } from '@baldrick/wasm';

await init();

const b = new Baldrick('1 + 2 * 3');
const result = b.run();
console.log(result.output);  // 7
```

## Supported TypeScript Subset

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

## Alternatives

| | Language completeness | Security | Startup latency | Snapshotting | Setup complexity |
|---|---|---|---|---|---|
| **Baldrick** | TypeScript subset | Sandbox at language level | **~2 µs** | Built-in, < 2 KB | `cargo add` / `npm install` |
| Docker + Node.js | Full Node.js | Container isolation | ~200-500 ms | Not built-in | Container runtime required |
| V8 Isolates | Full JS/TS | Isolate boundary | ~5-50 ms | Not built-in | V8 linkage (~20 MB) |
| Deno Deploy | Full TS | Isolate + permissions | ~10-50 ms | Not built-in | Cloud service |
| QuickJS | Full ES2023 | Process isolation | ~1-5 ms | Not built-in | C library |
| WASI/Wasmer | Depends on guest | Wasm sandbox | ~1-10 ms | Possible | Wasm runtime |

### Why not just use V8?

V8 is the gold standard for JavaScript execution. But it brings ~20 MB of binary size, millisecond startup times, and a vast API surface that must be carefully restricted for sandboxing. If you need full ECMAScript compliance, use V8. If you need microsecond startup, byte-sized snapshots, and a security model where "blocked by default" is the foundation rather than an afterthought, use Baldrick.

### Why not Docker?

Docker provides strong isolation but adds hundreds of milliseconds of cold-start latency, requires a container runtime, and doesn't support snapshotting execution state mid-function. For AI agent loops that execute thousands of small code snippets, the overhead dominates.

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
│   IR    │   BaldrickIR (statements, expressions, operators)
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
git clone https://github.com/TheUncharted/baldrick.git
cd baldrick

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

Baldrick is inspired by [Monty](https://github.com/pydantic/monty), Pydantic's Python subset interpreter that takes the same approach for Python.

## License

MIT
