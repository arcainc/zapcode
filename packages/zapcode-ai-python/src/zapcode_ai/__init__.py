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

import json
import math
import re
import time
from dataclasses import dataclass, field
from typing import Any, Callable, Awaitable

# Note: the native `zapcode` module is imported lazily inside `_execute_code`,
# so the system-prompt and argument-validation helpers can be used (and tested)
# without the compiled extension present.


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
class TraceSpan:
    """A single span in the execution trace. OTel-compatible shape."""
    name: str
    start_time: float  # ms since epoch
    end_time: float = 0.0
    duration_ms: float = 0.0
    status: str = "ok"  # "ok" or "error"
    attributes: dict[str, Any] = field(default_factory=dict)
    children: list[TraceSpan] = field(default_factory=list)


@dataclass
class ExecutionResult:
    """Result of executing guest code."""
    code: str
    output: Any
    stdout: str
    tool_calls: list[dict[str, Any]]
    error: str | None = None
    trace: TraceSpan | None = None


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

def _param_list(defn: ToolDefinition) -> str:
    return ", ".join(
        f"{pname}{'?' if pdef.optional else ''}: {pdef.type}"
        for pname, pdef in defn.parameters.items()
    )


def _generate_signature(name: str, defn: ToolDefinition) -> str:
    return f"{name}({_param_list(defn)})"


def _generate_named_object_signature(name: str, defn: ToolDefinition) -> str:
    return f"{name}({{ {_param_list(defn)} }})"


def _generate_declaration(name: str, defn: ToolDefinition) -> str:
    entries = list(defn.parameters.items())
    if len(entries) > 1:
        fields = "; ".join(
            f"{pname}{'?' if pdef.optional else ''}: {pdef.type}" for pname, pdef in entries
        )
        return f"declare function {name}(input: {{ {fields} }}): Promise<unknown>;"
    return f"declare function {name}({_param_list(defn)}): Promise<unknown>;"


