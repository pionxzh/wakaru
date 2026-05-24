import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  buildHarnessSource,
  classifyKnownBlocker,
  classifyTest,
  executeTestSource,
  discoverTestsFromReport,
  formatMarkdownSummary,
  isSloppyOnlyWakaruParseUnsupported,
  knownSwcFidelityIssueReason,
  knownWakaruParseUnsupportedReason,
  loadKnownBlockers,
  missingToolPackageSpecs,
  parseArgs,
  parseTestMetadata,
  resolvePipelineName,
  runnableVariants,
  runRoundTrip,
  transformSource,
} from "./test262-roundtrip.mjs";

test("parseTestMetadata reads inline and block list metadata", () => {
  const source = `/*---
description: sample
features: [optional-chaining, coalesce-expression]
includes:
  - propertyHelper.js
  - compareArray.js
flags: [noStrict]
---*/
assert.sameValue(1, 1);
`;

  const metadata = parseTestMetadata(source);

  assert.deepEqual(metadata.features, ["optional-chaining", "coalesce-expression"]);
  assert.deepEqual(metadata.includes, ["propertyHelper.js", "compareArray.js"]);
  assert.deepEqual(metadata.flags, ["noStrict"]);
  assert.equal(metadata.negative, null);
});

test("parseTestMetadata detects negative tests", () => {
  const source = `/*---
negative:
  phase: parse
  type: SyntaxError
---*/
`;

  assert.equal(parseTestMetadata(source).negative, true);
});

test("runnableVariants follows Test262 strict mode flags", () => {
  assert.deepEqual(
    runnableVariants({ flags: [], negative: null }),
    [
      { name: "sloppy", strict: false },
      { name: "strict", strict: true },
    ],
  );
  assert.deepEqual(runnableVariants({ flags: ["noStrict"], negative: null }), [
    { name: "sloppy", strict: false },
  ]);
  assert.deepEqual(runnableVariants({ flags: ["onlyStrict"], negative: null }), [
    { name: "strict", strict: true },
  ]);
  assert.deepEqual(runnableVariants({ flags: ["module"], negative: null }), []);
});

test("classifyTest skips unsupported host-sensitive tests", () => {
  assert.deepEqual(
    classifyTest("test/language/example.js", "$262.gc();", {
      flags: [],
      negative: null,
    }),
    { runnable: false, reason: "host-api" },
  );
  assert.deepEqual(
    classifyTest("test/language/example.js", "assert.sameValue(1, 1);", {
      flags: ["async"],
      negative: null,
    }),
    { runnable: false, reason: "flag:async" },
  );
  assert.deepEqual(
    classifyTest("test/language/example.js", "assert.sameValue(1, 1);", {
      flags: [],
      negative: null,
    }),
    { runnable: true, reason: null },
  );
});

