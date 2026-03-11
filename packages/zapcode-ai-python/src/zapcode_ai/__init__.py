"""
zapcode-ai — High-level AI SDK integration for Zapcode.

Works with any AI SDK:

    # Anthropic SDK
    from zapcode_ai import zapcode
    b = zapcode(tools={...})
    response = client.messages.create(system=b.system, tools=b.anthropic_tools, ...)
    result = b.handle_tool_call(code)

    # OpenAI SDK
    b = zapcode(tools={...})
    response = client.chat.completions.create(
        messages=[{"role": "system", "content": b.system}, ...],
        tools=b.openai_tools,
    )
    result = b.handle_tool_call(code)

    # Custom adapter
    from zapcode_ai import zapcode, Adapter
    class MyAdapter(Adapter):
        name = "my-sdk"
        def adapt(self, ctx):
            return {"system": ctx.system, "tool": ctx.tool_schema}
    b = zapcode(tools={...}, adapters=[MyAdapter()])
    config = b.custom["my-sdk"]
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Callable, Awaitable

from zapcode import Zapcode, ZapcodeSnapshot


# ---------------------------------------------------------------------------
# Public types
# ---------------------------------------------------------------------------

@dataclass
class ParamDef:
    """Schema for a single parameter."""
    type: str  # "string" | "number" | "boolean" | "object" | "array"
    description: str = ""
    optional: bool = False


@dataclass
class ToolDefinition:
    """Definition for a single tool that guest code can call."""
    description: str
    parameters: dict[str, ParamDef]
    execute: Callable[..., Any]  # (args: dict) -> Any or awaitable


@dataclass
class ExecutionResult:
    """Result of executing guest code."""
    output: Any
    stdout: str
    tool_calls: list[dict[str, Any]]


# ---------------------------------------------------------------------------
# Adapter protocol
# ---------------------------------------------------------------------------

@dataclass
class AdapterContext:
    """Context passed to custom adapters."""
    system: str
    tool_name: str
    tool_description: str
    tool_schema: dict[str, Any]
    handle_tool_call: Callable[[str], ExecutionResult]


class Adapter:
    """
    Base class for custom SDK adapters.

    Subclass this to add support for any AI SDK:

        class LangChainAdapter(Adapter):
            name = "langchain"
            def adapt(self, ctx: AdapterContext):
                from langchain_core.tools import StructuredTool
                return StructuredTool.from_function(
                    func=lambda code: ctx.handle_tool_call(code),
                    name=ctx.tool_name,
                    description=ctx.tool_description,
                )
    """
    name: str = ""

    def adapt(self, ctx: AdapterContext) -> Any:
        raise NotImplementedError


# ---------------------------------------------------------------------------
# System prompt generation
# ---------------------------------------------------------------------------

def _generate_signature(name: str, defn: ToolDefinition) -> str:
    params = ", ".join(
        f"{pname}{'?' if pdef.optional else ''}: {pdef.type}"
        for pname, pdef in defn.parameters.items()
    )
    return f"{name}({params})"


def _build_system_prompt(
    tools: dict[str, ToolDefinition],
    user_system: str | None = None,
) -> str:
    tool_docs = "\n".join(
        f"- await {_generate_signature(name, defn)}\n  {defn.description}"
        for name, defn in tools.items()
    )

    parts = []
    if user_system:
        parts.append(user_system)

    parts.append(
        f"""When you need to use tools or compute something, write TypeScript code and pass it to the execute_code tool.
The code runs in a sandboxed interpreter with these functions available (use await):

{tool_docs}

