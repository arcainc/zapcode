/**
 * Realistic document/compliance extraction workflows for Zapcode.
 *
 * Covers contract clause parsing with regex, party/date/amount normalization,
 * review/escalation decisions, schema validation for bad tool calls, and
 * structured document summaries.
 *
 * Run: npm run build && node tests/scenarios3-compliance.mjs
 */
import assert from "node:assert/strict";
import { execute, zapcode } from "../dist/index.js";

let passed = 0;
async function check(name, fn) {
  try {
    await fn();
    passed++;
    console.log("  PASS " + name);
  } catch (error) {
    console.error("  FAIL " + name);
    throw error;
  }
}

async function expectExecutionError(code, tools, expectedMessage) {
  await assert.rejects(
    () => execute(code, tools),
    error => {
      assert.match(String(error.message), expectedMessage);
      return true;
    }
  );
}

const msaSnippet = `
MASTER SERVICES AGREEMENT

This Master Services Agreement is entered into by Acme Robotics, Inc. ("Customer")
and Northwind Legal Services LLC ("Vendor").

Effective Date: June 15, 2026
Term: twelve months unless terminated earlier.
Fees: Customer will pay Vendor up to $1,250,000 for implementation services.
Governing Law: New York
Data Processing: Vendor may process Customer personal data only for support and
must notify Customer of a security incident within 72 hours.
Assignment: Vendor may not assign this Agreement without Customer's prior written consent.
`;

const riskyDpaSnippet = `
DATA PROCESSING ADDENDUM

Controller: Helio Bank N.A.
Processor: SmallCloud Analytics Ltd.
Effective Date: 2026-05-20
Processing Cap: USD 45,000
Governing Law: Delaware
Security Incident Notice: Processor shall notify Controller within 10 business days.
Subprocessors: Processor may appoint subprocessors without prior notice.
`;

function createComplianceTools({ summaries = [], escalations = [] } = {}) {
  return {
    normalizeParty: {
      description: "Normalize a raw party name and role from a legal document.",
      parameters: {
        rawName: { type: "string" },
        role: { type: "string" },
      },
      execute: async ({ rawName, role }) => {
        const cleanName = rawName.replace(/\s+/g, " ").replace(/[.]+$/g, "").trim();
        const id = cleanName
          .toUpperCase()
          .replace(/[^A-Z0-9]+/g, "_")
          .replace(/^_+|_+$/g, "");
        return { id, name: cleanName, role };
      },
    },
    normalizeDate: {
      description: "Normalize an extracted date to ISO yyyy-mm-dd.",
      parameters: {
        rawDate: { type: "string" },
      },
      execute: async ({ rawDate }) => {
        const parsed = Date.parse(rawDate);
        if (Number.isNaN(parsed)) {
          throw new Error("invalid date: " + rawDate);
        }
        return new Date(parsed).toISOString().slice(0, 10);
      },
    },
    normalizeAmountUsd: {
      description: "Normalize an extracted USD amount.",
      parameters: {
        rawAmount: { type: "string" },
      },
      execute: async ({ rawAmount }) => {
        const numeric = Number(rawAmount.replace(/[^0-9.]/g, ""));
        if (!Number.isFinite(numeric)) {
          throw new Error("invalid amount: " + rawAmount);
        }
        return numeric;
      },
    },
    submitComplianceSummary: {
      description: "Persist a structured compliance extraction summary.",
      parameters: {
        documentId: { type: "string" },
        summary: { type: "object" },
        riskScore: { type: "number" },
        tags: { type: "array" },
      },
      execute: async input => {
        summaries.push(input);
        return { saved: true, documentId: input.documentId, tagCount: input.tags.length };
      },
    },
    escalateForReview: {
      description: "Escalate a risky document to legal or compliance review.",
      parameters: {
        documentId: { type: "string" },
        reviewer: { type: "string" },
        reason: { type: "string" },
        severity: { type: "string" },
        evidence: { type: "object" },
      },
      execute: async input => {
        escalations.push(input);
        return { escalated: true, reviewer: input.reviewer };
      },
    },
  };
}

console.log("scenarios3 compliance e2e");

