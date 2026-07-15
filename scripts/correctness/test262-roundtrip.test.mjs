import assert from "node:assert/strict";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  buildHarnessSource,
  classifyKnownBlocker,
  classifyTest,
  collectStaticModuleSpecifiers,
  createToolValidationCache,
  executeTestSource,
  executeTestSourceOutcome,
  executeModuleGraph,
  discoverTestsFromReport,
  describeProducer,
  formatMarkdownSummary,
  formatBaselineComparison,
  isSloppyOnlyWakaruParseUnsupported,
  knownDecompiledRuntimeRejectReason,
  knownSwcFidelityIssueReason,
  knownTransformedRuntimeRejectReason,
  knownTransformRejectReason,
  knownWakaruParseUnsupportedReason,
  loadKnownBlockers,
  missingToolPackageSpecs,
  parseArgs,
  parseSourceOutcome,
  parseTestMetadata,
  readModuleGraph,
  resolvePipelineName,
  resolvePipelineToolRoot,
  runnableVariants,
  runWakaruAsync,
  runRoundTrip,
  transformSource,
  test262ReportExitCode,
  test262HarnessVersion,
  unsupportedTest262Capability,
} from "./test262-roundtrip.mjs";

test("runWakaruAsync accepts an empty program and decodes split UTF-8 chunks", async () => {
  const emptyOutput = await runWakaruAsync("void 0;", {
    level: "minimal",
    timeoutMs: 1000,
    wakaruCmd: { command: process.execPath, prefix: ["-e", "", "--"] },
  });
  assert.equal(emptyOutput, "");

  const output = await runWakaruAsync("void 0;", {
    level: "minimal",
    timeoutMs: 1000,
    wakaruCmd: {
      command: process.execPath,
      prefix: [
        "-e",
        "process.stdout.write(Buffer.from([0xe2])); setTimeout(() => process.stdout.write(Buffer.from([0x82, 0xac])), 10);",
        "--",
      ],
    },
  });
  assert.equal(output, "€");
});

test("module parser checks reject infrastructure failures and use the final error diagnostic", () => {
  const base = {
    source: "TypeError + ;",
    filename: "sample.js",
    strict: true,
    module: true,
    timeoutMs: 321,
  };

  assert.throws(
    () =>
      parseSourceOutcome({
        ...base,
        spawnSyncImpl: () => ({ error: new Error("spawn EAGAIN") }),
      }),
    /module parse check failed.*spawn EAGAIN/,
  );
  assert.throws(
    () =>
      parseSourceOutcome({
        ...base,
        spawnSyncImpl: () => ({ error: null, signal: "SIGKILL", status: null }),
      }),
    /terminated by SIGKILL/,
  );
  assert.throws(
    () =>
      parseSourceOutcome({
        ...base,
        spawnSyncImpl: () => ({
          error: null,
          signal: null,
          status: 1,
          stderr: "node check failed without a JavaScript diagnostic",
          stdout: "",
        }),
      }),
    /without a typed error diagnostic/,
  );

  let receivedOptions;
  const outcome = parseSourceOutcome({
    ...base,
    spawnSyncImpl: (_command, _args, options) => {
      receivedOptions = options;
      return {
        error: null,
        signal: null,
        status: 1,
        stderr: "TypeError + ;\n            ^\nSyntaxError: Unexpected token ';'\n",
        stdout: "",
      };
    },
  });
  assert.equal(receivedOptions.timeout, 321);
  assert.equal(outcome.phase, "parse");
  assert.equal(outcome.errorName, "SyntaxError");
});