Rules:
- Write ONLY TypeScript code, no markdown fences, no explanation.
- The last expression in your code is the return value.
- You can use variables, loops, conditionals, array methods, etc.
- All tool calls must use `await`.
- If the user's question doesn't need tools, you can compute the answer directly."""
    )

    return "\n\n".join(parts)


# ---------------------------------------------------------------------------
# Execution engine
# ---------------------------------------------------------------------------

def _execute_code(
    code: str,
    tool_defs: dict[str, ToolDefinition],
    *,
    memory_limit_bytes: int | None = None,
    time_limit_ms: int | None = None,
) -> ExecutionResult:
    tool_names = list(tool_defs.keys())
    tool_calls: list[dict[str, Any]] = []

    kwargs: dict[str, Any] = {"external_functions": tool_names}
    if time_limit_ms is not None:
        kwargs["time_limit_ms"] = time_limit_ms
    if memory_limit_bytes is not None:
        kwargs["memory_limit_bytes"] = memory_limit_bytes

    sandbox = Zapcode(code, **kwargs)
    state = sandbox.start()

    while state.get("suspended"):
        fn_name = state["function_name"]
        args = state["args"]

        tool_def = tool_defs.get(fn_name)
        if not tool_def:
            raise ValueError(
                f"Guest code called unknown function '{fn_name}'. "
                f"Available: {', '.join(tool_names)}"
            )

        # Build named args from positional args
        param_names = list(tool_def.parameters.keys())
        named_args = {
            param_names[i]: args[i]
            for i in range(min(len(param_names), len(args)))
        }

        result = tool_def.execute(named_args)
        tool_calls.append({"name": fn_name, "args": args, "result": result})

        snapshot: ZapcodeSnapshot = state["snapshot"]
        state = snapshot.resume(result)

    return ExecutionResult(
        output=state.get("output"),
        stdout=state.get("stdout", ""),
        tool_calls=tool_calls,
    )


# ---------------------------------------------------------------------------
# Tool schema
# ---------------------------------------------------------------------------

_CODE_TOOL_DESCRIPTION = (
    "Execute TypeScript code in a secure sandbox. "
    "The code can call the available tool functions using await. "
    "The last expression is the return value."
)

_CODE_TOOL_SCHEMA = {
    "type": "object",
    "properties": {
        "code": {
            "type": "string",
            "description": "TypeScript code to execute in the sandbox",
        },
    },
    "required": ["code"],
}


# ---------------------------------------------------------------------------
# Result object
# ---------------------------------------------------------------------------

@dataclass
class ZapcodeAI:
    """Result of `zapcode()` — adapters for every major AI SDK."""

    system: str
    """System prompt instructing the LLM to write TypeScript."""

    anthropic_tools: list[dict[str, Any]]
    """Anthropic SDK tool format. Use with `messages.create(tools=...)`."""

    openai_tools: list[dict[str, Any]]
    """OpenAI SDK tool format. Use with `chat.completions.create(tools=...)`."""

    handle_tool_call: Callable[[str], ExecutionResult]
    """Execute code from a tool call. Works with any SDK."""

    custom: dict[str, Any] = field(default_factory=dict)
    """Output from custom adapters, keyed by adapter name."""


# ---------------------------------------------------------------------------
# Main entry point
# ---------------------------------------------------------------------------

def zapcode(
    tools: dict[str, ToolDefinition],
    *,
    system: str | None = None,
    memory_limit_bytes: int | None = None,
    time_limit_ms: int = 10_000,
    adapters: list[Adapter] | None = None,
) -> ZapcodeAI:
    """
    Create AI SDK-compatible system prompt and tools for Zapcode.

    Returns adapters for every major AI SDK:
    - `anthropic_tools` → Anthropic SDK (`messages.create`)
    - `openai_tools` → OpenAI SDK (`chat.completions.create`)
    - `handle_tool_call(code)` → Universal handler for any SDK
    - `custom` → Output from custom adapters

    Example with Anthropic SDK::

        from zapcode_ai import zapcode, ToolDefinition, ParamDef
        import anthropic

        b = zapcode(
            tools={
                "getWeather": ToolDefinition(
                    description="Get weather for a city",
                    parameters={"city": ParamDef(type="string")},
                    execute=lambda args: get_weather(args["city"]),
                ),
            },
            system="You are a helpful travel assistant.",
        )

        client = anthropic.Anthropic()
        response = client.messages.create(
            model="claude-sonnet-4-20250514",
            system=b.system,
            tools=b.anthropic_tools,
            messages=[{"role": "user", "content": "Weather in Tokyo?"}],
        )

        for block in response.content:
            if block.type == "tool_use":
                result = b.handle_tool_call(block.input["code"])
                print(result.output)
    """
    system_prompt = _build_system_prompt(tools, system)

    def handle_tool_call(code: str) -> ExecutionResult:
        return _execute_code(
            code, tools,
            memory_limit_bytes=memory_limit_bytes,
            time_limit_ms=time_limit_ms,
        )

    # Anthropic SDK format
    anthropic_tools = [
        {
            "name": "execute_code",
            "description": _CODE_TOOL_DESCRIPTION,
            "input_schema": _CODE_TOOL_SCHEMA,
        }
    ]

    # OpenAI SDK format
    openai_tools = [
        {
            "type": "function",
            "function": {
                "name": "execute_code",
                "description": _CODE_TOOL_DESCRIPTION,
                "parameters": _CODE_TOOL_SCHEMA,
            },
        }
    ]

    # Run custom adapters
    custom: dict[str, Any] = {}
    if adapters:
        ctx = AdapterContext(
            system=system_prompt,
            tool_name="execute_code",
            tool_description=_CODE_TOOL_DESCRIPTION,
            tool_schema=_CODE_TOOL_SCHEMA,
            handle_tool_call=handle_tool_call,
        )
        for adapter in adapters:
            custom[adapter.name] = adapter.adapt(ctx)

    return ZapcodeAI(
        system=system_prompt,
        anthropic_tools=anthropic_tools,
        openai_tools=openai_tools,
        handle_tool_call=handle_tool_call,
        custom=custom,
    )


def execute(
    code: str,
    tools: dict[str, ToolDefinition],
    *,
    memory_limit_bytes: int | None = None,
    time_limit_ms: int | None = None,
) -> ExecutionResult:
    """
    Execute TypeScript code directly in a Zapcode sandbox with tool resolution.

    Lower-level API if you don't need AI SDK integration::

        from zapcode_ai import execute, ToolDefinition, ParamDef

        result = execute(
            'const w = await getWeather("Tokyo"); w.temp',
            tools={
                "getWeather": ToolDefinition(
                    description="Get weather",
                    parameters={"city": ParamDef(type="string")},
                    execute=lambda args: {"temp": 26, "condition": "Clear"},
                ),
            },
        )
        print(result.output)  # 26
    """
    return _execute_code(
        code, tools,
        memory_limit_bytes=memory_limit_bytes,
        time_limit_ms=time_limit_ms,
    )
