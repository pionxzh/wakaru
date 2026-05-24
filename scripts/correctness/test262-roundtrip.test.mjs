import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  buildHarnessSource,
  classifyTest,
  executeTestSource,
  isSloppyOnlyWakaruParseUnsupported,
  knownSwcFidelityIssueReason,
  knownWakaruParseUnsupportedReason,
  parseArgs,
  parseTestMetadata,
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
    transform: "none",
  });

  assert.equal(output, source);
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
  ]);

  assert.deepEqual(options.paths, [
    "test/language/a",
    "test/language/b",
    "test/language/expressions/class",
    "test/language/statements/class",
  ]);
  assert.equal(options.limit, Number.POSITIVE_INFINITY);
  assert.equal(options.level, "minimal");
  assert.equal(options.transform, "terser");
  assert.equal(options.terserProfile, "light");
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

function makeTempTest262() {
  const root = mkdtempSync(join(tmpdir(), "wakaru-test262-unit-"));
  const harness = join(root, "harness");
  mkdirSync(harness, { recursive: true });
  writeFileSync(join(harness, "assert.js"), "globalThis.assert = { sameValue() {} };\n");
  writeFileSync(join(harness, "sta.js"), "globalThis.staLoaded = true;\n");
  writeFileSync(join(harness, "extra.js"), "globalThis.extraLoaded = true;\n");
  return root;
}
