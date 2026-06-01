// EXPLORATORY stress-pass catalog (not part of the green test:e2e gate; run via `npm run test:scenarios`).
// Checks named BUG/MISSING document gaps found during the realistic-scenario pass; see ../../KNOWN_GAPS.md.
// Some were fixed after this file was written, so those checks now intentionally show as failing-to-flag-fixed.
/**
 * scenarios-text.mjs — String/text/regex/validation stress tests for Zapcode.
 *
 * BUG-labelled checks confirm interpreter defects and are expected to pass
 * (they assert the buggy actual behaviour so the suite stays green and the
 * bugs remain documented in-place).  Workaround-labelled checks show the
 * safe alternatives for real agent code.
 *
 * "Cannot convert undefined or null to object" from assert.equal/deepEqual
 * happens when output is null — tests that receive null must use
 * assert.strictEqual(output, null, ...) not assert.equal(output, expected).
 */
import assert from "node:assert/strict";
import { execute } from "../dist/index.js";

const results = [];
async function check(name, fn) {
  try {
    await fn();
    results.push([name, true]);
    console.log("  ✓ " + name);
  } catch (e) {
    results.push([name, false, e.message]);
    console.log("  ✗ " + name + " — " + e.message.split("\n")[0]);
  }
}

// ---------------------------------------------------------------------------
// 1. Slugify — replaceAll(string) works; regex .replace is broken.
// ---------------------------------------------------------------------------
await check("slugify: toLowerCase + replaceAll (string arg workaround)", async () => {
  const { output } = await execute(
    `"Hello World This is a Test 2025".toLowerCase().replaceAll(" ", "-")`,
    {}
  );
  assert.equal(output, "hello-world-this-is-a-test-2025");
});

await check("BUG: str.replace(/re/g, x) returns ORIGINAL string unchanged", async () => {
  const { output } = await execute(
    `"Hello, World! 2025".toLowerCase().replace(/[^a-z0-9]+/g, "-")`,
    {}
  );
  // Expected "hello-world-2025" — regex replace is a no-op
  assert.equal(output, "hello, world! 2025",
    "BUG confirmed: str.replace(regex, ...) returns original string");
});

await check("BUG: str.replace(/re/, x) non-global also returns original string", async () => {
  const { output } = await execute(`"abc".replace(/a/, "X")`, {});
  assert.equal(output, "abc",
    "BUG confirmed: regex-based replace (with or without /g) is a no-op");
});

// ---------------------------------------------------------------------------
// 2. Truncate with ellipsis — .slice and .length work.
// ---------------------------------------------------------------------------
await check("truncate: slice + length", async () => {
  const { output } = await execute(`
    const text = "The quick brown fox jumped over the lazy dog";
    const maxLen = 20;
    text.length > maxLen ? text.slice(0, maxLen - 3) + "..." : text
  `, {});
  assert.equal(output.length, 20);
  assert.ok(String(output).endsWith("..."));
});

await check("truncate: short string unchanged", async () => {
  const { output } = await execute(`
    const text = "Short";
    const maxLen = 20;
    text.length > maxLen ? text.slice(0, maxLen - 3) + "..." : text
  `, {});
  assert.equal(output, "Short");
});

await check("BUG: str.slice(0, str.indexOf(x)) throws __string__.slice", async () => {
  // str.method(str.method()) — using a method-call result as argument throws.
  // Workaround: store indexOf result in a variable first.
  let threw = false;
  try {
    await execute(`const url = "https://x.com"; url.slice(0, url.indexOf(":"))`, {});
  } catch (e) {
    threw = true;
    assert.match(e.message, /__string__\.slice is not a function/);
  }
  assert.ok(threw, "BUG confirmed: str.method(str.method()) throws");
});

// ---------------------------------------------------------------------------
// 3. Template literals — work correctly.
// ---------------------------------------------------------------------------
await check("template literal: interpolation from tool result", async () => {
  const { output } = await execute(`
    const user = await getUser({ id: "u1" });
    \`Hi \${user.name}, you have \${user.count} unread messages.\`
  `, {
    getUser: {
      description: "Get user info",
      parameters: { id: { type: "string" } },
      execute: async () => ({ name: "Alice", count: 5 }),
    },
  });
  assert.equal(output, "Hi Alice, you have 5 unread messages.");
});

