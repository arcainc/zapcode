# Contributing to Zapcode

Thanks for your interest in contributing to Zapcode!

## Getting started

### Prerequisites

- [Rust](https://rustup.rs/) (stable)
- [Node.js](https://nodejs.org/) 18+ (for JS bindings)
- [Python](https://python.org/) 3.10+ (for Python bindings)

### Build from source

```bash
git clone https://github.com/TheUncharted/zapcode.git
cd zapcode
cargo build
cargo test
```

### Run benchmarks

```bash
cargo bench -p zapcode-core
```

## Development workflow

1. Fork the repo and create a branch from `master`
2. Make your changes
3. Run `make lint` and `make test` to verify
4. Submit a PR

### Commit messages

We use [Conventional Commits](https://www.conventionalcommits.org/) for automated releases:

- `feat: add Promise.all support` → minor version bump
- `fix: snapshot resume with nested arrays` → patch version bump
- `feat!: rename API method` → major version bump (post-1.0)

### Code style

- `cargo fmt` for formatting
- `cargo clippy -- -D warnings` must pass with zero warnings
- No `unsafe` without a `// SAFETY:` comment

## Architecture

See [AGENTS.md](./AGENTS.md) for the full architecture reference before making changes to the core.

Key rules:
- `zapcode-core` is pure Rust with **zero I/O** — no filesystem, network, or env access
- Binding crates (`zapcode-js`, `zapcode-py`, `zapcode-wasm`) only marshal types — no business logic
- Every new instruction needs correct stack discipline and resource limit checks

## Testing

- Write tests before considering a feature done
- Core tests: `cargo test -p zapcode-core`
- Security tests: `cargo test -p zapcode-core --test security`
- E2E JS: build bindings then run `examples/typescript/basic/main.ts`
- E2E Python: build bindings then run `examples/python/basic/main.py`

## Reporting issues

- Use [GitHub Issues](https://github.com/TheUncharted/zapcode/issues)
- Include a minimal reproduction if possible
- For security vulnerabilities, email directly instead of opening a public issue

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](./LICENSE).