def _build_system_prompt(
    tools: dict[str, ToolDefinition],
    user_system: str | None = None,
) -> str:
    def _doc(name: str, defn: ToolDefinition) -> str:
        signature = (
            _generate_named_object_signature(name, defn)
            if len(defn.parameters) > 1
            else _generate_signature(name, defn)
        )
        return f"- {_generate_declaration(name, defn)}\n  Call shape: await {signature}\n  {defn.description}"

    tool_docs = "\n".join(_doc(name, defn) for name, defn in tools.items())

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
- Prefer the declared function signatures above exactly. For tools with more than one parameter, call them with one named object argument, e.g. `await toolName({{ key: value }})`.
- When a tool returns a structured object, access its properties directly instead of reparsing the result as text.
- If the user's question doesn't need tools, you can compute the answer directly."""
    )

    return "\n\n".join(parts)


# ---------------------------------------------------------------------------
# Tool name + argument validation (parity with the TS package)
# ---------------------------------------------------------------------------

_RESERVED_TOOL_NAMES = {
    "console", "JSON", "Object", "Array", "Math", "Promise", "Map", "Date",
    "eval", "Function", "process", "globalThis", "global", "require", "execute_code",
}

_IDENTIFIER_RE = re.compile(r"^[A-Za-z_$][A-Za-z0-9_$]*$")


def _validate_tool_definitions(tool_defs: dict[str, ToolDefinition]) -> None:
    for name in tool_defs:
        if not _IDENTIFIER_RE.match(name):
            raise ValueError(
                f"Invalid tool name '{name}': tool names must be valid JavaScript identifiers."
            )
        if name in _RESERVED_TOOL_NAMES:
            raise ValueError(f"Invalid tool name '{name}': this name is reserved by Zapcode.")


def _is_plain_object(value: Any) -> bool:
    return isinstance(value, dict)


def _js_type_name(value: Any) -> str:
    if isinstance(value, list):
        return "array"
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "boolean"
    if isinstance(value, (int, float)):
        return "number"
    if isinstance(value, str):
        return "string"
    if isinstance(value, dict):
        return "object"
    return type(value).__name__


def _matches_param_type(value: Any, pdef: ParamDef) -> bool:
    if pdef.type == "array":
        return isinstance(value, list)
    if pdef.type == "object":
        return _is_plain_object(value)
    if pdef.type == "number":
        # bool is an int subclass in Python — exclude it; require a finite number.
        return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)
    if pdef.type == "string":
        return isinstance(value, str)
    if pdef.type == "boolean":
        return isinstance(value, bool)
    return False


def _format_tool_signature(name: str, defn: ToolDefinition) -> str:
    return (
        _generate_named_object_signature(name, defn)
        if len(defn.parameters) > 1
        else _generate_signature(name, defn)
    )


def _should_treat_single_object_arg_as_named(defn: ToolDefinition, arg: dict[str, Any]) -> bool:
    entries = list(defn.parameters.items())
    if len(entries) != 1:
        return False
    name, param = entries[0]
    if name not in arg:
        return False
    # Non-object single params are unambiguous: foo({ id }) means named input.
    if param.type != "object":
        return True
    # For a single object param, support foo({ payload: {...} }) as named input,
    # but keep foo({ arbitrary: "shape" }) as the payload value itself.
    return len(arg) == 1 or any(key not in defn.parameters for key in arg)


def _build_named_args(fn_name: str, defn: ToolDefinition, args: list[Any]) -> dict[str, Any]:
    """Normalize the positional args emitted by guest code into a validated,
    named argument dict — mirroring the TypeScript package."""
    param_entries = list(defn.parameters.items())
    param_names = [name for name, _ in param_entries]
    single_object_arg = args[0] if len(args) == 1 and _is_plain_object(args[0]) else None
    uses_named_object = single_object_arg is not None and (
        len(param_entries) > 1
        or _should_treat_single_object_arg_as_named(defn, single_object_arg)
    )

    if len(param_entries) > 1 and not uses_named_object:
        raise ValueError(
            f"Invalid arguments for tool '{fn_name}': expected one named object argument. "
            f"Use {_format_tool_signature(fn_name, defn)}."
        )

    if uses_named_object:
        named: dict[str, Any] = dict(single_object_arg)
    else:
        named = {param_names[i]: args[i] for i in range(min(len(param_names), len(args)))}
        extra = len(args) - len(param_names)
        if extra > 0:
            raise ValueError(
                f"Invalid arguments for tool '{fn_name}': received {len(args)} positional "
                f"arguments but expected {len(param_names)}. Use {_format_tool_signature(fn_name, defn)}."
            )

    unexpected = [name for name in named if name not in defn.parameters]
    if unexpected:
        raise ValueError(
            f"Invalid arguments for tool '{fn_name}': unexpected parameter '{unexpected[0]}'. "
            f"Expected {_format_tool_signature(fn_name, defn)}."
        )

    for pname, pdef in param_entries:
        value = named.get(pname)
        # None covers an absent / undefined field (which crosses the boundary as
        # None). Optional → omit; required → a clear missing-field error.
        if value is None:
            if not pdef.optional:
                raise ValueError(
                    f"Invalid arguments for tool '{fn_name}': missing required parameter '{pname}'. "
                    f"Expected {_format_tool_signature(fn_name, defn)}."
                )
            named.pop(pname, None)
            continue
        if not _matches_param_type(value, pdef):
            raise ValueError(
                f"Invalid arguments for tool '{fn_name}': parameter '{pname}' expected "
                f"{pdef.type}, got {_js_type_name(value)}. Expected {_format_tool_signature(fn_name, defn)}."
            )

    return named


# ---------------------------------------------------------------------------
# Trace helpers
# ---------------------------------------------------------------------------

def _create_span(name: str, attributes: dict[str, Any] | None = None) -> TraceSpan:
    return TraceSpan(
        name=name,
        start_time=time.time() * 1000,
        attributes=attributes or {},
    )


def _end_span(span: TraceSpan, status: str | None = None) -> TraceSpan:
    span.end_time = time.time() * 1000
    span.duration_ms = span.end_time - span.start_time
    if status:
        span.status = status
    return span


def _print_trace(span: TraceSpan, indent: int = 0) -> None:
    prefix = "" if indent == 0 else "│ " * (indent - 1) + "├─ "
    icon = "✗" if span.status == "error" else "✓"
    duration = "<1ms" if span.duration_ms < 1 else f"{span.duration_ms:.0f}ms"
    attrs = " ".join(
        f"{k}={str(v)[:80]}" for k, v in span.attributes.items()
        if not k.startswith("zapcode.code")  # don't dump full code in trace
    )
    print(f"{prefix}{icon} {span.name} ({duration}){' ' + attrs if attrs else ''}")
    for child in span.children:
        _print_trace(child, indent + 1)


# ---------------------------------------------------------------------------
# Execution engine
# ---------------------------------------------------------------------------

def _execute_code(
    code: str,
    tool_defs: dict[str, ToolDefinition],
    *,
    memory_limit_bytes: int | None = None,
    time_limit_ms: int | None = None,
    debug: bool = False,
    auto_fix: bool = False,
) -> ExecutionResult:
    _validate_tool_definitions(tool_defs)
    tool_names = list(tool_defs.keys())
    tool_calls: list[dict[str, Any]] = []
    tracing = debug or auto_fix

    exec_span = _create_span("execute", {"zapcode.code": code}) if tracing else None

    try:
        from zapcode import Zapcode, ZapcodeSnapshot  # native module (lazy)

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

            tool_span = _create_span("tool_call", {
                "zapcode.tool.name": fn_name,
                "zapcode.tool.args": json.dumps(args, default=str),
            }) if tracing else None

            # Validate + normalize into named args (schema-aware, like the TS pkg).
            try:
                named_args = _build_named_args(fn_name, tool_def, args)
            except Exception as err:
                if tool_span:
                    tool_span.attributes["zapcode.tool.error"] = str(err)
                    _end_span(tool_span, "error")
                    exec_span.children.append(tool_span)
                raise

            if tool_span:
                tool_span.attributes["zapcode.tool.input"] = json.dumps(named_args, default=str)

            result = tool_def.execute(named_args)
            tool_calls.append(
                {"name": fn_name, "args": args, "input": named_args, "result": result}
            )

            if tool_span:
                tool_span.attributes["zapcode.tool.result"] = json.dumps(result, default=str)
                _end_span(tool_span)
                exec_span.children.append(tool_span)

            snapshot: ZapcodeSnapshot = state["snapshot"]
            state = snapshot.resume(result)

        stdout = state.get("stdout", "")

        if exec_span:
            exec_span.attributes["zapcode.output"] = json.dumps(state.get("output"), default=str)
            if stdout:
                exec_span.attributes["zapcode.stdout"] = stdout
            _end_span(exec_span)

        if debug and exec_span:
            _print_trace(exec_span)

        return ExecutionResult(
            code=code,
            output=state.get("output"),
            stdout=stdout,
            tool_calls=tool_calls,
            trace=exec_span,
        )
    except Exception as err:
        error_msg = str(err)

        if exec_span:
            exec_span.attributes["zapcode.error"] = error_msg
            _end_span(exec_span, "error")

        if not auto_fix:
            if debug and exec_span:
                _print_trace(exec_span)
            raise

        if debug and exec_span:
            _print_trace(exec_span)

        return ExecutionResult(
            code=code,
            output=None,
            stdout="",
            tool_calls=tool_calls,
            error=f"Execution failed: {error_msg}. Please fix your code and try again.",
            trace=exec_span,
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

    get_trace: Callable[[], TraceSpan | None] = field(default=lambda: None)
    """Get the full session trace tree. Available when debug or auto_fix is enabled."""

    print_trace: Callable[[], None] = field(default=lambda: None)
    """Print the full session trace tree to the console."""


# ---------------------------------------------------------------------------
# Main entry point
# ---------------------------------------------------------------------------

def zapcode(
    tools: dict[str, ToolDefinition],
    *,
    system: str | None = None,
    memory_limit_bytes: int | None = None,
    time_limit_ms: int = 10_000,
    debug: bool = False,
    auto_fix: bool = False,
    adapters: list[Adapter] | None = None,
) -> ZapcodeAI:
    """
    Create AI SDK-compatible system prompt and tools for Zapcode.

    Returns adapters for every major AI SDK:
    - `anthropic_tools` → Anthropic SDK (`messages.create`)
    - `openai_tools` → OpenAI SDK (`chat.completions.create`)
    - `handle_tool_call(code)` → Universal handler for any SDK
    - `custom` → Output from custom adapters

    Args:
        debug: Log generated code, tool calls, and output to the console.
        auto_fix: When True, execution errors are returned as tool results
            instead of raising. The LLM sees the error and can self-correct.

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
    _validate_tool_definitions(tools)
    system_prompt = _build_system_prompt(tools, system)
    tracing = debug or auto_fix

    # Session-level trace collects all attempts
    session_trace: TraceSpan | None = (
        _create_span("session", {"zapcode.tools": ", ".join(tools.keys())})
        if tracing else None
    )
    attempt_count = 0

    def handle_tool_call(code: str) -> ExecutionResult:
        nonlocal attempt_count
        attempt_count += 1
        result = _execute_code(
            code, tools,
            memory_limit_bytes=memory_limit_bytes,
            time_limit_ms=time_limit_ms,
            debug=debug,
            auto_fix=auto_fix,
        )
        if session_trace and result.trace:
            result.trace.name = f"attempt_{attempt_count}"
            result.trace.attributes["zapcode.attempt"] = attempt_count
            session_trace.children.append(result.trace)
        return result

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

    def get_trace() -> TraceSpan | None:
        if not session_trace:
            return None
        status = "ok" if any(c.status == "ok" for c in session_trace.children) else "error"
        _end_span(session_trace, status)
        return session_trace

    def print_session_trace() -> None:
        trace = get_trace()
        if trace:
            print("\n─── Zapcode Trace ───")
            _print_trace(trace)
            print("─────────────────────\n")

    return ZapcodeAI(
        system=system_prompt,
        anthropic_tools=anthropic_tools,
        openai_tools=openai_tools,
        handle_tool_call=handle_tool_call,
        custom=custom,
        get_trace=get_trace,
        print_trace=print_session_trace,
    )


def execute(
    code: str,
    tools: dict[str, ToolDefinition],
    *,
    memory_limit_bytes: int | None = None,
    time_limit_ms: int | None = None,
    debug: bool = False,
    auto_fix: bool = False,
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
        debug=debug,
        auto_fix=auto_fix,
    )
