/**
 * AI Agent with Zapcode — using Vercel AI SDK.
 *
 * This example shows the clean integration via @unchartedfr/zapcode-ai.
 * One call to `zapcode()` gives you `{ system, tools }` that plug
 * directly into `streamText` / `generateText` — just like CodeMode.
 *
 * Works with any AI SDK provider: Anthropic, OpenAI, Google, etc.
 *
 * Prerequisites:
 *   npm install @unchartedfr/zapcode-ai ai @ai-sdk/anthropic
 *   export ANTHROPIC_API_KEY=sk-...
 *
 * Run with: npx tsx ai-agent-vercel-ai.ts
 */

import { zapcode } from "@unchartedfr/zapcode-ai";
import { generateText, streamText } from "ai";
import { anthropic } from "@ai-sdk/anthropic";

// --- One call to set up everything ---

const { system, tools } = zapcode({
  system: "You are a helpful travel assistant.",
  tools: {
    getWeather: {
      description: "Get current weather for a city",
      parameters: {
        city: { type: "string", description: "City name" },
      },
      execute: async ({ city }) => {
        // In production, call a real weather API
        const data: Record<string, unknown> = {
          London: { condition: "Overcast", temp: 12, humidity: 80 },
          Tokyo: { condition: "Clear", temp: 26, humidity: 55 },
          Paris: { condition: "Sunny", temp: 22, humidity: 45 },
        };
        return data[city as string] ?? { condition: "Unknown", temp: 0 };
      },
    },
    sendEmail: {
      description: "Send an email",
      parameters: {
        to: { type: "string", description: "Recipient email" },
        subject: { type: "string", description: "Email subject" },
        body: { type: "string", description: "Email body" },
      },
      execute: async ({ to, subject, body }) => {
        console.log(`  [mock] Sending email to ${to}: "${subject}"`);
        return { sent: true, messageId: "msg_" + Math.random().toString(36).slice(2) };
      },
    },
  },
});

// --- That's it. Now use with generateText or streamText. ---

async function main() {
  // Example 1: generateText
  console.log("=== generateText ===\n");

  const result = await generateText({
    model: anthropic("claude-sonnet-4-20250514"),
    system,
    tools,
    messages: [
      { role: "user", content: "What's the weather in Tokyo?" },
    ],
  });
  console.log("Answer:", result.text);

  // Example 2: streamText
  console.log("\n=== streamText ===\n");

  const stream = streamText({
    model: anthropic("claude-sonnet-4-20250514"),
    system,
    tools,
    messages: [
      {
        role: "user",
        content:
          "Compare the weather in London and Paris, then email a summary to travel@example.com",
      },
    ],
  });

  for await (const chunk of stream.textStream) {
    process.stdout.write(chunk);
  }
  console.log();
}

main().catch(console.error);
