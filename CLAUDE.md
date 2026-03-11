# CLAUDE.md

> AI coding assistant instructions for the `zapcode` project.
> See AGENTS.md for the full architecture reference. This file adds
> Claude-Code-specific guidance on top of it.

---

## Read AGENTS.md first

Before writing any code in this repository, read `AGENTS.md` in full. It defines:
- What Zapcode is and what it must never do
- The full architecture (parser → IR → bytecode → VM → snapshot)
- The five sandbox invariants that must never be violated
- The definition of "done" for any feature

Do not skip this. The sandbox invariants in particular will save you from introducing
security vulnerabilities that are hard to detect and easy to ship.

---

## Codebase orientation

Start here when working on a new area:

| Area | Entry point |
|---|---|
| Parsing TypeScript | `crates/zapcode-core/src/parser/mod.rs` |
| IR definition | `crates/zapcode-core/src/parser/ir.rs` |
| Bytecode instructions | `crates/zapcode-core/src/compiler/instruction.rs` |
| Compiler (IR → bytecode) | `crates/zapcode-core/src/compiler/mod.rs` |
| VM main loop + dispatch | `crates/zapcode-core/src/vm/mod.rs` |
| Built-in functions | `crates/zapcode-core/src/vm/builtins.rs` |
| Value / type system | `crates/zapcode-core/src/value.rs` |
| Snapshot / resume | `crates/zapcode-core/src/snapshot.rs` |
| Resource limits | `crates/zapcode-core/src/sandbox.rs` |
| Error types | `crates/zapcode-core/src/error.rs` |
| JS bindings API | `crates/zapcode-js/src/lib.rs` |
| Python bindings API | `crates/zapcode-py/src/lib.rs` |
| WASM bindings API | `crates/zapcode-wasm/src/lib.rs` |

When in doubt about where something belongs: `zapcode-core` is pure Rust with zero I/O.
Bindings crates only translate types and marshal calls into `zapcode-core`. Never put
business logic in binding crates.

---

## How to add a new language feature

1. **Check the supported subset table in AGENTS.md first.** If the feature is explicitly listed
   as unsupported, do not add it without opening a discussion. Features are excluded intentionally.

2. **Add parser support** in `crates/zapcode-core/src/parser/`. The parser walks the `oxc` AST
   and emits `ZapcodeIR`. Unsupported nodes must emit `ZapcodeError::UnsupportedSyntax` with
   the node's span information.

3. **Add compiler support** in `crates/zapcode-core/src/compiler/`. The compiler lowers
   `ZapcodeIR` to bytecode instructions. Add new `Instruction` variants only when necessary —
   prefer reusing existing instructions.

4. **Add VM dispatch** in `crates/zapcode-core/src/vm/mod.rs`. The main `dispatch()` function
   matches on `Instruction`. Every new instruction needs:
   - Correct stack discipline (verify push/pop balance)
   - Resource limit check before any allocation
   - Use `push_call_frame()` helper for any function call setup

5. **Write tests** before considering the feature done. See AGENTS.md testing philosophy.

6. **Update JS, Python, and WASM bindings** if the feature affects the public API surface.

---

## Sandbox invariant checklist

Before submitting any code to `zapcode-core`, verify:

- [ ] No `std::fs::*` usage
- [ ] No `std::env::*` usage
- [ ] No `std::net::*` or `tokio::net::*` usage
- [ ] No `unsafe` block without a `// SAFETY:` comment
- [ ] No way for guest code to call any function not in the registered `externalFunctions` map
- [ ] No way for guest code to read or write to any memory outside the VM

If you are implementing an external function bridge: the bridge must validate that the
function name exists in the registered set before suspending. An unregistered name must
produce `ZapcodeError::UnknownExternalFunction`, not a panic or a silent no-op.

---

## oxc usage patterns

Zapcode uses `oxc_parser` for parsing and `oxc_ast` for AST traversal:

```rust
use oxc_parser::{Parser, ParserReturn};
use oxc_span::SourceType;
use oxc_allocator::Allocator;

// Always use SourceType::tsx() — it handles both TS and TSX
let allocator = Allocator::default();
let source_type = SourceType::tsx();
let ret: ParserReturn = Parser::new(&allocator, source, source_type).parse();

if !ret.errors.is_empty() {
    return Err(ZapcodeError::ParseError(format_oxc_errors(&ret.errors, source)));
}
```

When walking the AST:
- Use `match` exhaustively — never `_` wildcard on statement or expression nodes.
  An unhandled node should produce `ZapcodeError::UnsupportedSyntax`, not be silently ignored.
- Preserve span information in IR nodes for error messages.
- Do not use `oxc_transformer` or `oxc_semantic` — they add weight we don't need.

---

## Async / await implementation notes

Key invariants:

**The VM is single-threaded.** There is no Tokio runtime inside the VM.

**`await` on a host function** suspends the VM and returns `VmState::Suspended`. The caller
resolves the function externally and calls `resume()`.

**`await` on an internal `Promise`** (e.g., `Promise.resolve(42)`) is handled entirely
inside the VM without suspending.

**Do not** try to integrate `tokio::spawn` or `async_std` into the VM executor.

---

## Performance targets

| Metric | Target |
|---|---|
| First execution latency (simple expression) | < 5 µs |
| Snapshot size (typical agent code) | < 2 KB |
| Snapshot + resume round-trip | < 2 ms |

Run benchmarks with `cargo bench`. The benchmark suite is in `crates/zapcode-core/benches/`.

---

## What to do when you're unsure

1. **Unsupported syntax**: emit `ZapcodeError::UnsupportedSyntax` with the span. Do not silently skip.
2. **Sandbox boundary question**: if in doubt, block it.
3. **API design question**: follow `@pydantic/monty`'s API shape.
4. **Performance vs correctness**: always choose correctness.

---

## Quick reference: public API

### Rust

```rust
use zapcode_core::{ZapcodeRun, Value, ResourceLimits, VmState, ZapcodeSnapshot};

let runner = ZapcodeRun::new(
    code.to_string(),
    vec!["url".to_string()],
    vec!["fetch".to_string()],
    ResourceLimits::default(),
)?;

// Start — pauses at first external call
let state = runner.start(vec![
    ("url".to_string(), Value::String("https://...".into())),
])?;

match state {
    VmState::Suspended { function_name, args, snapshot } => {
        let bytes = snapshot.dump()?;
        let restored = ZapcodeSnapshot::load(&bytes)?;
        let final_state = restored.resume(Value::String("result".into()))?;
    }
    VmState::Complete(value) => println!("Result: {:?}", value),
}
```
