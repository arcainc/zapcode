/**
 * AI Agent with Baldrick — LOW-LEVEL approach using Anthropic SDK directly.
 *
 * This shows the manual snapshot/resume loop using @baldrick/core.
 * For most use cases, prefer @baldrick/ai instead — see ai-agent-baldrick-ai.ts
 * or ai-agent-vercel-ai.ts for the recommended approach.
 *
 * This example shows the "code as tool use" pattern:
 * 1. Claude writes TypeScript code that calls tools (external functions)
 * 2. Baldrick executes the code in a sandbox
 * 3. When the code calls a tool, Baldrick suspends and returns a snapshot
 * 4. Your app resolves the tool call, then resumes Baldrick with the result
 *
 * Prerequisites:
 *   npm install @anthropic-ai/sdk @baldrick/core
 *   export ANTHROPIC_API_KEY=sk-...
 *
 * Run with: npx tsx ai-agent-anthropic.ts
 */

import Anthropic from "@anthropic-ai/sdk";
import { Baldrick, BaldrickSnapshotHandle } from "@baldrick/core";

// --- Tool implementations (the real functions that run on your server) ---

async function getWeather(city: string): Promise<object> {
  // In production, call a real weather API
  const mockData: Record<string, object> = {
    London: { condition: "Partly cloudy", temp: 18, humidity: 72 },
    Tokyo: { condition: "Clear", temp: 26, humidity: 55 },
    "New York": { condition: "Rain", temp: 14, humidity: 88 },
  };
  return mockData[city] ?? { condition: "Unknown", temp: 0, humidity: 0 };
}

async function searchFlights(
  from: string,
  to: string,
  date: string
): Promise<object[]> {
  // In production, call a flight search API
  return [
    { airline: "BA", flight: "BA123", price: 450, departure: "08:00" },
    { airline: "AF", flight: "AF456", price: 380, departure: "14:30" },
  ];
}

// --- Map of available tools ---

const tools: Record<string, (...args: any[]) => Promise<any>> = {
  getWeather: (city: string) => getWeather(city),
  searchFlights: (from: string, to: string, date: string) =>
    searchFlights(from, to, date),
};

// --- The AI agent loop ---

async function runAgent(userQuery: string) {
  const client = new Anthropic();

  // Ask Claude to write code that answers the user's question
  const response = await client.messages.create({
    model: "claude-sonnet-4-20250514",
    max_tokens: 1024,
    system: `You are a helpful assistant. When the user asks a question that requires
external data (weather, flights, etc.), write TypeScript code that calls the
available functions and computes the answer.

Available functions (called with await):
- getWeather(city: string) → { condition: string, temp: number, humidity: number }
- searchFlights(from: string, to: string, date: string) → Array<{ airline: string, flight: string, price: number, departure: string }>

Available inputs: userQuery (the user's question as a string)

Return ONLY the TypeScript code, no markdown fences. The last expression is the output.`,
    messages: [{ role: "user", content: userQuery }],
  });

  const code =
    response.content[0].type === "text" ? response.content[0].text : "";
  console.log("Claude wrote:\n", code, "\n");

  // Execute the AI-generated code in Baldrick's sandbox
  const sandbox = new Baldrick(code, {
    inputs: ["userQuery"],
    externalFunctions: Object.keys(tools),
    timeLimitMs: 10000,
    memoryLimitMb: 16,
  });

  // Run with snapshot/resume loop to handle external function calls
  let state = sandbox.start({ userQuery });

  while (!state.completed) {
    const { functionName, args } = state;
    console.log(`  -> calling ${functionName}(${JSON.stringify(args)})`);

    // Look up the tool and call it with the arguments from the sandbox
    const toolFn = tools[functionName];
    if (!toolFn) {
      throw new Error(`Unknown tool: ${functionName}`);
    }
    const result = await toolFn(...args);
    console.log(`  <- ${functionName} returned:`, result);

    // Resume the sandbox with the tool's result
    const snapshot = BaldrickSnapshotHandle.load(state.snapshot);
    state = snapshot.resume(result);
  }

  console.log("\nFinal answer:", state.output);
  return state.output;
}

// --- Run it ---

async function main() {
  console.log("=== Example 1: Weather query ===\n");
  await runAgent("What's the weather like in Tokyo?");

  console.log("\n=== Example 2: Multi-tool query ===\n");
  await runAgent(
    "Compare the weather in London and New York, and find me cheap flights from London to New York for 2026-04-01."
  );
}

main().catch(console.error);
