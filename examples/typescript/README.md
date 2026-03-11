# TypeScript Examples

## Setup

First, build the Zapcode native addon (requires Rust toolchain):

```bash
cd ../../crates/zapcode-js
cargo build -p zapcode-js --release
```

Then install dependencies:

```bash
# Pick your package manager
npm install
yarn install
pnpm install
bun install
```

## Run

```bash
# Basic usage
npm run basic          # or: bun run basic / yarn basic / pnpm basic

# AI agent with @unchartedfr/zapcode-ai wrapper (recommended — requires ANTHROPIC_API_KEY)
export ANTHROPIC_API_KEY=sk-ant-...
npm run agent

# AI agent with raw Anthropic SDK
npm run agent:anthropic

# AI agent with Vercel AI SDK
npm run agent:vercel
```

## What's here

| File | Description |
|---|---|
| `basic.ts` | Simple expressions, inputs, data processing, classes, resource limits |
| `ai-agent-zapcode-ai.ts` | **Recommended** — uses `@unchartedfr/zapcode-ai` wrapper with Vercel AI SDK |
| `ai-agent-anthropic.ts` | Raw Anthropic SDK + manual snapshot/resume loop |
| `ai-agent-vercel-ai.ts` | Vercel AI SDK with manual code generation |
