#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import {
  existsSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import {
  dirname,
  extname,
  join,
  isAbsolute,
  relative,
  resolve,
  sep,
} from "node:path";
import { pathToFileURL, fileURLToPath } from "node:url";
import vm from "node:vm";

const repoRoot = resolve(fileURLToPath(new URL("../..", import.meta.url)));
const defaultTest262Root = resolve(repoRoot, "..", "test262");
const defaultToolRoot = join(repoRoot, "target", "correctness-tools", "test262-roundtrip");
const defaultKnownBlockersPath = join(repoRoot, "scripts", "correctness", "test262-known-blockers.json");
const defaultRewriteLevel = "minimal";
const defaultTransform = "terser";
const defaultPipeline = null;
const supportedTransforms = new Set(["none", "terser"]);
const supportedPipelines = new Set([
  "none",
  "terser-light",
  "terser-full",
  "babel-env-terser",
  "swc-minify",
  "esbuild-minify",
]);
const supportedLevels = new Set(["minimal", "standard", "aggressive"]);
const terserPackages = [{ name: "terser", spec: "terser@5.31.6" }];
const babelPackages = [
  { name: "@babel/core", spec: "@babel/core@7.25.2" },
  { name: "@babel/preset-env", spec: "@babel/preset-env@7.25.4" },
];
const swcPackages = [{ name: "@swc/core", spec: "@swc/core@1.7.26" }];
const esbuildPackages = [{ name: "esbuild", spec: "esbuild@0.23.1" }];
const defaultPaths = [
  "test/language/expressions/coalesce",
  "test/language/expressions/optional-chaining",
  "test/language/expressions/object",
  "test/language/expressions/array",
  "test/language/statements/for-of",
  "test/language/statements/let",
];
const pathPresets = {
  default: defaultPaths,
  classes: ["test/language/expressions/class", "test/language/statements/class"],
  destructuring: [
    "test/language/expressions/assignment/dstr",
    "test/language/statements/for-of/dstr",
    "test/language/statements/variable/dstr",
  ],
  "async-generators": [
    "test/language/expressions/async-arrow-function",
    "test/language/expressions/async-function",
    "test/language/expressions/async-generator",
    "test/language/expressions/generators",
    "test/language/statements/async-function",
    "test/language/statements/async-generator",
    "test/language/statements/generators",
  ],
  scope: [
    "test/language/statements/block",
    "test/language/statements/const",
    "test/language/statements/function",
    "test/language/expressions/function",
    "test/language/expressions/arrow-function",
    "test/language/statements/with",
  ],
  "control-flow": [
    "test/language/statements/if",
    "test/language/statements/switch",
    "test/language/statements/try",
    "test/language/statements/return",
    "test/language/statements/throw",
    "test/language/statements/break",
    "test/language/statements/continue",
    "test/language/statements/labeled",
    "test/language/statements/for",
    "test/language/statements/for-in",
    "test/language/statements/while",
    "test/language/statements/do-while",
    "test/language/expressions/conditional",
    "test/language/expressions/logical-and",
    "test/language/expressions/logical-or",
    "test/language/expressions/comma",
  ],
  calls: [
    "test/language/expressions/call",
    "test/language/expressions/new",
    "test/language/expressions/member-expression",
    "test/language/expressions/property-accessors",
    "test/language/expressions/this",
    "test/language/expressions/new.target",
  ],
  operators: [
    "test/language/expressions/addition",
    "test/language/expressions/subtraction",
    "test/language/expressions/multiplication",
    "test/language/expressions/division",
    "test/language/expressions/modulus",
    "test/language/expressions/exponentiation",
    "test/language/expressions/bitwise-and",
    "test/language/expressions/bitwise-or",
    "test/language/expressions/bitwise-xor",
    "test/language/expressions/bitwise-not",
    "test/language/expressions/left-shift",
    "test/language/expressions/right-shift",
    "test/language/expressions/unsigned-right-shift",
    "test/language/expressions/logical-not",
    "test/language/expressions/unary-minus",
    "test/language/expressions/unary-plus",
    "test/language/expressions/typeof",
    "test/language/expressions/void",
    "test/language/expressions/delete",
    "test/language/expressions/postfix-decrement",
    "test/language/expressions/postfix-increment",
    "test/language/expressions/prefix-decrement",
    "test/language/expressions/prefix-increment",
    "test/language/expressions/equals",
    "test/language/expressions/does-not-equals",
    "test/language/expressions/strict-equals",
    "test/language/expressions/strict-does-not-equals",
    "test/language/expressions/greater-than",
    "test/language/expressions/greater-than-or-equal",
    "test/language/expressions/less-than",
    "test/language/expressions/less-than-or-equal",
    "test/language/expressions/in",
    "test/language/expressions/instanceof",
    "test/language/expressions/relational",
    "test/language/expressions/assignment",
    "test/language/expressions/compound-assignment",
    "test/language/expressions/logical-assignment",
  ],
  templates: ["test/language/expressions/template-literal", "test/language/expressions/tagged-template"],
  literals: ["test/language/literals"],
  "block-scope-syntax": ["test/language/block-scope/syntax"],
  variables: ["test/language/statements/variable"],
  "assignment-target-type": ["test/language/expressions/assignmenttargettype"],
  "arguments-object": ["test/language/arguments-object"],
  identifiers: ["test/language/identifiers"],
  modules: ["test/language/module-code"],
};

