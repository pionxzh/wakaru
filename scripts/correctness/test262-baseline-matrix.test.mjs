import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  acceptBaselineMatrixCandidates,
  baselineSlices,
  buildBaselineMatrixJobs,
  moduleGraphBaselineProducers,
  normalBaselineProducers,
  parseMatrixArgs,
  runBaselineMatrix,
} from "./test262-baseline-matrix.mjs";

test("baseline matrix runs every slice for every producer", () => {
  const jobs = buildBaselineMatrixJobs();

  assert.equal(
    jobs.length,
    normalBaselineProducers.length * baselineSlices.length + moduleGraphBaselineProducers.length,
  );

  for (const producer of normalBaselineProducers) {
    assert.deepEqual(
      jobs
        .filter((job) => job.producer === producer && job.slice !== "module-graph")
        .map((job) => job.slice),
      baselineSlices,
    );
  }
  for (const producer of moduleGraphBaselineProducers) {
    assert.equal(
      jobs.filter((job) => job.producer === producer && job.slice === "module-graph").length,
      1,
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
  assert.match(jobs[0].candidate, /default\.json\.new$/);
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

test("baseline matrix creates canonical module graph jobs", () => {
  const jobs = buildBaselineMatrixJobs({
    producers: ["none", "babel-env-terser"],
    slices: ["module-graph"],
  });

  assert.equal(jobs.length, 2);
  assert.deepEqual(jobs.map((job) => job.slice), ["module-graph", "module-graph"]);
  assert.match(jobs[0].baseline, /module-graph[\\/]none\.json$/);
  assert.deepEqual(jobs[0].args.slice(1, 7), [
    "--preset",
    "modules",
    "--pipeline",
    "none",
    "--limit",
    "all",
  ]);
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
      acceptCandidates: false,
    },
  );
});

test("parseMatrixArgs rejects unknown producer or slice", () => {
  assert.deepEqual(
    parseMatrixArgs(["--producer", "none", "--slice", "module-graph"]).producers,
    ["none"],
  );
  assert.throws(() => parseMatrixArgs(["--producer", "unknown"]), /unsupported --producer unknown/);
  assert.throws(() => parseMatrixArgs(["--slice", "unknown"]), /unsupported --slice unknown/);
  assert.throws(() => parseMatrixArgs(["--accept", "--update"]), /cannot be combined/);
  assert.throws(() => parseMatrixArgs(["--accept", "--missing"]), /cannot be combined/);
});

test("parseMatrixArgs accepts candidate promotion", () => {
  const options = parseMatrixArgs(["--producer", "swc-minify", "--slice", "classes", "--accept"]);

  assert.equal(options.acceptCandidates, true);
  assert.equal(options.updateBaselines, false);
});

test("runBaselineMatrix rejects valid filters that select zero jobs", async (t) => {
  const error = t.mock.method(console, "error", () => {});
  const code = await runBaselineMatrix({
    producers: ["none"],
    slices: ["modules"],
    dryRun: true,
  });

  assert.equal(code, 1);
  assert.equal(error.mock.callCount(), 1);
  assert.match(error.mock.calls[0].arguments[0], /No Test262 baseline jobs match/);
});

test("acceptBaselineMatrixCandidates promotes only existing candidates", () => {
  const root = mkdtempSync(join(tmpdir(), "wakaru-test262-matrix-candidate-"));
  const baseline = join(root, "classes.json");
  const candidate = `${baseline}.new`;
  try {
    writeFileSync(
      candidate,
      `${JSON.stringify({
        schemaVersion: 3,
        _candidate: { reviewedBaselineSha256: null },
      })}\n`,
    );
    const accepted = acceptBaselineMatrixCandidates([
      { producer: "swc-minify", slice: "classes", baseline, candidate },
      {
        producer: "esbuild-minify",
        slice: "classes",
        baseline: join(root, "missing.json"),
        candidate: join(root, "missing.json.new"),
      },
    ]);

    assert.equal(accepted.length, 1);
    assert.equal(existsSync(baseline), true);
    assert.equal(existsSync(candidate), false);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("acceptBaselineMatrixCandidates validates every candidate before promotion", () => {
  const root = mkdtempSync(join(tmpdir(), "wakaru-test262-matrix-stale-candidate-"));
  const firstBaseline = join(root, "first.json");
  const secondBaseline = join(root, "second.json");
  const firstCandidate = `${firstBaseline}.new`;
  const secondCandidate = `${secondBaseline}.new`;
  const firstReviewed = `${JSON.stringify({ schemaVersion: 3, value: "reviewed-first" })}\n`;
  const secondReviewed = `${JSON.stringify({ schemaVersion: 3, value: "reviewed-second" })}\n`;
  try {
    writeFileSync(firstBaseline, firstReviewed);
    writeFileSync(secondBaseline, secondReviewed);
    writeFileSync(
      firstCandidate,
      `${JSON.stringify({
        schemaVersion: 3,
        value: "candidate-first",
        _candidate: { reviewedBaselineSha256: sha256(firstReviewed) },
      })}\n`,
    );
    writeFileSync(
      secondCandidate,
      `${JSON.stringify({
        schemaVersion: 3,
        value: "candidate-second",
        _candidate: { reviewedBaselineSha256: "0".repeat(64) },
      })}\n`,
    );

    assert.throws(
      () =>
        acceptBaselineMatrixCandidates([
          { producer: "swc-minify", slice: "first", baseline: firstBaseline, candidate: firstCandidate },
          { producer: "swc-minify", slice: "second", baseline: secondBaseline, candidate: secondCandidate },
        ]),
      /stale Test262 baseline candidate/,
    );
    assert.equal(readFileSync(firstBaseline, "utf8"), firstReviewed);
    assert.equal(readFileSync(secondBaseline, "utf8"), secondReviewed);
    assert.equal(existsSync(firstCandidate), true);
    assert.equal(existsSync(secondCandidate), true);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}
