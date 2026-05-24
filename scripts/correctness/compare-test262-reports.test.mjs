import assert from "node:assert/strict";
import test from "node:test";

import { compareReports, formatComparison } from "./compare-test262-reports.mjs";

test("compareReports summarizes totals and status transitions", () => {
  const before = {
    totals: {
      passed: 1,
      failed: 2,
      rejected: 0,
    },
    results: [
      { path: "a.js", status: "passed", variants: ["sloppy"] },
      { path: "b.js", status: "failed", phase: "decompiled-runtime" },
      { path: "c.js", status: "failed", phase: "wakaru" },
    ],
  };
  const after = {
    totals: {
      passed: 2,
      failed: 1,
      rejected: 1,
    },
    results: [
      { path: "a.js", status: "passed", variants: ["sloppy"] },
      { path: "b.js", status: "passed", variants: ["sloppy"] },
      { path: "c.js", status: "rejected", phase: "swc-fidelity", reason: "swc-array-binding-elision" },
      { path: "d.js", status: "failed", phase: "decompiled-runtime" },
    ],
  };

  const comparison = compareReports(before, after);

  assert.equal(comparison.deltas.passed, 1);
  assert.equal(comparison.deltas.failed, -1);
  assert.deepEqual(comparison.transitions, [
    { transition: "failed:decompiled-runtime -> passed", count: 1 },
    { transition: "failed:wakaru -> rejected:swc-array-binding-elision", count: 1 },
    { transition: "missing -> failed:decompiled-runtime", count: 1 },
  ]);
});

test("formatComparison prints readable deltas", () => {
  const output = formatComparison({
    beforeTotals: { passed: 1, failed: 1 },
    afterTotals: { passed: 2, failed: 0 },
    deltas: { passed: 1, failed: -1 },
    transitions: [{ transition: "failed:wakaru -> passed", count: 1 }],
    changed: [{ path: "a.js", from: "failed:wakaru", to: "passed" }],
  });

  assert.match(output, /passed: 1 -> 2 \(\+1\)/);
  assert.match(output, /failed: 1 -> 0 \(-1\)/);
  assert.match(output, /1 failed:wakaru -> passed/);
});
