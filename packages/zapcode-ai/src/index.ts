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

import {
  Zapcode,
  ZapcodeSnapshotHandle,
  ZapcodeSessionHandle,
  ZapcodeProgramHandle,
  type PromiseCombinator,
} from "@unchartedfr/zapcode";
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
  /**
   * Optional TypeScript type expression for the tool's RESOLVED value, e.g.
   * `"{ temp: number; city: string }"` or `"string[]"`. Used in the system
   * prompt's declared signature and by `typecheck()`, so agent code that
   * mis-uses a tool result fails the type pass instead of at runtime.
   * Defaults to `unknown` in prompts and `any` in the typechecker.
   */
  returns?: string;
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
  /**
   * Type-check generated code against the tool stubs before running it, so an
   * unknown tool or a wrong argument type fails fast as a compile error
   * (cheaper than a runtime trap). Requires the optional `typescript`
   * dependency. Default: false.
   */
  typeCheck?: boolean;
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
  /** Captured stderr (`console.error`/`console.warn`), separate from stdout. */
  stderr: string;
  toolCalls: Array<{
    name: string;
    /** Raw positional arguments emitted by the sandboxed code. */
    args: unknown[];
    /** Schema-validated named input passed to the host tool. */
    input: Record<string, unknown>;
    result: unknown;
    /**
     * Present when the tool threw. The error was raised back into the sandbox
     * (catchable by guest `try`/`catch`); `result` is undefined in that case.
     */
    error?: string;
    /** Wall-clock the host spent resolving this call (suspension → resume). */
    durationMs: number;
  }>;
  /** Present when autoFix is enabled and execution failed. */
  error?: string;
  /**
   * Structured, host-loggable summary of the run — the "execution receipt".
   * Always present, so a caller (or an agent in a retry loop) can branch on
   * `report.completed` / `report.error` without parsing strings.
   */
  report: RunReport;
  /** Execution trace. Present when debug or autoFix is enabled. */
  trace?: TraceSpan;
}

/** A structured summary of one execution — see {@link ExecutionResult.report}. */
export interface RunReport {
  /** True when the code ran to completion; false when it threw. */
  completed: boolean;
  /** Total wall-clock of the run (compile + execute + tool round-trips). */
  durationMs: number;
  /** Number of tool calls the code made. */
  toolCallCount: number;
  /** When the run failed: the message plus the source location, when the
   * core attached one (`at line:col` in the error). `script` is the caller's
   * `scriptName` label, when one was supplied — for error provenance across a
   * fleet of agent scripts / session chunks. */
  error?: { message: string; line?: number; column?: number; script?: string };
}

/** Pull `at <line>:<col>` out of a core error message, when present. */
function parseErrorLocation(message: string): { line?: number; column?: number } {
  const m = /\bat (\d+):(\d+)\b/.exec(message);
  if (!m) return {};
  return { line: Number(m[1]), column: Number(m[2]) };
}

const RESERVED_TOOL_NAMES = new Set([
  "console",
  "JSON",
  "Object",
  "Array",
  "Math",
  "Promise",
  "Map",
  "Date",
  "eval",
  "Function",
  "process",
  "globalThis",
  "global",
  "require",
  "execute_code",
]);

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

function generateNamedObjectSignature(name: string, def: ToolDefinition): string {
  const params = Object.entries(def.parameters)
    .map(([pName, pDef]) => {
      const opt = pDef.optional ? "?" : "";
      return `${pName}${opt}: ${pDef.type}`;
    })
    .join(", ");
  return `${name}({ ${params} })`;
}

function generateDeclaration(name: string, def: ToolDefinition): string {
  const entries = Object.entries(def.parameters);
  if (entries.length > 1) {
    const fields = entries
      .map(([pName, pDef]) => `${pName}${pDef.optional ? "?" : ""}: ${pDef.type}`)
      .join("; ");
    return `declare function ${name}(input: { ${fields} }): Promise<unknown>;`;
  }

  const params = entries
    .map(([pName, pDef]) => `${pName}${pDef.optional ? "?" : ""}: ${pDef.type}`)
    .join(", ");
  return `declare function ${name}(${params}): Promise<unknown>;`;
}