export function parseArgs(argv) {
  const options = {
    test262Root: defaultTest262Root,
    paths: [],
    presets: [],
    limit: 25,
    pipeline: defaultPipeline,
    transform: defaultTransform,
    terserProfile: "light",
    level: defaultRewriteLevel,
    json: null,
    summary: null,
    knownBlockers: defaultKnownBlockersPath,
    caseTimeoutMs: 5_000,
    rerunFrom: null,
    rerunStatuses: [],
    details: false,
    keepTemp: false,
    toolRoot: defaultToolRoot,
  };

  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--test262") {
      options.test262Root = resolve(readRequiredValue(argv, ++i, arg));
    } else if (arg === "--path") {
      options.paths.push(readRequiredValue(argv, ++i, arg));
    } else if (arg === "--preset") {
      options.presets.push(readRequiredValue(argv, ++i, arg));
    } else if (arg === "--limit") {
      options.limit = parseLimit(readRequiredValue(argv, ++i, arg), arg);
    } else if (arg === "--pipeline") {
      options.pipeline = readRequiredValue(argv, ++i, arg);
    } else if (arg === "--transform") {
      options.transform = readRequiredValue(argv, ++i, arg);
    } else if (arg === "--level") {
      options.level = readRequiredValue(argv, ++i, arg);
    } else if (arg === "--terser-profile") {
      options.terserProfile = readRequiredValue(argv, ++i, arg);
    } else if (arg === "--json") {
      options.json = resolve(readRequiredValue(argv, ++i, arg));
    } else if (arg === "--summary") {
      options.summary = resolve(readRequiredValue(argv, ++i, arg));
    } else if (arg === "--known-blockers") {
      options.knownBlockers = resolve(readRequiredValue(argv, ++i, arg));
    } else if (arg === "--case-timeout-ms") {
      options.caseTimeoutMs = parsePositiveInteger(readRequiredValue(argv, ++i, arg), arg);
    } else if (arg === "--rerun-from") {
      options.rerunFrom = resolve(readRequiredValue(argv, ++i, arg));
    } else if (arg === "--rerun-status") {
      options.rerunStatuses.push(readRequiredValue(argv, ++i, arg));
    } else if (arg === "--tool-root") {
      options.toolRoot = resolve(readRequiredValue(argv, ++i, arg));
    } else if (arg === "--details") {
      options.details = true;
    } else if (arg === "--keep-temp") {
      options.keepTemp = true;
    } else if (arg === "--help" || arg === "-h") {
      options.help = true;
    } else if (arg.startsWith("-")) {
      throw new Error(`unknown option: ${arg}`);
    } else {
      options.paths.push(arg);
    }
  }

  if (!supportedTransforms.has(options.transform)) {
    throw new Error(`unsupported --transform ${options.transform}`);
  }
  if (options.pipeline && !supportedPipelines.has(options.pipeline)) {
    throw new Error(`unsupported --pipeline ${options.pipeline}`);
  }
  if (!supportedLevels.has(options.level)) {
    throw new Error(`unsupported --level ${options.level}`);
  }
  if (!["light", "full"].includes(options.terserProfile)) {
    throw new Error(`unsupported --terser-profile ${options.terserProfile}`);
  }
  for (const status of options.rerunStatuses) {
    if (!["failed", "rejected", "unsupported"].includes(status)) {
      throw new Error(`unsupported --rerun-status ${status}`);
    }
  }
  for (const preset of options.presets) {
    if (!Object.hasOwn(pathPresets, preset)) {
      throw new Error(`unsupported --preset ${preset}`);
    }
    options.paths.push(...pathPresets[preset]);
  }
  if (options.paths.length === 0 && !options.rerunFrom) {
    options.paths = [...defaultPaths];
  }
  if (options.rerunFrom && options.rerunStatuses.length === 0) {
    options.rerunStatuses = ["failed"];
  }
  options.paths = [...new Set(options.paths)];
  return options;
}

export function usage() {
  return `Usage:
  node scripts/correctness/test262-roundtrip.mjs [options]

Options:
  --test262 <dir>       Test262 checkout. Default: ../test262
  --path <path>         Test file or directory relative to Test262 root. Repeatable.
  --preset <name>       Named path set: ${Object.keys(pathPresets).join(" | ")}
  --limit <n|all>       Maximum runnable tests to execute. Default: 25
  --pipeline <name>     none | terser-light | terser-full | babel-env-terser | swc-minify | esbuild-minify
  --transform <name>    none | terser. Default: terser
  --terser-profile <p>  light | full. Default: light
  --level <level>       minimal | standard | aggressive. Default: minimal
  --json <file>         Write full JSON report
  --summary <file>      Write deterministic Markdown summary
  --known-blockers <f>  Known non-Wakaru blocker manifest
  --case-timeout-ms <n> Per-test timeout. Default: 5000
  --rerun-from <json>   Run paths from a previous JSON report
  --rerun-status <s>    failed | rejected | unsupported. Repeatable. Default: failed
  --details             Print skip/failure details
  --keep-temp           Keep temporary transformed files
`;
}

