import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  applyTest262Baseline,
  compareTest262Baseline,
  createTest262Baseline,
  fingerprintTest262Outcome,
  loadTest262Baseline,
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

function report(results) {
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
    },
    totals: {
      discovered: results.length,
      runnable: results.length - counts.skipped,
      ...counts,
    },
    results,
  };
}
