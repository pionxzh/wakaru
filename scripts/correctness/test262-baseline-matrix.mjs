#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";
import { join, relative, resolve } from "node:path";

const repoRoot = resolve(fileURLToPath(new URL("../..", import.meta.url)));
const roundTripScript = join(repoRoot, "scripts", "correctness", "test262-roundtrip.mjs");

export const baselineProducers = ["terser-light", "swc-minify", "esbuild-minify"];

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
    caseTimeoutMs: null,
    toolRoot: null,
    details: false,
    keepTemp: false,
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
    } else if (arg === "--help" || arg === "-h") {
      options.help = true;
    } else {
      throw new Error(`unknown option: ${arg}`);
    }
  }

  validateRequestedValues("--producer", options.producers, baselineProducers);
  validateRequestedValues("--slice", options.slices, baselineSlices);
  return options;
}

export function buildBaselineMatrixJobs(options = {}) {
  const producers = unique(options.producers?.length ? options.producers : baselineProducers);
  const slices = unique(options.slices?.length ? options.slices : baselineSlices);
  const limit = options.limit ?? "all";

  const jobs = producers.flatMap((producer) =>
    slices.map((slice) => {
      const summary = join(repoRoot, "docs", "test262-baselines", producer, `${slice}.md`);
      const args = [
        roundTripScript,
        "--preset",
        slice,
        "--pipeline",
        producer,
        "--limit",
        String(limit),
        "--summary",
        summary,
      ];

      pushOptionalPair(args, "--test262", options.test262Root);
      pushOptionalPair(args, "--level", options.level);
      pushOptionalPair(args, "--known-blockers", options.knownBlockers);
      pushOptionalPair(args, "--case-timeout-ms", options.caseTimeoutMs);
      pushOptionalPair(args, "--tool-root", options.toolRoot);
      if (options.details) {
        args.push("--details");
      }
      if (options.keepTemp) {
        args.push("--keep-temp");
      }

      return {
        producer,
        slice,
        summary,
        command: process.execPath,
        args,
      };
    }),
  );

  if (options.missingOnly) {
    return jobs.filter((job) => !isCompleteSummary(job.summary));
  }

  return jobs;
}

export function formatCommand(job) {
  return [job.command, ...job.args].map(shellQuote).join(" ");
}

export function runBaselineMatrix(options) {
  const jobs = buildBaselineMatrixJobs(options);
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

  for (const job of jobs) {
    const displaySummary = relative(repoRoot, job.summary);
    console.log(`\n=== ${job.producer} / ${job.slice} -> ${displaySummary} ===`);
    const result = spawnSync(job.command, job.args, {
      cwd: repoRoot,
      stdio: "inherit",
    });
    if (result.status !== 0) {
      return result.status ?? 1;
    }
  }

  return 0;
}

export function usage() {
  return `Usage:
  node scripts/correctness/test262-baseline-matrix.mjs [options]

Options:
  --producer <name>       Producer to run. Repeatable. Default: ${baselineProducers.join(", ")}
  --slice <name>          Test262 slice to run. Repeatable. Default: ${baselineSlices.join(", ")}
  --limit <n|all>         Runnable test limit passed through. Default: all
  --test262 <dir>         Test262 checkout passed through
  --level <level>         Wakaru rewrite level passed through
  --known-blockers <file> Known blocker manifest passed through
  --case-timeout-ms <n>   Per-test timeout passed through
  --tool-root <dir>       Tool package directory passed through
  --details               Print detailed round-trip output
  --keep-temp             Keep temporary round-trip files
  --missing               Run only missing or incomplete summaries
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
    process.exitCode = runBaselineMatrix(options);
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