await check("template literal: nested expressions and .join", async () => {
  const { output } = await execute(`
    const items = ["apple", "banana", "cherry"];
    \`Items (\${items.length}): \${items.join(", ")}\`
  `, {});
  assert.equal(output, "Items (3): apple, banana, cherry");
});

// ---------------------------------------------------------------------------
// 4. CSV parsing — for-loop works; forEach closures are broken.
// ---------------------------------------------------------------------------
await check("csv parse: for-loop (workaround forEach closure bug)", async () => {
  const { output } = await execute(`
    const csv = "name,age,city\\nAlice,30,NYC\\nBob,25,LA\\nCarol,35,Chicago";
    const lines = csv.split("\\n");
    const headers = lines[0].split(",");
    const rows = [];
    for (let i = 1; i < lines.length; i++) {
      const vals = lines[i].split(",");
      const obj = {};
      for (let j = 0; j < headers.length; j++) { obj[headers[j]] = vals[j]; }
      rows.push(obj);
    }
    rows
  `, {});
  assert.deepEqual(output, [
    { name: "Alice", age: "30", city: "NYC" },
    { name: "Bob",   age: "25", city: "LA"  },
    { name: "Carol", age: "35", city: "Chicago" },
  ]);
});

await check("BUG: forEach arrow fn cannot mutate outer object", async () => {
  const { output } = await execute(`
    const obj = {};
    ["a","b","c"].forEach((k,i) => { obj[k] = i; });
    obj
  `, {});
  assert.deepEqual(output, {},
    "BUG confirmed: forEach arrow fn mutations to outer-scope variables are lost");
});

await check("BUG: forEach cannot push to outer array", async () => {
  const { output } = await execute(`
    const arr = [];
    [1,2,3].forEach(x => arr.push(x));
    arr
  `, {});
  assert.deepEqual(output, [],
    "BUG confirmed: forEach push to outer array has no effect");
});

await check("BUG: for-of over object array — push to outer array gets no items", async () => {
  const { output } = await execute(`
    const out = [];
    for (const r of [{n:"A"},{n:"B"},{n:"C"}]) { out.push(r.n); }
    out
  `, {});
  assert.deepEqual(output, [],
    "BUG confirmed: for-of over object array produces no outer mutations");
});

await check("for-of over primitive array push works", async () => {
  const { output } = await execute(`
    const out = [];
    for (const s of ["a","b","c"]) { out.push(s + "!"); }
    out
  `, {});
  assert.deepEqual(output, ["a!", "b!", "c!"]);
});

await check("BUG: arr.push(method_call_result) throws __array__.push", async () => {
  let threw = false;
  try {
    await execute(`const a = []; a.push([1,2].join(",")); a`, {});
  } catch (e) {
    threw = true;
    assert.match(e.message, /__array__\.push is not a function/);
  }
  assert.ok(threw, "BUG confirmed: push(method_call_result) throws");
});

await check("csv build summary via reduce (workaround Set + forEach)", async () => {
  const { output } = await execute(`
    const csv = "name,age,city\\nAlice,30,NYC\\nBob,25,LA\\nCarol,35,NYC";
    const lines = csv.split("\\n");
    const headers = lines[0].split(",");
    const rows = [];
    for (let i = 1; i < lines.length; i++) {
      const vals = lines[i].split(",");
      const obj = {};
      for (let j = 0; j < headers.length; j++) { obj[headers[j]] = vals[j]; }
      rows.push(obj);
    }
    const cityMap = rows.reduce((acc, r) => { acc[r.city] = true; return acc; }, {});
    const cities = Object.keys(cityMap).sort();
    ({ totalRows: rows.length, uniqueCities: cities })
  `, {});
  assert.equal(output.totalRows, 3);
  assert.deepEqual(output.uniqueCities, ["LA", "NYC"]);
});

