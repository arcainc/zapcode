/**
 * @unchartedfr/zapcode-ai — High-level AI SDK integration for Zapcode.
 *
 * Works with any AI SDK:
 *
 * ```typescript
 * // AI SDK (recommended)
 * import { zapcode } from "@unchartedfr/zapcode-ai";
 * const { system, tools } = zapcode({ tools: { ... } });
 * await generateText({ model, system, tools, messages });
 *
 * // OpenAI SDK
 * import { zapcode } from "@unchartedfr/zapcode-ai";
 * const { system, openaiTools, handleToolCall } = zapcode({ tools: { ... } });
 * const response = await openai.chat.completions.create({
 *   messages: [{ role: "system", content: system }, ...],
 *   tools: openaiTools,
 * });
 *
 * // Anthropic SDK
 * import { zapcode } from "@unchartedfr/zapcode-ai";
 * const { system, anthropicTools, handleToolCall } = zapcode({ tools: { ... } });
 * const response = await anthropic.messages.create({
 *   system, tools: anthropicTools, messages,
 * });
 * ```
 */

import { Zapcode, ZapcodeSnapshotHandle } from "@unchartedfr/zapcode";
import { jsonSchema, tool, type ToolSet } from "ai";

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/** Definition for a single tool that guest code can call. */
export interface ToolDefinition {
  /** Human-readable description shown to the LLM. */
  description: string;
  /** Parameter schema — keys are parameter names. */
  parameters: Record<string, ParamDef>;
  /** The actual implementation. Called when guest code invokes this tool. */
  execute: (args: Record<string, unknown>) => unknown | Promise<unknown>;
}

/** Schema for a single parameter. */
export interface ParamDef {
  type: "string" | "number" | "boolean" | "object" | "array";
  description?: string;
  optional?: boolean;
}

/** Configuration for the `zapcode()` wrapper. */
export interface ZapcodeAIOptions {
  /** Tools available to guest code. */
  tools: Record<string, ToolDefinition>;
  /** Extra system prompt to prepend (optional). */
  system?: string;
  /** Memory limit in MB (default: 32). */
  memoryLimitMb?: number;
  /** Execution time limit in ms (default: 10000). */
  timeLimitMs?: number;
  /** Custom adapters for additional AI SDKs. */
  adapters?: ZapcodeAdapter[];
  /**
   * Log generated code, tool calls, and output to the console.
   * Useful for understanding what the LLM generates.
   */
  debug?: boolean;
  /**
   * When true, execution errors are returned as tool results instead of
   * throwing. The LLM sees the error and can self-correct on the next step.
   * Works with `maxSteps` in the Vercel AI SDK. Default: false.
   */
  autoFix?: boolean;
}

/** A single span in the execution trace. OTel-compatible shape. */
export interface TraceSpan {
  /** Span name (e.g. "execute", "tool_call", "error", "retry"). */
  name: string;
  /** When the span started (ms since epoch). */
  startTime: number;
  /** When the span ended (ms since epoch). */
  endTime: number;
  /** Duration in ms. */
  durationMs: number;
  /** "ok" or "error". */
  status: "ok" | "error";
  /** Structured attributes — keys map to OTel attribute naming. */
  attributes: Record<string, unknown>;
  /** Child spans. */
  children: TraceSpan[];
}

/** Result of executing guest code. */
export interface ExecutionResult {
  /** The TypeScript code that the LLM generated. */
  code: string;
  output: unknown;
  stdout: string;
  toolCalls: Array<{ name: string; args: unknown[]; result: unknown }>;
  /** Present when autoFix is enabled and execution failed. */
  error?: string;
  /** Execution trace. Present when debug or autoFix is enabled. */
  trace?: TraceSpan;
}

/** What `zapcode()` returns — adapters for every major AI SDK. */
export interface ZapcodeAIResult {
  /** System prompt instructing the LLM to write TypeScript. */
  system: string;

  /**
   * AI SDK tool format.
   * Use with `generateText({ tools })` or `streamText({ tools })`.
   */
  tools: ToolSet;

