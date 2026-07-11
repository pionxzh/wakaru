#!/usr/bin/env node

import { spawn, spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";
import { join, relative, resolve } from "node:path";

import {
  acceptTest262BaselineCandidate,
  test262BaselineCandidatePath,
} from "./test262-baseline.mjs";

const repoRoot = resolve(fileURLToPath(new URL("../..", import.meta.url)));
const roundTripScript = join(repoRoot, "scripts", "correctness", "test262-roundtrip.mjs");

export const normalBaselineProducers = ["terser-light", "swc-minify", "esbuild-minify"];
export const moduleGraphBaselineProducers = [
  "none",
  "babel-env-terser",
];
export const baselineProducers = unique([
  ...normalBaselineProducers,
  ...moduleGraphBaselineProducers,
]);

export const baselineSlices = [
  "default",
  "classes",
  "destructuring",
  "async-generators",
  "scope",
  "control-flow",
  "calls",
  "operators",
  "templates",
  "literals",
  "block-scope-syntax",
  "variables",
  "assignment-target-type",
  "arguments-object",
  "identifiers",
  "function-code",
  "asi",
  "keywords",
  "reserved-words",
  "modules",
];

export function parseMatrixArgs(argv) {
  const options = {
    dryRun: false,
    missingOnly: false,
    skipBuild: false,
    producers: [],
    slices: [],
    limit: "all",
    test262Root: null,
    level: null,
    knownBlockers: null,
    caseTimeoutMs: "15000",
    toolRoot: null,
    details: false,
    keepTemp: false,
    updateBaselines: false,
    acceptCandidates: false,
  };

  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--dry-run") {
      options.dryRun = true;
    } else if (arg === "--missing") {
      options.missingOnly = true;
    } else if (arg === "--skip-build") {
      options.skipBuild = true;
    } else if (arg === "--producer") {
      options.producers.push(readRequiredValue(argv, ++i, arg));
    } else if (arg === "--slice") {
      options.slices.push(readRequiredValue(argv, ++i, arg));
    } else if (arg === "--limit") {
      options.limit = readRequiredValue(argv, ++i, arg);
    } else if (arg === "--test262") {
      options.test262Root = readRequiredValue(argv, ++i, arg);
    } else if (arg === "--level") {
      options.level = readRequiredValue(argv, ++i, arg);
    } else if (arg === "--known-blockers") {
      options.knownBlockers = readRequiredValue(argv, ++i, arg);
    } else if (arg === "--case-timeout-ms") {
      options.caseTimeoutMs = readRequiredValue(argv, ++i, arg);
    } else if (arg === "--tool-root") {
      options.toolRoot = readRequiredValue(argv, ++i, arg);
    } else if (arg === "--details") {
      options.details = true;
    } else if (arg === "--keep-temp") {
      options.keepTemp = true;
    } else if (arg === "--update") {
      options.updateBaselines = true;
    } else if (arg === "--accept") {
      options.acceptCandidates = true;
    } else if (arg === "--help" || arg === "-h") {
      options.help = true;
    } else {
      throw new Error(`unknown option: ${arg}`);
    }
  }

  validateRequestedValues("--producer", options.producers, baselineProducers);
  validateRequestedValues("--slice", options.slices, [...baselineSlices, "module-graph"]);
  if (options.acceptCandidates && options.updateBaselines) {
    throw new Error("--accept cannot be combined with --update");
  }
  if (options.acceptCandidates && options.missingOnly) {
    throw new Error("--accept cannot be combined with --missing");
  }
  return options;
}

export function buildBaselineMatrixJobs(options = {}) {
  const producers = unique(options.producers?.length ? options.producers : baselineProducers);
  const requestedSlices = unique(options.slices ?? []);
  const slices = requestedSlices.length > 0
    ? requestedSlices.filter((slice) => slice !== "module-graph")
    : baselineSlices;
  const limit = options.limit ?? "all";

  const jobs = producers.flatMap((producer) => {
    const producerJobs = normalBaselineProducers.includes(producer)
      ? slices.map((slice) => createMatrixJob({ producer, slice, preset: slice, limit, options }))
      : [];
    if (
      moduleGraphBaselineProducers.includes(producer) &&
      (requestedSlices.length === 0 || requestedSlices.includes("module-graph"))
    ) {
      producerJobs.push(
        createMatrixJob({
          producer,
          slice: "module-graph",
          preset: "modules",
          limit,
          options,
        }),
      );
    }
    return producerJobs;
  });

  if (options.missingOnly) {
    return jobs.filter((job) => !isCompleteSummary(job.summary));
  }

  return jobs;
}

function createMatrixJob({ producer, slice, preset, limit, options }) {
  const outputDir = slice === "module-graph"
    ? join(repoRoot, "docs", "test262-baselines", "module-graph")
    : join(repoRoot, "docs", "test262-baselines", producer);
  const outputName = slice === "module-graph" ? producer : slice;
  const summary = join(outputDir, `${outputName}.md`);
  const baseline = join(outputDir, `${outputName}.json`);
  const candidate = test262BaselineCandidatePath(baseline);
  const args = [
    roundTripScript,
    "--preset",
    preset,
    "--pipeline",
    producer,
    "--limit",
    String(limit),
    "--summary",
    summary,
    "--baseline",
    baseline,
  ];
  if (options.updateBaselines) args.push("--update-baseline");
  pushOptionalPair(args, "--test262", options.test262Root);
  pushOptionalPair(args, "--level", options.level);
  pushOptionalPair(args, "--known-blockers", options.knownBlockers);
  pushOptionalPair(args, "--case-timeout-ms", options.caseTimeoutMs);
  pushOptionalPair(args, "--tool-root", options.toolRoot);
  if (options.details) args.push("--details");
  if (options.keepTemp) args.push("--keep-temp");
  return {
    producer,
    slice,
    summary,
    baseline,
    candidate,
    command: process.execPath,
    args,
  };
}

