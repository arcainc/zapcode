"""
Basic Zapcode example — execute TypeScript from Python.

Prerequisites: build zapcode-py (see README)
Run with: python examples/python/basic.py
"""

from zapcode import Zapcode, ZapcodeSnapshot

# --- 1. Simple expression ---
b = Zapcode("1 + 2 * 3")
result = b.run()
print("1 + 2 * 3 =", result["output"])  # 7

# --- 2. Using inputs ---
b = Zapcode(
    """
    const greeting = `Hello, ${name}! You are ${age} years old.`;
    greeting
    """,
    inputs=["name", "age"],
)
result = b.run({"name": "Zapcode", "age": 30})
print(result["output"])  # "Hello, Zapcode! You are 30 years old."

# --- 3. Data processing ---
b = Zapcode("""
    const items = [
        { name: "Widget", price: 25.99, qty: 3 },
        { name: "Gadget", price: 49.99, qty: 1 },
        { name: "Doohickey", price: 9.99, qty: 10 },
    ];
    const total = items.reduce((sum, item) => sum + item.price * item.qty, 0);
    const expensive = items.filter(item => item.price > 20).map(i => i.name);
    { total, expensive }
""")
result = b.run()
print(result["output"])  # {'total': 227.86, 'expensive': ['Widget', 'Gadget']}

# --- 4. External function (snapshot/resume) ---
b = Zapcode(
    """
    const weather = await getWeather(city);
    const summary = `Weather in ${city}: ${weather.condition}, ${weather.temp}°C`;
    summary
    """,
    inputs=["city"],
    external_functions=["getWeather"],
)

state = b.start({"city": "London"})

if state.get("suspended"):
    print(f"Suspended on: {state['function_name']}({state['args']})")

    # In a real app, call an actual weather API
    mock_weather = {"condition": "Partly cloudy", "temp": 18}

    # Resume with the result
    snapshot = state["snapshot"]
    final = snapshot.resume(mock_weather)
    print(final["output"])  # "Weather in London: Partly cloudy, 18°C"

# --- 5. Resource limits ---
try:
    b = Zapcode("while (true) {}", time_limit_ms=100)
    b.run()
except RuntimeError as e:
    print(f"Caught: {e}")  # allocation limit or time limit

# --- 6. Snapshot serialization ---
b = Zapcode(
    "const data = await fetchData(url); data.length",
    inputs=["url"],
    external_functions=["fetchData"],
)

state = b.start({"url": "https://example.com"})
if state.get("suspended"):
    # Serialize to bytes — store in a database, send over a queue, etc.
    snapshot_bytes = state["snapshot"].dump()
    print(f"Snapshot size: {len(snapshot_bytes)} bytes")

    # Later (possibly in a different process): restore and resume
    restored = ZapcodeSnapshot.load(snapshot_bytes)
    final = restored.resume("hello world")
    print(f"Restored result: {final['output']}")  # 11
