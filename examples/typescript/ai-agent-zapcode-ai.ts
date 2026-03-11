/**
 * AI Agent using @zapcode/ai — the high-level wrapper.
 *
 * This is the recommended way to use Zapcode with AI models.
 * One call to `zapcode()` gives you `{ system, tools }` that plug
 * directly into Vercel AI SDK's `generateText` / `streamText`.
 *
 * Prerequisites:
 *   npm install @zapcode/ai ai @ai-sdk/anthropic
 *   export ANTHROPIC_API_KEY=sk-...
 *
 * Run with: npx tsx ai-agent-zapcode-ai.ts
 */

import { zapcode } from "@zapcode/ai";
import { generateText } from "ai";
import { anthropic } from "@ai-sdk/anthropic";

// Define your tools — just description, params, and execute
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
          "New York": { condition: "Rain", temp: 14, humidity: 88 },
          Paris: { condition: "Sunny", temp: 22, humidity: 45 },
        };
        return data[city as string] ?? { condition: "Unknown", temp: 0 };
      },
    },
    searchFlights: {
      description: "Search flights between two cities",
      parameters: {
        from: { type: "string", description: "Departure city" },
        to: { type: "string", description: "Arrival city" },
        date: { type: "string", description: "Date (YYYY-MM-DD)" },
      },
      execute: async ({ from, to, date }) => {
        // In production, call a flight search API
        return [
          { airline: "BA", flight: "BA123", price: 450, departure: "08:00" },
          { airline: "AF", flight: "AF456", price: 380, departure: "14:30" },
        ];
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

// That's it! Now use with any AI SDK model.

async function main() {
  console.log("=== Simple weather query ===\n");

  const result1 = await generateText({
    model: anthropic("claude-sonnet-4-20250514"),
    system,
    tools,
    messages: [{ role: "user", content: "What's the weather in Tokyo?" }],
  });
  console.log("Answer:", result1.text);

  console.log("\n=== Complex multi-tool query ===\n");

  const result2 = await generateText({
    model: anthropic("claude-sonnet-4-20250514"),
    system,
    tools,
    messages: [
      {
        role: "user",
        content:
          "Compare the weather in London, Tokyo, and Paris. " +
          "Then find flights from the coldest city to the warmest for 2026-04-15. " +
          "Email the results to travel@example.com.",
      },
    ],
  });
  console.log("Answer:", result2.text);
}

main().catch(console.error);