test("tool validation cache validates once and retries failures", () => {
  const validateOnce = createToolValidationCache();
  let successfulCalls = 0;
  validateOnce("esbuild", () => successfulCalls++);
  validateOnce("esbuild", () => successfulCalls++);
  assert.equal(successfulCalls, 1);

  let attempts = 0;
  assert.throws(() =>
    validateOnce("swc", () => {
      attempts += 1;
      throw new Error("not ready");
    }),
  );
  validateOnce("swc", () => attempts++);
  assert.equal(attempts, 2);
});

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

  assert.deepEqual(parseTestMetadata(source).negative, {
    phase: "parse",
    type: "SyntaxError",
  });
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
  assert.deepEqual(runnableVariants({ flags: ["module"], negative: null }), [
    { name: "module", strict: true, module: true },
  ]);
});

test("classifyTest leaves runtime semantics to typed execution", () => {
  assert.deepEqual(
    classifyTest("test/language/example.js", "$262.gc();", {
      flags: [],
      negative: null,
    }),
    { runnable: true, reason: null },
  );
  assert.deepEqual(
    classifyTest("test/language/example.js", "assert.sameValue(1, 1);", {
      flags: ["async"],
      negative: null,
    }),
    { runnable: true, reason: null },
  );
  assert.deepEqual(
    classifyTest("test/language/module-code/example_FIXTURE.js", "export const value = 1;", {
      flags: [],
      negative: null,
    }),
    { runnable: false, reason: "fixture" },
  );
  assert.deepEqual(
    classifyTest("test/language/example.js", "assert.sameValue(1, 1);", {
      flags: [],
      negative: null,
    }),
    { runnable: true, reason: null },
  );
});

