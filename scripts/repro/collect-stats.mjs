#!/usr/bin/env node

// Runs every reproduction matrix with --json and writes a summary to
// scripts/repro/stats.json.  Re-run after rule changes to update the
// checked-in baseline so other sessions can read it without re-running
// all matrices.
//
// Usage:
//   node scripts/repro/collect-stats.mjs            # update stats.json
//   node scripts/repro/collect-stats.mjs --check     # exit non-zero if stats.json is stale
//   node scripts/repro/collect-stats.mjs --jobs 1    # cap harness concurrency
//                                                    # (1 = at most one wakaru
//                                                    # process at any time)

import { execSync, spawn } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { defaultConcurrency, parseReproJobs, runPool, wakaruCommand } from "./lib/runner.mjs";
import { planMatrixJobs } from "./lib/matrix-jobs.mjs";
import { findReproStatDrift } from "./lib/doc-stats.mjs";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(scriptDir, "../..");
const statsPath = join(scriptDir, "stats.json");

// --jobs N (or WAKARU_REPRO_JOBS) is the wakaru-process budget for the whole
// run. It is split between concurrent matrix processes, each of which gets
// its share through WAKARU_REPRO_JOBS (see lib/matrix-jobs.mjs).
let jobBudget;
try {
  const jobsFlagIndex = process.argv.indexOf("--jobs");
  jobBudget = jobsFlagIndex !== -1
    ? parseReproJobs(process.argv[jobsFlagIndex + 1], "--jobs")
    : defaultConcurrency();
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
}

const matrices = [
  "array-spread-rest",
  "async-await",
  "cjs-export-storage",
  "class-accessor",
  "closure-compiler",
  "conditional-switch",
  "enum",
  "for-of-iteration",
  "object-rest-spread",
  "optional-nullish",
  "parameters",
  "swc-minifier",
  "template-literal",
  "tslib-helpers",
];

function gitCommit() {
  try {
    return execSync("git rev-parse --short HEAD", { cwd: repoRoot, encoding: "utf8" }).trim();
  } catch {
    return "unknown";
  }
}

function spawnMatrix(name, env) {
  const script = join(scriptDir, `${name}-matrix/matrix.mjs`);
  return new Promise((resolvePromise) => {
    const child = spawn("node", [script, "--json"], {
      cwd: repoRoot,
      env,
      timeout: 5 * 60 * 1000,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk) => (stdout += chunk));
    child.stderr.resume();
    child.on("error", () => resolvePromise({ status: null, stdout }));
    child.on("close", (status) => resolvePromise({ status, stdout }));
  });
}

async function runMatrix(name, env) {
  const result = await spawnMatrix(name, env);
  if (result.status !== 0) {
    console.error(`  ${name}: failed (exit ${result.status})`);
    return null;
  }
  try {
    const data = JSON.parse(result.stdout);
    const matrix = {
      name,
      yes: data.summary.yes,
      no: data.summary.no,
      error: data.summary.error ?? 0,
      info: data.summary.info ?? 0,
      total: data.summary.yes + data.summary.no,
      pct: data.summary.pct,
    };
    const errorSamples = (data.rows ?? [])
      .filter((row) => row.status !== "yes" && row.status !== "no" && !row.status.startsWith("info-"))
      .slice(0, 3)
      .map((row) => ({
        snippet: row.snippet,
        tools: row.tools,
        status: row.status,
        notes: row.notes,
      }));
    Object.defineProperty(matrix, "errorSamples", { value: errorSamples });
    return matrix;
  } catch {
    console.error(`  ${name}: invalid JSON output`);
    return null;
  }
}

function comparableStats(stats) {
  return { aggregate: stats.aggregate, matrices: stats.matrices };
}

function formatMatrix(matrix) {
  if (!matrix) return "<missing>";
  return `${matrix.yes}/${matrix.total} (${matrix.pct}%), no=${matrix.no}, error=${matrix.error ?? 0}`;
}

function formatValue(value) {
  return value === undefined ? "<missing>" : JSON.stringify(value);
}

function formatErrorSample(sample) {
  const tools = Array.isArray(sample.tools) ? sample.tools.join(", ") : "<unknown tool>";
  const note = String(sample.notes ?? "").replace(/\s+/g, " ").trim();
  return `${sample.snippet ?? "<unknown snippet>"} / ${tools} / ${sample.status}: ${note}`;
}

