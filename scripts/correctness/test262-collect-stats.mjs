#!/usr/bin/env node

// Reads committed test262 baseline summaries from docs/test262-baselines/ and
// writes a compact stats file to scripts/correctness/test262-stats.json.
// Does NOT re-run tests — it parses the markdown summaries that are already
// checked in.  Re-run baselines first if you want fresh numbers.
//
// Usage:
//   node scripts/correctness/test262-collect-stats.mjs                               # update all
//   node scripts/correctness/test262-collect-stats.mjs --producer swc-minify          # update one producer
//   node scripts/correctness/test262-collect-stats.mjs --slice classes --slice scope  # update specific slices
//   node scripts/correctness/test262-collect-stats.mjs --check                        # exit non-zero if stale
//
// When --producer or --slice is given, only matching entries are re-collected;
// the rest are kept from the existing stats file.

import { execSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join, dirname, basename } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(scriptDir, "../..");
const baselinesDir = join(repoRoot, "docs", "test262-baselines");
const statsPath = join(scriptDir, "test262-stats.json");

const allProducers = ["terser-light", "swc-minify", "esbuild-minify"];

function parseArgs(argv) {
  const options = { producers: [], slices: [], check: false };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--check") {
      options.check = true;
    } else if (arg === "--producer") {
      const value = argv[++i];
      if (!value || value.startsWith("-")) throw new Error("--producer requires a value");
      if (!allProducers.includes(value)) throw new Error(`unknown producer: ${value}`);
      options.producers.push(value);
    } else if (arg === "--slice") {
      const value = argv[++i];
      if (!value || value.startsWith("-")) throw new Error("--slice requires a value");
      options.slices.push(value);
    } else {
      throw new Error(`unknown option: ${arg}`);
    }
  }
  return options;
}

function gitCommit() {
  try {
    return execSync("git rev-parse --short HEAD", { cwd: repoRoot, encoding: "utf8" }).trim();
  } catch {
    return "unknown";
  }
}

function parseSummaryTotals(content) {
  const match = content.match(
    /\|\s*(\d+)\s*\|\s*(\d+)\s*\|\s*(\d+)\s*\|\s*(\d+)\s*\|\s*(\d+)\s*\|\s*(\d+)\s*\|\s*(\d+)\s*\|/,
  );
  if (!match) return null;
  return {
    discovered: Number(match[1]),
    runnable: Number(match[2]),
    skipped: Number(match[3]),
    unsupported: Number(match[4]),
    rejected: Number(match[5]),
    passed: Number(match[6]),
    failed: Number(match[7]),
  };
}

function isComplete(content) {
  return /^- complete: true$/m.test(content);
}

function collectSlices(producerDir) {
  if (!existsSync(producerDir)) return [];
  return readdirSync(producerDir)
    .filter((f) => f.endsWith(".md"))
    .map((f) => basename(f, ".md"))
    .sort();
}

function loadExistingStats() {
  try {
    return JSON.parse(readFileSync(statsPath, "utf8"));
  } catch {
    return null;
  }
}

function entryKey(producer, slice) {
  return `${producer}\0${slice}`;
}

function recomputeAggregate(entries) {
  let runnable = 0;
  let passed = 0;
  let failed = 0;
  for (const entry of entries) {
    runnable += entry.runnable;
    passed += entry.passed;
    failed += entry.failed;
  }
  const decidable = passed + failed;
  const pct = decidable > 0 ? +((passed / decidable) * 100).toFixed(1) : 0;
  return {
    producers: allProducers.length,
    entries: entries.length,
    runnable,
    passed,
    failed,
    pct,
  };
}

const options = parseArgs(process.argv.slice(2));
const filterProducers = options.producers.length > 0 ? options.producers : allProducers;
const filterSlices = new Set(options.slices);
const isFiltered = options.producers.length > 0 || options.slices.length > 0;

console.log(
  isFiltered
    ? `Collecting test262 baseline stats (${filterProducers.join(", ")}${filterSlices.size > 0 ? ` / ${[...filterSlices].join(", ")}` : ""})...`
    : "Collecting test262 baseline stats...",
);

const freshEntries = new Map();

for (const producer of filterProducers) {
  const dir = join(baselinesDir, producer);
  let slices = collectSlices(dir);
  if (filterSlices.size > 0) {
    slices = slices.filter((s) => filterSlices.has(s));
  }
  for (const slice of slices) {
    const summaryPath = join(dir, `${slice}.md`);
    const content = readFileSync(summaryPath, "utf8");
    const complete = isComplete(content);
    const totals = parseSummaryTotals(content);
    if (!totals) {
      console.error(`  ${producer}/${slice}: could not parse totals`);
      continue;
    }
    if (!complete) {
      console.error(`  ${producer}/${slice}: incomplete`);
    }
    const entry = { producer, slice, complete, ...totals };
    freshEntries.set(entryKey(producer, slice), entry);
    process.stdout.write(`  ${producer}/${slice}: ${totals.passed}/${totals.runnable}`);
    console.log(totals.failed > 0 ? ` (${totals.failed} FAILED)` : "");
  }
}

// Merge: start from existing entries, overwrite with freshly collected ones.
const existing = loadExistingStats();
const mergedMap = new Map();
if (isFiltered && existing) {
  for (const entry of existing.entries) {
    mergedMap.set(entryKey(entry.producer, entry.slice), entry);
  }
}
for (const [key, entry] of freshEntries) {
  mergedMap.set(key, entry);
}
const mergedEntries = [...mergedMap.values()].sort(
  (a, b) => a.producer.localeCompare(b.producer) || a.slice.localeCompare(b.slice),
);

const aggregate = recomputeAggregate(mergedEntries);

const stats = {
  commit: gitCommit(),
  date: new Date().toISOString().slice(0, 10),
  aggregate,
  entries: mergedEntries,
};

const json = JSON.stringify(stats, null, 2) + "\n";

if (options.check) {
  if (!existing) {
    console.error("\ntest262-stats.json does not exist. Run without --check to create it.");
    process.exit(1);
  }
  const existingNormalized = { ...existing, commit: stats.commit, date: stats.date };
  if (JSON.stringify(existingNormalized) !== JSON.stringify(stats)) {
    console.error("\ntest262-stats.json is stale. Run `node scripts/correctness/test262-collect-stats.mjs` to update.");
    process.exit(1);
  }
  console.log(`\ntest262-stats.json is up to date: ${aggregate.passed}/${aggregate.passed + aggregate.failed} (${aggregate.pct}%)`);
} else {
  writeFileSync(statsPath, json);
  console.log(`\nWrote ${statsPath}: ${aggregate.passed}/${aggregate.passed + aggregate.failed} (${aggregate.pct}%)`);
}