export function parseTestMetadata(source) {
  const match = source.match(/\/\*---([\s\S]*?)---\*\//);
  if (!match) {
    return {
      flags: [],
      includes: [],
      features: [],
      negative: null,
      raw: "",
    };
  }

  const raw = match[1];
  return {
    flags: readYamlList(raw, "flags"),
    includes: readYamlList(raw, "includes"),
    features: readYamlList(raw, "features"),
    negative: readYamlBlock(raw, "negative"),
    raw,
  };
}

export function runnableVariants(metadata) {
  if (metadata.flags.includes("raw")) {
    return [];
  }
  if (metadata.flags.includes("module")) {
    return [{ name: "module", strict: true, module: true }];
  }
  if (metadata.flags.includes("async")) {
    return [];
  }
  if (metadata.negative) {
    return [];
  }
  if (metadata.flags.includes("onlyStrict")) {
    return [{ name: "strict", strict: true }];
  }
  if (metadata.flags.includes("noStrict")) {
    return [{ name: "sloppy", strict: false }];
  }
  return [
    { name: "sloppy", strict: false },
    { name: "strict", strict: true },
  ];
}

export function classifyTest(filePath, source, metadata) {
  const normalized = filePath.split(sep).join("/");
  if (normalized.includes("_FIXTURE")) {
    return { runnable: false, reason: "fixture" };
  }
  if (normalized.includes("/intl402/")) {
    return { runnable: false, reason: "intl402" };
  }
  if (metadata.negative) {
    return { runnable: false, reason: "negative" };
  }
  for (const flag of ["raw", "async"]) {
    if (metadata.flags.includes(flag)) {
      return { runnable: false, reason: `flag:${flag}` };
    }
  }
  if (source.includes("$262") || source.includes("detachArrayBuffer")) {
    return { runnable: false, reason: "host-api" };
  }
  if (source.includes("$DONE") || source.includes("print(")) {
    return { runnable: false, reason: "async-or-print" };
  }
  return { runnable: true, reason: null };
}

export function buildHarnessSource(test262Root, metadata) {
  const harnessDir = join(test262Root, "harness");
  const harnessFiles = ["assert.js", "sta.js", ...metadata.includes];
  return harnessFiles
    .map((file) => {
      const path = join(harnessDir, file);
      if (!existsSync(path)) {
        throw new Error(`missing Test262 harness file: ${file}`);
      }
      return `\n// harness/${file}\n${readFileSync(path, "utf8")}\n`;
    })
    .join("\n");
}

export async function executeTestSource({ harnessSource, testSource, filename, strict }) {
  const unhandledRejections = [];
  const onUnhandledRejection = (reason) => {
    unhandledRejections.push(reason);
  };
  process.prependListener("unhandledRejection", onUnhandledRejection);
  const context = createTestContext();
  try {
    vm.runInContext(harnessSource, context, {
      filename: "test262-harness.js",
      timeout: 1000,
    });

    const source = strict ? `"use strict";\n${testSource}` : testSource;
    const result = vm.runInContext(source, context, {
      filename,
      timeout: 1000,
    });
    if (isThenable(result)) {
      await result;
    }
    await new Promise((resolve) => setImmediate(resolve));
    if (unhandledRejections.length > 0) {
      throw unhandledRejections[0];
    }
  } finally {
    process.removeListener("unhandledRejection", onUnhandledRejection);
  }
}

function isThenable(value) {
  return value != null && typeof value.then === "function";
}

export async function minifyWithTerser(source, options, transformOptions = {}) {
  await ensureTerser(options.toolRoot);
  const toolRequire = createRequire(pathToFileURL(join(options.toolRoot, "package.json")));
  const terserModule = await import(pathToFileURL(toolRequire.resolve("terser")).href);
  const result = await terserModule.minify(source, {
    module: transformOptions.module === true,
    format: {
      ascii_only: true,
      comments: false,
    },
    parse: {
      bare_returns: false,
    },
    ...(options.terserProfile === "full"
      ? {
          compress: {
            passes: 2,
          },
          mangle: {
            toplevel: true,
          },
        }
      : {
          compress: false,
          mangle: false,
        }),
  });
  if (result.error) {
    throw result.error;
  }
  if (!result.code) {
    throw new Error("terser produced empty output");
  }
  return `${result.code}\n`;
}

export async function transformWithBabelEnv(source, options, transformOptions = {}) {
  await ensureBabel(options.toolRoot);
  const toolRequire = createRequire(pathToFileURL(join(options.toolRoot, "package.json")));
  const babel = toolRequire("@babel/core");
  const presetEnv = toolRequire("@babel/preset-env");
  const result = await babel.transformAsync(source, {
    babelrc: false,
    configFile: false,
    sourceType: transformOptions.module === true ? "module" : "script",
    comments: false,
    presets: [
      [
        presetEnv,
        {
          bugfixes: true,
          modules: false,
          targets: {
            ie: "11",
          },
        },
      ],
    ],
  });
  if (!result?.code) {
    throw new Error("babel produced empty output");
  }
  return `${result.code}\n`;
}

export async function transformWithSwcMinify(source, options, transformOptions = {}) {
  ensureSwc(options.toolRoot);
  const toolRequire = createRequire(pathToFileURL(join(options.toolRoot, "package.json")));
  const swc = toolRequire("@swc/core");
  const result = await swc.minify(source, {
    compress: false,
    mangle: false,
    format: {
      ascii_only: true,
      comments: false,
    },
    module: transformOptions.module === true,
  });
  if (!result?.code) {
    throw new Error("swc produced empty output");
  }
  return `${result.code}\n`;
}

export async function transformWithEsbuildMinify(source, options, transformOptions = {}) {
  ensureEsbuild(options.toolRoot);
  const toolRequire = createRequire(pathToFileURL(join(options.toolRoot, "package.json")));
  const esbuild = toolRequire("esbuild");
  const result = await esbuild.transform(source, {
    loader: "js",
    format: transformOptions.module === true ? "esm" : "iife",
    minifyWhitespace: true,
    minifySyntax: true,
    minifyIdentifiers: false,
    legalComments: "none",
    target: "es2020",
  });
  if (!result?.code) {
    throw new Error("esbuild produced empty output");
  }
  return `${result.code}\n`;
}

export async function transformSource(source, options, transformOptions = {}) {
  const pipeline = resolvePipelineName(options);
  if (pipeline === "none") {
    return source;
  }
  if (pipeline === "terser-light") {
    return minifyWithTerser(source, { ...options, terserProfile: "light" }, transformOptions);
  }
  if (pipeline === "terser-full") {
    return minifyWithTerser(source, { ...options, terserProfile: "full" }, transformOptions);
  }
  if (pipeline === "babel-env-terser") {
    ensureBabelEnvTerser(options.toolRoot);
    const transpiled = await transformWithBabelEnv(source, options, transformOptions);
    return minifyWithTerser(transpiled, { ...options, terserProfile: "light" }, transformOptions);
  }
  if (pipeline === "swc-minify") {
    return transformWithSwcMinify(source, options, transformOptions);
  }
  if (pipeline === "esbuild-minify") {
    return transformWithEsbuildMinify(source, options, transformOptions);
  }
  throw new Error(`unsupported pipeline: ${pipeline}`);
}

export function resolvePipelineName(options) {
  if (options.pipeline) {
    return options.pipeline;
  }
  if (options.transform === "none") {
    return "none";
  }
  if (options.transform === "terser") {
    return options.terserProfile === "full" ? "terser-full" : "terser-light";
  }
  throw new Error(`unsupported transform: ${options.transform}`);
}

export function discoverTests(test262Root, paths) {
  const files = [];
  const root = resolve(test262Root);
  for (const requestedPath of paths) {
    const absolute = resolve(root, requestedPath);
    const relativeToRoot = relative(root, absolute);
    if (relativeToRoot.startsWith("..") || isAbsolute(relativeToRoot)) {
      throw new Error(`path escapes Test262 root: ${requestedPath}`);
    }
    if (!existsSync(absolute)) {
      throw new Error(`missing Test262 path: ${requestedPath}`);
    }
    collectJsFiles(absolute, files);
  }
  files.sort();
  return files;
}

export async function runRoundTrip(options) {
  const tests = options.rerunFrom
    ? discoverTestsFromReport(options.test262Root, options.rerunFrom, options.rerunStatuses)
    : discoverTests(options.test262Root, options.paths);
  const knownBlockers = loadKnownBlockers(options.knownBlockers ?? defaultKnownBlockersPath);
  const tmpRoot = mkdtempSync(join(tmpdir(), "wakaru-test262-"));
  const report = {
    complete: false,
    options: {
      test262Root: options.test262Root,
      paths: options.paths,
      limit: Number.isFinite(options.limit) ? options.limit : "all",
      pipeline: resolvePipelineName(options),
      transform: options.transform,
      terserProfile: options.terserProfile,
      level: options.level,
      knownBlockers: knownBlockers.path ? relative(repoRoot, knownBlockers.path).split(sep).join("/") : null,
      caseTimeoutMs: options.caseTimeoutMs,
      rerunFrom: options.rerunFrom ? relative(repoRoot, options.rerunFrom).split(sep).join("/") : null,
      rerunStatuses: options.rerunStatuses,
    },
    totals: {
      discovered: tests.length,
      processed: 0,
      skipped: 0,
      unsupported: 0,
      rejected: 0,
      runnable: 0,
      passed: 0,
      failed: 0,
    },
    results: [],
  };
  writeReportOutputs(report, options);

  try {
    for (const filePath of tests) {
      const source = readFileSync(filePath, "utf8");
      const metadata = parseTestMetadata(source);
      const classification = classifyTest(filePath, source, metadata);
      const relativePath = relative(options.test262Root, filePath).split(sep).join("/");

      if (!classification.runnable) {
        report.totals.skipped += 1;
        report.results.push({
          path: relativePath,
          status: "skipped",
          reason: classification.reason,
        });
        report.totals.processed += 1;
        writeReportOutputs(report, options);
        continue;
      }

      if (Number.isFinite(options.limit) && report.totals.runnable >= options.limit) {
        break;
      }

      const variants = runnableVariants(metadata);
      if (variants.length === 0) {
        report.totals.skipped += 1;
        report.results.push({
          path: relativePath,
          status: "skipped",
          reason: "no-runnable-variant",
        });
        report.totals.processed += 1;
        writeReportOutputs(report, options);
        continue;
      }

      report.totals.runnable += 1;
      const harnessSource = buildHarnessSource(options.test262Root, metadata);
      const result = await runOneTestWithTimeout({
        filePath,
        relativePath,
        source,
        harnessSource,
        variants,
        tmpRoot,
        options,
        knownBlockers,
      });
      report.results.push(result);
      report.totals.processed += 1;
      if (result.status === "passed") {
        report.totals.passed += 1;
      } else if (result.status === "unsupported") {
        report.totals.unsupported += 1;
      } else if (result.status === "rejected") {
        report.totals.rejected += 1;
      } else {
        report.totals.failed += 1;
      }
      writeReportOutputs(report, options);
    }
    report.complete = true;
    writeReportOutputs(report, options);
  } finally {
    if (!options.keepTemp) {
      rmSync(tmpRoot, { recursive: true, force: true });
    }
  }

  return report;
}

export function discoverTestsFromReport(test262Root, reportPath, statuses) {
  const report = JSON.parse(readFileSync(reportPath, "utf8"));
  const selected = report.results
    .filter((result) => statuses.includes(result.status))
    .map((result) => result.path);
  return discoverTests(test262Root, [...new Set(selected)]);
}

async function runOneTestWithTimeout(args) {
  const timeoutMs = args.options.caseTimeoutMs;
  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) {
    return runOneTest(args);
  }
  let timer = null;
  try {
    return await Promise.race([
      runOneTest(args),
      new Promise((resolve) => {
        timer = setTimeout(() => {
          resolve(
            rejected(
              args.relativePath,
              "case-timeout",
              new Error(`case timed out after ${timeoutMs}ms`),
              "case-timeout",
            ),
          );
        }, timeoutMs);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

async function runOneTest({
  filePath,
  relativePath,
  source,
  harnessSource,
  variants,
  tmpRoot,
  options,
  knownBlockers,
}) {
  if (variants.some((variant) => variant.module)) {
    return runOneModuleTest({
      filePath,
      relativePath,
      harnessSource,
      tmpRoot,
      options,
      knownBlockers,
    });
  }

  try {
    for (const variant of variants) {
      await executeTestSource({
        harnessSource,
        testSource: source,
        filename: `${relativePath}:${variant.name}:original`,
        strict: variant.strict,
      });
    }
  } catch (error) {
    return unsupported(relativePath, "baseline", error, "node-vm-baseline");
  }

  let transformed;
  try {
    transformed = await transformSource(source, options);
  } catch (error) {
    return rejected(
      relativePath,
      "transform",
      error,
      knownTransformRejectReason({ path: relativePath, error, variants, knownBlockers }) ??
        "transform-reject",
    );
  }

  try {
    for (const variant of variants) {
      await executeTestSource({
        harnessSource,
        testSource: transformed,
        filename: `${relativePath}:${variant.name}:transformed`,
        strict: variant.strict,
      });
    }
  } catch (error) {
    return rejected(
      relativePath,
      "transformed-runtime",
      error,
      knownTransformedRuntimeRejectReason({ path: relativePath, error, variants, knownBlockers }) ??
        "transform-runtime",
    );
  }

  let decompiled;
  try {
    decompiled = runWakaru(transformed, {
      level: options.level,
      tmpRoot,
      basename: relativePath.replaceAll("/", "__"),
      timeoutMs: options.caseTimeoutMs,
    });
  } catch (error) {
    if (isTimeoutError(error)) {
      return rejected(relativePath, "case-timeout", error, "case-timeout");
    }
    const parseUnsupportedReason = knownWakaruParseUnsupportedReason(
      error,
      variants,
      relativePath,
      knownBlockers,
    );
    if (parseUnsupportedReason) {
      return unsupported(relativePath, "wakaru-parse", error, parseUnsupportedReason);
    }
    return failure(relativePath, "wakaru", error, { transformed });
  }

  try {
    for (const variant of variants) {
      await executeTestSource({
        harnessSource,
        testSource: decompiled,
        filename: `${relativePath}:${variant.name}:decompiled`,
        strict: variant.strict,
      });
    }
  } catch (error) {
    const fidelityReason = knownSwcFidelityIssueReason({
      path: relativePath,
      error,
      decompiled,
      knownBlockers,
    });
    if (fidelityReason) {
      return rejected(relativePath, "swc-fidelity", error, fidelityReason, {
        transformed,
        decompiled,
      });
    }
    return failure(relativePath, "decompiled-runtime", error, {
      transformed,
      decompiled,
    });
  }

  return {
    path: relativePath,
    status: "passed",
    variants: variants.map((variant) => variant.name),
  };
}

async function runOneModuleTest({
  filePath,
  relativePath,
  harnessSource,
  tmpRoot,
  options,
  knownBlockers,
}) {
  let originalGraph;
  try {
    originalGraph = readModuleGraph(options.test262Root, filePath);
  } catch (error) {
    return unsupported(relativePath, "baseline", error, "module-graph-baseline");
  }

  try {
    executeModuleGraph({
      harnessSource,
      entryPath: relativePath,
      sources: originalGraph.sources,
      tmpRoot,
      phase: "original",
      timeoutMs: options.caseTimeoutMs,
    });
  } catch (error) {
    return unsupported(relativePath, "baseline", error, "node-module-baseline");
  }

  let transformedSources;
  try {
    transformedSources = new Map();
    for (const [path, moduleSource] of originalGraph.sources) {
      transformedSources.set(path, await transformSource(moduleSource, options, { module: true }));
    }
  } catch (error) {
    return rejected(
      relativePath,
      "transform",
      error,
      knownTransformRejectReason({
        path: relativePath,
        error,
        variants: [{ name: "module", strict: true, module: true }],
        knownBlockers,
      }) ?? "transform-reject",
    );
  }

  try {
    executeModuleGraph({
      harnessSource,
      entryPath: relativePath,
      sources: transformedSources,
      tmpRoot,
      phase: "transformed",
      timeoutMs: options.caseTimeoutMs,
    });
  } catch (error) {
    return rejected(
      relativePath,
      "transformed-runtime",
      error,
      knownTransformedRuntimeRejectReason({
        path: relativePath,
        error,
        variants: [{ name: "module", strict: true, module: true }],
        knownBlockers,
      }) ?? "transform-runtime",
    );
  }

  const decompiledSources = new Map();
  try {
    for (const [path, moduleSource] of transformedSources) {
      decompiledSources.set(
        path,
        runWakaru(moduleSource, {
          level: options.level,
          tmpRoot,
          basename: `${relativePath.replaceAll("/", "__")}__${path.replaceAll("/", "__")}`,
          timeoutMs: options.caseTimeoutMs,
        }),
      );
    }
  } catch (error) {
    if (isTimeoutError(error)) {
      return rejected(relativePath, "case-timeout", error, "case-timeout");
    }
    const parseUnsupportedReason = knownWakaruParseUnsupportedReason(
      error,
      [{ name: "module", strict: true, module: true }],
      relativePath,
      knownBlockers,
    );
    if (parseUnsupportedReason) {
      return unsupported(relativePath, "wakaru-parse", error, parseUnsupportedReason);
    }
    return failure(relativePath, "wakaru", error, {
      transformed: Object.fromEntries(transformedSources),
    });
  }

  try {
    executeModuleGraph({
      harnessSource,
      entryPath: relativePath,
      sources: decompiledSources,
      tmpRoot,
      phase: "decompiled",
      timeoutMs: options.caseTimeoutMs,
    });
  } catch (error) {
    const decompiled = decompiledSources.get(relativePath) ?? "";
    const fidelityReason = knownSwcFidelityIssueReason({
      path: relativePath,
      error,
      decompiled,
      knownBlockers,
    });
    if (fidelityReason) {
      return rejected(relativePath, "swc-fidelity", error, fidelityReason, {
        transformed: Object.fromEntries(transformedSources),
        decompiled: Object.fromEntries(decompiledSources),
      });
    }
    return failure(relativePath, "decompiled-runtime", error, {
      transformed: Object.fromEntries(transformedSources),
      decompiled: Object.fromEntries(decompiledSources),
    });
  }

  return {
    path: relativePath,
    status: "passed",
    variants: ["module"],
    modules: originalGraph.sources.size,
  };
}

function failure(path, phase, error, extra = {}) {
  return {
    path,
    status: "failed",
    phase,
    error: formatError(error),
    ...extra,
  };
}

function unsupported(path, phase, error, reason = null) {
  return {
    path,
    status: "unsupported",
    phase,
    reason,
    error: formatError(error),
  };
}

function rejected(path, phase, error, reason = null, extra = {}) {
  return {
    path,
    status: "rejected",
    phase,
    reason,
    error: formatError(error),
    ...extra,
  };
}

export function isSloppyOnlyWakaruParseUnsupported(error, variants) {
  return knownWakaruParseUnsupportedReason(error, variants, "") === "sloppy-only-strict-ident";
}

export function knownWakaruParseUnsupportedReason(
  error,
  variants,
  path,
  knownBlockers = defaultKnownBlockers(),
) {
  return classifyKnownBlocker({
    knownBlockers,
    status: "unsupported",
    phase: "wakaru-parse",
    path,
    error,
    variants,
  });
}

export function knownSwcFidelityIssueReason({
  path,
  error,
  decompiled,
  knownBlockers = defaultKnownBlockers(),
}) {
  return classifyKnownBlocker({
    knownBlockers,
    status: "rejected",
    phase: "swc-fidelity",
    path,
    error,
    decompiled,
  });
}

export function knownTransformRejectReason({
  path,
  error,
  variants = [],
  knownBlockers = defaultKnownBlockers(),
}) {
  return classifyKnownBlocker({
    knownBlockers,
    status: "rejected",
    phase: "transform",
    path,
    error,
    variants,
  });
}

export function knownTransformedRuntimeRejectReason({
  path,
  error,
  variants = [],
  knownBlockers = defaultKnownBlockers(),
}) {
  return classifyKnownBlocker({
    knownBlockers,
    status: "rejected",
    phase: "transformed-runtime",
    path,
    error,
    variants,
  });
}

let cachedDefaultKnownBlockers = null;

function defaultKnownBlockers() {
  cachedDefaultKnownBlockers ??= loadKnownBlockers(defaultKnownBlockersPath);
  return cachedDefaultKnownBlockers;
}

export function loadKnownBlockers(path) {
  if (!path) {
    return { path: null, entries: [] };
  }
  const normalizedPath = resolve(path);
  const manifest = JSON.parse(readFileSync(normalizedPath, "utf8"));
  if (manifest.version !== 1 || !Array.isArray(manifest.entries)) {
    throw new Error(`invalid known blocker manifest: ${path}`);
  }
  return {
    path: normalizedPath,
    entries: manifest.entries.map((entry) => validateKnownBlockerEntry(entry, path)),
  };
}

function validateKnownBlockerEntry(entry, path) {
  if (!entry || typeof entry !== "object") {
    throw new Error(`invalid known blocker entry in ${path}`);
  }
  for (const key of ["reason", "status", "phase", "when"]) {
    if (!entry[key]) {
      throw new Error(`known blocker entry missing ${key} in ${path}`);
    }
  }
  return entry;
}

export function classifyKnownBlocker({
  knownBlockers,
  status,
  phase,
  path,
  error = null,
  decompiled = "",
  variants = [],
}) {
  const normalizedPath = path.split("\\").join("/");
  const message = error ? formatError(error) : "";
  for (const entry of knownBlockers.entries) {
    if (entry.status !== status || entry.phase !== phase) {
      continue;
    }
    if (matchesKnownBlocker(entry.when, {
      path: normalizedPath,
      error: message,
      decompiled,
      variants,
    })) {
      return entry.reason;
    }
  }
  return null;
}

function matchesKnownBlocker(when, context) {
  if (when.variants === "sloppy-only" && !isSloppyOnly(context.variants)) {
    return false;
  }
  if (!allIncludes(context.path, when.pathIncludes)) {
    return false;
  }
  if (!anyStartsWith(context.path, when.pathStartsWith)) {
    return false;
  }
  if (!allIncludes(context.error, when.errorIncludes)) {
    return false;
  }
  if (!allRegexMatch(context.path, when.pathRegex)) {
    return false;
  }
  if (!allRegexMatch(context.error, when.errorRegex)) {
    return false;
  }
  if (!allRegexMatch(context.decompiled, when.decompiledRegex)) {
    return false;
  }
  return true;
}

function isSloppyOnly(variants) {
  return variants.length > 0 && variants.every((variant) => !variant.strict);
}

function allIncludes(value, needles) {
  return !needles || needles.every((needle) => value.includes(needle));
}

function anyStartsWith(value, prefixes) {
  return !prefixes || prefixes.some((prefix) => value.startsWith(prefix));
}

function allRegexMatch(value, patterns) {
  return !patterns || patterns.every((pattern) => new RegExp(pattern).test(value));
}

function runWakaru(source, { level, tmpRoot, basename, timeoutMs }) {
  const input = join(tmpRoot, `${basename}.js`);
  writeFileSync(input, source);

  const configured = process.env.WAKARU;
  if (configured) {
    return runChecked(configured, ["--level", level, input], { timeoutMs }).stdout;
  }

  const debugBinary = join(repoRoot, "target", "debug", process.platform === "win32" ? "wakaru.exe" : "wakaru");
  if (existsSync(debugBinary)) {
    return runChecked(debugBinary, ["--level", level, input], { timeoutMs }).stdout;
  }

  throw new Error(
    `missing wakaru binary: run "cargo build -p wakaru-cli" first, or set WAKARU to a wakaru executable`,
  );
}

async function ensureTerser(toolRoot) {
  ensureToolPackages(toolRoot, terserPackages);
}

function ensureBabel(toolRoot) {
  ensureBabelPackages(toolRoot, babelPackages);
}

function ensureBabelEnvTerser(toolRoot) {
  ensureBabelPackages(toolRoot, [...babelPackages, ...terserPackages]);
}

function ensureBabelPackages(toolRoot, packages) {
  ensureToolPackages(toolRoot, packages);
  try {
    assertBabelUsable(toolRoot);
  } catch {
    rmSync(join(toolRoot, "node_modules"), { recursive: true, force: true });
    rmSync(join(toolRoot, "package-lock.json"), { force: true });
    ensureToolPackages(toolRoot, packages);
    assertBabelUsable(toolRoot);
  }
}

function ensureSwc(toolRoot) {
  ensureToolPackages(toolRoot, swcPackages);
  try {
    assertSwcUsable(toolRoot);
  } catch {
    rmSync(join(toolRoot, "node_modules"), { recursive: true, force: true });
    rmSync(join(toolRoot, "package-lock.json"), { force: true });
    ensureToolPackages(toolRoot, swcPackages);
    assertSwcUsable(toolRoot);
  }
}

function ensureEsbuild(toolRoot) {
  ensureToolPackages(toolRoot, esbuildPackages);
  try {
    assertEsbuildUsable(toolRoot);
  } catch {
    rmSync(join(toolRoot, "node_modules"), { recursive: true, force: true });
    rmSync(join(toolRoot, "package-lock.json"), { force: true });
    ensureToolPackages(toolRoot, esbuildPackages);
    assertEsbuildUsable(toolRoot);
  }
}

function ensureToolPackages(toolRoot, packages) {
  const missing = missingToolPackageSpecs(toolRoot, packages);
  if (missing.length === 0) {
    return;
  }

  const packageJson = join(toolRoot, "package.json");
  mkdirSync(toolRoot, { recursive: true });
  if (!existsSync(packageJson)) {
    writeFileSync(
      packageJson,
      JSON.stringify(
        {
          private: true,
          type: "module",
          dependencies: {},
        },
        null,
        2,
      ),
    );
  }
  try {
    installToolPackages(toolRoot, packages);
  } catch (error) {
    rmSync(join(toolRoot, "node_modules"), { recursive: true, force: true });
    rmSync(join(toolRoot, "package-lock.json"), { force: true });
    installToolPackages(toolRoot, packages);
  }
}

export function missingToolPackageSpecs(toolRoot, packages) {
  const packageJson = join(toolRoot, "package.json");
  const toolRequire = existsSync(packageJson)
    ? createRequire(pathToFileURL(packageJson))
    : null;
  return packages.filter(({ name }) => {
    if (!toolRequire) {
      return true;
    }
    try {
      toolRequire.resolve(name);
      return false;
    } catch {
      return true;
    }
  });
}

function installToolPackages(toolRoot, packages) {
  runChecked("npm", ["install", "--silent", "--no-save", ...packages.map(({ spec }) => spec)], {
    cwd: toolRoot,
  });
}

function assertBabelUsable(toolRoot) {
  const toolRequire = createRequire(pathToFileURL(join(toolRoot, "package.json")));
  const babel = toolRequire("@babel/core");
  const presetEnv = toolRequire("@babel/preset-env");
  const result = babel.transformSync("let value = input ?? 1;", {
    babelrc: false,
    configFile: false,
    sourceType: "script",
    presets: [[presetEnv, { modules: false, targets: { ie: "11" } }]],
  });
  if (!result?.code) {
    throw new Error("babel validation produced empty output");
  }
}

function assertSwcUsable(toolRoot) {
  const toolRequire = createRequire(pathToFileURL(join(toolRoot, "package.json")));
  const swc = toolRequire("@swc/core");
  const result = swc.minifySync("let value = input ?? 1;", {
    compress: false,
    mangle: false,
  });
  if (!result?.code) {
    throw new Error("swc validation produced empty output");
  }
}

function assertEsbuildUsable(toolRoot) {
  const toolRequire = createRequire(pathToFileURL(join(toolRoot, "package.json")));
  const esbuild = toolRequire("esbuild");
  const result = esbuild.transformSync("let value = input ?? 1;", {
    loader: "js",
    minifyWhitespace: true,
  });
  if (!result?.code) {
    throw new Error("esbuild validation produced empty output");
  }
}

function createTestContext() {
  const context = {
    console,
    setTimeout,
    clearTimeout,
    setImmediate,
    clearImmediate,
  };
  context.print = () => {};
  context.globalThis = context;
  context.$262 = {
    global: context,
    evalScript(source) {
      return vm.runInContext(source, vmContext, { timeout: 1000 });
    },
    createRealm() {
      const realm = createTestContext();
      return realm.$262;
    },
    gc() {
      throw new Error("$262.gc is not available in this runner");
    },
  };
  const vmContext = vm.createContext(context);
  return vmContext;
}

function collectJsFiles(path, files) {
  const stat = statSync(path);
  if (stat.isFile()) {
    if (extname(path) === ".js") {
      files.push(path);
    }
    return;
  }
  for (const entry of readdirSync(path)) {
    collectJsFiles(join(path, entry), files);
  }
}

function readYamlList(raw, key) {
  const inline = raw.match(new RegExp(`^${escapeRegExp(key)}:\\s*\\[([^\\]]*)\\]`, "m"));
  if (inline) {
    return inline[1]
      .split(",")
      .map((item) => item.trim().replace(/^['"]|['"]$/g, ""))
      .filter(Boolean);
  }

  const block = raw.match(new RegExp(`^${escapeRegExp(key)}:\\s*\\n((?:\\s*-\\s*[^\\n]+\\n?)+)`, "m"));
  if (!block) {
    return [];
  }
  return block[1]
    .split(/\r?\n/)
    .map((line) => line.match(/^\s*-\s*(.+)$/)?.[1]?.trim().replace(/^['"]|['"]$/g, ""))
    .filter(Boolean);
}

function readYamlBlock(raw, key) {
  const match = raw.match(new RegExp(`^${escapeRegExp(key)}:\\s*(?:\\n|$)`, "m"));
  return match ? true : null;
}

function runChecked(command, args, options = {}) {
  const result = spawnForPlatform(command, args, {
    cwd: options.cwd ?? repoRoot,
    input: options.input,
    encoding: "utf8",
    maxBuffer: 20 * 1024 * 1024,
    timeout: options.timeoutMs,
  });
  if (result.error) {
    throw new Error(`${command} ${args.join(" ")} failed: ${result.error.message}`);
  }
  if (result.status !== 0) {
    throw new Error(
      `${command} ${args.join(" ")} failed with exit ${result.status}\n${result.stderr || result.stdout}`,
    );
  }
  return {
    stdout: result.stdout,
    stderr: result.stderr,
  };
}

function spawnForPlatform(command, args, options) {
  const result = spawnSync(command, args, options);
  if (result.error?.code === "ENOENT" && process.platform === "win32" && !command.endsWith(".cmd")) {
    return spawnSync("cmd.exe", ["/d", "/s", "/c", `${command}.cmd`, ...args], options);
  }
  return result;
}

function formatError(error) {
  if (error && typeof error.stack === "string") {
    return error.stack.split(/\r?\n/).slice(0, 8).join("\n");
  }
  return String(error);
}

function isTimeoutError(error) {
  const message = formatError(error);
  return /\bETIMEDOUT\b/.test(message) || /spawnSync .* ETIMEDOUT/.test(message);
}

export function formatMarkdownSummary(report) {
  const lines = [
    "# Test262 Round-Trip Summary",
    "",
    "## Options",
    "",
    `- complete: ${report.complete}`,
    `- paths: ${report.options.paths.join(", ")}`,
    `- limit: ${report.options.limit}`,
    `- pipeline: ${report.options.pipeline}`,
    `- transform: ${report.options.transform}`,
    `- terserProfile: ${report.options.terserProfile}`,
    `- level: ${report.options.level}`,
    `- knownBlockers: ${report.options.knownBlockers ?? "none"}`,
    `- caseTimeoutMs: ${report.options.caseTimeoutMs}`,
    `- rerunFrom: ${report.options.rerunFrom ?? "none"}`,
    `- rerunStatuses: ${(report.options.rerunStatuses ?? []).join(", ") || "none"}`,
    "",
    "## Totals",
    "",
    "| Discovered | Runnable | Skipped | Unsupported | Rejected | Passed | Failed |",
    "|---:|---:|---:|---:|---:|---:|---:|",
    `| ${report.totals.discovered} | ${report.totals.runnable} | ${report.totals.skipped} | ${report.totals.unsupported} | ${report.totals.rejected} | ${report.totals.passed} | ${report.totals.failed} |`,
    "",
    "## Reasons",
    "",
  ];

  const reasonCounts = summarizeReasons(report.results);
  if (reasonCounts.length === 0) {
    lines.push("No unsupported, rejected, or skipped reasons recorded.", "");
  } else {
    lines.push("| Status | Reason | Count |", "|---|---|---:|");
    for (const item of reasonCounts) {
      lines.push(`| ${item.status} | ${item.reason} | ${item.count} |`);
    }
    lines.push("");
  }

  const failed = report.results.filter((result) => result.status === "failed");
  lines.push("## Failures", "");
  if (failed.length === 0) {
    lines.push("No Wakaru correctness failures.", "");
  } else {
    for (const result of failed) {
      lines.push(`- ${result.path} (${result.phase})`);
    }
    lines.push("");
  }

  return `${lines.join("\n")}\n`;
}

function summarizeReasons(results) {
  const counts = new Map();
  for (const result of results) {
    if (!["skipped", "unsupported", "rejected"].includes(result.status)) {
      continue;
    }
    const reason = result.reason ?? result.phase ?? "unknown";
    const key = `${result.status}\0${reason}`;
    counts.set(key, (counts.get(key) ?? 0) + 1);
  }
  return [...counts.entries()]
    .map(([key, count]) => {
      const [status, reason] = key.split("\0");
      return { status, reason, count };
    })
    .sort((a, b) => a.status.localeCompare(b.status) || a.reason.localeCompare(b.reason));
}

function writeReportOutputs(report, options) {
  if (options.json) {
    mkdirSync(dirname(options.json), { recursive: true });
    writeFileSync(options.json, `${JSON.stringify(report, null, 2)}\n`);
  }
  if (options.summary) {
    mkdirSync(dirname(options.summary), { recursive: true });
    writeFileSync(options.summary, formatMarkdownSummary(report));
  }
}

function printReport(report, details) {
  console.log("# Test262 round-trip");
  console.log(`discovered: ${report.totals.discovered}`);
  console.log(`runnable: ${report.totals.runnable}`);
  console.log(`skipped: ${report.totals.skipped}`);
  console.log(`unsupported: ${report.totals.unsupported}`);
  console.log(`rejected: ${report.totals.rejected}`);
  console.log(`passed: ${report.totals.passed}`);
  console.log(`failed: ${report.totals.failed}`);
  console.log("");

  for (const result of report.results) {
    if (result.status === "skipped" && !details) {
      continue;
    }
    if (result.status === "passed") {
      console.log(`PASS ${result.path} [${result.variants.join(", ")}]`);
    } else if (result.status === "skipped") {
      console.log(`SKIP ${result.path} (${result.reason})`);
    } else if (result.status === "unsupported") {
      console.log(`UNSUPPORTED ${result.path} (${formatPhase(result)})`);
      if (details) {
        console.log(indent(result.error));
      }
    } else if (result.status === "rejected") {
      console.log(`REJECT ${result.path} (${formatPhase(result)})`);
      if (details) {
        console.log(indent(result.error));
      }
    } else {
      console.log(`FAIL ${result.path} (${result.phase})`);
      if (details) {
        console.log(indent(result.error));
      }
    }
  }
}

function formatPhase(result) {
  return result.reason ? `${result.phase}:${result.reason}` : result.phase;
}

function indent(text) {
  return String(text)
    .split(/\r?\n/)
    .map((line) => `  ${line}`)
    .join("\n");
}

export function readModuleGraph(test262Root, entryPath) {
  const root = resolve(test262Root);
  const entryAbsolute = resolve(entryPath);
  const sources = new Map();
  const visiting = new Set();

  function visit(filePath) {
    const absolute = resolve(filePath);
    const relativeToRoot = relative(root, absolute);
    if (relativeToRoot.startsWith("..") || isAbsolute(relativeToRoot)) {
      throw new Error(`module import escapes Test262 root: ${absolute}`);
    }
    const normalizedPath = relativeToRoot.split(sep).join("/");
    if (sources.has(normalizedPath)) {
      return;
    }
    if (visiting.has(normalizedPath)) {
      return;
    }
    if (!existsSync(absolute)) {
      throw new Error(`missing module import: ${normalizedPath}`);
    }

    visiting.add(normalizedPath);
    const source = readFileSync(absolute, "utf8");
    sources.set(normalizedPath, source);
    for (const specifier of collectStaticModuleSpecifiers(source)) {
      if (!specifier.startsWith(".") && !specifier.startsWith("/")) {
        throw new Error(`unsupported bare module import ${specifier} in ${normalizedPath}`);
      }
      visit(resolveModuleSpecifier(absolute, specifier));
    }
    visiting.delete(normalizedPath);
  }

  visit(entryAbsolute);
  return { entryPath: relative(root, entryAbsolute).split(sep).join("/"), sources };
}

export function collectStaticModuleSpecifiers(source) {
  const specifiers = [];
  const patterns = [
    /\bimport\s*(?:["']([^"']+)["']|(?:[\s\S]*?)\s+from\s*["']([^"']+)["'])/g,
    /\bexport\s+(?:[\s\S]*?)\s+from\s*["']([^"']+)["']/g,
  ];
  for (const pattern of patterns) {
    for (const match of source.matchAll(pattern)) {
      specifiers.push(match[1] ?? match[2]);
    }
  }
  return [...new Set(specifiers)];
}

function resolveModuleSpecifier(fromPath, specifier) {
  const basePath = specifier.startsWith("/")
    ? resolve(specifier.slice(1))
    : resolve(dirname(fromPath), specifier);
  if (existsSync(basePath)) {
    return basePath;
  }
  if (!extname(basePath) && existsSync(`${basePath}.js`)) {
    return `${basePath}.js`;
  }
  return basePath;
}

export function executeModuleGraph({ harnessSource, entryPath, sources, tmpRoot, phase, timeoutMs }) {
  const graphRoot = join(tmpRoot, `module-${phase}-${sanitizePathForFile(entryPath)}`);
  rmSync(graphRoot, { recursive: true, force: true });
  mkdirSync(graphRoot, { recursive: true });
  writeFileSync(join(graphRoot, "package.json"), "{\"type\":\"module\"}\n");
  for (const [path, source] of sources) {
    const outputPath = join(graphRoot, path);
    mkdirSync(dirname(outputPath), { recursive: true });
    writeFileSync(outputPath, source);
  }

  const bootstrapPath = join(graphRoot, "__wakaru_module_bootstrap__.mjs");
  writeFileSync(
    bootstrapPath,
    `globalThis.print = () => {};\n` +
      `globalThis.$262 = {\n` +
      `  global: globalThis,\n` +
      `  evalScript(source) { return (0, eval)(source); },\n` +
      `  createRealm() { return globalThis.$262; },\n` +
      `  gc() { throw new Error("$262.gc is not available in this runner"); }\n` +
      `};\n` +
      `(0, eval)(${JSON.stringify(harnessSource)});\n` +
      `await import(${JSON.stringify(pathToFileURL(join(graphRoot, entryPath)).href)});\n`,
  );

  return runChecked("node", [bootstrapPath], {
    cwd: graphRoot,
    timeoutMs,
  });
}

function sanitizePathForFile(path) {
  return path.replace(/[^A-Za-z0-9._-]+/g, "__");
}

function readRequiredValue(argv, index, option) {
  const value = argv[index];
  if (!value || value.startsWith("-")) {
    throw new Error(`${option} requires a value`);
  }
  return value;
}

function parseLimit(value, option) {
  if (value === "all") {
    return Number.POSITIVE_INFINITY;
  }
  const parsed = Number.parseInt(value, 10);
  if (!Number.isInteger(parsed) || parsed < 1) {
    throw new Error(`${option} must be a positive integer or all`);
  }
  return parsed;
}

function parsePositiveInteger(value, option) {
  const parsed = Number.parseInt(value, 10);
  if (!Number.isInteger(parsed) || parsed < 1) {
    throw new Error(`${option} must be a positive integer`);
  }
  return parsed;
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function isMain() {
  return process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url);
}

if (isMain()) {
  try {
    const options = parseArgs(process.argv.slice(2));
    if (options.help) {
      console.log(usage());
      process.exitCode = 0;
    } else {
      const report = await runRoundTrip(options);
      printReport(report, options.details);
      writeReportOutputs(report, options);
      process.exitCode = report.totals.failed === 0 ? 0 : 1;
    }
  } catch (error) {
    console.error(formatError(error));
    process.exitCode = 1;
  }
}