// ---------------------------------------------------------------------------
// 5. Validation — Array.includes works; regex .test broken.
// ---------------------------------------------------------------------------
await check("validation: valid input — indexOf-based email check", async () => {
  const { output } = await execute(`
    function validate(input) {
      const errors = [];
      const hasAt = input.email.indexOf("@") > 0;
      const hasDot = input.email.lastIndexOf(".") > input.email.indexOf("@");
      if (!hasAt || !hasDot) errors.push("invalid email");
      if (!input.name || input.name.trim().length === 0) errors.push("name required");
      if (input.name && input.name.length > 50) errors.push("name too long");
      const roles = ["admin", "editor", "viewer"];
      if (!roles.includes(input.role)) errors.push("invalid role");
      return { ok: errors.length === 0, errors };
    }
    validate({ email: "alice@example.com", name: "Alice", role: "editor" })
  `, {});
  assert.equal(output.ok, true);
  assert.deepEqual(output.errors, []);
});

await check("validation: multiple errors flagged correctly", async () => {
  const { output } = await execute(`
    function validate(input) {
      const errors = [];
      const hasAt = input.email.indexOf("@") > 0;
      const hasDot = input.email.lastIndexOf(".") > input.email.indexOf("@");
      if (!hasAt || !hasDot) errors.push("invalid email");
      if (!input.name || input.name.trim().length === 0) errors.push("name required");
      const roles = ["admin", "editor", "viewer"];
      if (!roles.includes(input.role)) errors.push("invalid role");
      return { ok: errors.length === 0, errors };
    }
    validate({ email: "not-an-email", name: "   ", role: "superuser" })
  `, {});
  assert.equal(output.ok, false);
  assert.ok(output.errors.includes("invalid email"));
  assert.ok(output.errors.includes("name required"));
  assert.ok(output.errors.includes("invalid role"));
});

await check("BUG: regex .test() throws on both stored and inline regex", async () => {
  let threw1 = false, threw2 = false;
  try { await execute(`/abc/.test("xabcx")`, {}); } catch { threw1 = true; }
  try { await execute(`const re=/abc/; re.test("xabcx")`, {}); } catch { threw2 = true; }
  assert.ok(threw1 && threw2, "BUG confirmed: .test() not available on RegExp");
});

// ---------------------------------------------------------------------------
// 6. Regex — comprehensive BUG documentation
// ---------------------------------------------------------------------------
await check("regex: str.match(/literal/) works (no metacharacters)", async () => {
  const { output } = await execute(`"xabcx".match(/abc/)`, {});
  assert.deepEqual(output, ["abc"]);
});

await check("BUG: str.match(/[a-z]/) char class returns null", async () => {
  const { output } = await execute(`"hello".match(/[a-z]/)`, {});
  assert.strictEqual(output, null,
    "BUG confirmed: char-class regex match returns null");
});

await check("BUG: str.match(/a+/) quantifier returns null", async () => {
  const { output } = await execute(`"aaa".match(/a+/)`, {});
  assert.strictEqual(output, null,
    "BUG confirmed: quantifier in regex match returns null");
});

await check("BUG: str.match(/\\d+/) escape-sequence returns null", async () => {
  const { output } = await execute(`"abc123".match(/\\d+/)`, {});
  assert.strictEqual(output, null,
    "BUG confirmed: \\d/\\w/\\s escape sequences in regex are broken");
});

await check("BUG: str.match(/re/g) returns only first match (not all)", async () => {
  const { output } = await execute(`"cat bat sat".match(/at/g)`, {});
  // Expected ["at","at","at"] — /g flag ignored, returns same as non-global
  assert.deepEqual(output, ["at"],
    "BUG confirmed: /g flag ignored, match returns only first occurrence");
});

await check("BUG: str.match(/(group)/) capture groups return null", async () => {
  const { output } = await execute(
    `"2025-12-31".match(/(\\d{4})-(\\d{2})-(\\d{2})/)`,
    {}
  );
  assert.strictEqual(output, null,
    "BUG confirmed: capture groups in match return null");
});