test("unsupportedTest262Capability is metadata-driven", () => {
  assert.equal(
    unsupportedTest262Capability({
      flags: [],
      includes: ["agent.js"],
      features: [],
    }),
    "host:$262.agent",
  );
  assert.equal(
    unsupportedTest262Capability({
      flags: [],
      includes: [],
      features: ["IsHTMLDDA"],
    }),
    "host:IsHTMLDDA",
  );
  assert.equal(
    unsupportedTest262Capability({ flags: [], includes: [], features: [] }),
    null,
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

test("executeTestSource runs harness and strict test source in a script realm", async () => {
  const harnessSource = `
globalThis.assert = {
  sameValue(actual, expected) {
    if (!Object.is(actual, expected)) {
      throw new Error(\`expected \${expected}, got \${actual}\`);
    }
  }
};
`;

  await executeTestSource({
    harnessSource,
    testSource: "assert.sameValue(function f() { return this; }(), undefined);",
    filename: "strict-fixture.js",
    strict: true,
  });
});

test("executeTestSource captures unhandled rejections as test failures", async () => {
  await assert.rejects(
    executeTestSource({
      harnessSource: "",
      testSource: "async function f() { await new Promise(); }\nf();",
      filename: "unhandled-rejection-fixture.js",
      strict: false,
    }),
    /Promise resolver undefined is not a function/,
  );
});

test("executeTestSourceOutcome distinguishes parse and runtime failures", async () => {
  const parse = await executeTestSourceOutcome({
    harnessSource: "",
    testSource: "let x = ;",
    filename: "parse-negative.js",
    strict: false,
  });
  const runtime = await executeTestSourceOutcome({
    harnessSource: "",
    testSource: "throw new TypeError('expected');",
    filename: "runtime-negative.js",
    strict: false,
  });

  assert.equal(parse.phase, "parse");
  assert.equal(parse.errorName, "SyntaxError");
  assert.equal(runtime.phase, "runtime");
  assert.equal(runtime.errorName, "TypeError");
});

test("executeTestSourceOutcome treats every falsy $DONE value as success", async () => {
  for (const value of ["undefined", "null", "false", "0", "''", "NaN"]) {
    const outcome = await executeTestSourceOutcome({
      harnessSource: "",
      testSource: `
const realm = $262.createRealm();
if (!realm.global) throw new Error("missing realm global");
Promise.resolve().then(() => $DONE(${value}));
`,
      filename: "async.js",
      strict: false,
      async: true,
      timeoutMs: 1000,
    });

    assert.equal(outcome.phase, "success", `$DONE(${value})`);
  }
});

test("collectStaticModuleSpecifiers finds imports and re-exports", () => {
  assert.deepEqual(
    collectStaticModuleSpecifiers(`
import "./side-effect.js";
import value, { named } from "./dep.js";
export { named } from "./re-export.js";
export * as ns from "./namespace.js";
`),
    ["./side-effect.js", "./dep.js", "./re-export.js", "./namespace.js"],
  );
});

test("readModuleGraph recursively follows relative module imports", () => {
  const root = makeTempTest262();
  try {
    const testDir = join(root, "test", "language", "module-code");
    mkdirSync(testDir, { recursive: true });
    writeFileSync(
      join(testDir, "main.js"),
      `import { value } from "./dep.js";\nassert.sameValue(value, 2);\n`,
    );
    writeFileSync(
      join(testDir, "dep.js"),
      `export { value } from "./nested.js";\n`,
    );
    writeFileSync(join(testDir, "nested.js"), `export const value = 2;\n`);

    const graph = readModuleGraph(root, join(testDir, "main.js"));

    assert.deepEqual([...graph.sources.keys()].sort(), [
      "test/language/module-code/dep.js",
      "test/language/module-code/main.js",
      "test/language/module-code/nested.js",
    ]);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("executeModuleGraph runs a Test262-style ESM graph with harness globals", () => {
  const root = mkdtempSync(join(tmpdir(), "wakaru-module-graph-unit-"));
  try {
    const harnessSource = `
function Test262Error(message) { this.message = message; }
globalThis.assert = {
  sameValue(actual, expected) {
    if (!Object.is(actual, expected)) throw new Test262Error(\`\${actual} !== \${expected}\`);
  }
};
`;
    const sources = new Map([
      [
        "test/language/module-code/main.js",
        `import { value } from "./dep.js";\nassert.sameValue(this, undefined);\nawait Promise.resolve();\nassert.sameValue(value, 2);\n`,
      ],
      ["test/language/module-code/dep.js", `export let value = 1;\nvalue += 1;\n`],
    ]);

    executeModuleGraph({
      harnessSource,
      entryPath: "test/language/module-code/main.js",
      sources,
      tmpRoot: root,
      phase: "unit",
      timeoutMs: 1000,
    });
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
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

test("runRoundTrip executes module graphs", async () => {
  const root = makeTempTest262();
  try {
    const testDir = join(root, "test", "language", "module-code");
    mkdirSync(testDir, { recursive: true });
    writeFileSync(
      join(testDir, "main.js"),
      `/*---\nflags: [module]\n---*/\nimport { value } from "./dep.js";\nassert.sameValue(value, 2);\n`,
    );
    writeFileSync(join(testDir, "dep.js"), `export let value = 1;\nvalue += 1;\n`);

    const report = await runRoundTrip({
      test262Root: root,
      paths: ["test/language/module-code/main.js"],
      limit: 1,
      pipeline: "none",
      transform: "terser",
      terserProfile: "light",
      level: "minimal",
      toolRoot: join(root, "tools"),
      keepTemp: false,
      caseTimeoutMs: 5000,
    });

    assert.equal(report.totals.runnable, 1);
    assert.equal(report.totals.passed, 1);
    assert.equal(report.totals.failed, 0);
    assert.equal(report.results[0].variants[0], "module");
    assert.equal(report.results[0].modules, 2);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("runRoundTrip handles parse negatives, runtime negatives, async, raw, and host cases", async () => {
  const root = makeTempTest262();
  try {
    const testDir = join(root, "test", "language", "sample");
    mkdirSync(testDir, { recursive: true });
    writeFileSync(
      join(testDir, "parse-negative.js"),
      "/*---\nnegative: { phase: parse, type: SyntaxError }\n---*/\nlet value = ;\n",
    );
    writeFileSync(
      join(testDir, "runtime-negative.js"),
      "/*---\nnegative: { phase: runtime, type: TypeError }\n---*/\nthrow new TypeError('expected');\n",
    );
    writeFileSync(
      join(testDir, "async.js"),
      "/*---\nflags: [async]\n---*/\nPromise.resolve().then(() => $DONE());\n",
    );
    writeFileSync(
      join(testDir, "raw.js"),
      "/*---\nflags: [raw]\n---*/\nif (typeof assert !== 'undefined') throw new Error('raw harness leak');\n",
    );
    writeFileSync(
      join(testDir, "detach.js"),
      "/*---\n---*/\nconst buffer = new ArrayBuffer(8); $262.detachArrayBuffer(buffer); if (buffer.byteLength !== 0) throw new Error('not detached');\n",
    );
    writeFileSync(
      join(testDir, "agent.js"),
      "/*---\nincludes: [agent.js]\n---*/\nvoid 0;\n",
    );

    const report = await runRoundTrip({
      test262Root: root,
      paths: ["test/language/sample"],
      limit: Number.POSITIVE_INFINITY,
      pipeline: "none",
      transform: "terser",
      terserProfile: "light",
      level: "minimal",
      toolRoot: join(root, "tools"),
      keepTemp: false,
      caseTimeoutMs: 2000,
    });

    assert.equal(report.totals.passed, 5);
    assert.equal(report.totals.unsupported, 1);
    assert.equal(report.totals.failed, 0);
    assert.equal(
      report.results.find((result) => result.path.endsWith("parse-negative.js")).lane,
      "parser-boundary",
    );
    assert.equal(
      report.results.find((result) => result.path.endsWith("agent.js")).reason,
      "host:$262.agent",
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("runRoundTrip preserves module resolution/runtime negatives and async completion", async () => {
  const root = makeTempTest262();
  try {
    const testDir = join(root, "test", "language", "module-code");
    mkdirSync(testDir, { recursive: true });
    writeFileSync(
      join(testDir, "runtime-negative.js"),
      "/*---\nflags: [module]\nnegative: { phase: runtime, type: TypeError }\n---*/\nthrow new TypeError('expected');\n",
    );
    writeFileSync(
      join(testDir, "resolution-negative.js"),
      "/*---\nflags: [module]\nnegative: { phase: resolution, type: SyntaxError }\n---*/\nimport { missing } from './resolution_FIXTURE.js'; void missing;\n",
    );
    writeFileSync(
      join(testDir, "resolution_FIXTURE.js"),
      "export const present = 1;\n",
    );
    writeFileSync(
      join(testDir, "async-module.js"),
      "/*---\nflags: [module, async]\n---*/\nawait Promise.resolve(); $DONE(0);\n",
    );
    writeFileSync(
      join(testDir, "parse-negative.js"),
      "/*---\nflags: [module]\nnegative: { phase: parse, type: SyntaxError }\n---*/\nexport const value = ;\n",
    );

    const report = await runRoundTrip({
      test262Root: root,
      paths: ["test/language/module-code"],
      limit: Number.POSITIVE_INFINITY,
      pipeline: "none",
      transform: "terser",
      terserProfile: "light",
      level: "minimal",
      toolRoot: join(root, "tools"),
      keepTemp: false,
      caseTimeoutMs: 3000,
    });

    assert.equal(report.totals.passed, 4);
    assert.equal(report.totals.skipped, 1);
    assert.equal(report.totals.failed, 0);
    assert.equal(report.totals.unsupported, 0);
    assert.equal(
      report.results.find((result) => result.path.endsWith("parse-negative.js")).lane,
      "parser-boundary",
    );
    assert.equal(
      report.results.find((result) => result.path.endsWith("resolution-negative.js")).status,
      "passed",
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("resolvePipelineName maps legacy transform options", () => {
  assert.equal(resolvePipelineName({ pipeline: "babel-env-terser" }), "babel-env-terser");
  assert.equal(resolvePipelineName({ transform: "none", terserProfile: "light" }), "none");
  assert.equal(resolvePipelineName({ transform: "terser", terserProfile: "light" }), "terser-light");
  assert.equal(resolvePipelineName({ transform: "terser", terserProfile: "full" }), "terser-full");
  assert.equal(resolvePipelineName({ pipeline: "swc-minify" }), "swc-minify");
  assert.equal(resolvePipelineName({ pipeline: "esbuild-minify" }), "esbuild-minify");
});

test("resolvePipelineToolRoot isolates producer dependencies", () => {
  assert.match(
    resolvePipelineToolRoot("target/tools", "esbuild-minify"),
    /target[\\/]tools[\\/]esbuild-minify$/,
  );
});

test("describeProducer records a versioned configuration hash", () => {
  const producer = describeProducer({ pipeline: "swc-minify" });

  assert.deepEqual(producer, {
    name: "swc-minify",
    version: "1.7.26",
    configHash: producer.configHash,
  });
  assert.match(producer.configHash, /^[0-9a-f]{64}$/);
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

test("parseArgs accepts explicit complete baseline updates", () => {
  const options = parseArgs([
    "--preset",
    "operators",
    "--limit",
    "all",
    "--baseline",
    "target/test262-baseline.json",
    "--update-baseline",
  ]);

  assert.match(options.baseline, /target[\\/]test262-baseline\.json$/);
  assert.equal(options.updateBaseline, true);
  assert.deepEqual(options.presets, ["operators"]);
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
      new Error('failed to parse input.js: Error { error: (26..31, Expected("(", "await")) }'),
      [{ name: "sloppy", strict: false }],
      "test/language/expressions/generators/static-init-await-binding.js",
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
      new Error("failed to parse input.js: Error { error: (1..6, TS1109) }"),
      [{ name: "sloppy", strict: false }],
      "test/language/expressions/assignmenttargettype/simple-basic-identifierreference-yield.js",
    ),
    "swc-parse-yield-ident",
  );
  assert.equal(
    knownWakaruParseUnsupportedReason(
      new Error("failed to parse input.js: Error { error: (8..13, TS1109) }"),
      [{ name: "sloppy", strict: false }],
      "test/language/expressions/arrow-function/syntax/arrowparameters-bindingidentifier-yield.js",
    ),
    "swc-parse-yield-arrow-parameter",
  );
  assert.equal(
    knownWakaruParseUnsupportedReason(
      new Error("failed to parse input.js: Error { error: (1..6, TS1109) }"),
      [{ name: "sloppy", strict: false }],
      "test/language/statements/labeled/value-yield-non-strict.js",
    ),
    "swc-parse-yield-label",
  );
  assert.equal(
    knownWakaruParseUnsupportedReason(
      new Error('failed to parse input.js: Error { error: (36..41, Expected("(", "yield")) }'),
      [{ name: "sloppy", strict: false }],
      "test/language/expressions/object/method-definition/yield-as-function-expression-binding-identifier.js",
    ),
    "swc-parse-yield-function-name",
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
      new Error("thread 'main' has overflowed its stack"),
      [{ name: "sloppy", strict: false }],
      "test/language/statements/function/S13.2.1_A1_T1.js",
    ),
    "swc-parse-deep-iife-stack-overflow",
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
      path: "test/language/statements/for-await-of/async-gen-decl-dstr-array-elision-iter-nrml-close.js",
      error: new Error("Test262Error"),
      decompiled: "for await ([] of [iterable]) {}",
    }),
    "swc-array-binding-elision",
  );
  assert.equal(
    knownSwcFidelityIssueReason({
      path: "test/language/expressions/assignment/dstr/array-elision-iter-nrml-close.js",
      error: new Error("Test262Error"),
      decompiled: "[x] = iterable;",
    }),
    "swc-array-binding-elision",
  );
  assert.equal(
    knownSwcFidelityIssueReason({
      path: "test/language/expressions/assignment/dstr/array-iteration.js",
      error: new Error("Test262Error"),
      decompiled: "result = vals;\n[] = vals;",
    }),
    "swc-array-binding-elision",
  );
  assert.equal(
    knownSwcFidelityIssueReason({
      path: "test/language/expressions/arrow-function/dstr/ary-ptrn-elision.js",
      error: new Error("Test262Error"),
      decompiled: "f = ([])=>{};",
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
  assert.equal(
    knownSwcFidelityIssueReason({
      path: "test/language/expressions/arrow-function/throw-new.js",
      error: new Error("SyntaxError: Malformed arrow function parameter list"),
      decompiled: "new ()=>{};",
    }),
    "swc-print-new-arrow-parens",
  );
  assert.equal(
    knownSwcFidelityIssueReason({
      path: "test/language/module-code/instn-named-bndng-dflt-expr.js",
      error: new Error("binding is created but not initialized"),
      decompiled: "export default function() {};",
    }),
    "swc-print-export-default-function-expression",
  );
});

test("knownTransformRejectReason classifies producer transform rejects", () => {
  assert.equal(
    knownTransformRejectReason({
      path: "test/language/module-code/top-level-await/syntax/await-expr.js",
      error: new Error('ERROR: Top-level await is not available in the configured target environment ("es2020")'),
    }),
    "transform-reject-top-level-await",
  );
  assert.equal(
    knownTransformRejectReason({
      path: "test/language/module-code/export-expname-binding-string.js",
      error: new Error('ERROR: Using the string "☿" as an export name is not supported'),
    }),
    "transform-reject-string-export-name",
  );
  assert.equal(
    knownTransformRejectReason({
      path: "test/language/module-code/other.js",
      error: new Error("unexpected producer failure"),
    }),
    null,
  );
});

test("knownTransformedRuntimeRejectReason classifies producer runtime drift", () => {
  assert.equal(
    knownTransformedRuntimeRejectReason({
      path: "test/language/expressions/arrow-function/arrow/binding-tests-1.js",
      error: new Error("This binding initialization was incorrect for arrow capturing this from closure"),
    }),
    "transform-runtime-arrow-this",
  );
  assert.equal(
    knownTransformedRuntimeRejectReason({
      path: "test/language/expressions/arrow-function/dstr/ary-ptrn-elem-id-init-fn-name-class.js",
      error: new Error('Expected SameValue(«"xCls"», «"xCls"») to be false'),
    }),
    "transform-runtime-inferred-name",
  );
  assert.equal(
    knownTransformedRuntimeRejectReason({
      path: "test/language/module-code/eval-export-dflt-expr-fn-anon.js",
      error: new Error('correct name is assigned Expected SameValue(«"stdin_default"», «"default"») to be true'),
    }),
    "transform-runtime-module-default-name",
  );
  assert.equal(
    knownTransformedRuntimeRejectReason({
      path: "test/language/module-code/eval-this.js",
      error: new Error("Expected SameValue(«[object Object]», «undefined») to be true"),
    }),
    "transform-runtime-module-this",
  );
  assert.equal(
    knownTransformedRuntimeRejectReason({
      path: "test/language/statements/with/S12.10_A1.1_T1.js",
      error: new Error("producer changed with environment behavior"),
    }),
    "transform-runtime-with-environment",
  );
  assert.equal(
    knownTransformedRuntimeRejectReason({
      path: "test/language/expressions/other.js",
      error: new Error("unexpected producer runtime failure"),
    }),
    null,
  );
});

test("knownDecompiledRuntimeRejectReason classifies script global lexical redeclaration", () => {
  assert.equal(
    knownDecompiledRuntimeRejectReason({
      path: "test/language/reserved-words/unreserved-words.js",
      error: new Error("SyntaxError: Identifier 'assert' has already been declared"),
      decompiled: "const assert = 1;",
    }),
    "script-global-var-lexical-redeclaration",
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
  assert.match(summary, /- test262Revision: unmanaged/);
  assert.match(summary, /- harnessVersion: unrecorded/);
  assert.match(summary, /- caseTimeoutMs: 5000/);
  assert.match(summary, /\| 3 \| 2 \| 1 \| 0 \| 1 \| 0 \| 1 \|/);
  assert.match(summary, /\| rejected \| swc-array-binding-elision \| 1 \|/);
  assert.match(summary, /- c\.js \(decompiled-runtime\)/);
});

test("reviewed baseline failures pass only while the comparison stays clean", () => {
  assert.equal(
    test262ReportExitCode({
      totals: { failed: 2 },
      baselineComparison: { clean: true },
    }),
    0,
  );
  assert.equal(
    test262ReportExitCode({
      totals: { failed: 0 },
      baselineComparison: { clean: false },
    }),
    1,
  );
  assert.equal(test262ReportExitCode({ totals: { failed: 1 } }), 1);
});

test("formatBaselineComparison identifies changed paths", () => {
  const output = formatBaselineComparison({
    clean: false,
    candidatePath: "baseline.json.new",
    totalsChanged: true,
    newOutcomes: [{ path: "new.js", status: "failed", kind: "runtime" }],
    unexpectedPasses: [{ path: "fixed.js", status: "rejected", kind: "known" }],
  });

  assert.match(output, /\+ new\.js \[failed:runtime\]/);
  assert.match(output, /- fixed\.js \[rejected:known\]/);
  assert.match(output, /candidate: baseline\.json\.new/);
});

test("runRoundTrip reports baseline failures as unsupported inputs", async () => {
  const root = makeTempTest262();
  try {
    const testDir = join(root, "test", "language", "sample");
    mkdirSync(testDir, { recursive: true });
    writeFileSync(
      join(testDir, "baseline-fails.js"),
      "/*---\n---*/\nthrow new Error('host gap');\n",
    );

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

test("runRoundTrip aborts identity mismatches before writing outputs", async () => {
  const root = makeTempTest262();
  try {
    const paths = ["test/language/sample"];
    const testDir = join(root, paths[0]);
    const baselinePath = join(root, "baseline.json");
    const candidatePath = `${baselinePath}.new`;
    const summaryPath = join(root, "summary.md");
    const reportPath = join(root, "report.json");
    mkdirSync(testDir, { recursive: true });
    writeFileSync(join(testDir, "case.js"), "/*---\n---*/\nvoid 0;\n");
    writeFileSync(summaryPath, "reviewed summary\n");
    writeFileSync(reportPath, "reviewed report\n");
    writeFileSync(
      baselinePath,
      `${JSON.stringify(
        {
          schemaVersion: 3,
          test262: { revision: "unmanaged" },
          harness: { version: test262HarnessVersion },
          environment: {
            nodeMajor: Number.parseInt(process.versions.node.split(".")[0], 10) + 1,
          },
          producer: describeProducer({ pipeline: "none" }),
          wakaru: { level: "minimal", caseTimeoutMs: 1000 },
          selection: { presets: ["default"], paths },
          totals: {},
          outcomes: [],
        },
        null,
        2,
      )}\n`,
    );
    const reviewedBaseline = readFileSync(baselinePath, "utf8");
    writeFileSync(candidatePath, "reviewed candidate\n");

    await assert.rejects(
      runRoundTrip({
        test262Root: root,
        paths,
        limit: Number.POSITIVE_INFINITY,
        pipeline: "none",
        transform: "terser",
        terserProfile: "light",
        level: "minimal",
        toolRoot: join(root, "tools"),
        keepTemp: false,
        caseTimeoutMs: 1000,
        baseline: baselinePath,
        summary: summaryPath,
        json: reportPath,
        updateBaseline: false,
        presets: ["default"],
      }),
      /runtime environment mismatch/,
    );

    assert.equal(readFileSync(summaryPath, "utf8"), "reviewed summary\n");
    assert.equal(readFileSync(reportPath, "utf8"), "reviewed report\n");
    assert.equal(readFileSync(baselinePath, "utf8"), reviewedBaseline);
    assert.equal(readFileSync(candidatePath, "utf8"), "reviewed candidate\n");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("runRoundTrip reports malformed metadata without aborting the corpus", async () => {
  const root = makeTempTest262();
  try {
    const testDir = join(root, "test", "language", "sample");
    mkdirSync(testDir, { recursive: true });
    writeFileSync(join(testDir, "bad.js"), "/*---\nflags: [futureFlag]\n---*/\nvoid 0;\n");
    writeFileSync(join(testDir, "dep_FIXTURE.js"), "export const value = 1;\n");

    const report = await runRoundTrip({
      test262Root: root,
      paths: ["test/language/sample"],
      limit: 1,
      pipeline: "none",
      transform: "terser",
      terserProfile: "light",
      level: "minimal",
      toolRoot: join(root, "tools"),
      keepTemp: false,
      caseTimeoutMs: 1000,
    });

    assert.equal(report.totals.failed, 1);
    assert.equal(report.totals.skipped, 1);
    assert.equal(report.results[0].phase, "harness-configuration");
    assert.match(report.results[0].error, /unknown Test262 flag/);
    assert.equal(report.results[1].reason, "fixture");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("runRoundTrip records missing harness includes without aborting", async () => {
  const root = makeTempTest262();
  try {
    const testDir = join(root, "test", "language", "sample");
    mkdirSync(testDir, { recursive: true });
    writeFileSync(
      join(testDir, "missing-include.js"),
      "/*---\nincludes: [does-not-exist.js]\n---*/\nvoid 0;\n",
    );

    const report = await runRoundTrip({
      test262Root: root,
      paths: ["test/language/sample"],
      limit: Number.POSITIVE_INFINITY,
      pipeline: "none",
      transform: "terser",
      terserProfile: "light",
      level: "minimal",
      toolRoot: join(root, "tools"),
      keepTemp: false,
      caseTimeoutMs: 1000,
    });

    assert.equal(report.complete, true);
    assert.equal(report.totals.failed, 1);
    assert.equal(report.results[0].phase, "harness-configuration");
    assert.match(report.results[0].error, /missing Test262 harness file/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("runRoundTrip reports Wakaru decompile timeouts as failures", async () => {
  const root = makeTempTest262();
  const previousWakaru = process.env.WAKARU;
  try {
    const testDir = join(root, "test", "language", "sample");
    const hangingWakaru = join(root, "hanging-wakaru.mjs");
    mkdirSync(testDir, { recursive: true });
    writeFileSync(join(testDir, "case.js"), "/*---\n---*/\nvoid 0;\n");
    writeFileSync(hangingWakaru, "#!/usr/bin/env node\nsetTimeout(() => {}, 10000);\n");
    chmodSync(hangingWakaru, 0o755);
    process.env.WAKARU = hangingWakaru;

    const report = await runRoundTrip({
      test262Root: root,
      paths: ["test/language/sample"],
      limit: Number.POSITIVE_INFINITY,
      pipeline: "none",
      transform: "terser",
      terserProfile: "light",
      level: "minimal",
      toolRoot: join(root, "tools"),
      keepTemp: false,
      caseTimeoutMs: 50,
    });

    assert.equal(report.totals.failed, 1);
    assert.equal(report.totals.rejected, 0);
    assert.equal(report.results[0].phase, "wakaru-timeout");
    assert.equal(test262ReportExitCode(report), 1);
  } finally {
    if (previousWakaru === undefined) delete process.env.WAKARU;
    else process.env.WAKARU = previousWakaru;
    rmSync(root, { recursive: true, force: true });
  }
});

test("runRoundTrip writes incremental JSON and final completion state", async () => {
  const root = makeTempTest262();
  try {
    const testDir = join(root, "test", "language", "sample");
    const reportPath = join(root, "report.json");
    mkdirSync(testDir, { recursive: true });
    writeFileSync(
      join(testDir, "baseline-fails.js"),
      "/*---\n---*/\nthrow new Error('host gap');\n",
    );

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
