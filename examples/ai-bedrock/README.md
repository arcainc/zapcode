# Zapcode + AWS Bedrock Example

End-to-end example using Zapcode with the Vercel AI SDK and AWS Bedrock.

## Prerequisites

- AWS credentials configured (`~/.aws/credentials`, env vars, or IAM role)
- Access to a Bedrock model (default: `moonshotai.kimi-k2.5` in `eu-west-2`)

## TypeScript

```bash
npm install
npm start
```

Override model/region:
```bash
MODEL_ID=eu.anthropic.claude-sonnet-4-20250514-v1:0 AWS_REGION=eu-west-1 npm start
```

## Python

```bash
uv venv .venv && source .venv/bin/activate
uv pip install zapcode-ai boto3
python main.py
```

Override model/region:
```bash
MODEL_ID=eu.anthropic.claude-sonnet-4-20250514-v1:0 AWS_REGION=eu-west-1 python main.py
```