  /**
   * OpenAI SDK tool format.
   * Use with `openai.chat.completions.create({ tools: openaiTools })`.
   */
  openaiTools: OpenAITool[];

  /**
   * Anthropic SDK tool format.
   * Use with `anthropic.messages.create({ tools: anthropicTools })`.
   */
  anthropicTools: AnthropicTool[];

  /**
   * Execute code from a tool call response.
   * Works with any SDK — just extract the `code` argument from the
   * `execute_code` tool call and pass it here.
   */
  handleToolCall: (code: string) => Promise<ExecutionResult>;

  /**
   * Output from custom adapters, keyed by adapter name.
   * Access with `result.custom["my-adapter-name"]`.
   */
  custom: Record<string, unknown>;

  /**
   * Get the full session trace tree (all attempts).
   * Available when debug or autoFix is enabled.
   * Call after generateText/streamText completes.
   */
  getTrace: () => TraceSpan | undefined;

  /**
   * Print the full session trace tree to the console.
   * Available when debug or autoFix is enabled.
   */
  printTrace: () => void;
}

// ---------------------------------------------------------------------------
// SDK-specific tool shapes
// ---------------------------------------------------------------------------

/** OpenAI SDK tool shape. */
export interface OpenAITool {
  type: "function";
  function: {
    name: string;
    description: string;
    parameters: {
      type: "object";
      properties: Record<string, unknown>;
      required: string[];
    };
  };
}

/** Anthropic SDK tool shape. */
export interface AnthropicTool {
  name: string;
  description: string;
  input_schema: {
    type: "object";
    properties: Record<string, unknown>;
    required: string[];
  };
}

// ---------------------------------------------------------------------------
// System prompt generation
// ---------------------------------------------------------------------------

function generateSignature(name: string, def: ToolDefinition): string {
  const params = Object.entries(def.parameters)
    .map(([pName, pDef]) => {
      const opt = pDef.optional ? "?" : "";
      return `${pName}${opt}: ${pDef.type}`;
    })
    .join(", ");
  return `${name}(${params})`;
}

function buildSystemPrompt(
  tools: Record<string, ToolDefinition>,
  userSystem?: string
): string {
  const toolDocs = Object.entries(tools)
    .map(([name, def]) => `- await ${generateSignature(name, def)}\n  ${def.description}`)
    .join("\n");

  const parts: string[] = [];

  if (userSystem) {
    parts.push(userSystem);
  }

  parts.push(`When you need to use tools or compute something, write TypeScript code and pass it to the execute_code tool.
The code runs in a sandboxed interpreter with these functions available (use await):

${toolDocs}

Rules:
- Write ONLY TypeScript code, no markdown fences, no explanation.
- The last expression in your code is the return value.
- You can use variables, loops, conditionals, array methods, etc.
- All tool calls must use \`await\`.
- When a tool returns a structured object, access its properties directly instead of reparsing the result as text.
- If the user's question doesn't need tools, you can compute the answer directly.`);

  return parts.join("\n\n");
}

// ---------------------------------------------------------------------------
// Tool schema (shared across SDK formats)
// ---------------------------------------------------------------------------

const CODE_TOOL_SCHEMA = {
  type: "object" as const,
  properties: {
    code: {
      type: "string",
      description: "TypeScript code to execute in the sandbox",
    },
  },
  required: ["code"],
};

const CODE_TOOL_DESCRIPTION =
  "Execute TypeScript code in a secure sandbox. " +
  "The code can call the available tool functions using await. " +
  "The last expression is the return value.";

// ---------------------------------------------------------------------------
// Trace helpers
// ---------------------------------------------------------------------------

function createSpan(name: string, attributes: Record<string, unknown> = {}): TraceSpan {
  return {
    name,
    startTime: Date.now(),
    endTime: 0,
    durationMs: 0,
    status: "ok",
    attributes,
    children: [],
  };
}

function endSpan(span: TraceSpan, status?: "ok" | "error"): TraceSpan {
  span.endTime = Date.now();
  span.durationMs = span.endTime - span.startTime;
  if (status) span.status = status;
  return span;
}