function buildSystemPrompt(
  tools: Record<string, ToolDefinition>,
  userSystem?: string
): string {
  const toolDocs = Object.entries(tools)
    .map(([name, def]) => {
      const signature =
        Object.keys(def.parameters).length > 1
          ? generateNamedObjectSignature(name, def)
          : generateSignature(name, def);
      return `- ${generateDeclaration(name, def)}\n  Call shape: await ${signature}\n  ${def.description}`;
    })
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
- Prefer the declared function signatures above exactly. For tools with more than one parameter, call them with one named object argument, e.g. \`await toolName({ key: value })\`.
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

function isValidIdentifier(name: string): boolean {
  return /^[A-Za-z_$][A-Za-z0-9_$]*$/.test(name);
}

function validateToolDefinitions(toolDefs: Record<string, ToolDefinition>): void {
  for (const name of Object.keys(toolDefs)) {
    if (!isValidIdentifier(name)) {
      throw new Error(`Invalid tool name '${name}': tool names must be valid JavaScript identifiers.`);
    }
    if (RESERVED_TOOL_NAMES.has(name)) {
      throw new Error(`Invalid tool name '${name}': this name is reserved by Zapcode.`);
    }
  }
}

// ---------------------------------------------------------------------------
// Tool argument validation
// ---------------------------------------------------------------------------

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * Make a host tool's return value safe to marshal across the native boundary.
 *
 * The native binding deserializes the value into a `serde_json::Value`, which
 * cannot represent `undefined`, non-finite numbers, or `BigInt` — those used to
 * abort the whole process (an uncatchable Rust panic / serde error). This
 * deeply normalizes the value to JSON-compatible types, matching the
 * interpreter's own semantics: `undefined`/functions/symbols → `null` (and
 * dropped as object properties), non-finite numbers → `null`, `BigInt` →
 * `Number`, `Date` → ISO string, `Map`/`Set` → object/array.
 */
function sanitizeToolResult(value: unknown, seen: WeakSet<object> = new WeakSet()): unknown {
  if (value === undefined || value === null) return null;
  switch (typeof value) {
    case "bigint":
      return Number(value);
    case "number":
      return Number.isFinite(value) ? value : null;
    case "string":
    case "boolean":
      return value;
    case "function":
    case "symbol":
      return null;
  }
  if (value instanceof Date) return value.toISOString();
  if (value instanceof Map) {
    const out: Record<string, unknown> = {};
    for (const [k, v] of value) out[String(k)] = sanitizeToolResult(v, seen);
    return out;
  }
  if (value instanceof Set) return Array.from(value, v => sanitizeToolResult(v, seen));
  if (Array.isArray(value)) return value.map(v => sanitizeToolResult(v, seen));
  if (typeof value === "object") {
    if (seen.has(value)) return null; // break accidental cycles
    seen.add(value);
    const obj = value as Record<string, unknown>;
    const out: Record<string, unknown> = {};
    for (const k of Object.keys(obj)) {
      if (obj[k] === undefined) continue; // JSON drops undefined-valued props
      out[k] = sanitizeToolResult(obj[k], seen);
    }
    return out;
  }
  return null;
}

function jsTypeName(value: unknown): string {
  if (Array.isArray(value)) return "array";
  if (value === null) return "null";
  return typeof value;
}

function matchesParamType(value: unknown, param: ParamDef): boolean {
  switch (param.type) {
    case "array":
      return Array.isArray(value);
    case "object":
      return isPlainObject(value);
    case "number":
      return typeof value === "number" && Number.isFinite(value);
    case "string":
      return typeof value === "string";
    case "boolean":
      return typeof value === "boolean";
  }
}

function formatToolSignature(name: string, def: ToolDefinition): string {
  const params = Object.entries(def.parameters)
    .map(([paramName, param]) => `${paramName}${param.optional ? "?" : ""}: ${param.type}`)
    .join(", ");
  return Object.keys(def.parameters).length > 1
    ? `${name}({ ${params} })`
    : `${name}(${params})`;
}

function buildNamedArgs(
  functionName: string,
  toolDef: ToolDefinition,
  args: unknown[]
): Record<string, unknown> {
  const paramEntries = Object.entries(toolDef.parameters);
  const paramNames = paramEntries.map(([name]) => name);
  const singleObjectArg = args.length === 1 && isPlainObject(args[0]) ? args[0] : undefined;
  // A no-arg tool called as `tool({})` (a common LLM habit): accept the empty
  // object as an empty named call rather than rejecting it as positional.
  if (
    paramEntries.length === 0 &&
    singleObjectArg !== undefined &&
    Object.keys(singleObjectArg).length === 0
  ) {
    return {};
  }
  const usesNamedObject =
    singleObjectArg !== undefined &&
    (paramEntries.length > 1 || shouldTreatSingleObjectArgAsNamed(toolDef, singleObjectArg));
  if (paramEntries.length > 1 && !usesNamedObject) {
    throw new Error(
      `Invalid arguments for tool '${functionName}': expected one named object argument. ` +
        `Use ${formatToolSignature(functionName, toolDef)}.`
    );
  }
  const namedArgs =
    usesNamedObject
      ? { ...singleObjectArg }
      : Object.fromEntries(paramNames.map((name, index) => [name, args[index]]));

  if (!usesNamedObject) {
    const extraCount = args.length - paramNames.length;
    if (extraCount > 0) {
      throw new Error(
        `Invalid arguments for tool '${functionName}': received ${args.length} positional ` +
          `arguments but expected ${paramNames.length}. Use ${formatToolSignature(functionName, toolDef)}.`
      );
    }
  }

  const unexpected = Object.keys(namedArgs).filter(name => !toolDef.parameters[name]);
  if (unexpected.length > 0) {
    throw new Error(
      `Invalid arguments for tool '${functionName}': unexpected parameter '${unexpected[0]}'. ` +
        `Expected ${formatToolSignature(functionName, toolDef)}.`
    );
  }

  for (const [paramName, paramDef] of paramEntries) {
    const value = namedArgs[paramName];
    if (value === undefined) {
      if (!paramDef.optional) {
        throw new Error(
          `Invalid arguments for tool '${functionName}': missing required parameter '${paramName}'. ` +
            `Expected ${formatToolSignature(functionName, toolDef)}.`
        );
      }
      delete namedArgs[paramName];
      continue;
    }
    // An omitted optional field written as `{ field: undefined }` in guest code
    // crosses the JSON boundary as `null`. No declared param type is nullable,
    // so for an *optional* param treat null as "omitted" — this makes the common
    // agent pattern work. A required param still rejects null as a type error.
    if (value === null && paramDef.optional) {
      delete namedArgs[paramName];
      continue;
    }

    if (!matchesParamType(value, paramDef)) {
      throw new Error(
        `Invalid arguments for tool '${functionName}': parameter '${paramName}' expected ` +
          `${paramDef.type}, got ${jsTypeName(value)}. Expected ${formatToolSignature(functionName, toolDef)}.`
      );
    }
  }

  return namedArgs;
}

function shouldTreatSingleObjectArgAsNamed(
  toolDef: ToolDefinition,
  arg: Record<string, unknown>
): boolean {
  const entries = Object.entries(toolDef.parameters);
  if (entries.length !== 1) return false;

  const [[name, param]] = entries;
  if (!Object.hasOwn(arg, name)) return false;

  // Non-object single params are unambiguous: foo({ id }) means named input.
  if (param.type !== "object") return true;

  // For a single object param, support foo({ payload: {...} }) as named input,
  // but keep foo({ arbitrary: "shape" }) as the payload value itself.
  return Object.keys(arg).length === 1 || Object.keys(arg).some(key => !toolDef.parameters[key]);
}

// ---------------------------------------------------------------------------
// Optional type-check pre-pass
// ---------------------------------------------------------------------------

/** A `declare function` stub for type-checking. Uses `any` returns so property
 * access on results isn't flagged — the goal is to catch unknown tool names and
 * wrong argument types/shapes before running, not to police result usage. */
function declarationForTypeCheck(name: string, def: ToolDefinition): string {
  // A declared return type makes downstream misuse a type error; without
  // one, `any` keeps unannotated agent code from false-positive noise.
  const ret = def.returns ? `Promise<${def.returns}>` : "Promise<any>";
  const entries = Object.entries(def.parameters);
  if (entries.length > 1) {
    const fields = entries
      .map(([p, d]) => `${p}${d.optional ? "?" : ""}: ${d.type}`)
      .join("; ");
    return `declare function ${name}(input: { ${fields} }): ${ret};`;
  }
  if (entries.length === 0) {
    // The runtime tolerates `tool({})` for a no-arg tool (a common LLM
    // habit) — the checker must too.
    return `declare function ${name}(input?: Record<string, never>): ${ret};`;
  }
  // Single-param tools accept BOTH shapes at runtime (positional value or a
  // one-key named object) — declare both as overloads so neither flags.
  const [p, d] = entries[0];
  const positional = `${p}${d.optional ? "?" : ""}: ${d.type}`;
  return (
    `declare function ${name}(${positional}): ${ret};\n` +
    `declare function ${name}(input: { ${positional} }): ${ret};`
  );
}

/**
 * A single diagnostic from {@link typecheck}, positioned in the AGENT'S code
 * (1-based line/column; the internal wrapper offset is already subtracted).
 */
export interface TypeDiagnostic {
  line: number;
  column: number;
  message: string;
  /** TypeScript error code (e.g. 2345), when the engine reports one. */
  code?: number;
  severity: "error" | "warning";
}

/** Result of {@link typecheck}. */
export interface TypecheckResult {
  /** True when no error-severity diagnostics were produced. */
  ok: boolean;
  diagnostics: TypeDiagnostic[];
  /** Which engine ran: tsgo (native preview) or the in-process tsc API. */
  engine: "tsgo" | "typescript";
  /** The ambient declarations the agent code was checked against. */
  toolDeclarations: string;
}

export interface TypecheckOptions {
  /**
   * Engine selection. "auto" (default) prefers `@typescript/native-preview`
   * (tsgo, the native TypeScript compiler — ~10x faster) when it is
   * installed, falling back to the in-process `typescript` API.
   */
  engine?: "auto" | "tsgo" | "typescript";
  /**
   * Strict checking (default true). Maps to `strict: true` with
   * `noImplicitAny: false` — agent code rarely annotates callback
   * parameters, and contextual inference covers most of them; implicit-any
   * noise would drown the real findings. Set false for the legacy loose
   * check (only blatant errors).
   */
  strict?: boolean;
}

const TYPECHECK_LIB_DECLS =
  '\ndeclare const console: { log(...args: any[]): void; error(...args: any[]): void; warn(...args: any[]): void };\n';

function buildToolDeclarations(toolDefs: Record<string, ToolDefinition>): string {
  return (
    Object.entries(toolDefs)
      .map(([name, def]) => declarationForTypeCheck(name, def))
      .join("\n") + TYPECHECK_LIB_DECLS
  );
}

/** Lines the wrapper prepends before the agent's first line in __main__.ts. */
const TYPECHECK_WRAPPER_LINES = 2; // "export {};" + "async function __zapcode_main__() {"

function wrapAgentCode(code: string): string {
  return `export {};\nasync function __zapcode_main__() {\n${code}\n}\n`;
}

/**
 * Typecheck agent-generated code against the registered tools' signatures
 * WITHOUT running it. A type error handed back to the model before execution
 * is the cheapest self-correction signal there is — declare `returns` on
 * your tools and misuse of their results becomes a pre-execution failure.
 *
 * Engines: prefers `@typescript/native-preview` (tsgo — Microsoft's native
 * TypeScript compiler) when installed; otherwise uses the in-process
 * `typescript` API. Install one of them; with neither, this throws.
 */
export async function typecheck(
  code: string,
  toolDefs: Record<string, ToolDefinition>,
  options: TypecheckOptions = {}
): Promise<TypecheckResult> {
  const engine = options.engine ?? "auto";
  const strict = options.strict ?? true;
  const toolDeclarations = buildToolDeclarations(toolDefs);
  const main = wrapAgentCode(code);

  if (engine === "tsgo" || engine === "auto") {
    const tsgo = await resolveTsgo();
    if (tsgo) {
      const diagnostics = await runTsgo(tsgo, toolDeclarations, main, strict);
      return {
        ok: !diagnostics.some((d) => d.severity === "error"),
        diagnostics,
        engine: "tsgo",
        toolDeclarations,
      };
    }
    if (engine === "tsgo") {
      throw new Error(
        "engine 'tsgo' requested but '@typescript/native-preview' is not installed (npm i -D @typescript/native-preview)"
      );
    }
  }

  const diagnostics = await runTscApi(toolDeclarations, main, strict);
  return {
    ok: !diagnostics.some((d) => d.severity === "error"),
    diagnostics,
    engine: "typescript",
    toolDeclarations,
  };
}

/**
 * Render diagnostics as a compact, line-anchored report for the MODEL —
 * each finding cites the offending source line with a caret so the agent
 * can self-correct without re-deriving positions.
 */
export function formatDiagnosticsForModel(result: TypecheckResult, code: string): string {
  if (result.ok && result.diagnostics.length === 0) return "";
  const lines = code.split("\n");
  const parts: string[] = [];
  for (const d of result.diagnostics.slice(0, 10)) {
    const src = lines[d.line - 1];
    let block = `${d.severity === "error" ? "Type error" : "Type warning"} at line ${d.line}, column ${d.column}${d.code ? ` (TS${d.code})` : ""}: ${d.message}`;
    if (src !== undefined) {
      block += `\n    ${src}\n    ${" ".repeat(Math.max(0, d.column - 1))}^`;
    }
    parts.push(block);
  }
  if (result.diagnostics.length > 10) {
    parts.push(`…and ${result.diagnostics.length - 10} more`);
  }
  return parts.join("\n");
}

/** Locate the tsgo launcher from `@typescript/native-preview`, if installed. */
async function resolveTsgo(): Promise<string | null> {
  try {
    const { createRequire } = await import("node:module");
    const path = await import("node:path");
    const fs = await import("node:fs/promises");
    const req = createRequire(import.meta.url);
    const pkgPath = req.resolve("@typescript/native-preview/package.json");
    const pkg = JSON.parse(await fs.readFile(pkgPath, "utf8"));
    const bin = typeof pkg.bin === "string" ? pkg.bin : pkg.bin?.tsgo;
    return bin ? path.join(path.dirname(pkgPath), bin) : null;
  } catch {
    return null;
  }
}

/** Run tsgo over a temp project and parse its tsc-style diagnostics. */
async function runTsgo(
  tsgoBin: string,
  toolDecls: string,
  main: string,
  strict: boolean
): Promise<TypeDiagnostic[]> {
  const fs = await import("node:fs/promises");
  const os = await import("node:os");
  const path = await import("node:path");
  const { execFile } = await import("node:child_process");

  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "zapcode-typecheck-"));
  try {
    await fs.writeFile(path.join(dir, "__tools__.d.ts"), toolDecls);
    await fs.writeFile(path.join(dir, "__main__.ts"), main);
    await fs.writeFile(
      path.join(dir, "tsconfig.json"),
      JSON.stringify({
        compilerOptions: {
          noEmit: true,
          strict,
          noImplicitAny: false,
          skipLibCheck: true,
          target: "es2022",
          lib: ["es2022"],
          types: [],
        },
        files: ["__tools__.d.ts", "__main__.ts"],
      })
    );
    const output = await new Promise<string>((resolve) => {
      execFile(
        process.execPath,
        [tsgoBin, "--project", dir, "--pretty", "false"],
        { cwd: dir, timeout: 30_000 },
        (_err, stdout, stderr) => resolve(`${stdout}\n${stderr}`)
      );
    });
    return parseTscOutput(output);
  } finally {
    await fs.rm(dir, { recursive: true, force: true });
  }
}

/** Parse `--pretty false` tsc/tsgo output lines into agent-positioned diagnostics. */
function parseTscOutput(output: string): TypeDiagnostic[] {
  const diagnostics: TypeDiagnostic[] = [];
  const re = /^(.*?)\((\d+),(\d+)\): (error|warning) TS(\d+): (.*)$/;
  for (const line of output.split("\n")) {
    const m = re.exec(line.trim());
    if (!m) {
      // Continuation lines of a multi-line message attach to the previous one.
      if (diagnostics.length > 0 && /^\s+\S/.test(line)) {
        diagnostics[diagnostics.length - 1].message += `\n${line.trim()}`;
      }
      continue;
    }
    const [, file, lineStr, colStr, sev, codeStr, message] = m;
    if (!file.endsWith("__main__.ts")) continue; // tool-decl noise is ours, not the agent's
    const agentLine = Number(lineStr) - TYPECHECK_WRAPPER_LINES;
    if (agentLine < 1) continue;
    diagnostics.push({
      line: agentLine,
      column: Number(colStr),
      message,
      code: Number(codeStr),
      severity: sev === "warning" ? "warning" : "error",
    });
  }
  return diagnostics;
}

/** In-process fallback: the `typescript` compiler API over a virtual FS. */
async function runTscApi(
  toolDecls: string,
  main: string,
  strict: boolean
): Promise<TypeDiagnostic[]> {
  let ts: any;
  try {
    const mod: any = await import("typescript");
    ts = mod.default ?? mod;
  } catch {
    throw new Error(
      "Type-checking requires '@typescript/native-preview' (preferred) or the 'typescript' dependency. " +
        "Install one (npm i -D @typescript/native-preview) or disable the typeCheck option."
    );
  }

  const options = {
    noEmit: true,
    strict,
    noImplicitAny: false,
    skipLibCheck: true,
    target: ts.ScriptTarget.ES2022,
    lib: ["lib.es2022.d.ts"],
    moduleDetection: ts.ModuleDetectionKind?.Force,
  };
  const virtual: Record<string, string> = { "__tools__.d.ts": toolDecls, "__main__.ts": main };
  const host = ts.createCompilerHost(options);
  const origGetSourceFile = host.getSourceFile.bind(host);
  host.getSourceFile = (name: string, langVersion: any, ...rest: any[]) =>
    virtual[name] !== undefined
      ? ts.createSourceFile(name, virtual[name], langVersion)
      : origGetSourceFile(name, langVersion, ...rest);
  const origReadFile = host.readFile.bind(host);
  host.readFile = (name: string) => (virtual[name] !== undefined ? virtual[name] : origReadFile(name));
  const origFileExists = host.fileExists.bind(host);
  host.fileExists = (name: string) => virtual[name] !== undefined || origFileExists(name);

  const program = ts.createProgram(Object.keys(virtual), options, host);
  const raw = ts
    .getPreEmitDiagnostics(program)
    .filter((d: any) => d.file && d.file.fileName === "__main__.ts");

  const out: TypeDiagnostic[] = [];
  for (const d of raw) {
    const message = ts.flattenDiagnosticMessageText(d.messageText, "\n");
    if (typeof d.start !== "number") {
      out.push({ line: 1, column: 1, message, code: d.code, severity: "error" });
      continue;
    }
    const { line, character } = d.file.getLineAndCharacterOfPosition(d.start);
    const agentLine = line + 1 - TYPECHECK_WRAPPER_LINES;
    if (agentLine < 1) continue;
    out.push({
      line: agentLine,
      column: character + 1,
      message,
      code: d.code,
      severity: d.category === ts.DiagnosticCategory.Warning ? "warning" : "error",
    });
  }
  return out;
}

/**
 * Legacy string-list adapter used by the `typeCheck` execute option.
 */
async function typeCheckGeneratedCode(
  code: string,
  toolDefs: Record<string, ToolDefinition>
): Promise<string[]> {
  // The execute pre-pass keeps the historical loose profile; the standalone
  // typecheck() defaults to strict.
  const result = await typecheck(code, toolDefs, { strict: false });
  return result.diagnostics.map(
    (d) => `line ${d.line}, col ${d.column}: ${d.message}`
  );
}

// ---------------------------------------------------------------------------
// Execution engine
// ---------------------------------------------------------------------------

async function executeCode(
  code: string,
  toolDefs: Record<string, ToolDefinition>,
  options: {
    memoryLimitMb?: number;
    timeLimitMs?: number;
    debug?: boolean;
    autoFix?: boolean;
    typeCheck?: boolean;
    /**
     * Optional label identifying this script (monty's `script_name`). When set,
     * a failure prefixes the error with `[scriptName]` and records it as
     * `report.error.script`, so a thrown error names *which* agent script erred.
     */
    scriptName?: string;
    /**
     * Produce the initial VM state instead of compiling `code` fresh — used
     * by {@link prepare} to start from an already-compiled, reusable program
     * (parse + compile paid once, not per run). When omitted, a new `Zapcode`
     * is compiled and started as usual.
     */
    starter?: () => ReturnType<Zapcode["start"]>;
  }
): Promise<ExecutionResult> {
  validateToolDefinitions(toolDefs);
  const toolNames = Object.keys(toolDefs);
  const toolCalls: ExecutionResult["toolCalls"] = [];
  const debug = options.debug ?? false;
  const autoFix = options.autoFix ?? false;
  const tracing = debug || autoFix;

  const execSpan = tracing ? createSpan("execute", { "zapcode.code": code }) : undefined;
  const runStartedAt = Date.now();

  try {
    // Optional type-check pre-pass: catch unknown tools / wrong argument types
    // as compile errors before running anything.
    if (options.typeCheck) {
      const diagnostics = await typeCheckGeneratedCode(code, toolDefs);
      if (diagnostics.length > 0) {
        throw new Error(`Type error before execution:\n${diagnostics.join("\n")}`);
      }
    }

    // `prepare()` supplies a starter that resumes from a pre-compiled program;
    // otherwise compile and start a fresh sandbox here.
    let state = options.starter
      ? options.starter()
      : new Zapcode(code, {
          externalFunctions: toolNames,
          timeLimitMs: options.timeLimitMs ?? 10_000,
          memoryLimitMb: options.memoryLimitMb ?? 32,
        }).start();
    let stdout = "";
    let stderr = "";

    // Validate + run one tool call, recording its span and a toolCalls entry.
    // Throws on a *malformed* call (a code bug → abort/autoFix). Returns a
    // discriminated outcome for a *runtime* tool failure so the caller can raise
    // it back into the sandbox as a catchable error.
    type ToolOutcome = { ok: true; result: unknown } | { ok: false; message: string; name: string };
    const invokeTool = async (name: string, rawArgs: unknown[]): Promise<ToolOutcome> => {
      const toolDef = toolDefs[name];
      if (!toolDef) {
        throw new Error(
          `Guest code called unknown function '${name}'. Available: ${toolNames.join(", ")}`
        );
      }
      const toolSpan = tracing
        ? createSpan("tool_call", {
            "zapcode.tool.name": name,
            "zapcode.tool.args": JSON.stringify(rawArgs),
          })
        : undefined;

      let namedArgs: Record<string, unknown>;
      try {
        namedArgs = buildNamedArgs(name, toolDef, rawArgs);
      } catch (err: any) {
        if (toolSpan) {
          toolSpan.attributes["zapcode.tool.error"] = err.message ?? String(err);
          endSpan(toolSpan, "error");
          execSpan!.children.push(toolSpan);
        }
        throw err; // malformed call — not catchable by the guest
      }
      if (toolSpan) {
        toolSpan.attributes["zapcode.tool.input"] = JSON.stringify(namedArgs);
      }

      const startedAt = Date.now();
      try {
        const result = sanitizeToolResult(await toolDef.execute(namedArgs));
        toolCalls.push({
          name,
          args: rawArgs,
          input: namedArgs,
          result,
          durationMs: Date.now() - startedAt,
        });
        if (toolSpan) {
          toolSpan.attributes["zapcode.tool.result"] = JSON.stringify(result);
          endSpan(toolSpan);
          execSpan!.children.push(toolSpan);
        }
        return { ok: true, result };
      } catch (err: any) {
        const message = err?.message ?? String(err);
        const errName = (typeof err?.name === "string" && err.name) || "Error";
        toolCalls.push({
          name,
          args: rawArgs,
          input: namedArgs,
          result: undefined,
          error: message,
          durationMs: Date.now() - startedAt,
        });
        if (toolSpan) {
          toolSpan.attributes["zapcode.tool.error"] = message;
          endSpan(toolSpan, "error");
          execSpan!.children.push(toolSpan);
        }
        return { ok: false, message, name: errName };
      }
    };

    // Snapshot/resume loop — resolve tool calls as the VM suspends.
    while (!state.completed) {
      const snapshot = ZapcodeSnapshotHandle.load(state.snapshot);

      if (state.kind === "suspended_many") {
        // Parallel batch (Promise.{all,race,any,allSettled}). Run every call
        // concurrently as a real JS promise so race/any honor real settle
        // timing, then settle with the matching combinator. A malformed call
        // throws and aborts the whole execution.
        state = await settleBatch(snapshot, state.combinator, state.calls, invokeTool);
        continue;
      }

      // Single external call.
      const outcome = await invokeTool(state.functionName, state.args);
      state = outcome.ok
        ? snapshot.resume(outcome.result)
        : snapshot.resumeErrorObject(outcome.message, outcome.name);
    }

    // Console output is cumulative across snapshot restores, so the final
    // (completed) state carries the whole run's stdout/stderr. (The binding used
    // to hardcode these empty on the resume path, dropping all console output
    // from any program that called a tool — this reads the real values.)
    stdout = state.stdout ?? "";
    stderr = state.stderr ?? "";

    if (execSpan) {
      execSpan.attributes["zapcode.output"] = JSON.stringify(state.output);
      if (stdout) execSpan.attributes["zapcode.stdout"] = stdout;
      if (stderr) execSpan.attributes["zapcode.stderr"] = stderr;
      endSpan(execSpan);
    }

    if (debug && execSpan) {
      printTrace(execSpan);
    }

    return {
      code,
      output: state.output,
      stdout,
      stderr,
      toolCalls,
      report: {
        completed: true,
        durationMs: Date.now() - runStartedAt,
        toolCallCount: toolCalls.length,
      },
      ...(execSpan ? { trace: execSpan } : {}),
    };
  } catch (err: any) {
    const errorMsg = err.message ?? String(err);
    const label = options.scriptName ? `[${options.scriptName}] ` : "";

    if (execSpan) {
      execSpan.attributes["zapcode.error"] = errorMsg;
      endSpan(execSpan, "error");
    }

    if (!autoFix) {
      // Name the erring script on the thrown error itself, so a host catching
      // it across many scripts knows which one failed (opt-in: no label → no
      // change to the message).
      if (label && err instanceof Error) err.message = label + err.message;
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
      stderr: "",
      toolCalls,
      error: `Execution failed: ${label}${errorMsg}. Please fix your code and try again.`,
      report: {
        completed: false,
        durationMs: Date.now() - runStartedAt,
        toolCallCount: toolCalls.length,
        error: {
          message: errorMsg,
          ...parseErrorLocation(errorMsg),
          ...(options.scriptName ? { script: options.scriptName } : {}),
        },
      },
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
  const { tools: toolDefs, system: userSystem, memoryLimitMb, timeLimitMs, adapters, debug, autoFix, typeCheck } = options;
  validateToolDefinitions(toolDefs);

  const system = buildSystemPrompt(toolDefs, userSystem);

  const execOptions = { memoryLimitMb, timeLimitMs, debug, autoFix, typeCheck };
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

  const handleExecuteCodeInput = async (args: unknown): Promise<ExecutionResult> => {
    if (!isPlainObject(args) || typeof args.code !== "string") {
      const actual = !isPlainObject(args) ? jsTypeName(args) : jsTypeName(args.code);
      const message = `Invalid execute_code input: expected object with code: string, got ${actual}.`;
      if (!autoFix) {
        throw new Error(message);
      }

      const trace = createSpan("attempt_0", { "zapcode.code": "" });
      trace.attributes["zapcode.error"] = message;
      endSpan(trace, "error");
      if (sessionTrace) {
        sessionTrace.children.push(trace);
      }

      return {
        code: "",
        output: null,
        stdout: "",
        stderr: "",
        toolCalls: [],
        error: `Execution failed: ${message}. Please fix your code and try again.`,
        report: {
          completed: false,
          durationMs: 0,
          toolCallCount: 0,
          error: { message },
        },
        trace,
      };
    }

    return handleToolCall(args.code);
  };

  // AI SDK format — use tool() + jsonSchema() for proper integration
  const tools: ToolSet = {
    execute_code: tool({
      description: CODE_TOOL_DESCRIPTION,
      inputSchema: jsonSchema(CODE_TOOL_SCHEMA),
      execute: handleExecuteCodeInput,
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
  options?: {
    memoryLimitMb?: number;
    timeLimitMs?: number;
    debug?: boolean;
    autoFix?: boolean;
    typeCheck?: boolean;
    /** Label identifying this script — named in error messages / `report.error.script`. */
    scriptName?: string;
  }
): Promise<ExecutionResult> {
  return executeCode(code, tools, options ?? {});
}

// ---------------------------------------------------------------------------
// dryRun — pre-flight: does agent code typecheck AND not instantly error?
// ---------------------------------------------------------------------------

/** Outcome of {@link dryRun}. */
export interface DryRunResult {
  /** True when the code typechecks AND ran to completion without throwing. */
  ok: boolean;
  /** Static type diagnostics (from {@link typecheck}). */
  typeErrors: TypeDiagnostic[];
  /** Did the (stubbed) execution reach completion without throwing? */
  reachedCompletion: boolean;
  /** Completion value under the stub tools (when it completed). */
  output?: unknown;
  /** The throw site, when execution failed (message + source location). */
  error?: { message: string; line?: number; column?: number };
  /** The sequence of tool calls the code WOULD make (names + validated input). */
  toolCalls: Array<{ name: string; input: Record<string, unknown> }>;
  stdout: string;
  stderr: string;
}

/**
 * Pre-flight agent-written code WITHOUT real side effects: static-typecheck it
 * against the tool signatures, then execute it with auto-stubbed tools under a
 * tight time budget. Answers the question that matters for write-once-run-
 * immediately code — "does this instantly error, and what would it call?" —
 * before any real tool runs.
 *
 * Tools are stubbed with a permissive empty object (member access yields
 * `undefined` rather than throwing), so a reported error is a real signal
 * (e.g. calling a method on an assumed-present field) rather than stub noise.
 * The recorded `toolCalls` are the call sequence the code would make.
 */
export async function dryRun(
  code: string,
  toolDefs: Record<string, ToolDefinition>,
  options?: { timeLimitMs?: number; memoryLimitMb?: number; engine?: TypecheckOptions["engine"] }
): Promise<DryRunResult> {
  const tc = await typecheck(code, toolDefs, { engine: options?.engine });

  // Stub every tool: same schema (so argument validation still runs and the
  // call is recorded), but a side-effect-free implementation returning a
  // permissive empty object.
  const stubs: Record<string, ToolDefinition> = {};
  for (const [name, def] of Object.entries(toolDefs)) {
    stubs[name] = { ...def, execute: async () => ({}) };
  }

  const result = await executeCode(code, stubs, {
    autoFix: true, // capture failure as a structured result instead of throwing
    timeLimitMs: options?.timeLimitMs ?? 2_000,
    memoryLimitMb: options?.memoryLimitMb,
  });

  return {
    ok: tc.ok && result.report.completed,
    typeErrors: tc.diagnostics,
    reachedCompletion: result.report.completed,
    output: result.report.completed ? result.output : undefined,
    error: result.report.error,
    toolCalls: result.toolCalls.map((c) => ({ name: c.name, input: c.input })),
    stdout: result.stdout,
    stderr: result.stderr,
  };
}

// ---------------------------------------------------------------------------
// Forking — branch / checkpoint / rollback over VM state
// ---------------------------------------------------------------------------

/**
 * Fork a suspended VM: load its snapshot bytes into an independent, runnable
 * continuation. Loading the SAME bytes more than once yields multiple forks
 * that diverge only by what you resume them with — each is deterministic, and
 * (since compiled programs are shared) cheap. This is the primitive behind
 * agent **checkpoint / rollback / speculative branching**: snapshot at a tool
 * boundary, fork, try a branch, and discard or keep it.
 *
 * `snapshotBytes` come from a `suspended` execution state (its `snapshot`
 * field). The returned handle is resumed with `resume(value)` /
 * `resumeError(msg)` / `resumeMany(values)` exactly like the original.
 */
export function forkSnapshot(snapshotBytes: Buffer): ZapcodeSnapshotHandle {
  return ZapcodeSnapshotHandle.load(snapshotBytes);
}

// ---------------------------------------------------------------------------
// prepare — compile agent code ONCE, run it many times
// ---------------------------------------------------------------------------

/** A compiled, reusable program returned by {@link prepare}. */
export interface PreparedProgram {
  /** The source this program was compiled from. */
  readonly code: string;
  /**
   * Run the compiled program, driving the tool loop with the same validation,
   * trace, and structured report as {@link execute} — but WITHOUT re-parsing
   * or re-compiling. Each call is an independent run.
   */
  run(
    inputs?: Record<string, unknown>,
    options?: { debug?: boolean; autoFix?: boolean }
  ): Promise<ExecutionResult>;
}

/**
 * Compile agent-written code once into a reusable program. A run-many host
 * (the same code over many inputs, or a hot path) pays parse + compile a
 * single time, then each `run()` only constructs a VM and dispatches — the
 * package-level surface for the cycle-1 program cache. Tool validation, the
 * tool-call trace, and the structured run report are identical to `execute`.
 */
export function prepare(
  code: string,
  toolDefs: Record<string, ToolDefinition>,
  options?: { memoryLimitMb?: number; timeLimitMs?: number }
): PreparedProgram {
  validateToolDefinitions(toolDefs);
  const handle = ZapcodeProgramHandle.compile(code, {
    externalFunctions: Object.keys(toolDefs),
    timeLimitMs: options?.timeLimitMs ?? 10_000,
    memoryLimitMb: options?.memoryLimitMb ?? 32,
  });
  return {
    code,
    run(inputs, runOptions) {
      return executeCode(code, toolDefs, {
        ...runOptions,
        memoryLimitMb: options?.memoryLimitMb,
        timeLimitMs: options?.timeLimitMs,
        // Reuse the whole executeCode loop, but start from the pre-compiled
        // program rather than recompiling `code`.
        starter: () =>
          handle.start(
            inputs as Record<string, unknown> | undefined
          ) as ReturnType<Zapcode["start"]>,
      });
    },
  };
}

// ---------------------------------------------------------------------------
// Durable sessions
// ---------------------------------------------------------------------------

/** Outcome of resolving a single tool call inside a session driver. */
type ToolOutcome = { ok: true; result: unknown } | { ok: false; message: string; name: string };

/**
 * Validate + run one tool call, recording a toolCalls entry. Throws on a
 * *malformed* call (a code bug → abort). Returns a discriminated outcome for a
 * *runtime* tool failure so the caller can raise it back into the sandbox.
 */
async function invokeToolCall(
  toolDefs: Record<string, ToolDefinition>,
  toolNames: string[],
  name: string,
  rawArgs: unknown[],
  toolCalls: ExecutionResult["toolCalls"]
): Promise<ToolOutcome> {
  const toolDef = toolDefs[name];
  if (!toolDef) {
    throw new Error(
      `Guest code called unknown function '${name}'. Available: ${toolNames.join(", ")}`
    );
  }
  const namedArgs = buildNamedArgs(name, toolDef, rawArgs); // throws → abort
  const startedAt = Date.now();
  try {
    const result = sanitizeToolResult(await toolDef.execute(namedArgs));
    toolCalls.push({
      name,
      args: rawArgs,
      input: namedArgs,
      result,
      durationMs: Date.now() - startedAt,
    });
    return { ok: true, result };
  } catch (err: any) {
    const message = err?.message ?? String(err);
    const errName = (typeof err?.name === "string" && err.name) || "Error";
    toolCalls.push({
      name,
      args: rawArgs,
      input: namedArgs,
      result: undefined,
      error: message,
      durationMs: Date.now() - startedAt,
    });
    return { ok: false, message, name: errName };
  }
}

/** A batched external call exposed by a `suspended_many` suspension. */
type BatchCall = { name: string; args: unknown[] };

/**
 * The subset of a snapshot/session handle a batch resume needs. Both
 * `ZapcodeSnapshotHandle` and `ZapcodeSessionHandle` satisfy this.
 */
interface BatchResumable<S> {
  resumeMany(results: unknown[]): S;
  resumeError(error: unknown): S;
  resumeErrorObject(message: string, name?: string): S;
}

/**
 * Settle a `Promise.{all,race,any,allSettled}` batch using REAL JS promises so
 * race/any honor real settle timing, then resume the VM per combinator:
 *   all        -> Promise.all        -> resumeMany(values) | resumeError(first)
 *   allSettled -> Promise.allSettled -> resumeMany(settledObjects)  (never rejects)
 *   race       -> Promise.race       -> resumeMany([value]) | resumeError(reason)
 *   any        -> Promise.any        -> resumeMany([value]) | resumeError(AggregateError)
 *
 * A *malformed* call (thrown by `invoke`) aborts the whole execution; a tool
 * *rejection* is surfaced as a rejected JS promise so the combinator sees it.
 */
async function settleBatch<S>(
  handle: BatchResumable<S>,
  combinator: PromiseCombinator,
  calls: BatchCall[],
  invoke: (name: string, args: unknown[]) => Promise<ToolOutcome>
): Promise<S> {
  // A *malformed* call (unknown function / bad args) throws a plain error and
  // must ABORT the whole execution — it is never catchable by the guest. Track
  // the first such fatal error so we can re-throw it after settling.
  let fatal: unknown;
  let sawFatal = false;

  // Each call becomes a REAL JS promise so race/any honor real settle timing.
  // A fulfilled tool resolves with its result; a *runtime* tool failure rejects
  // with a `BatchToolError` (catchable by the combinator). A malformed call's
  // throw is recorded as fatal and re-surfaced as a never-settling rejection so
  // it can't accidentally win a race/any.
  const promises = calls.map(async call => {
    let outcome: ToolOutcome;
    try {
      outcome = await invoke(call.name, call.args);
    } catch (err) {
      if (!sawFatal) {
        sawFatal = true;
        fatal = err;
      }
      // Fatal: surface via the shared `fatal` holder (checked below). Reject so
      // this call never resolves to a bogus value.
      throw new BatchToolError("__fatal__");
    }
    if (outcome.ok) return outcome.result;
    throw new BatchToolError(outcome.message, outcome.name);
  });

  // Run the real combinator for value + timing semantics, then translate the
  // settled outcome into a VM resume. Re-throw any fatal malformed-call error.
  let resume: () => S;
  try {
    switch (combinator) {
      case "all": {
        const results = await Promise.all(promises);
        resume = () => handle.resumeMany(results);
        break;
      }
      case "allSettled": {
        const settled = await Promise.allSettled(promises);
        const objects = settled.map(s =>
          s.status === "fulfilled"
            ? { status: "fulfilled", value: s.value }
            : { status: "rejected", reason: batchErrorReason(s.reason) }
        );
        resume = () => handle.resumeMany(objects);
        break;
      }
      case "race": {
        const value = await Promise.race(promises);
        resume = () => handle.resumeMany([value]);
        break;
      }
      case "any": {
        const value = await Promise.any(promises);
        resume = () => handle.resumeMany([value]);
        break;
      }
      default: {
        // Exhaustiveness guard — an unknown combinator is a binding/version skew.
        const never: never = combinator;
        throw new Error(`unknown promise combinator: ${String(never)}`);
      }
    }
  } catch (err) {
    if (sawFatal) throw fatal; // malformed call — abort
    // A catchable tool rejection (all/race) or an all-rejected any. Raise as a
    // real Error object, preserving both the host tool's error name (TypeError,
    // …) and — for `any` — the `AggregateError` name.
    return handle.resumeErrorObject(batchErrorMessage(err), batchErrorName(err));
  }
  if (sawFatal) throw fatal; // fatal lost the race but must still abort
  return resume();
}

/**
 * A tool rejection carried through a real JS promise inside {@link settleBatch}.
 * Holds the host error's `name` (e.g. `TypeError`) separately so it survives back
 * into the guest — `this.name` is left as `"Error"` so the fatal path's toString
 * is unsurprising.
 */
class BatchToolError extends Error {
  readonly toolErrorName: string;
  constructor(message: string, toolErrorName = "Error") {
    super(message);
    this.toolErrorName = toolErrorName;
  }
}

/** Best-effort message extraction for a rejection reason or AggregateError. */
function batchErrorMessage(err: unknown): string {
  if (err instanceof AggregateError) {
    const inner = err.errors?.map(e => batchErrorMessage(e)).join(", ");
    return inner ? `AggregateError: ${inner}` : "AggregateError: all promises were rejected";
  }
  if (err instanceof Error) return err.message;
  return String(err);
}

/**
 * The error NAME to surface for a batch rejection: the host tool's subclass
 * (`TypeError`, …) for a single rejection, or `AggregateError` when every call
 * in a `Promise.any` rejected.
 */
function batchErrorName(err: unknown): string {
  if (err instanceof AggregateError) return "AggregateError";
  if (err instanceof BatchToolError) return err.toolErrorName;
  if (err instanceof Error && typeof err.name === "string") return err.name;
  return "Error";
}

/**
 * Build a guest-visible Error for a `Promise.allSettled` rejected `reason`. The
 * `__error__` brand makes `reason instanceof Error` hold (and hides `name`/
 * `message`/`stack` from enumeration, matching a real Error) once `resumeMany`
 * marshals it into the VM — so allSettled reasons are real Errors like every
 * other rejection path, not bare strings.
 */
function batchErrorReason(err: unknown): {
  __error__: true;
  name: string;
  message: string;
  stack: string;
} {
  const name = batchErrorName(err);
  const message = batchErrorMessage(err);
  return { __error__: true, name, message, stack: `${name}: ${message}` };
}

/** Options for {@link createSession} / {@link loadSession}. */
export interface SessionOptions {
  /** Tools available to guest code across the whole session. */
  tools: Record<string, ToolDefinition>;
  /** Memory limit in MB (default: 32). */
  memoryLimitMb?: number;
  /** Execution time limit per chunk in ms (default: 10000). */
  timeLimitMs?: number;
  /**
   * Optional label for this session (monty's `script_name`). When set, an error
   * thrown by any chunk is prefixed `[scriptName #<chunkIndex>]`, so a failure
   * names which session — and which chunk within it — erred. Chunks are indexed
   * from 1 in the order `runChunk` is called.
   */
  scriptName?: string;
}

/** Result of running one session chunk. */
export interface SessionChunkResult {
  output: unknown;
  stdout: string;
  /** Captured stderr (`console.error`/`console.warn`) for this chunk. */
  stderr: string;
  toolCalls: ExecutionResult["toolCalls"];
}

/**
 * A durable, multi-chunk Zapcode session. Top-level bindings, functions, and
 * classes declared in one chunk are available to later chunks. The entire VM
 * state serializes to compact bytes via {@link ZapcodeSession.dump}, so a
 * workflow an agent defines now can be stored and resumed later — in another
 * process, a Temporal activity, or after a restart — with {@link loadSession}.
 *
 * Tool *implementations* are not serialized (only the VM state is), so the same
 * `tools` must be provided again when reloading.
 *
 * @example
 * ```typescript
 * const session = createSession({ tools: { fetchRow: {...} } });
 * await session.runChunk(`async function step(id) { return await fetchRow(id); }`);
 * const bytes = session.dump();              // hand to a Temporal activity
 * // ...later, elsewhere...
 * const resumed = loadSession(bytes, { tools: { fetchRow: {...} } });
 * const out = await resumed.runChunk(`await step("42")`);
 * ```
 */
export interface ZapcodeSession {
  /** Run a chunk to completion, resolving tool calls (incl. parallel batches). */
  runChunk(code: string, inputs?: Record<string, unknown>): Promise<SessionChunkResult>;
  /** Serialize the whole session state to bytes for storage / transport. */
  dump(): Buffer;
}

function makeSession(
  initialBytes: Buffer,
  toolDefs: Record<string, ToolDefinition>,
  scriptName?: string
): ZapcodeSession {
  validateToolDefinitions(toolDefs);
  const toolNames = Object.keys(toolDefs);
  let sessionBytes = initialBytes;
  let chunkIndex = 0;

  const runChunk = async (
    code: string,
    inputs?: Record<string, unknown>
  ): Promise<SessionChunkResult> => {
    chunkIndex++;
    const toolCalls: ExecutionResult["toolCalls"] = [];
    try {
      let state = ZapcodeSessionHandle.load(sessionBytes).runChunk(code, inputs ?? {});

      while (!state.completed) {
        const handle = ZapcodeSessionHandle.load(state.session);
        if (state.kind === "suspended_many") {
          state = await settleBatch(handle, state.combinator, state.calls, (name, args) =>
            invokeToolCall(toolDefs, toolNames, name, args, toolCalls)
          );
        } else {
          const outcome = await invokeToolCall(toolDefs, toolNames, state.functionName, state.args, toolCalls);
          state = outcome.ok
            ? handle.resume(outcome.result)
            : handle.resumeErrorObject(outcome.message, outcome.name);
        }
      }

      // Persist the new idle state for the next chunk / dump().
      sessionBytes = state.session;
      return { output: state.output, stdout: state.stdout ?? "", stderr: state.stderr ?? "", toolCalls };
    } catch (err: any) {
      // Name the erring session + chunk on the thrown error (opt-in via
      // scriptName); the failed chunk does NOT advance sessionBytes, so the
      // last good checkpoint stays usable.
      if (scriptName && err instanceof Error) {
        err.message = `[${scriptName} #${chunkIndex}] ${err.message}`;
      }
      throw err;
    }
  };

  return { runChunk, dump: () => sessionBytes };
}

/** Create a new durable session. */
export function createSession(options: SessionOptions): ZapcodeSession {
  validateToolDefinitions(options.tools);
  const handle = ZapcodeSessionHandle.create({
    externalFunctions: Object.keys(options.tools),
    memoryLimitMb: options.memoryLimitMb,
    timeLimitMs: options.timeLimitMs,
  });
  return makeSession(handle.dump(), options.tools, options.scriptName);
}

/** Reload a durable session from bytes produced by {@link ZapcodeSession.dump}. */
export function loadSession(bytes: Buffer, options: SessionOptions): ZapcodeSession {
  // Validate the bytes are loadable up front (throws on a bad/incompatible blob).
  ZapcodeSessionHandle.load(bytes);
  return makeSession(bytes, options.tools, options.scriptName);
}
