"""
AI Agent with Zapcode — LOW-LEVEL approach using Anthropic SDK directly.

This shows the manual snapshot/resume loop using the zapcode package.
For most use cases, prefer zapcode-ai instead — see ai_agent_zapcode_ai.py
for the recommended approach.

The "code as tool use" pattern:
1. Claude writes TypeScript code that calls tools (external functions)
2. Zapcode executes the code in a sandbox
3. When the code calls a tool, Zapcode suspends and returns a snapshot
4. Your app resolves the tool call, then resumes Zapcode with the result

Prerequisites:
    pip install anthropic zapcode
    # or: uv add anthropic zapcode

Run with: python ai_agent_anthropic.py
"""

import anthropic
from zapcode import Zapcode, ZapcodeSnapshot

# --- Tool implementations (the real functions that run on your server) ---

MOCK_WEATHER = {
    "London": {"condition": "Overcast", "temp": 12, "humidity": 80},
    "Tokyo": {"condition": "Clear", "temp": 26, "humidity": 55},
    "New York": {"condition": "Rain", "temp": 14, "humidity": 88},
}

MOCK_FLIGHTS = [
    {"airline": "BA", "flight": "BA123", "price": 450, "departure": "08:00"},
    {"airline": "AF", "flight": "AF456", "price": 380, "departure": "14:30"},
]


def get_weather(city: str) -> dict:
    """In production, call a real weather API."""
    return MOCK_WEATHER.get(city, {"condition": "Unknown", "temp": 0, "humidity": 0})


def search_flights(origin: str, destination: str, date: str) -> list[dict]:
    """In production, call a flight search API."""
    return MOCK_FLIGHTS


# Map of tool name -> implementation
TOOLS = {
    "getWeather": lambda city: get_weather(city),
    "searchFlights": lambda origin, dest, date: search_flights(origin, dest, date),
}

SYSTEM_PROMPT = """\
You are a helpful assistant. When the user asks a question that requires
external data (weather, flights, etc.), write TypeScript code that calls the
available functions and computes the answer.

Available functions (called with await):
- getWeather(city: string) → { condition: string, temp: number, humidity: number }
- searchFlights(origin: string, destination: string, date: string) → Array<{ airline: string, flight: string, price: number, departure: string }>

Available inputs: userQuery (the user's question as a string)

Return ONLY the TypeScript code, no markdown fences. The last expression is the output."""


def execute_in_sandbox(code: str, inputs: dict) -> any:
    """Execute AI-generated TypeScript in Zapcode's sandbox."""
    sandbox = Zapcode(
        code,
        inputs=list(inputs.keys()),
        external_functions=list(TOOLS.keys()),
        time_limit_ms=10_000,
    )

    state = sandbox.start(inputs)

    # Snapshot/resume loop: resolve each external call as it comes
    while state.get("suspended"):
        fn_name = state["function_name"]
        args = state["args"]
        print(f"  -> calling {fn_name}({args})")

        tool_fn = TOOLS.get(fn_name)
        if not tool_fn:
            raise ValueError(f"Unknown function: {fn_name}")

        result = tool_fn(*args)
        print(f"  <- {fn_name} returned: {result}")

        # Resume the sandbox with the tool's return value
        snapshot: ZapcodeSnapshot = state["snapshot"]
        state = snapshot.resume(result)

    return state["output"]


def run_agent(user_query: str):
    """Ask Claude to write code, then execute it in Zapcode."""
    client = anthropic.Anthropic()

    response = client.messages.create(
        model="claude-sonnet-4-20250514",
        max_tokens=1024,
        system=SYSTEM_PROMPT,
        messages=[{"role": "user", "content": user_query}],
    )

    code = response.content[0].text
    print(f"Claude wrote:\n{code}\n")

    result = execute_in_sandbox(code, {"userQuery": user_query})
    print(f"\nFinal answer: {result}")
    return result


def main():
    print("=== Example 1: Weather query ===\n")
    run_agent("What's the weather like in Tokyo?")

    print("\n=== Example 2: Multi-tool query ===\n")
    run_agent(
        "Compare the weather in London and New York, "
        "and find me cheap flights from London to New York for 2026-04-01."
    )


if __name__ == "__main__":
    main()
