#!/usr/bin/env node

// Runs every reproduction matrix with --json and writes a summary to
// scripts/repro/stats.json.  Re-run after rule changes to update the
// checked-in baseline so other sessions can read it without re-running
// all matrices (~2 min).
//
// Usage:
//   node scripts/repro/collect-stats.mjs            # update stats.json
//   node scripts/repro/collect-stats.mjs --check     # exit non-zero if stats.json is stale

import { execSync, spawnSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(scriptDir, "../..");
const statsPath = join(scriptDir, "stats.json");

const matrices = [
  "array-spread-rest",
  "async-await",
  "conditional-switch",
  "enum",
  "for-of-iteration",
  "object-rest-spread",
  "optional-nullish",
  "parameters",
  "swc-minifier",
  "template-literal",
];

function gitCommit() {
  try {
    return execSync("git rev-parse --short HEAD", { cwd: repoRoot, encoding: "utf8" }).trim();
  } catch {
    return "unknown";
  }
}

function runMatrix(name) {
  const script = join(scriptDir, `${name}-matrix/matrix.mjs`);
  const result = spawnSync("node", [script, "--json"], {
    cwd: repoRoot,
    encoding: "utf8",
    maxBuffer: 1024 * 1024 * 50,
    timeout: 5 * 60 * 1000,
    stdio: ["pipe", "pipe", "pipe"],
  });
  if (result.status !== 0) {
    console.error(`  ${name}: failed (exit ${result.status})`);
    return null;
  }
  try {
    const data = JSON.parse(result.stdout);
    return {
      name,
      yes: data.summary.yes,
      no: data.summary.no,
      error: data.summary.error ?? 0,
      total: data.summary.yes + data.summary.no,
      pct: data.summary.pct,
    };
  } catch {
    console.error(`  ${name}: invalid JSON output`);
    return null;
  }
}

const checkMode = process.argv.includes("--check");

console.log("Running reproduction matrices...");
const results = [];
for (const name of matrices) {
  process.stdout.write(`  ${name}...`);
  const result = runMatrix(name);
  if (result) {
    console.log(` ${result.yes}/${result.total} (${result.pct}%)`);
    results.push(result);
  }
}

const yes = results.reduce((s, r) => s + r.yes, 0);
const total = results.reduce((s, r) => s + r.total, 0);
const pct = total > 0 ? +((yes / total) * 100).toFixed(1) : 0;

const stats = {
  commit: gitCommit(),
  date: new Date().toISOString().slice(0, 10),
  aggregate: { yes, total, pct },
  matrices: results,
};

const json = JSON.stringify(stats, null, 2) + "\n";

if (checkMode) {
  let existing;
  try {
    existing = readFileSync(statsPath, "utf8");
  } catch {
    console.error("\nstats.json does not exist. Run without --check to create it.");
    process.exit(1);
  }
  if (existing !== json) {
    console.error("\nstats.json is stale. Run `node scripts/repro/collect-stats.mjs` to update.");
    process.exit(1);
  }
  console.log(`\nstats.json is up to date: ${yes}/${total} (${pct}%)`);
} else {
  writeFileSync(statsPath, json);
  console.log(`\nWrote ${statsPath}: ${yes}/${total} (${pct}%)`);
}
