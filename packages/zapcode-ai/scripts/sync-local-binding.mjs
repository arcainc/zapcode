#!/usr/bin/env node
/**
 * Rebuild the local napi addon (crates/zapcode-js) and link it into this
 * package's node_modules so e2e tests run against local Rust changes instead
 * of the published npm binary.
 *
 * `index.js` in @unchartedfr/zapcode prefers a `.node` sitting next to it
 * (the "development" path), so we copy the freshly built artifacts there.
 */
import { execSync } from "node:child_process";
import { copyFileSync, existsSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const aiPkg = join(here, "..");
const repoRoot = join(aiPkg, "..", "..");
const jsCrate = join(repoRoot, "crates", "zapcode-js");
const dest = join(aiPkg, "node_modules", "@unchartedfr", "zapcode");

if (!existsSync(dest)) {
  throw new Error(
    `Cannot find ${dest}. Run \`npm install\` in packages/zapcode-ai first.`
  );
}

console.log("[sync] building napi addon (release)…");
execSync("npm run build", { cwd: jsCrate, stdio: "inherit" });

const nodeBinaries = readdirSync(jsCrate).filter(f => f.endsWith(".node"));
if (nodeBinaries.length === 0) {
  throw new Error(`No .node artifact produced in ${jsCrate}`);
}

for (const file of [...nodeBinaries, "index.js", "index.d.ts"]) {
  const from = join(jsCrate, file);
  if (!existsSync(from)) continue;
  copyFileSync(from, join(dest, file));
  console.log(`[sync] linked ${file}`);
}

console.log("[sync] local binding ready.");