await check("BUG: named capture groups return null", async () => {
  const { output } = await execute(
    `"2025-12-31".match(/(?<year>\\d{4})-(?<month>\\d{2})-(?<day>\\d{2})/)`,
    {}
  );
  assert.strictEqual(output, null,
    "BUG confirmed: named capture groups return null");
});

await check("BUG: lookbehind (?<=) returns null", async () => {
  const { output } = await execute(`"$5 $10".match(/(?<=\\$)\\d+/g)`, {});
  assert.strictEqual(output, null, "BUG confirmed: lookbehind returns null");
});

await check("BUG: lookahead (?=) in replace is a no-op", async () => {
  const { output } = await execute(`"foo123".replace(/(?=\\d)/, " ")`, {});
  assert.equal(output, "foo123", "BUG confirmed: lookahead replace does nothing");
});

await check("BUG: str.split(/regex/) returns [originalString]", async () => {
  const { output } = await execute(`"a  b  c".split(/\\s+/)`, {});
  assert.deepEqual(output, ["a  b  c"],
    "BUG confirmed: split with regex arg does not split");
});

await check("str.split(string) works correctly", async () => {
  const { output } = await execute(`"a,b,c".split(",")`, {});
  assert.deepEqual(output, ["a", "b", "c"]);
});

// ---------------------------------------------------------------------------
// 7. replaceAll (string arg) — WORKS
// ---------------------------------------------------------------------------
await check("replaceAll: string arg replaces all occurrences", async () => {
  const { output } = await execute(
    `"foo bar foo baz foo".replaceAll("foo", "qux")`,
    {}
  );
  assert.equal(output, "qux bar qux baz qux");
});

// ---------------------------------------------------------------------------
// 8. Markdown table — nested map bug; use direct property access workaround
// ---------------------------------------------------------------------------
await check("markdown table: direct property access in map (workaround)", async () => {
  const { output } = await execute(`
    const rows = [
      { name: "Alice", score: 95, grade: "A" },
      { name: "Bob",   score: 82, grade: "B" },
      { name: "Carol", score: 78, grade: "C" },
    ];
    const head = "| name | score | grade |";
    const sep  = "| --- | --- | --- |";
    const body = rows.map(r => "| " + r.name + " | " + r.score + " | " + r.grade + " |").join("\\n");
    head + "\\n" + sep + "\\n" + body
  `, {});
  assert.equal(
    output,
    "| name | score | grade |\n| --- | --- | --- |\n| Alice | 95 | A |\n| Bob | 82 | B |\n| Carol | 78 | C |"
  );
});

await check("BUG: rows.map(r => keys.map(k => r[k])) — inner r is always first row", async () => {
  const { output } = await execute(`
    const rows = [{a:1,b:2},{a:3,b:4},{a:5,b:6}];
    const keys = ["a","b"];
    rows.map(r => keys.map(k => r[k]))
  `, {});
  // BUG: expected [[1,2],[3,4],[5,6]] but inner map always sees rows[0]
  assert.deepEqual(output, [[1,2],[1,2],[1,2]],
    "BUG confirmed: nested map outer variable captured as first element");
});

// ---------------------------------------------------------------------------
// 9. Markdown bulleted list — single-level map works
// ---------------------------------------------------------------------------
await check("markdown bullets: map with boolean toggle", async () => {
  const { output } = await execute(`
    const items = [
      { label: "Deploy to staging", done: true },
      { label: "Run tests", done: false },
      { label: "Update docs", done: true },
    ];
    items.map(i => (i.done ? "- [x] " : "- [ ] ") + i.label).join("\\n")
  `, {});
  assert.equal(
    output,
    "- [x] Deploy to staging\n- [ ] Run tests\n- [x] Update docs"
  );
});