export function formatCommand(job) {
  return [job.command, ...job.args].map(shellQuote).join(" ");
}

function defaultConcurrency() {
  return 1;
}

async function runPool(items, worker, concurrency = defaultConcurrency()) {
  let cursor = 0;
  const run = async () => {
    while (cursor < items.length) {
      const index = cursor++;
      await worker(items[index], index);
    }
  };
  await Promise.all(Array.from({ length: Math.min(concurrency, items.length) }, run));
}

function spawnJobAsync(job) {
  return new Promise((resolvePromise) => {
    const child = spawn(job.command, job.args, {
      cwd: repoRoot,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stderr = "";
    child.stdout.resume();
    child.stderr.on("data", (chunk) => (stderr += chunk));
    child.on("error", (err) => resolvePromise({ code: 1, stderr: err.message }));
    child.on("close", (code) => resolvePromise({ code: code ?? 1, stderr }));
  });
}

export async function runBaselineMatrix(options) {
  const jobs = buildBaselineMatrixJobs(options);
  if (options.acceptCandidates) {
    if (options.dryRun) {
      for (const job of jobs) {
        console.log(`ACCEPT ${relative(repoRoot, job.candidate)} -> ${relative(repoRoot, job.baseline)}`);
      }
      return 0;
    }
    const accepted = acceptBaselineMatrixCandidates(jobs);
    for (const item of accepted) {
      console.log(`ACCEPTED ${item.producer} / ${item.slice} -> ${relative(repoRoot, item.baseline)}`);
    }
    if (accepted.length === 0) {
      console.error("No Test262 baseline candidates found for the selected jobs.");
      return 1;
    }
    return 0;
  }
  if (options.dryRun) {
    for (const job of jobs) {
      console.log(formatCommand(job));
    }
    return 0;
  }

  if (!options.skipBuild && !process.env.WAKARU) {
    console.log("Building wakaru debug binary...");
    const result = spawnSync("cargo", ["build", "-p", "wakaru-cli"], {
      cwd: repoRoot,
      stdio: "inherit",
    });
    if (result.status !== 0) {
      return result.status ?? 1;
    }
  }

  const concurrency = defaultConcurrency();
  let failCount = 0;

  console.log(`Running ${jobs.length} jobs with concurrency ${concurrency}...`);

  await runPool(jobs, async (job) => {
    const displaySummary = relative(repoRoot, job.summary);
    const { code, stderr } = await spawnJobAsync(job);
    if (code !== 0) {
      console.log(`FAIL ${job.producer} / ${job.slice} -> ${displaySummary} (exit ${code})`);
      if (stderr.trim()) console.log(stderr.trim());
      failCount++;
    } else {
      console.log(`DONE ${job.producer} / ${job.slice} -> ${displaySummary}`);
    }
  }, concurrency);

  return failCount > 0 ? 1 : 0;
}

export function acceptBaselineMatrixCandidates(jobs) {
  const accepted = [];
  for (const job of jobs) {
    if (!existsSync(job.candidate)) continue;
    acceptTest262BaselineCandidate(job.baseline);
    accepted.push(job);
  }
  return accepted;
}

export function usage() {
  return `Usage:
  node scripts/correctness/test262-baseline-matrix.mjs [options]

Options:
  --producer <name>       Producer to run. Repeatable. Default: ${baselineProducers.join(", ")}
  --slice <name>          Test262 slice to run. Repeatable; also accepts module-graph
  --limit <n|all>         Runnable test limit passed through. Default: all
  --test262 <dir>         Test262 checkout passed through
  --level <level>         Wakaru rewrite level passed through
  --known-blockers <file> Known blocker manifest passed through
  --case-timeout-ms <n>   Per-test timeout. Default: 15000 for parallel matrix stability
  --tool-root <dir>       Tool package directory passed through
  --details               Print detailed round-trip output
  --keep-temp             Keep temporary round-trip files
  --missing               Run only missing or incomplete summaries
  --update                Explicitly rewrite selected JSON baselines and summaries
  --accept                Accept selected .json.new candidates without rerunning tests
  --skip-build            Do not build wakaru before running jobs
  --dry-run               Print commands without running them
`;
}

function readRequiredValue(argv, index, flag) {
  const value = argv[index];
  if (value == null || value.startsWith("-")) {
    throw new Error(`${flag} requires a value`);
  }
  return value;
}

function validateRequestedValues(flag, requested, supported) {
  for (const value of requested) {
    if (!supported.includes(value)) {
      throw new Error(`unsupported ${flag} ${value}`);
    }
  }
}

function pushOptionalPair(args, flag, value) {
  if (value != null) {
    args.push(flag, String(value));
  }
}

function unique(values) {
  return [...new Set(values)];
}

function isCompleteSummary(path) {
  return existsSync(path) && /^- complete: true$/m.test(readFileSync(path, "utf8"));
}

function shellQuote(value) {
  if (/^[A-Za-z0-9_./:\\-]+$/.test(value)) {
    return value;
  }
  return `"${value.replaceAll('"', '\\"')}"`;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const options = parseMatrixArgs(process.argv.slice(2));
    if (options.help) {
      console.log(usage());
      process.exit(0);
    }
    process.exitCode = await runBaselineMatrix(options);
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
