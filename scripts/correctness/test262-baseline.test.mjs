import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  acceptTest262BaselineCandidate,
  applyTest262Baseline,
  compareTest262Baseline,
  createTest262Baseline,
  fingerprintTest262Outcome,
  loadTest262Baseline,
  saveTest262Baseline,
  test262BaselineCandidatePath,
  validateTest262BaselineOptions,
} from "./test262-baseline.mjs";

test("baseline stores environment identity and only non-passing outcomes", () => {
  const baseline = createTest262Baseline(
    report([
      { path: "pass.js", status: "passed", variants: ["sloppy", "strict"] },
      {
        path: "known.js",
        status: "rejected",
        phase: "transform",
        reason: "transform-reject",
        error: "Error: unsupported\n    at machine/path.js:1:1",
      },
    ]),
  );

  assert.equal(baseline.schemaVersion, 3);
  assert.equal(baseline.test262.revision, "abc123");
  assert.equal(baseline.harness.version, 2);
  assert.equal(baseline.environment.nodeMajor, 22);
  assert.equal(baseline.producer.name, "terser-light");
  assert.equal(baseline.outcomes.length, 1);
  assert.equal(baseline.outcomes[0].path, "known.js");
  assert.doesNotMatch(baseline.outcomes[0].summary, /machine\/path/);
});

test("baseline outcomes use locale-independent UTF-16 code-unit ordering", () => {
  const baseline = createTest262Baseline(
    report([
      { path: "a.js", status: "unsupported", reason: "host" },
      { path: "Z.js", status: "unsupported", reason: "host" },
    ]),
  );

  assert.deepEqual(baseline.outcomes.map((outcome) => outcome.path), ["Z.js", "a.js"]);
});

test("comparison detects new outcomes, changed fingerprints, and unexpected passes", () => {
  const expected = createTest262Baseline(
    report([{ path: "case.js", status: "rejected", reason: "known", error: "first" }]),
  );
  const actual = createTest262Baseline(
    report([{ path: "case.js", status: "rejected", reason: "known", error: "second" }]),
  );

  const comparison = compareTest262Baseline(expected, actual);
  assert.equal(comparison.clean, false);
  assert.equal(comparison.newOutcomes.length, 1);
  assert.equal(comparison.unexpectedPasses.length, 1);
});

test("comparison is clean for identical reviewed outcomes", () => {
  const expected = createTest262Baseline(
    report([{ path: "case.js", status: "unsupported", reason: "host" }]),
  );
  const actual = structuredClone(expected);

  assert.equal(compareTest262Baseline(expected, actual).clean, true);
});

test("comparison rejects environment identity drift", () => {
  const expected = createTest262Baseline(report([]));
  const actual = structuredClone(expected);
  actual.environment.nodeMajor = 24;

  assert.throws(
    () => compareTest262Baseline(expected, actual),
    /runtime environment mismatch/,
  );
});

test("comparison rejects harness identity drift", () => {
  const expected = createTest262Baseline(report([]));
  const actual = structuredClone(expected);
  actual.harness.version += 1;

  assert.throws(
    () => compareTest262Baseline(expected, actual),
    /harness version mismatch/,
  );
});