function printTrace(span: TraceSpan, indent = 0): void {
  const prefix = indent === 0 ? "" : "│ ".repeat(indent - 1) + "├─ ";
  const icon = span.status === "error" ? "✗" : "✓";
  const duration = span.durationMs < 1 ? "<1ms" : `${span.durationMs}ms`;
  const attrs = Object.entries(span.attributes)
    .map(([k, v]) => {
      const str = typeof v === "string" && v.length > 80 ? v.slice(0, 77) + "..." : String(v);
      return `${k}=${str}`;
    })
    .join(" ");

  console.log(`${prefix}${icon} ${span.name} (${duration})${attrs ? " " + attrs : ""}`);
  for (const child of span.children) {
    printTrace(child, indent + 1);
  }
}

// ---------------------------------------------------------------------------
// Execution engine
// ---------------------------------------------------------------------------

async function executeCode(
  code: string,
  toolDefs: Record<string, ToolDefinition>,
  options: { memoryLimitMb?: number; timeLimitMs?: number; debug?: boolean; autoFix?: boolean }
): Promise<ExecutionResult> {
  const toolNames = Object.keys(toolDefs);
  const toolCalls: ExecutionResult["toolCalls"] = [];
  const debug = options.debug ?? false;
  const autoFix = options.autoFix ?? false;
  const tracing = debug || autoFix;

  const execSpan = tracing ? createSpan("execute", { "zapcode.code": code }) : undefined;

  try {
    const sandbox = new Zapcode(code, {
      externalFunctions: toolNames,
      timeLimitMs: options.timeLimitMs ?? 10_000,
      memoryLimitMb: options.memoryLimitMb ?? 32,
    });

    let state = sandbox.start();
    let stdout = "";

    // Snapshot/resume loop — resolve each tool call as the VM suspends
    while (!state.completed) {
      const { functionName, args } = state;

      const toolDef = toolDefs[functionName];
      if (!toolDef) {
        throw new Error(
          `Guest code called unknown function '${functionName}'. ` +
          `Available: ${toolNames.join(", ")}`
        );
      }

      // Build named args from positional args using the parameter schema
      const paramNames = Object.keys(toolDef.parameters);
      const namedArgs: Record<string, unknown> = {};
      for (let i = 0; i < paramNames.length && i < args.length; i++) {
        namedArgs[paramNames[i]] = args[i];
      }

      const toolSpan = tracing ? createSpan("tool_call", {
        "zapcode.tool.name": functionName,
        "zapcode.tool.args": JSON.stringify(args),
      }) : undefined;

      const result = await toolDef.execute(namedArgs);
      toolCalls.push({ name: functionName, args, result });

      if (toolSpan) {
        toolSpan.attributes["zapcode.tool.result"] = JSON.stringify(result);
        endSpan(toolSpan);
        execSpan!.children.push(toolSpan);
      }

      // Resume the VM with the tool's return value
      const snapshot = ZapcodeSnapshotHandle.load(state.snapshot);
      state = snapshot.resume(result);
    }

    if (state.stdout) {
      stdout = state.stdout;
    }

    if (execSpan) {
      execSpan.attributes["zapcode.output"] = JSON.stringify(state.output);
      if (stdout) execSpan.attributes["zapcode.stdout"] = stdout;
      endSpan(execSpan);
    }

    if (debug && execSpan) {
      printTrace(execSpan);
    }

    return {
      code,
      output: state.output,
      stdout,
      toolCalls,
      ...(execSpan ? { trace: execSpan } : {}),
    };
  } catch (err: any) {
    const errorMsg = err.message ?? String(err);

    if (execSpan) {
      execSpan.attributes["zapcode.error"] = errorMsg;
      endSpan(execSpan, "error");
    }

    if (!autoFix) {
      if (debug && execSpan) printTrace(execSpan);
      throw err;
    }

    if (debug && execSpan) {
      printTrace(execSpan);
    }

    return {
      code,
      output: null,
      stdout: "",
      toolCalls,
      error: `Execution failed: ${errorMsg}. Please fix your code and try again.`,
      ...(execSpan ? { trace: execSpan } : {}),
    };
  }
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

/**
 * Create AI SDK-compatible system prompt and tools for Zapcode.
 *
 * Returns adapters for every major AI SDK:
 * - `tools` → Vercel AI SDK (`generateText`, `streamText`)
 * - `openaiTools` → OpenAI SDK (`chat.completions.create`)
 * - `anthropicTools` → Anthropic SDK (`messages.create`)
 * - `handleToolCall(code)` → Universal handler for any SDK
 *
 * @example
 * ```typescript
 * // Vercel AI SDK
 * const { system, tools } = zapcode({ tools: { getWeather: { ... } } });
 * await generateText({ model, system, tools, messages });
 *
 * // OpenAI SDK
 * const { system, openaiTools, handleToolCall } = zapcode({ tools: { ... } });
 * const res = await openai.chat.completions.create({
 *   messages: [{ role: "system", content: system }, ...],
 *   tools: openaiTools,
 * });
 * const code = res.choices[0].message.tool_calls[0].function.arguments;
 * const result = await handleToolCall(JSON.parse(code).code);
 *
 * // Anthropic SDK
 * const { system, anthropicTools, handleToolCall } = zapcode({ tools: { ... } });
 * const res = await anthropic.messages.create({
 *   system, tools: anthropicTools, messages,
 * });
 * const toolUse = res.content.find(b => b.type === "tool_use");
 * const result = await handleToolCall(toolUse.input.code);
 * ```
 */
export function zapcode(options: ZapcodeAIOptions): ZapcodeAIResult {
  const { tools: toolDefs, system: userSystem, memoryLimitMb, timeLimitMs, adapters, debug, autoFix } = options;

  const system = buildSystemPrompt(toolDefs, userSystem);

  const execOptions = { memoryLimitMb, timeLimitMs, debug, autoFix };
  const tracing = debug || autoFix;

  // Session-level trace collects all attempts
  const sessionTrace: TraceSpan | undefined = tracing
    ? createSpan("session", { "zapcode.tools": Object.keys(toolDefs).join(", ") })
    : undefined;
  let attemptCount = 0;

  // Universal handler
  const handleToolCall = async (code: string): Promise<ExecutionResult> => {
    attemptCount++;
    const result = await executeCode(code, toolDefs, execOptions);

    if (sessionTrace && result.trace) {
      result.trace.name = `attempt_${attemptCount}`;
      result.trace.attributes["zapcode.attempt"] = attemptCount;
      sessionTrace.children.push(result.trace);
    }

    return result;
  };

  // AI SDK format — use tool() + jsonSchema() for proper integration
  const tools: ToolSet = {
    execute_code: tool({
      description: CODE_TOOL_DESCRIPTION,
      inputSchema: jsonSchema(CODE_TOOL_SCHEMA),
      execute: async (args: unknown) => handleToolCall((args as { code: string }).code),
    }),
  };

  // OpenAI SDK format
  const openaiTools: OpenAITool[] = [
    {
      type: "function",
      function: {
        name: "execute_code",
        description: CODE_TOOL_DESCRIPTION,
        parameters: CODE_TOOL_SCHEMA,
      },
    },
  ];

  // Anthropic SDK format
  const anthropicTools: AnthropicTool[] = [
    {
      name: "execute_code",
      description: CODE_TOOL_DESCRIPTION,
      input_schema: CODE_TOOL_SCHEMA,
    },
  ];

  // Run custom adapters
  const custom: Record<string, unknown> = {};
  if (adapters) {
    const adapterContext: AdapterContext = {
      system,
      toolName: "execute_code",
      toolDescription: CODE_TOOL_DESCRIPTION,
      toolSchema: CODE_TOOL_SCHEMA,
      handleToolCall,
    };
    for (const adapter of adapters) {
      custom[adapter.name] = adapter.adapt(adapterContext);
    }
  }

  const getTrace = (): TraceSpan | undefined => {
    if (!sessionTrace) return undefined;
    endSpan(sessionTrace, sessionTrace.children.some(c => c.status === "ok") ? "ok" : "error");
    return sessionTrace;
  };

  const printSessionTrace = (): void => {
    const trace = getTrace();
    if (trace) {
      console.log(`\n─── Zapcode Trace ───`);
      printTrace(trace);
      console.log(`─────────────────────\n`);
    }
  };

  return { system, tools, openaiTools, anthropicTools, handleToolCall, custom, getTrace, printTrace: printSessionTrace };
}

// ---------------------------------------------------------------------------
// Custom adapter support
// ---------------------------------------------------------------------------

/**
 * Adapter interface for integrating Zapcode with any AI SDK.
 *
 * Implement this to add support for a new SDK. Your adapter receives
 * the system prompt, tool description/schema, and a `handleToolCall`
 * function, and returns whatever shape your SDK needs.
 *
 * @example
 * ```typescript
 * import { zapcode, createAdapter, ZapcodeAdapter } from "@unchartedfr/zapcode-ai";
 *
 * // Example: adapter for a hypothetical SDK
 * const myAdapter: ZapcodeAdapter<MySDKConfig> = {
 *   name: "my-sdk",
 *   adapt({ system, toolDescription, toolSchema, handleToolCall }) {
 *     return {
 *       systemMessage: system,
 *       actions: [{
 *         id: "execute_code",
 *         desc: toolDescription,
 *         schema: toolSchema,
 *         run: async (input) => handleToolCall(input.code),
 *       }],
 *     };
 *   },
 * };
 *
 * const { system, tools, custom } = zapcode({
 *   tools: { ... },
 *   adapters: [myAdapter],
 * });
 *
 * const myConfig = custom["my-sdk"]; // typed as MySDKConfig
 * ```
 */
export interface ZapcodeAdapter<TOutput = unknown> {
  /** Unique name for this adapter (used as key in `custom` output). */
  name: string;
  /** Transform Zapcode's tool definition into your SDK's format. */
  adapt(context: AdapterContext): TOutput;
}

/** Context passed to adapters. */
export interface AdapterContext {
  /** The generated system prompt. */
  system: string;
  /** Tool name (always "execute_code"). */
  toolName: string;
  /** Human-readable tool description. */
  toolDescription: string;
  /** JSON Schema for the tool parameters. */
  toolSchema: {
    type: "object";
    properties: Record<string, unknown>;
    required: string[];
  };
  /** Execute code in the sandbox. Pass the `code` string from the tool call. */
  handleToolCall: (code: string) => Promise<ExecutionResult>;
}

/**
 * Helper to create a typed adapter.
 *
 * @example
 * ```typescript
 * const langchainAdapter = createAdapter("langchain", (ctx) => {
 *   return new DynamicStructuredTool({
 *     name: ctx.toolName,
 *     description: ctx.toolDescription,
 *     func: async ({ code }) => JSON.stringify(await ctx.handleToolCall(code)),
 *   });
 * });
 * ```
 */
export function createAdapter<TOutput>(
  name: string,
  adapt: (context: AdapterContext) => TOutput
): ZapcodeAdapter<TOutput> {
  return { name, adapt };
}

// ---------------------------------------------------------------------------
// Convenience: standalone execution without AI SDK
// ---------------------------------------------------------------------------

/**
 * Execute TypeScript code directly in a Zapcode sandbox with tool resolution.
 *
 * This is the lower-level API if you don't need AI SDK integration — you
 * provide the code yourself and Zapcode executes it with tool calls resolved.
 *
 * @example
 * ```typescript
 * import { execute } from "@unchartedfr/zapcode-ai";
 *
 * const result = await execute(
 *   `const w = await getWeather("Tokyo"); w.temp`,
 *   {
 *     getWeather: {
 *       description: "Get weather",
 *       parameters: { city: { type: "string" } },
 *       execute: async ({ city }) => ({ temp: 26, condition: "Clear" }),
 *     },
 *   },
 * );
 * console.log(result.output); // 26
 * ```
 */
export async function execute(
  code: string,
  tools: Record<string, ToolDefinition>,
  options?: { memoryLimitMb?: number; timeLimitMs?: number; debug?: boolean; autoFix?: boolean }
): Promise<ExecutionResult> {
  return executeCode(code, tools, options ?? {});
}
