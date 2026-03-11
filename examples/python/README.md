# Python Examples

## Setup

### With uv (recommended)

```bash
uv sync                    # install dependencies + build zapcode from source
uv sync --extra ai         # also install anthropic SDK for the AI agent example
```

### With pip

```bash
pip install maturin
cd ../../crates/zapcode-py
maturin develop --release
cd ../../examples/python
pip install anthropic      # for the AI agent example
```

## Run

```bash
# Basic usage
python basic.py                     # or: uv run basic.py

# AI agent with zapcode-ai wrapper (recommended — requires ANTHROPIC_API_KEY)
export ANTHROPIC_API_KEY=sk-ant-...
python ai_agent_zapcode_ai.py

# AI agent with raw Anthropic SDK
python ai_agent_anthropic.py
```

## What's here

| File | Description |
|---|---|
| `basic.py` | Simple expressions, inputs, data processing, snapshot/resume, serialization |
| `ai_agent_zapcode_ai.py` | **Recommended** — uses `zapcode-ai` wrapper with Anthropic SDK |
| `ai_agent_anthropic.py` | Raw Anthropic SDK + manual snapshot/resume loop |