// ---------------------------------------------------------------------------
// 10. Whitespace normalization
// ---------------------------------------------------------------------------
await check("BUG: replace(/\\s+/g) returns trimmed-but-uncollapsed string", async () => {
  const { output } = await execute(`"  Hello   World  ".trim().replace(/\\s+/g, " ")`, {});
  assert.equal(output, "Hello   World",
    "BUG confirmed: regex replace on whitespace returns original (minus leading/trailing trim)");
});

await check("normalize: split(' ').filter().join() workaround", async () => {
  const { output } = await execute(`
    "  Hello   World   from   JS  ".trim().split(" ").filter(w => w.length > 0).join(" ")
  `, {});
  assert.equal(output, "Hello World from JS");
});

await check("case-fold: toUpperCase / toLowerCase", async () => {
  const { output } = await execute(
    `["Hello World".toUpperCase(), "Hello World".toLowerCase()]`,
    {}
  );
  assert.deepEqual(output, ["HELLO WORLD", "hello world"]);
});

// ---------------------------------------------------------------------------
// 11. padStart / padEnd
// ---------------------------------------------------------------------------
await check("padStart: zero-pad numbers", async () => {
  const { output } = await execute(
    `[1,10,100].map(n => String(n).padStart(4,"0"))`,
    {}
  );
  assert.deepEqual(output, ["0001","0010","0100"]);
});

await check("padEnd: right-pad strings", async () => {
  const { output } = await execute(
    `["Name","Score","Grade"].map(h => h.padEnd(8," "))`,
    {}
  );
  assert.deepEqual(output, ["Name    ","Score   ","Grade   "]);
});

// ---------------------------------------------------------------------------
// 12. repeat
// ---------------------------------------------------------------------------
await check("repeat: separator line", async () => {
  const { output } = await execute(`"-".repeat(20)`, {});
  assert.equal(output, "--------------------");
});

// ---------------------------------------------------------------------------
// 13. Word frequency — reduce (works) instead of forEach (broken)
// ---------------------------------------------------------------------------
await check("word frequency: reduce builds correct map", async () => {
  const { output } = await execute(`
    "the cat sat on the mat the cat"
      .split(" ")
      .reduce((acc, w) => { acc[w] = (acc[w] || 0) + 1; return acc; }, {})
  `, {});
  assert.equal(output.the, 3);
  assert.equal(output.cat, 2);
  assert.equal(output.sat, 1);
});

// ---------------------------------------------------------------------------
// 14. Redact / mask token
// ---------------------------------------------------------------------------
await check("redact: mask all but last 4 chars (repeat + slice)", async () => {
  const { output } = await execute(`
    const apiKey = "sk-abcdef1234567890WXYZ";
    "*".repeat(apiKey.length - 4) + apiKey.slice(-4)
  `, {});
  // "sk-abcdef1234567890WXYZ" = 23 chars; 19 stars + "WXYZ"
  assert.equal(output, "*******************WXYZ");
  assert.equal(String(output).length, 23);
});

// ---------------------------------------------------------------------------
// 15. JSON.stringify / JSON.parse
// ---------------------------------------------------------------------------
await check("BUG: JSON.stringify(obj, null, 2) — indent arg silently ignored", async () => {
  const { output } = await execute(
    `const r = JSON.stringify({a:1,b:[2,3]},null,2); [r.includes("\\n"),r.includes("  ")]`,
    {}
  );
  assert.deepEqual(output, [false, false],
    "BUG confirmed: JSON.stringify indent parameter silently ignored");
});

await check("JSON.parse + JSON.stringify roundtrip", async () => {
  const { output } = await execute(
    `JSON.stringify(JSON.parse('{"a":1,"b":[2,3],"c":{"d":true}}'))`,
    {}
  );
  assert.equal(output, '{"a":1,"b":[2,3],"c":{"d":true}}');
});

// ---------------------------------------------------------------------------
// 16. Pluralize and number formatting
// ---------------------------------------------------------------------------
await check("pluralize: singular and plural forms", async () => {
  const { output } = await execute(`
    function p(n,s,pl) { return n+" "+(n===1?s:pl); }
    [p(1,"item","items"),p(3,"item","items"),p(0,"item","items")]
  `, {});
  assert.deepEqual(output, ["1 item","3 items","0 items"]);
});