function printStatsDiff(recorded, measured) {
  console.error("  matrix diffs:");

  let printed = false;
  const recordedMatrices = new Map((recorded.matrices ?? []).map((matrix) => [matrix.name, matrix]));
  const measuredMatrices = new Map((measured.matrices ?? []).map((matrix) => [matrix.name, matrix]));
  const names = [
    ...matrices,
    ...[...recordedMatrices.keys()].filter((name) => !matrices.includes(name)),
    ...[...measuredMatrices.keys()].filter((name) => !matrices.includes(name) && !recordedMatrices.has(name)),
  ];

  const aggregateFields = ["yes", "total", "pct"];
  const aggregateDiffs = aggregateFields
    .filter((field) => recorded.aggregate?.[field] !== measured.aggregate?.[field])
    .map((field) => `${field}: ${formatValue(recorded.aggregate?.[field])} -> ${formatValue(measured.aggregate?.[field])}`);
  if (aggregateDiffs.length > 0) {
    printed = true;
    console.error(`    aggregate: ${aggregateDiffs.join(", ")}`);
  }

  for (const name of names) {
    const before = recordedMatrices.get(name);
    const after = measuredMatrices.get(name);
    if (!before || !after) {
      printed = true;
      console.error(`    ${name}: recorded ${formatMatrix(before)}; measured ${formatMatrix(after)}`);
      continue;
    }

    const diffs = ["yes", "no", "error", "info", "total", "pct"]
      .filter((field) => (before[field] ?? 0) !== (after[field] ?? 0))
      .map((field) => `${field}: ${formatValue(before[field] ?? 0)} -> ${formatValue(after[field] ?? 0)}`);
    if (diffs.length > 0) {
      printed = true;
      console.error(`    ${name}: recorded ${formatMatrix(before)}; measured ${formatMatrix(after)}; ${diffs.join(", ")}`);
      for (const sample of after.errorSamples ?? []) {
        console.error(`      measured error sample: ${formatErrorSample(sample)}`);
      }
    }
  }

  if (!printed) {
    console.error("    no scalar field differences found; compare stats.json ordering or extra fields");
  }
}

const checkMode = process.argv.includes("--check");

const plan = planMatrixJobs(jobBudget, matrices.length);
// Resolve (and, without $WAKARU, build) the CLI once for every child.
const matrixEnv = { ...process.env, WAKARU: wakaruCommand(), WAKARU_REPRO_JOBS: String(plan.perMatrix) };

console.log(`Running reproduction matrices (${plan.matrices} at a time, ${plan.perMatrix} job(s) each)...`);
// Results keep the declared matrix order; stats.json comparison depends on it.
const slots = new Array(matrices.length).fill(null);
await runPool(matrices, async (name, index) => {
  const result = await runMatrix(name, matrixEnv);
  if (result) {
    const errorSuffix = result.error ? ` / ${result.error} error` : "";
    console.log(`  ${name}: ${result.yes}/${result.total}${errorSuffix} (${result.pct}%)`);
  }
  slots[index] = result;
}, plan.matrices);
const results = slots.filter(Boolean);

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
    existing = JSON.parse(readFileSync(statsPath, "utf8"));
  } catch {
    console.error("\nstats.json is missing or invalid. Run without --check to create it.");
    process.exit(1);
  }
  // Compare only the measured numbers. The commit/date fields are
  // provenance: they change on every commit and every day, so including
  // them would make --check permanently stale anywhere but the machine
  // that just regenerated the file (e.g. always-failing in CI).
  const measured = (s) => JSON.stringify(comparableStats(s), null, 2);
  if (measured(existing) !== measured(stats)) {
    console.error("\nstats.json is stale. Run `node scripts/repro/collect-stats.mjs` to update.");
    console.error(`  recorded:  ${existing.aggregate?.yes}/${existing.aggregate?.total} (${existing.aggregate?.pct}%)`);
    console.error(`  measured:  ${yes}/${total} (${pct}%)`);
    printStatsDiff(comparableStats(existing), comparableStats(stats));
    process.exit(1);
  }
  console.log(`\nstats.json is up to date: ${yes}/${total} (${pct}%)`);
} else {
  writeFileSync(statsPath, json);
  console.log(`\nWrote ${statsPath}: ${yes}/${total} (${pct}%)`);
}

// The aggregate is also cited in human-facing copy; regenerating stats.json
// without moving those citations is silent drift, so both modes verify them.
const drift = findReproStatDrift(stats.aggregate, {
  readme: readFileSync(join(repoRoot, "README.md"), "utf8"),
  website: readFileSync(join(repoRoot, "website/index.html"), "utf8"),
  docsSite: readFileSync(join(repoRoot, "docs-site/content/docs/project/correctness.mdx"), "utf8"),
});
if (drift.length > 0) {
  console.error("\nStat citations are out of sync with the measured aggregate:");
  for (const message of drift) {
    console.error(`  ${message}`);
  }
  process.exit(1);
}