await check("system prompt emits named object signatures for compliance tools", async () => {
  const { system } = zapcode({ tools: createComplianceTools() });
  assert.match(system, /declare function normalizeParty\(input: \{ rawName: string; role: string \}\): Promise<unknown>;/);
  assert.match(system, /declare function submitComplianceSummary\(input: \{ documentId: string; summary: object; riskScore: number; tags: array \}\): Promise<unknown>;/);
  assert.match(system, /Call shape: await escalateForReview\(\{ documentId: string, reviewer: string/);
});

await check("parse MSA clauses, normalize fields, and save structured summary", async () => {
  const summaries = [];
  const escalations = [];
  const result = await execute(
    `
    const documentText = ${JSON.stringify(msaSnippet)};
    const docId = "MSA-ACME-2026";

    const partyMatch = /by\\s+([^\\n]+?)\\s+\\("Customer"\\)\\s+and\\s+([^\\n]+?)\\s+\\("Vendor"\\)/.exec(documentText);
    const dateMatch = /Effective Date:\\s*([^\\n]+)/.exec(documentText);
    const amountMatch = /(?:Fees|Processing Cap):[^$U\\n]*(?:USD\\s*)?(\\$?[0-9][0-9,]*(?:\\.[0-9]+)?)/.exec(documentText);
    const governingLawMatch = /Governing Law:\\s*([^\\n]+)/.exec(documentText);
    const incidentMatch = /within\\s+([0-9]+)\\s+hours/.exec(documentText);
    const assignmentMatch = /Assignment:\\s*([^\\n]+)/.exec(documentText);

    const customer = await normalizeParty({ rawName: partyMatch[1], role: "customer" });
    const vendor = await normalizeParty({ rawName: partyMatch[2], role: "vendor" });
    const effectiveDate = await normalizeDate({ rawDate: dateMatch[1] });
    const amountUsd = await normalizeAmountUsd({ rawAmount: amountMatch[1] });

    const incidentNoticeHours = incidentMatch ? Number(incidentMatch[1]) : null;
    const assignmentRequiresConsent = assignmentMatch[1].indexOf("without Customer's prior written consent") >= 0;
    const riskScore = amountUsd >= 1000000 ? 70 : 25;
    const tags = ["contract", "data-processing", riskScore >= 70 ? "high-value" : "standard"];
    const summary = {
      parties: [customer, vendor],
      effectiveDate,
      amountUsd,
      governingLaw: governingLawMatch[1].trim(),
      clauses: {
        incidentNoticeHours,
        assignmentRequiresConsent,
      },
    };

    await submitComplianceSummary({ documentId: docId, summary, riskScore, tags });
    ({ docId, customerId: customer.id, vendorId: vendor.id, effectiveDate, amountUsd, riskScore, assignmentRequiresConsent })
    `,
    createComplianceTools({ summaries, escalations })
  );

  assert.deepEqual(result.output, {
    docId: "MSA-ACME-2026",
    customerId: "ACME_ROBOTICS_INC",
    vendorId: "NORTHWIND_LEGAL_SERVICES_LLC",
    effectiveDate: "2026-06-15",
    amountUsd: 1250000,
    riskScore: 70,
    assignmentRequiresConsent: true,
  });
  assert.equal(summaries.length, 1);
  assert.equal(escalations.length, 0);
  assert.equal(summaries[0].summary.governingLaw, "New York");
  assert.equal(summaries[0].summary.clauses.incidentNoticeHours, 72);
  assert.deepEqual(summaries[0].tags, ["contract", "data-processing", "high-value"]);
  assert.deepEqual(result.toolCalls.at(-1).input, summaries[0]);
});

await check("detect risky DPA clauses and escalate with evidence", async () => {
  const summaries = [];
  const escalations = [];
  const result = await execute(
    `
    const documentText = ${JSON.stringify(riskyDpaSnippet)};
    const docId = "DPA-HELIO-2026";

    const controllerMatch = /Controller:\\s*([^\\n]+)/.exec(documentText);
    const processorMatch = /Processor:\\s*([^\\n]+)/.exec(documentText);
    const dateMatch = /Effective Date:\\s*([^\\n]+)/.exec(documentText);
    const amountMatch = /Processing Cap:\\s*(USD\\s*[0-9,]+)/.exec(documentText);
    const noticeMatch = /within\\s+([0-9]+)\\s+business days/.exec(documentText);
    const subprocessorOpen = documentText.indexOf("without prior notice") >= 0;

    const controller = await normalizeParty({ rawName: controllerMatch[1], role: "controller" });
    const processor = await normalizeParty({ rawName: processorMatch[1], role: "processor" });
    const effectiveDate = await normalizeDate({ rawDate: dateMatch[1] });
    const amountUsd = await normalizeAmountUsd({ rawAmount: amountMatch[1] });
    const noticeBusinessDays = Number(noticeMatch[1]);

    const findings = [];
    if (noticeBusinessDays > 3) findings.push("incident notice exceeds 3 business days");
    if (subprocessorOpen) findings.push("subprocessors allowed without notice");

    const riskScore = 35 + findings.length * 25;
    const summary = {
      parties: [controller, processor],
      effectiveDate,
      amountUsd,
      clauses: { noticeBusinessDays, subprocessorOpen },
      findings,
    };
    await submitComplianceSummary({
      documentId: docId,
      summary,
      riskScore,
      tags: ["dpa", riskScore >= 75 ? "review-required" : "standard-review"],
    });

    if (riskScore >= 75) {
      await escalateForReview({
        documentId: docId,
        reviewer: "privacy-counsel",
        severity: "high",
        reason: findings.join("; "),
        evidence: { noticeBusinessDays, subprocessorOpen, processorId: processor.id },
      });
    }

    ({ riskScore, findings, escalated: riskScore >= 75, effectiveDate, amountUsd })
    `,
    createComplianceTools({ summaries, escalations })
  );

  assert.deepEqual(result.output, {
    riskScore: 85,
    findings: ["incident notice exceeds 3 business days", "subprocessors allowed without notice"],
    escalated: true,
    effectiveDate: "2026-05-20",
    amountUsd: 45000,
  });
  assert.equal(summaries.length, 1);
  assert.equal(escalations.length, 1);
  assert.equal(escalations[0].reviewer, "privacy-counsel");
  assert.equal(escalations[0].evidence.noticeBusinessDays, 10);
  assert.equal(result.toolCalls.at(-1).name, "escalateForReview");
  assert.deepEqual(result.toolCalls.at(-1).input, escalations[0]);
});

await check("schema validation rejects malformed compliance summary calls", async () => {
  const tools = createComplianceTools();

  await expectExecutionError(
    `
    await submitComplianceSummary({
      documentId: "BAD-1",
      summary: { ok: true },
      tags: ["contract"],
    })
    `,
    tools,
    /Invalid arguments for tool 'submitComplianceSummary': missing required parameter 'riskScore'/
  );

  await expectExecutionError(
    `
    await submitComplianceSummary({
      documentId: "BAD-2",
      summary: "not structured",
      riskScore: 10,
      tags: ["contract"],
    })
    `,
    tools,
    /Invalid arguments for tool 'submitComplianceSummary': parameter 'summary' expected object, got string/
  );

  await expectExecutionError(
    `
    await escalateForReview({
      documentId: "BAD-3",
      reviewer: "privacy-counsel",
      severity: "high",
      reason: "typo field",
      evidnce: { finding: "missing schema key" },
    })
    `,
    tools,
    /Invalid arguments for tool 'escalateForReview': unexpected parameter 'evidnce'/
  );
});

await check("autoFix returns structured errors and prevents invalid review side effects", async () => {
  const escalations = [];
  const result = await execute(
    `
    await escalateForReview({
      documentId: "DPA-BAD-4",
      reviewer: "privacy-counsel",
      severity: "high",
      reason: "invalid evidence shape",
      evidence: [],
    })
    `,
    createComplianceTools({ escalations }),
    { autoFix: true }
  );

  assert.equal(result.output, null);
  assert.match(result.error, /parameter 'evidence' expected object, got array/);
  assert.deepEqual(escalations, []);
  assert.equal(result.trace.status, "error");
  const failedToolSpan = result.trace.children.find(span => span.name === "tool_call");
  assert.equal(failedToolSpan.attributes["zapcode.tool.name"], "escalateForReview");
  assert.match(failedToolSpan.attributes["zapcode.tool.error"], /expected object/);
});

console.log(`\n${passed} compliance scenario checks passed.`);