test("explicit update writes a deterministic baseline", () => {
  const root = mkdtempSync(join(tmpdir(), "wakaru-test262-baseline-unit-"));
  const path = join(root, "baseline.json");
  try {
    const result = applyTest262Baseline(report([]), { path, update: true });
    const first = readFileSync(path, "utf8");
    applyTest262Baseline(report([]), { path, update: true });

    assert.equal(result.updated, true);
    assert.equal(readFileSync(path, "utf8"), first);
    assert.equal(loadTest262Baseline(path).schemaVersion, 3);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("failed comparison writes a reviewable candidate that can be accepted", () => {
  const root = mkdtempSync(join(tmpdir(), "wakaru-test262-baseline-candidate-"));
  const path = join(root, "baseline.json");
  const candidatePath = test262BaselineCandidatePath(path);
  try {
    applyTest262Baseline(
      report([{ path: "case.js", status: "rejected", reason: "known", error: "old" }]),
      { path, update: true },
    );
    const reviewed = readFileSync(path, "utf8");
    const comparison = applyTest262Baseline(
      report([{ path: "case.js", status: "rejected", reason: "known", error: "new" }]),
      { path, update: false },
    );

    assert.equal(comparison.clean, false);
    assert.equal(comparison.candidatePath, candidatePath);
    assert.equal(readFileSync(path, "utf8"), reviewed);
    assert.equal(existsSync(candidatePath), true);

    acceptTest262BaselineCandidate(path);
    assert.equal(existsSync(candidatePath), false);
    assert.notEqual(readFileSync(path, "utf8"), reviewed);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("clean comparison and explicit update remove stale candidates", () => {
  const root = mkdtempSync(join(tmpdir(), "wakaru-test262-baseline-clean-"));
  const path = join(root, "baseline.json");
  const candidatePath = test262BaselineCandidatePath(path);
  const currentReport = report([]);
  try {
    applyTest262Baseline(currentReport, { path, update: true });
    saveTest262Baseline(candidatePath, createTest262Baseline(currentReport));
    assert.equal(applyTest262Baseline(currentReport, { path, update: false }).clean, true);
    assert.equal(existsSync(candidatePath), false);

    saveTest262Baseline(candidatePath, createTest262Baseline(currentReport));
    applyTest262Baseline(currentReport, { path, update: true });
    assert.equal(existsSync(candidatePath), false);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("identity mismatch preserves the actual baseline as a candidate", () => {
  const root = mkdtempSync(join(tmpdir(), "wakaru-test262-baseline-identity-"));
  const path = join(root, "baseline.json");
  const candidatePath = test262BaselineCandidatePath(path);
  try {
    applyTest262Baseline(report([]), { path, update: true });
    assert.throws(
      () => applyTest262Baseline(report([], { nodeMajor: 24 }), { path, update: false }),
      /Candidate baseline written/,
    );
    assert.equal(loadTest262Baseline(candidatePath).environment.nodeMajor, 24);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("filtered runs cannot compare or update a complete baseline", () => {
  assert.throws(
    () =>
      validateTest262BaselineOptions({
        baseline: "baseline.json",
        updateBaseline: true,
        limit: 5,
        presets: ["default"],
      }),
    /requires --limit all/,
  );
  assert.throws(
    () =>
      validateTest262BaselineOptions({
        baseline: "baseline.json",
        updateBaseline: true,
        limit: Number.POSITIVE_INFINITY,
        presets: [],
      }),
    /exactly one --preset/,
  );
});

test("fingerprints include deterministic emitted-code evidence", () => {
  const first = fingerprintTest262Outcome({
    status: "failed",
    phase: "decompiled-runtime",
    error: "TypeError: wrong",
    decompiled: "const value = 1;",
  });
  const second = fingerprintTest262Outcome({
    status: "failed",
    phase: "decompiled-runtime",
    error: "TypeError: wrong",
    decompiled: "const value = 2;",
  });

  assert.notEqual(first, second);
});

test("fingerprints ignore unstable VM source carets", () => {
  const left = fingerprintTest262Outcome({
    status: "unsupported",
    reason: "node-vm-baseline",
    error: "case.js:18\n  return f;\n         ^\n\nRangeError: Maximum call stack size exceeded",
  });
  const right = fingerprintTest262Outcome({
    status: "unsupported",
    reason: "node-vm-baseline",
    error: "case.js:18\n  return f;\n              ^\n\nRangeError: Maximum call stack size exceeded",
  });

  assert.equal(left, right);
});

function report(results, optionOverrides = {}) {
  const counts = {
    skipped: 0,
    unsupported: 0,
    rejected: 0,
    passed: 0,
    failed: 0,
  };
  for (const result of results) {
    counts[result.status] += 1;
  }
  return {
    complete: true,
    options: {
      test262Revision: "abc123",
      harnessVersion: 2,
      nodeMajor: 22,
      producer: {
        name: "terser-light",
        version: "5.31.6",
        configHash: "config",
      },
      level: "minimal",
      caseTimeoutMs: 15000,
      presets: ["default"],
      paths: ["test/language/sample"],
      ...optionOverrides,
    },
    totals: {
      discovered: results.length,
      runnable: results.length - counts.skipped,
      ...counts,
    },
    results,
  };
}
