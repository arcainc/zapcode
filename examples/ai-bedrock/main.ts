/**
 * Zapcode + Vercel AI SDK + AWS Bedrock example.
 *
 * Demonstrates using Zapcode's sandboxed TypeScript execution
 * with Claude on Bedrock via the Vercel AI SDK.
 *
 * Prerequisites:
 *   npm install
 *   AWS credentials configured (env vars, ~/.aws/credentials, or IAM role)
 *
 * Run: npm start
 */

import { zapcode } from "@unchartedfr/zapcode-ai";
import { generateText } from "ai";
import { createAmazonBedrock } from "@ai-sdk/amazon-bedrock";
import { fromNodeProviderChain } from "@aws-sdk/credential-providers";

// --- Bedrock setup ---
const REGION = process.env.AWS_REGION ?? "eu-west-2";

const bedrock = createAmazonBedrock({
  credentialProvider: fromNodeProviderChain(),
  region: REGION,
});

const MODEL_ID = process.env.MODEL_ID ?? "moonshotai.kimi-k2.5";
const model = bedrock(MODEL_ID);

// --- Zapcode setup ---
const { system, tools } = zapcode({
  system: "You are a helpful assistant that can look up weather and do math.",
  tools: {
    getWeather: {
      description: "Get current weather for a city",
      parameters: {
        city: { type: "string", description: "City name" },
      },
      execute: async ({ city }) => {
        // Mock — replace with a real API call
        const data: Record<string, { condition: string; temp: number }> = {
          London: { condition: "Overcast", temp: 12 },
          Tokyo: { condition: "Clear", temp: 26 },
          Paris: { condition: "Sunny", temp: 22 },
          "New York": { condition: "Rain", temp: 14 },
        };
        return data[city as string] ?? { condition: "Unknown", temp: 0 };
      },
    },
    searchFlights: {
      description: "Search flights between two cities",
      parameters: {
        from: { type: "string", description: "Departure city" },
        to: { type: "string", description: "Arrival city" },
      },
      execute: async ({ from, to }) => {
        return [
          { from, to, airline: "BA", flight: "BA123", price: 450, departure: "08:00" },
          { from, to, airline: "AF", flight: "AF456", price: 380, departure: "14:30" },
        ];
      },
    },
  },
});

// --- Run ---
async function main() {
  console.log(`Model: ${MODEL_ID} | Region: ${REGION}\n`);

  const t0 = performance.now();

  const result = await generateText({
    model,
    system,
    tools,
    maxSteps: 5,
    messages: [
      {
        role: "user",
        content:
          "What's the weather in Tokyo and Paris? Find flights from the colder city to the warmer one.",
      },
    ],
  });

  const totalMs = (performance.now() - t0).toFixed(0);

  console.log("Answer:", result.text);
  console.log(`\n--- Timing ---`);
  console.log(`Total (LLM + Zapcode): ${totalMs}ms`);
  console.log(`Steps: ${result.steps.length}`);
  const toolCallCount = result.steps.reduce(
    (count, step) => count + step.toolCalls.length,
    0,
  );
  console.log(`Tool calls: ${toolCallCount}`);
  console.log(`Usage: ${result.usage.promptTokens} prompt + ${result.usage.completionTokens} completion = ${result.usage.totalTokens} tokens`);
}

main().catch(console.error);