await check("BUG: Number.prototype.toFixed throws 'undefined is not a function'", async () => {
  let threw = false;
  try { await execute(`(3.14159).toFixed(2)`, {}); } catch { threw = true; }
  assert.ok(threw, "BUG confirmed: toFixed not available");
});

await check("BUG: comma-formatting regex \\B+lookahead returns original string", async () => {
  const { output } = await execute(
    `String(1234567).replace(/\\B(?=(\\d{3})+(?!\\d))/g, ",")`,
    {}
  );
  assert.equal(output, "1234567",
    "BUG confirmed: complex regex with lookahead+quantifier is a no-op");
});

await check("Math.round workaround for .toFixed(1)", async () => {
  const { output } = await execute(`Math.round(0.023 * 1000) / 10`, {});
  assert.equal(output, 2.3);
});

// ---------------------------------------------------------------------------
// 17. startsWith / endsWith / includes / indexOf
// ---------------------------------------------------------------------------
await check("startsWith / endsWith / includes / indexOf", async () => {
  const { output } = await execute(`
    const s = "https://example.com/path?q=1";
    ({
      startsHttps: s.startsWith("https://"),
      endsQuery:   s.endsWith("?q=1"),
      hasExample:  s.includes("example"),
      dotAt:       s.indexOf(".com"),
    })
  `, {});
  assert.equal(output.startsHttps, true);
  assert.equal(output.endsQuery, true);
  assert.equal(output.hasExample, true);
  assert.equal(output.dotAt, 15);
});

// ---------------------------------------------------------------------------
// 18. slice / substring — store index in variable first
// ---------------------------------------------------------------------------
await check("slice: works with literal args and stored index vars", async () => {
  const { output } = await execute(`
    const url = "https://example.com/users/42";
    const colonIdx = url.indexOf("://");
    const slashIdx = url.lastIndexOf("/");
    [url.slice(0, colonIdx), url.slice(slashIdx + 1)]
  `, {});
  assert.deepEqual(output, ["https", "42"]);
});

await check("BUG: substring(indexOf()) throws — needs stored var", async () => {
  let threw = false;
  try {
    await execute(`
      const url = "https://example.com/path";
      url.substring(url.indexOf("//")+2, url.indexOf("/",url.indexOf("//")+2))
    `, {});
  } catch (e) {
    threw = true;
  }
  assert.ok(threw, "BUG confirmed: str.method(str.method()) throws for substring too");
});

await check("BUG: indexOf(str, startPos) — startPos arg silently ignored", async () => {
  const { output } = await execute(
    `["abcabc".indexOf("a",1), "abcabc".indexOf("a",3), "abcabc".indexOf("a",4)]`,
    {}
  );
  // Standard JS: [3, 3, -1] — but startPos ignored, always returns first match
  assert.deepEqual(output, [0, 0, 0],
    "BUG confirmed: indexOf startPos argument ignored, always returns first occurrence");
});

// ---------------------------------------------------------------------------
// 19. split / join round-trip
// ---------------------------------------------------------------------------
await check("split/join round-trip with string separator", async () => {
  const { output } = await execute(
    `"javascript,typescript,node,react".split(",").join(" | ")`,
    {}
  );
  assert.equal(output, "javascript | typescript | node | react");
});

// ---------------------------------------------------------------------------
// 20. String spread and reverse
// ---------------------------------------------------------------------------
await check("spread string into chars and reverse", async () => {
  const { output } = await execute(`[..."Hello"].reverse().join("")`, {});
  assert.equal(output, "olleH");
});

// ---------------------------------------------------------------------------
// 21. Number / String coercions
// ---------------------------------------------------------------------------
await check("Number() / String() coercions", async () => {
  const { output } = await execute(`
    const n = Number("  42.5  ".trim());
    [n, String(n) + " units"]
  `, {});
  assert.deepEqual(output, [42.5, "42.5 units"]);
});

