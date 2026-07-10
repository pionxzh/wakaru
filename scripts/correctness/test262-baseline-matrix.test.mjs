import assert from "node:assert/strict";
import test from "node:test";

import {
  baselineProducers,
  baselineSlices,
  buildBaselineMatrixJobs,
  parseMatrixArgs,
} from "./test262-baseline-matrix.mjs";

test("baseline matrix runs every slice for every producer", () => {
  const jobs = buildBaselineMatrixJobs();

  assert.equal(jobs.length, baselineProducers.length * baselineSlices.length);

  for (const producer of baselineProducers) {
    assert.deepEqual(
      jobs.filter((job) => job.producer === producer).map((job) => job.slice),
      baselineSlices,
    );
  }
});

test("baseline matrix writes summaries under producer directories", () => {
  const jobs = buildBaselineMatrixJobs({
    producers: ["terser-light"],
    slices: ["default", "scope"],
    limit: "all",
  });

  assert.equal(jobs.length, 2);
  assert.match(jobs[0].summary, /docs[\\/]test262-baselines[\\/]terser-light[\\/]default\.md$/);
  assert.match(jobs[1].summary, /docs[\\/]test262-baselines[\\/]terser-light[\\/]scope\.md$/);
  assert.match(jobs[0].baseline, /docs[\\/]test262-baselines[\\/]terser-light[\\/]default\.json$/);
  assert.deepEqual(jobs[0].args.slice(1, 7), [
    "--preset",
    "default",
    "--pipeline",
    "terser-light",
    "--limit",
    "all",
  ]);
});

test("baseline matrix deduplicates repeatable filters", () => {
  const jobs = buildBaselineMatrixJobs({
    producers: ["swc-minify", "swc-minify"],
    slices: ["calls", "calls"],
  });

  assert.equal(jobs.length, 1);
  assert.equal(jobs[0].producer, "swc-minify");
  assert.equal(jobs[0].slice, "calls");
});

test("parseMatrixArgs supports repeatable producer and slice filters", () => {
  assert.deepEqual(
    parseMatrixArgs([
      "--producer",
      "swc-minify",
      "--producer",
      "esbuild-minify",
      "--slice",
      "calls",
      "--slice",
      "operators",
      "--limit",
      "5",
      "--missing",
      "--skip-build",
      "--dry-run",
      "--update",
    ]),
    {
      dryRun: true,
      missingOnly: true,
      skipBuild: true,
      producers: ["swc-minify", "esbuild-minify"],
      slices: ["calls", "operators"],
      limit: "5",
      test262Root: null,
      level: null,
      knownBlockers: null,
      caseTimeoutMs: "15000",
      toolRoot: null,
      details: false,
      keepTemp: false,
      updateBaselines: true,
    },
  );
});

test("parseMatrixArgs rejects unknown producer or slice", () => {
  assert.throws(() => parseMatrixArgs(["--producer", "unknown"]), /unsupported --producer unknown/);
  assert.throws(() => parseMatrixArgs(["--slice", "unknown"]), /unsupported --slice unknown/);
});
