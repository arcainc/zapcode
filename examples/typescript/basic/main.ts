/**
 * Basic Zapcode example — execute TypeScript from Node.js.
 *
 * Prerequisites: npm install
 * Run with: npx tsx main.ts
 */

import { Zapcode, ZapcodeSnapshotHandle } from "@unchartedfr/zapcode";

// --- 1. Simple expression ---
const simple = new Zapcode("1 + 2 * 3");
const result = simple.run();
console.log("1 + 2 * 3 =", result.output); // 7

// --- 2. Using inputs ---
const greeter = new Zapcode(
  `
    const greeting = \`Hello, \${name}! You are \${age} years old.\`;
    greeting
  `,
  { inputs: ["name", "age"] }
);
const greetResult = greeter.run({ name: "Zapcode", age: 30 });
console.log(greetResult.output); // "Hello, Zapcode! You are 30 years old."

// --- 3. Array/object manipulation ---
const dataProcessor = new Zapcode(`
    const items = [
        { name: "Widget", price: 25.99, qty: 3 },
        { name: "Gadget", price: 49.99, qty: 1 },
        { name: "Doohickey", price: 9.99, qty: 10 },
    ];
    const total = items.reduce((sum, item) => sum + item.price * item.qty, 0);
    const expensive = items.filter(item => item.price > 20);
    ({ total, expensive: expensive.map(i => i.name) })
`);
const dataResult = dataProcessor.run();
console.log(dataResult.output);
// { total: 227.86, expensive: ["Widget", "Gadget"] }

// --- 4. External function (snapshot/resume) ---
const weatherApp = new Zapcode(
  `
    const weather = await getWeather(city);
    const summary = \`Weather in \${city}: \${weather.condition}, \${weather.temp}°C\`;
    summary
  `,
  {
    inputs: ["city"],
    externalFunctions: ["getWeather"],
    timeLimitMs: 5000,
  }
);

const state = weatherApp.start({ city: "London" });

if (!state.completed) {
  console.log(`Suspended on: ${state.functionName}(${state.args})`);
  // "Suspended on: getWeather(London)"

  // In a real app, you'd call an actual weather API here
  const mockWeather = { condition: "Partly cloudy", temp: 18 };

  // Resume with the result
  const snapshot = ZapcodeSnapshotHandle.load(state.snapshot);
  const final_ = snapshot.resume(mockWeather);
  console.log(final_.output);
  // "Weather in London: Partly cloudy, 18°C"
}

// --- 5. Resource limits ---
try {
  const dangerous = new Zapcode("while (true) {}", {
    timeLimitMs: 100,
  });
  dangerous.run();
} catch (e) {
  console.log("Caught:", (e as Error).message);
  // "Caught: allocation limit exceeded" or similar
}

// --- 6. Classes and generators ---
const classExample = new Zapcode(`
    class Counter {
        count: number;
        constructor(start: number) {
            this.count = start;
        }
        increment() {
            this.count += 1;
            return this.count;
        }
    }
    const c = new Counter(10);
    [c.increment(), c.increment(), c.increment()]
`);
console.log(classExample.run().output); // [11, 12, 13]

// --- 7. Async map with multiple external calls ---
// arr.map(async fn => await external()) now works —
// each external call suspends/resumes sequentially.
const asyncMapExample = new Zapcode(
  `
    const cities = ["London", "Tokyo", "Paris"];
    const results = cities.map(async (city) => {
        const weather = await getWeather(city);
        return weather;
    });
    results
  `,
  { externalFunctions: ["getWeather"] }
);

const mockWeatherData: Record<string, unknown> = {
  London: { condition: "Rainy", temp: 12 },
  Tokyo: { condition: "Clear", temp: 26 },
  Paris: { condition: "Sunny", temp: 22 },
};

let mapState = asyncMapExample.start();
while (!mapState.completed) {
  const city = mapState.args![0] as string;
  console.log(`  -> getWeather(${city})`);
  const snap = ZapcodeSnapshotHandle.load(mapState.snapshot!);
  mapState = snap.resume(mockWeatherData[city]);
}
console.log("Async map result:", mapState.output);
// [{condition: "Rainy", temp: 12}, {condition: "Clear", temp: 26}, ...]
