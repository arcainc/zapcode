/**
 * Shared helpers for the differential parity harnesses (differential.mjs and
 * fuzz-differential.mjs). Every snippet is the BODY of an async function
 * ending in an explicit `return`; it runs through BOTH zapcode and real Node
 * (same process) and the normalized results must agree.
 */
import { isDeepStrictEqual } from "node:util";
import { execute } from "../dist/index.js";

export const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;

/** Normalize per the host-boundary marshalling rules (cluster L). */
export function normalize(v) {
  if (v === undefined) return null;
  if (typeof v === "number" && !Number.isFinite(v)) return null;
  if (Array.isArray(v)) return v.map(normalize);
  if (v && typeof v === "object") {
    const out = {};
    for (const [k, val] of Object.entries(v)) {
      if (val === undefined) continue; // dropped at the boundary
      out[k] = normalize(val);
    }
    return out;
  }
  return v;
}

export async function runZapcode(body) {
  const r = await execute(`async function main() { ${body} } main();`, {});
  return normalize(r.output);
}

export async function runNode(body) {
  return normalize(await new AsyncFunction(body)());
}

/**
 * Run one body through both engines, capturing errors.
 * Returns { node, nodeErr, zapcode, zapErr } — the *Err fields are
 * `undefined` when that engine produced a value.
 */
export async function runBoth(body) {
  let node, nodeErr, zapcode, zapErr;
  try {
    node = await runNode(body);
  } catch (e) {
    nodeErr = String(e);
  }
  try {
    zapcode = await runZapcode(body);
  } catch (e) {
    zapErr = String(e);
  }
  return { node, nodeErr, zapcode, zapErr };
}

/**
 * Divergence rule (same as differential.mjs): both sides must agree on
 * *whether* the body fails (message text may differ between engines); if
 * both succeed, the normalized results must be deeply equal.
 */
export function isDivergent(r) {
  if ((r.nodeErr === undefined) !== (r.zapErr === undefined)) return true;
  if (r.nodeErr !== undefined) return false; // both errored — agreement
  return !isDeepStrictEqual(r.zapcode, r.node);
}
