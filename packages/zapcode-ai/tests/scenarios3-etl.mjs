/**
 * Realistic ETL/data-cleaning agent workflow scenarios for zapcode-ai.
 *
 * Covers CSV-ish object arrays, dedupe with Map/Set, regex/string cleanup,
 * invalid external tool arguments, object rest/destructuring, and stable
 * outputs across durable session dump/load boundaries.
 *
 * Run: npm run build && node tests/scenarios3-etl.mjs
 */
import assert from "node:assert/strict";
import { createSession, execute, loadSession } from "../dist/index.js";

let passed = 0;
async function test(name, fn) {
  try {
    await fn();
    passed++;
    console.log(`  PASS ${name}`);
  } catch (err) {
    console.error(`  FAIL ${name}`);
    throw err;
  }
}

console.log("scenarios3 ETL e2e");

const RAW_CUSTOMERS = [
  {
    row: 1,
    "Customer ID": " c-001 ",
    "Full Name": " Ada  Lovelace ",
    Email: "ADA@Example.COM ",
    Phone: "(555) 010-0001",
    Tags: " VIP | math |vip ",
    status: " Active ",
    city: "London",
  },
  {
    row: 2,
    "Customer ID": "c-002",
    "Full Name": "Grace\tHopper",
    Email: "grace@example.com",
    Phone: "555.010.0002",
    Tags: "navy|Compiler",
    status: "active",
    city: "Arlington",
  },
  {
    row: 3,
    "Customer ID": " c-001",
    "Full Name": "Ada Byron",
    Email: "ada@example.com",
    Phone: "+1 555 010 0001",
    Tags: "math|priority",
    status: "active",
    city: "London",
  },
  {
    row: 4,
    "Customer ID": "c-003",
    "Full Name": "  Alan   Turing ",
    Email: "ALAN@Example.COM",
    Phone: "5550100003",
    Tags: " crypto | research ",
    status: "inactive",
    city: "Manchester",
  },
  {
    row: 5,
    "Customer ID": "",
    "Full Name": "No Email",
    Email: "not-an-email",
    Phone: "",
    Tags: "",
    status: "active",
    city: "Unknown",
  },
];

function cloneRows(rows = RAW_CUSTOMERS) {
  return JSON.parse(JSON.stringify(rows));
}

function createEtlTools(state, rows = RAW_CUSTOMERS) {
  return {
    fetchRawCustomers: {
      description: "Fetch raw CSV-ish customer rows.",
      parameters: {
        source: { type: "string", optional: true },
      },
      execute: async ({ source }) => {
        state.fetches.push(source ?? "default");
        return cloneRows(rows);
      },
    },
    persistCleanCustomers: {
      description: "Persist cleaned customer rows and a summary.",
      parameters: {
        rows: { type: "array" },
        summary: { type: "object" },
      },
      execute: async input => {
        state.saved.push(input);
        return { ok: true, rowCount: input.rows.length, statuses: input.summary.statuses };
      },
    },
  };
}

const CLEANING_PROGRAM = `
const raw = await fetchRawCustomers({ source: "crm-export.csv" });
const byId = new Map();
const allTags = new Set();
for (const row of raw) {
  const { row: rowNumber, Email, Tags, status, ...rest } = row;
  const id = String(rest["Customer ID"] || "").trim().toLowerCase();
  const email = String(Email || "").trim().toLowerCase();
  if (!id || !/^.+@.+\\..+$/.test(email)) continue;

  const name = String(rest["Full Name"] || "").replace(/\\s+/g, " ").trim();
  const phoneDigits = String(rest.Phone || "").replace(/\\D/g, "");
  const tags = String(Tags || "")
    .toLowerCase()
    .split("|")
    .map(t => t.trim().replace(/[^a-z0-9-]+/g, "-"))
    .filter(t => t.length > 0);
  for (let i = 0; i < tags.length; i++) allTags.add(tags[i]);

  const previous = byId.get(id);
  const cleaned = {
    id,
    email,
    name,
    phoneDigits,
    status: String(status || "").trim().toLowerCase(),
    city: rest.city,
    tags,
    sourceRows: previous ? previous.sourceRows.concat([rowNumber]) : [rowNumber],
  };
  byId.set(id, previous ? { ...cleaned, tags: previous.tags.concat(tags) } : cleaned);
}

const rows = [];
for (const row of byId.values()) {
  const tags = [];
  const seenTags = new Set();
  for (let i = 0; i < row.tags.length; i++) {
    const tag = row.tags[i];
    if (!seenTags.has(tag)) {
      seenTags.add(tag);
      tags.push(tag);
    }
  }
  rows.push({ ...row, tags });
}

rows.sort((a, b) => a.id.localeCompare(b.id));
const statuses = {};
for (const row of rows) statuses[row.status] = (statuses[row.status] || 0) + 1;

const uniqueTags = [];
for (const tag of allTags) uniqueTags.push(tag);

const summary = {
  imported: rows.length,
  skipped: raw.length - rows.length,
  statuses,
  uniqueTags,
};

await persistCleanCustomers({ rows, summary });
({ rows, summary })
`;