await check("BUG: Number('abc') returns null instead of NaN", async () => {
  const { output } = await execute(`Number("abc")`, {});
  // Standard JS: Number("abc") === NaN; sandbox returns null
  assert.strictEqual(output, null,
    "BUG confirmed: Number of non-numeric string returns null not NaN");
});

// ---------------------------------------------------------------------------
// 22. String.fromCharCode — BUG
// ---------------------------------------------------------------------------
await check("BUG: String.fromCharCode throws despite typeof === 'function'", async () => {
  let threw = false;
  try { await execute(`String.fromCharCode(65)`, {}); } catch { threw = true; }
  assert.ok(threw, "BUG confirmed: String.fromCharCode not callable");
});

await check("charCodeAt works correctly", async () => {
  const { output } = await execute(`["A","B","Z"].map(c => c.charCodeAt(0))`, {});
  assert.deepEqual(output, [65, 66, 90]);
});

// ---------------------------------------------------------------------------
// 23. Set — BUG (missing)
// ---------------------------------------------------------------------------
await check("BUG: new Set() throws 'undefined is not a constructor'", async () => {
  let threw = false;
  try { await execute(`[...new Set([1,2,1,3])]`, {}); } catch { threw = true; }
  assert.ok(threw, "BUG confirmed: Set constructor not available");
});

// ---------------------------------------------------------------------------
// 24. Map — BUG (.get() returns null)
// ---------------------------------------------------------------------------
await check("BUG: new Map() .get() returns null for present key", async () => {
  const { output } = await execute(`new Map([["a",1],["b",2]]).get("a")`, {});
  assert.strictEqual(output, null,
    "BUG confirmed: Map.get() returns null for present keys");
});

// ---------------------------------------------------------------------------
// 25. localeCompare — BUG (missing)
// ---------------------------------------------------------------------------
await check("BUG: String.prototype.localeCompare throws", async () => {
  let threw = false;
  try { await execute(`"apple".localeCompare("banana")`, {}); } catch { threw = true; }
  assert.ok(threw, "BUG confirmed: localeCompare not available");
});

// ---------------------------------------------------------------------------
// 26. matchAll — BUG (missing)
// ---------------------------------------------------------------------------
await check("BUG: String.prototype.matchAll throws", async () => {
  let threw = false;
  try { await execute(`[..."a1b2".matchAll(/[a-z](\\d)/g)]`, {}); } catch { threw = true; }
  assert.ok(threw, "BUG confirmed: matchAll not available");
});

// ---------------------------------------------------------------------------
// 27. parseInt / parseFloat / isNaN — BUG (missing globals)
// ---------------------------------------------------------------------------
await check("BUG: parseInt not available as global function", async () => {
  let threw = false;
  try { await execute(`parseInt("42px")`, {}); } catch { threw = true; }
  assert.ok(threw, "BUG confirmed: parseInt not available");
});

await check("BUG: parseFloat not available as global function", async () => {
  let threw = false;
  try { await execute(`parseFloat("3.14")`, {}); } catch { threw = true; }
  assert.ok(threw, "BUG confirmed: parseFloat not available");
});

await check("BUG: isNaN not available as global function", async () => {
  let threw = false;
  try { await execute(`isNaN(NaN)`, {}); } catch { threw = true; }
  assert.ok(threw, "BUG confirmed: isNaN not available");
});

await check("Number() is a working alternative for string-to-number", async () => {
  const { output } = await execute(`[Number("42"), Number("3.14")]`, {});
  assert.deepEqual(output, [42, 3.14]);
});

// ---------------------------------------------------------------------------
// 28. reduce — reliable (use instead of forEach)
// ---------------------------------------------------------------------------
await check("reduce: sum", async () => {
  const { output } = await execute(`[1,2,3,4,5].reduce((a,v) => a+v, 0)`, {});
  assert.equal(output, 15);
});

await check("reduce: build object (safe forEach replacement)", async () => {
  const { output } = await execute(
    `["a","b","c"].reduce((o,k,i) => { o[k]=i; return o; }, {})`,
    {}
  );
  assert.deepEqual(output, {a:0,b:1,c:2});
});