test("buildHarnessSource loads required Test262 harness files", () => {
  const root = makeTempTest262();
  try {
    const harness = buildHarnessSource(root, {
      includes: ["extra.js"],
    });

    assert.match(harness, /harness\/assert\.js/);
    assert.match(harness, /harness\/sta\.js/);
    assert.match(harness, /harness\/extra\.js/);
    assert.match(harness, /globalThis.extraLoaded = true/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("executeTestSource runs harness and strict test source in a script realm", () => {
  const harnessSource = `
globalThis.assert = {
  sameValue(actual, expected) {
    if (!Object.is(actual, expected)) {
      throw new Error(\`expected \${expected}, got \${actual}\`);
    }
  }
};
`;

  executeTestSource({
    harnessSource,
    testSource: "assert.sameValue(function f() { return this; }(), undefined);",
    filename: "strict-fixture.js",
    strict: true,
  });
});

test("transformSource supports no-op mode without external tools", async () => {
  const source = "const value = 1;\n";
  const output = await transformSource(source, {
    pipeline: "none",
    transform: "terser",
    terserProfile: "light",
  });

  assert.equal(output, source);
});

test("resolvePipelineName maps legacy transform options", () => {
  assert.equal(resolvePipelineName({ pipeline: "babel-env-terser" }), "babel-env-terser");
  assert.equal(resolvePipelineName({ transform: "none", terserProfile: "light" }), "none");
  assert.equal(resolvePipelineName({ transform: "terser", terserProfile: "light" }), "terser-light");
  assert.equal(resolvePipelineName({ transform: "terser", terserProfile: "full" }), "terser-full");
  assert.equal(resolvePipelineName({ pipeline: "swc-minify" }), "swc-minify");
  assert.equal(resolvePipelineName({ pipeline: "esbuild-minify" }), "esbuild-minify");
});

test("missingToolPackageSpecs checks package resolution instead of directory presence", () => {
  const root = mkdtempSync(join(tmpdir(), "wakaru-tools-unit-"));
  try {
    mkdirSync(join(root, "node_modules", "@babel", "core"), { recursive: true });
    writeFileSync(
      join(root, "package.json"),
      JSON.stringify({
        private: true,
        type: "module",
      }),
    );

    assert.deepEqual(
      missingToolPackageSpecs(root, [
        { name: "@babel/core", spec: "@babel/core@7.25.2" },
      ]),
      [{ name: "@babel/core", spec: "@babel/core@7.25.2" }],
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("isSloppyOnlyWakaruParseUnsupported detects sloppy-only strict parser rejects", () => {
  const error = new Error('failed to parse input.js: InvalidIdentInStrict("yield")');

  assert.equal(
    isSloppyOnlyWakaruParseUnsupported(error, [{ name: "sloppy", strict: false }]),
    true,
  );
  assert.equal(
    isSloppyOnlyWakaruParseUnsupported(error, [{ name: "strict", strict: true }]),
    false,
  );
  assert.equal(
    isSloppyOnlyWakaruParseUnsupported(new Error("runtime failed"), [
      { name: "sloppy", strict: false },
    ]),
    false,
  );
});

test("parseArgs accepts repeatable paths, presets, and all limit", () => {
  const options = parseArgs([
    "--path",
    "test/language/a",
    "--path",
    "test/language/b",
    "--preset",
    "classes",
    "--limit",
    "all",
    "--pipeline",
    "babel-env-terser",
    "--summary",
    "target/test262-summary.md",
    "--known-blockers",
    "scripts/correctness/test262-known-blockers.json",
    "--case-timeout-ms",
    "1234",
    "--rerun-from",
    "target/previous.json",
    "--rerun-status",
    "failed",
    "--rerun-status",
    "rejected",
  ]);

  assert.deepEqual(options.paths, [
    "test/language/a",
    "test/language/b",
    "test/language/expressions/class",
    "test/language/statements/class",
  ]);
  assert.equal(options.limit, Number.POSITIVE_INFINITY);
  assert.equal(options.level, "minimal");
  assert.equal(options.pipeline, "babel-env-terser");
  assert.equal(options.caseTimeoutMs, 1234);
  assert.deepEqual(options.rerunStatuses, ["failed", "rejected"]);
  assert.match(options.rerunFrom, /target[\\/]previous\.json$/);
  assert.equal(options.transform, "terser");
  assert.equal(options.terserProfile, "light");
  assert.match(options.summary, /target[\\/]test262-summary\.md$/);
  assert.match(options.knownBlockers, /scripts[\\/]correctness[\\/]test262-known-blockers\.json$/);
});

test("discoverTestsFromReport reruns selected result statuses", () => {
  const root = makeTempTest262();
  const reportPath = join(root, "report.json");
  try {
    const testDir = join(root, "test", "language", "sample");
    mkdirSync(testDir, { recursive: true });
    writeFileSync(join(testDir, "failed.js"), "assert.sameValue(1, 1);\n");
    writeFileSync(join(testDir, "passed.js"), "assert.sameValue(1, 1);\n");
    writeFileSync(
      reportPath,
      JSON.stringify({
        results: [
          { path: "test/language/sample/failed.js", status: "failed" },
          { path: "test/language/sample/passed.js", status: "passed" },
        ],
      }),
    );

    const tests = discoverTestsFromReport(root, reportPath, ["failed"]);

    assert.equal(tests.length, 1);
    assert.match(tests[0], /failed\.js$/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("loadKnownBlockers and classifyKnownBlocker use manifest entries", () => {
  const root = mkdtempSync(join(tmpdir(), "wakaru-known-blockers-unit-"));
  const manifestPath = join(root, "known-blockers.json");
  try {
    writeFileSync(
      manifestPath,
      JSON.stringify({
        version: 1,
        entries: [
          {
            reason: "tool-printer-gap",
            status: "rejected",
            phase: "swc-fidelity",
            when: {
              pathIncludes: ["sample"],
              errorIncludes: ["SyntaxError"],
              decompiledRegex: ["class\\s+extends\\s+\\(\\)\\s*=>"],
            },
          },
        ],
      }),
    );

    const knownBlockers = loadKnownBlockers(manifestPath);
    assert.equal(
      classifyKnownBlocker({
        knownBlockers,
        status: "rejected",
        phase: "swc-fidelity",
        path: "test/language/sample.js",
        error: new Error("SyntaxError"),
        decompiled: "class extends () => {}",
      }),
      "tool-printer-gap",
    );
    assert.equal(
      classifyKnownBlocker({
        knownBlockers,
        status: "unsupported",
        phase: "wakaru-parse",
        path: "test/language/sample.js",
        error: new Error("SyntaxError"),
        decompiled: "class extends () => {}",
      }),
      null,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("knownWakaruParseUnsupportedReason classifies SWC parser gaps", () => {
  assert.equal(
    knownWakaruParseUnsupportedReason(new Error("InvalidIdentInAsync"), [{ name: "strict", strict: true }], ""),
    "swc-parse-async-ident",
  );
  assert.equal(
    knownWakaruParseUnsupportedReason(
      new Error("ExpectedIdent"),
      [{ name: "sloppy", strict: false }],
      "test/language/statements/let/static-init-await-binding-valid.js",
    ),
    "swc-parse-static-init-await",
  );
  assert.equal(
    knownWakaruParseUnsupportedReason(
      new Error("failed to parse input.js: Error { error: (39..44, TS1109) }"),
      [{ name: "sloppy", strict: false }],
      "test/language/expressions/object/method-definition/yield-as-identifier-in-nested-function.js",
    ),
    "swc-parse-yield-ident",
  );
  assert.equal(
    knownWakaruParseUnsupportedReason(
      new Error("failed to parse input.js: Error { error: (26..37, AsyncConstructor) }"),
      [{ name: "sloppy", strict: false }],
      "test/language/expressions/class/elements/syntax/valid/grammar-static-ctor-async-meth-valid.js",
    ),
    "swc-parse-static-async-constructor-method",
  );
  assert.equal(
    knownWakaruParseUnsupportedReason(
      new Error('failed to parse input.js: Error { error: (13..18, Expected("{", "await")) }'),
      [{ name: "sloppy", strict: false }],
      "test/language/expressions/class/class-name-ident-await.js",
    ),
    "swc-parse-await-class-name",
  );
  assert.equal(
    knownWakaruParseUnsupportedReason(
      new Error('failed to parse input.js: Error { error: (1128..1133, Expected("(", "await")) }'),
      [{ name: "sloppy", strict: false }],
      "test/language/expressions/class/class-name-ident-await-escaped.js",
    ),
    "swc-parse-await-class-name",
  );
});

test("knownSwcFidelityIssueReason classifies array binding elision printer gaps", () => {
  assert.equal(
    knownSwcFidelityIssueReason({
      path: "test/language/statements/for-of/dstr/array-iteration.js",
      error: new Error("Test262Error"),
      decompiled: "for ([] of [g()]) {}",
    }),
    "swc-array-binding-elision",
  );
  assert.equal(
    knownSwcFidelityIssueReason({
      path: "test/language/statements/for-of/dstr/obj-id.js",
      error: new Error("TypeError"),
      decompiled: "for ({ x } of values) {}",
    }),
    null,
  );
  assert.equal(
    knownSwcFidelityIssueReason({
      path: "test/language/expressions/class/elements/syntax/valid/grammar-static-ctor-meth-valid.js",
      error: new Error("SyntaxError: A class may only have one constructor"),
      decompiled: "class {\nconstructor(){}\nconstructor(){}\n}",
    }),
    "swc-print-static-constructor-method",
  );
  assert.equal(
    knownSwcFidelityIssueReason({
      path: "test/language/expressions/class/heritage-arrow-function.js",
      error: new Error("SyntaxError: Unexpected token '=>'"),
      decompiled: "var C = class extends ()=>{} {\n};",
    }),
    "swc-print-class-extends-arrow-parens",
  );
});

test("formatMarkdownSummary emits stable totals, reasons, and failures", () => {
  const summary = formatMarkdownSummary({
    options: {
      paths: ["test/language/sample"],
      limit: "all",
      pipeline: "terser-light",
      transform: "terser",
      terserProfile: "light",
      level: "minimal",
      knownBlockers: "scripts/correctness/test262-known-blockers.json",
      caseTimeoutMs: 5000,
      rerunFrom: null,
      rerunStatuses: [],
    },
    complete: false,
    totals: {
      discovered: 3,
      runnable: 2,
      skipped: 1,
      unsupported: 0,
      rejected: 1,
      passed: 0,
      failed: 1,
    },
    results: [
      { path: "a.js", status: "skipped", reason: "flag:async" },
      { path: "b.js", status: "rejected", reason: "swc-array-binding-elision" },
      { path: "c.js", status: "failed", phase: "decompiled-runtime" },
    ],
  });

  assert.match(summary, /# Test262 Round-Trip Summary/);
  assert.match(summary, /- complete: false/);
  assert.match(summary, /- caseTimeoutMs: 5000/);
  assert.match(summary, /\| 3 \| 2 \| 1 \| 0 \| 1 \| 0 \| 1 \|/);
  assert.match(summary, /\| rejected \| swc-array-binding-elision \| 1 \|/);
  assert.match(summary, /- c\.js \(decompiled-runtime\)/);
});

test("runRoundTrip reports baseline failures as unsupported inputs", async () => {
  const root = makeTempTest262();
  try {
    const testDir = join(root, "test", "language", "sample");
    mkdirSync(testDir, { recursive: true });
    writeFileSync(join(testDir, "baseline-fails.js"), "throw new Error('host gap');\n");

    const report = await runRoundTrip({
      test262Root: root,
      paths: ["test/language/sample"],
      limit: 1,
      transform: "none",
      terserProfile: "light",
      level: "minimal",
      toolRoot: join(root, "tools"),
      keepTemp: false,
    });

    assert.equal(report.totals.runnable, 1);
    assert.equal(report.totals.unsupported, 1);
    assert.equal(report.totals.failed, 0);
    assert.equal(report.results[0].status, "unsupported");
    assert.equal(report.results[0].phase, "baseline");
    assert.equal(report.results[0].reason, "node-vm-baseline");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("runRoundTrip writes incremental JSON and final completion state", async () => {
  const root = makeTempTest262();
  try {
    const testDir = join(root, "test", "language", "sample");
    const reportPath = join(root, "report.json");
    mkdirSync(testDir, { recursive: true });
    writeFileSync(join(testDir, "baseline-fails.js"), "throw new Error('host gap');\n");

    await runRoundTrip({
      test262Root: root,
      paths: ["test/language/sample"],
      limit: 1,
      pipeline: "none",
      transform: "terser",
      terserProfile: "light",
      level: "minimal",
      toolRoot: join(root, "tools"),
      keepTemp: false,
      json: reportPath,
      caseTimeoutMs: 1000,
    });

    const report = JSON.parse(readFileSync(reportPath, "utf8"));
    assert.equal(report.complete, true);
    assert.equal(report.totals.processed, 1);
    assert.equal(report.results[0].status, "unsupported");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

function makeTempTest262() {
  const root = mkdtempSync(join(tmpdir(), "wakaru-test262-unit-"));
  const harness = join(root, "harness");
  mkdirSync(harness, { recursive: true });
  writeFileSync(join(harness, "assert.js"), "globalThis.assert = { sameValue() {} };\n");
  writeFileSync(join(harness, "sta.js"), "globalThis.staLoaded = true;\n");
  writeFileSync(join(harness, "extra.js"), "globalThis.extraLoaded = true;\n");
  return root;
}