await test("cleans CSV-ish rows, dedupes with Map/Set, and persists deterministic output", async () => {
  const state = { fetches: [], saved: [] };
  const result = await execute(CLEANING_PROGRAM, createEtlTools(state));

  assert.deepEqual(state.fetches, ["crm-export.csv"]);
  assert.equal(state.saved.length, 1);
  assert.deepEqual(result.output, {
    rows: [
      {
        id: "c-001",
        email: "ada@example.com",
        name: "Ada Byron",
        phoneDigits: "15550100001",
        status: "active",
        city: "London",
        tags: ["vip", "math", "priority"],
        sourceRows: [1, 3],
      },
      {
        id: "c-002",
        email: "grace@example.com",
        name: "Grace Hopper",
        phoneDigits: "5550100002",
        status: "active",
        city: "Arlington",
        tags: ["navy", "compiler"],
        sourceRows: [2],
      },
      {
        id: "c-003",
        email: "alan@example.com",
        name: "Alan Turing",
        phoneDigits: "5550100003",
        status: "inactive",
        city: "Manchester",
        tags: ["crypto", "research"],
        sourceRows: [4],
      },
    ],
    summary: {
      imported: 3,
      skipped: 2,
      statuses: { active: 2, inactive: 1 },
      uniqueTags: ["vip", "math", "navy", "compiler", "priority", "crypto", "research"],
    },
  });
  assert.deepEqual(state.saved[0], result.toolCalls.at(-1).input);
});

await test("rejects invalid external tool arguments before host persistence runs", async () => {
  const state = { fetches: [], saved: [] };
  const tools = createEtlTools(state);

  await assert.rejects(
    () => execute(`await persistCleanCustomers({ rows: {}, summary: { imported: 0 } })`, tools),
    /Invalid arguments for tool 'persistCleanCustomers': parameter 'rows' expected array, got object/
  );
  await assert.rejects(
    () => execute(`await persistCleanCustomers({ rows: [], summary: [] })`, tools),
    /Invalid arguments for tool 'persistCleanCustomers': parameter 'summary' expected object, got array/
  );
  await assert.rejects(
    () => execute(`await persistCleanCustomers({ rows: [], summary: {}, extra: true })`, tools),
    /Invalid arguments for tool 'persistCleanCustomers': unexpected parameter 'extra'/
  );
  await assert.rejects(
    () => execute(`await persistCleanCustomers([], {})`, tools),
    /Invalid arguments for tool 'persistCleanCustomers': expected one named object argument/
  );

  assert.deepEqual(state.saved, []);
});

await test("durable session preserves ETL helpers and deterministic output after dump/load", async () => {
  const state = { fetches: [], saved: [] };
  let session = createSession({ tools: createEtlTools(state) });

  const setup = await session.runChunk(`
    function normalizeEmail(value) {
      return String(value || "").trim().toLowerCase();
    }
    function cleanCustomerRows(rawRows) {
      const byId = new Map();
      for (const row of rawRows) {
        const { row: rowNumber, Email, Tags, status, ...rest } = row;
        const id = String(rest["Customer ID"] || "").trim().toLowerCase();
        const email = normalizeEmail(Email);
        if (!id || !/^.+@.+\\..+$/.test(email)) continue;
        const tags = String(Tags || "")
          .toLowerCase()
          .split("|")
          .map(t => t.trim())
          .filter(t => t.length > 0);
        const previous = byId.get(id);
        byId.set(id, {
          id,
          email,
          name: String(rest["Full Name"] || "").replace(/\\s+/g, " ").trim(),
          status: String(status || "").trim().toLowerCase(),
          tags: previous ? previous.tags.concat(tags) : tags,
          sourceRows: previous ? previous.sourceRows.concat([rowNumber]) : [rowNumber],
        });
      }
      const rows = [];
      for (const row of byId.values()) {
        const tags = [];
        const seen = new Set();
        for (let i = 0; i < row.tags.length; i++) {
          const tag = row.tags[i];
          if (!seen.has(tag)) {
            seen.add(tag);
            tags.push(tag);
          }
        }
        rows.push({ ...row, tags });
      }
      rows.sort((a, b) => a.id.localeCompare(b.id));
      return rows;
    }
    "helpers-ready"
  `);
  assert.equal(setup.output, "helpers-ready");

  session = loadSession(session.dump(), { tools: createEtlTools(state) });
  const firstRun = await session.runChunk(`
    const durableRawRows = await fetchRawCustomers({ source: "resume-pass.csv" });
    const durableRows = cleanCustomerRows(durableRawRows);
    const durableSummary = { imported: durableRows.length, firstId: durableRows[0].id, lastId: durableRows.at(-1).id };
    await persistCleanCustomers({ rows: durableRows, summary: durableSummary });
    ({ rows: durableRows, summary: durableSummary })
  `);

  session = loadSession(session.dump(), { tools: createEtlTools(state) });
  const secondRead = await session.runChunk(`
    ({
      ids: durableRows.map(row => row.id).join(","),
      imported: durableSummary.imported,
      firstSourceRows: durableRows[0].sourceRows,
      firstTags: durableRows[0].tags,
    })
  `);

  assert.deepEqual(firstRun.output.summary, { imported: 3, firstId: "c-001", lastId: "c-003" });
  assert.deepEqual(secondRead.output, {
    ids: "c-001,c-002,c-003",
    imported: 3,
    firstSourceRows: [1, 3],
    firstTags: ["vip", "math", "priority"],
  });
  assert.equal(state.saved.length, 1);
  assert.deepEqual(state.saved[0].summary, firstRun.output.summary);
});

console.log(`\n${passed} scenarios3 ETL checks passed.`);