// ---------------------------------------------------------------------------
// 29. String.prototype.at — works
// ---------------------------------------------------------------------------
await check("string.at(0) and .at(-1)", async () => {
  const { output } = await execute(`["hello".at(0),"hello".at(-1)]`, {});
  assert.deepEqual(output, ["h","o"]);
});

// ---------------------------------------------------------------------------
// 30. Object.keys/values/entries, Array.from/isArray, Math
// ---------------------------------------------------------------------------
await check("Object.keys and Object.values work", async () => {
  const { output } = await execute(
    `[Object.keys({a:1,b:2}), Object.values({a:1,b:2})]`,
    {}
  );
  assert.deepEqual(output, [["a","b"],[1,2]]);
});

await check("BUG: Object.entries destructuring in map broken (k is array, v is undefined)", async () => {
  const { output } = await execute(
    `Object.entries({a:1,b:2}).map(([k,v]) => k+":"+v)`,
    {}
  );
  // BUG: expected ["a:1","b:2"] but destructuring in map arg is broken
  assert.deepEqual(output, ["a,1:undefined","b,2:undefined"],
    "BUG confirmed: destructured parameter ([k,v]) in map arrow fn is broken");
});

await check("Array.isArray and Array.from work", async () => {
  const { output } = await execute(`[Array.isArray([1,2,3]), Array.from("hi")]`, {});
  assert.equal(output[0], true);
  assert.deepEqual(output[1], ["h","i"]);
});

await check("Math functions work (floor/ceil/round/abs/pow/sqrt)", async () => {
  const { output } = await execute(
    `[Math.floor(3.9), Math.ceil(3.1), Math.round(3.5), Math.abs(-7), Math.pow(2,8), Math.sqrt(25)]`,
    {}
  );
  assert.deepEqual(output, [3,4,4,7,256,5]);
});

// ---------------------------------------------------------------------------
// 31. Agent report — realistic tool output (note: products need .map workaround)
// ---------------------------------------------------------------------------
await check("report: multi-line output with tool data (map workaround for product list)", async () => {
  const { output } = await execute(`
    const data = await getSummary({ period: "weekly" });
    const churnPct = Math.round(data.churnRate * 1000) / 10;
    const productLines = data.topProducts.map((p, i) => (i + 1) + ". " + p).join("\\n");
    const lines = [
      "## Weekly Summary",
      "",
      "- Total revenue: $" + data.revenue,
      "- New users: " + data.newUsers,
      "- Churn rate: " + churnPct + "%",
      "",
      productLines,
    ];
    lines.join("\\n")
  `, {
    getSummary: {
      description: "Get weekly business summary",
      parameters: { period: { type: "string" } },
      execute: async () => ({
        revenue: 125000,
        newUsers: 342,
        churnRate: 0.023,
        topProducts: ["Pro Plan", "Enterprise Plan", "Add-ons"],
      }),
    },
  });
  assert.ok(String(output).includes("## Weekly Summary"));
  assert.ok(String(output).includes("125000"));
  assert.ok(String(output).includes("342"));
  assert.ok(String(output).includes("2.3%"));
  assert.ok(String(output).includes("1. Pro Plan"));
});

await check("BUG: arr.push(tool_obj[prop]) silently fails — push gets no item", async () => {
  const { output } = await execute(`
    const data = await getData();
    const lines = [];
    lines.push(data.arr[0]);
    lines.push(data.arr[1]);
    lines
  `, {
    getData: {
      description: "get data",
      parameters: {},
      execute: async () => ({ arr: ["x", "y"] }),
    },
  });
  assert.deepEqual(output, [],
    "BUG confirmed: push(tool_obj[index]) silently discards the value");
});

// ---------------------------------------------------------------------------
// Final tally
// ---------------------------------------------------------------------------
const failed = results.filter(r => !r[1]);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
if (failed.length) {
  console.log("\nFailed:");
  failed.forEach(([name, , msg]) => console.log(`  - ${name}: ${msg.split("\n")[0]}`));
  process.exit(1);
}