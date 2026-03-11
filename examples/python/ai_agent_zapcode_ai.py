"""
AI Agent using zapcode-ai — the high-level wrapper.

This is the recommended way to use Zapcode with AI models.
One call to `zapcode()` gives you system prompt + tool definitions
that plug directly into any AI SDK.

Prerequisites:
    pip install zapcode-ai anthropic
    # or: uv add zapcode-ai anthropic
    export ANTHROPIC_API_KEY=sk-ant-...

Run with: python ai_agent_zapcode_ai.py
"""

import anthropic
from zapcode_ai import zapcode, ToolDefinition, ParamDef


# --- Define your tools ---

MOCK_WEATHER = {
    "London": {"condition": "Overcast", "temp": 12, "humidity": 80},
    "Tokyo": {"condition": "Clear", "temp": 26, "humidity": 55},
    "Paris": {"condition": "Sunny", "temp": 22, "humidity": 45},
}


def get_weather(args: dict) -> dict:
    city = args["city"]
    return MOCK_WEATHER.get(city, {"condition": "Unknown", "temp": 0})


def search_flights(args: dict) -> list[dict]:
    return [
        {"airline": "BA", "flight": "BA123", "price": 450, "departure": "08:00"},
        {"airline": "AF", "flight": "AF456", "price": 380, "departure": "14:30"},
    ]


# --- One call to set up everything ---

b = zapcode(
    system="You are a helpful travel assistant.",
    tools={
        "getWeather": ToolDefinition(
            description="Get current weather for a city",
            parameters={"city": ParamDef(type="string", description="City name")},
            execute=get_weather,
        ),
        "searchFlights": ToolDefinition(
            description="Search flights between two cities",
            parameters={
                "from": ParamDef(type="string", description="Departure city"),
                "to": ParamDef(type="string", description="Arrival city"),
                "date": ParamDef(type="string", description="Date (YYYY-MM-DD)"),
            },
            execute=search_flights,
        ),
    },
)


# --- Use with Anthropic SDK ---

def ask(question: str):
    client = anthropic.Anthropic()

    response = client.messages.create(
        model="claude-sonnet-4-20250514",
        max_tokens=1024,
        system=b.system,
        tools=b.anthropic_tools,
        messages=[{"role": "user", "content": question}],
    )

    for block in response.content:
        if block.type == "text":
            print(f"Claude: {block.text}")
        elif block.type == "tool_use":
            print(f"  [executing code...]")
            result = b.handle_tool_call(block.input["code"])
            print(f"  Output: {result.output}")
            if result.tool_calls:
                for tc in result.tool_calls:
                    print(f"  Tool call: {tc['name']}({tc['args']}) -> {tc['result']}")


def main():
    print("=== Simple weather query ===\n")
    ask("What's the weather in Tokyo?")

    print("\n=== Multi-tool query ===\n")
    ask(
        "Compare the weather in London, Tokyo, and Paris. "
        "Find flights from the coldest to the warmest city for 2026-04-15."
    )


if __name__ == "__main__":
    main()
