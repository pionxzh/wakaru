#!/usr/bin/env node

import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
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
import { cpus, tmpdir } from "node:os";
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

import {
  assertPinnedTest262Corpus,
  defaultManagedTest262Root,
  readTest262Revision,
} from "./test262-corpus.mjs";
import {
  parseTestMetadata,
  runnableVariants,
} from "./test262-metadata.mjs";
import {
  applyTest262Baseline,
  validateTest262BaselineOptions,
} from "./test262-baseline.mjs";

export { parseTestMetadata, runnableVariants } from "./test262-metadata.mjs";

const repoRoot = resolve(fileURLToPath(new URL("../..", import.meta.url)));
const defaultTest262Root = defaultManagedTest262Root;
const defaultToolRoot = join(repoRoot, "target", "correctness-tools", "test262-roundtrip");
const defaultKnownBlockersPath = join(repoRoot, "scripts", "correctness", "test262-known-blockers.json");
const defaultRewriteLevel = "minimal";
export const test262HarnessVersion = 2;
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
const validateToolOnce = createToolValidationCache();
const producerDefinitions = {
  none: {
    version: "builtin",
    config: { transform: "none" },
  },
  "terser-light": {
    version: "5.31.6",
    config: { compress: false, mangle: false, asciiOnly: true, comments: false },
  },
  "terser-full": {
    version: "5.31.6",
    config: { compress: { passes: 2 }, mangle: { toplevel: true }, asciiOnly: true },
  },
  "babel-env-terser": {
    version: "babel-7.25.2+preset-env-7.25.4+terser-5.31.6",
    config: { babelTargets: { ie: "11" }, bugfixes: true, modules: false, terser: "light" },
  },
  "swc-minify": {
    version: "1.7.26",
    config: { compress: false, mangle: false, asciiOnly: true, comments: false },
  },
  "esbuild-minify": {
    version: "0.23.1",
    config: {
      minifyWhitespace: true,
      minifySyntax: true,
      minifyIdentifiers: false,
      target: "es2020",
    },
  },
};
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
    "test/language/expressions/await",
    "test/language/expressions/generators",
    "test/language/statements/async-function",
    "test/language/statements/async-generator",
    "test/language/statements/for-await-of",
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
  "function-code": ["test/language/function-code"],
  asi: ["test/language/asi"],
  keywords: ["test/language/keywords"],
  "reserved-words": ["test/language/reserved-words"],
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
    baseline: null,
    updateBaseline: false,
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
    } else if (arg === "--baseline") {
      options.baseline = resolve(readRequiredValue(argv, ++i, arg));
    } else if (arg === "--update-baseline") {
      options.updateBaseline = true;
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
  validateTest262BaselineOptions(options);
  return options;
}

export function usage() {
  return `Usage:
  node scripts/correctness/test262-roundtrip.mjs [options]

Options:
  --test262 <dir>       Test262 checkout. Default: managed pinned checkout
  --path <path>         Test file or directory relative to Test262 root. Repeatable.
  --preset <name>       Named path set: ${Object.keys(pathPresets).join(" | ")}
  --limit <n|all>       Maximum runnable tests to execute. Default: 25
  --pipeline <name>     none | terser-light | terser-full | babel-env-terser | swc-minify | esbuild-minify
  --transform <name>    none | terser. Default: terser
  --terser-profile <p>  light | full. Default: light
  --level <level>       minimal | standard | aggressive. Default: minimal
  --json <file>         Write full JSON report
  --summary <file>      Write deterministic Markdown summary
  --baseline <file>     Compare against a canonical per-case JSON baseline
  --update-baseline     Explicitly replace the selected complete baseline
  --known-blockers <f>  Known non-Wakaru blocker manifest
  --case-timeout-ms <n> Per-test timeout. Default: 5000
  --rerun-from <json>   Run paths from a previous JSON report
  --rerun-status <s>    failed | rejected | unsupported. Repeatable. Default: failed
  --details             Print skip/failure details
  --keep-temp           Keep temporary transformed files
`;
}

export function classifyTest(filePath, source, metadata) {
  const pathClassification = classifyTestPath(filePath);
  if (!pathClassification.runnable) {
    return pathClassification;
  }
  return { runnable: true, reason: null };
}

export function classifyTestPath(filePath) {
  const normalized = filePath.split(sep).join("/");
  if (normalized.includes("_FIXTURE")) {
    return { runnable: false, reason: "fixture" };
  }
  if (normalized.includes("/intl402/")) {
    return { runnable: false, reason: "intl402" };
  }
  return { runnable: true, reason: null };
}

export function buildHarnessSource(test262Root, metadata) {
  const harnessDir = join(test262Root, "harness");
  const harnessFiles = [
    ...(metadata.flags?.includes("raw") ? [] : ["assert.js", "sta.js"]),
    ...(metadata.includes ?? []),
  ];
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

export async function executeTestSource({
  harnessSource,
  testSource,
  filename,
  strict,
  timeoutMs = 1000,
  async = false,
}) {
  const outcome = await executeTestSourceOutcome({
    harnessSource,
    testSource,
    filename,
    strict,
    timeoutMs,
    async,
  });
  if (outcome.phase !== "success") {
    throw outcome.error;
  }
}

export async function executeTestSourceOutcome({
  harnessSource,
  testSource,
  filename,
  strict,
  timeoutMs = 1000,
  async = false,
}) {
  const unhandledRejections = [];
  const onUnhandledRejection = (reason) => {
    unhandledRejections.push(reason);
  };
  process.prependListener("unhandledRejection", onUnhandledRejection);
  let resolveDone;
  let rejectDone;
  const donePromise = async
    ? new Promise((resolvePromise, rejectPromise) => {
        resolveDone = resolvePromise;
        rejectDone = rejectPromise;
      })
    : null;
  const realm = createTestContext({
    timeoutMs,
    onPrint(message) {
      if (!async) return;
      if (message === "Test262:AsyncTestComplete") {
        resolveDone();
      } else if (message.startsWith("Test262:AsyncTestFailure:")) {
        rejectDone(new Error(message));
      }
    },
  });
  const { context } = realm;
  if (async) {
    context.$DONE = (error) => {
      if (error == null) {
        resolveDone();
      } else {
        rejectDone(error instanceof Error ? error : new Error(String(error)));
      }
    };
  }
  try {
    try {
      vm.runInContext(harnessSource, context, {
        filename: "test262-harness.js",
        timeout: timeoutMs,
      });
    } catch (error) {
      return executionOutcome("harness", error);
    }

    const source = strict ? `"use strict";\n${testSource}` : testSource;
    let script;
    try {
      script = new vm.Script(source, { filename });
    } catch (error) {
      return executionOutcome("parse", error);
    }
    try {
      const result = script.runInContext(context, { timeout: timeoutMs });
      if (isThenable(result)) {
        await withPromiseTimeout(Promise.resolve(result), timeoutMs);
      }
      if (async) {
        await withPromiseTimeout(donePromise, timeoutMs);
      }
      await new Promise((resolvePromise) => setImmediate(resolvePromise));
      if (unhandledRejections.length > 0) {
        throw unhandledRejections[0];
      }
      return { phase: "success", error: null, errorName: null };
    } catch (error) {
      return executionOutcome("runtime", error);
    }
  } finally {
    process.removeListener("unhandledRejection", onUnhandledRejection);
    realm.cleanup();
  }
}

function executionOutcome(phase, error) {
  return {
    phase,
    error,
    errorName: error?.name ?? error?.constructor?.name ?? typeof error,
  };
}

function withPromiseTimeout(promise, timeoutMs) {
  let handle;
  const timeout = new Promise((_, rejectPromise) => {
    handle = setTimeout(
      () => rejectPromise(new Error(`async test timed out after ${timeoutMs}ms`)),
      timeoutMs,
    );
  });
  return Promise.race([promise, timeout]).finally(() => clearTimeout(handle));
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

export function resolvePipelineToolRoot(toolRoot, pipeline) {
  return join(resolve(toolRoot), pipeline);
}

export function describeProducer(options) {
  const name = resolvePipelineName(options);
  const definition = producerDefinitions[name];
  if (!definition) {
    throw new Error(`missing producer definition for ${name}`);
  }
  return {
    name,
    version: definition.version,
    configHash: createHash("sha256")
      .update(JSON.stringify(definition.config))
      .digest("hex"),
  };
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

function defaultConcurrency() {
  return Math.max(1, Math.min(16, (cpus().length || 4) - 2));
}

async function runPool(items, worker, concurrency = defaultConcurrency()) {
  let cursor = 0;
  const run = async () => {
    while (cursor < items.length) {
      const index = cursor++;
      await worker(items[index], index);
    }
  };
  await Promise.all(Array.from({ length: Math.min(concurrency, items.length) }, run));
}

function withTimeout(promise, timeoutMs, relativePath) {
  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) return promise;
  let timer;
  return Promise.race([
    promise,
    new Promise((resolve) => {
      timer = setTimeout(() => {
        resolve({
          result: rejected(
            relativePath,
            "case-timeout",
            new Error(`case timed out after ${timeoutMs}ms`),
            "case-timeout",
          ),
        });
      }, timeoutMs);
    }),
  ]).finally(() => clearTimeout(timer));
}

function resolveWakaruCmd() {
  const configured = process.env.WAKARU;
  if (configured) {
    return { command: configured, prefix: [] };
  }
  const debugBinary = join(
    repoRoot,
    "target",
    "debug",
    process.platform === "win32" ? "wakaru.exe" : "wakaru",
  );
  if (existsSync(debugBinary)) {
    return { command: debugBinary, prefix: [] };
  }
  throw new Error(
    `missing wakaru binary: run "cargo build -p wakaru-cli" first, or set WAKARU to a wakaru executable`,
  );
}

export function runWakaruAsync(source, { level, timeoutMs, wakaruCmd }) {
  const { command, prefix } = wakaruCmd;
  return new Promise((resolvePromise, reject) => {
    const child = spawn(command, [...prefix, "--level", level, "-"], {
      cwd: repoRoot,
      stdio: ["pipe", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    let done = false;
    let timer = null;

    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");

    function finish(error, result) {
      if (done) return;
      done = true;
      clearTimeout(timer);
      if (error) reject(error);
      else resolvePromise(result);
    }

    if (Number.isFinite(timeoutMs) && timeoutMs > 0) {
      timer = setTimeout(() => {
        child.kill();
        const err = new Error(`wakaru timed out after ${timeoutMs}ms`);
        err.isTimeout = true;
        finish(err);
      }, timeoutMs);
    }

    child.stdin.on("error", () => {});
    child.stdout.on("data", (chunk) => (stdout += chunk));
    child.stderr.on("data", (chunk) => (stderr += chunk));
    child.on("error", (err) => finish(err));
    child.on("close", (code) => {
      if (code === 0 && stdout.trim().length === 0) {
        finish(new Error("wakaru exited successfully but produced empty output"));
      } else if (code === 0) {
        finish(null, stdout);
      } else {
        finish(new Error(`wakaru exited ${code}\n${stderr || stdout}`));
      }
    });
    child.stdin.end(source);
  });
}

export async function runRoundTrip(options) {
  const pipeline = resolvePipelineName(options);
  options = {
    ...options,
    pipeline,
    toolRoot: resolvePipelineToolRoot(options.toolRoot, pipeline),
  };
  validateTest262BaselineOptions(options);
  if (resolve(options.test262Root) === resolve(defaultManagedTest262Root)) {
    assertPinnedTest262Corpus({ root: options.test262Root });
  }
  const test262Revision = readTest262Revision(options.test262Root) ?? "unmanaged";
  const tests = options.rerunFrom
    ? discoverTestsFromReport(options.test262Root, options.rerunFrom, options.rerunStatuses)
    : discoverTests(options.test262Root, options.paths);
  const knownBlockers = loadKnownBlockers(options.knownBlockers ?? defaultKnownBlockersPath);
  const tmpRoot = mkdtempSync(join(tmpdir(), "wakaru-test262-"));
  const report = {
    complete: false,
    options: {
      test262Root: options.test262Root,
      test262Revision,
      harnessVersion: test262HarnessVersion,
      nodeMajor: Number.parseInt(process.versions.node.split(".")[0], 10),
      producer: describeProducer(options),
      presets: options.presets ?? [],
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
      baseline: options.baseline
        ? relative(repoRoot, options.baseline).split(sep).join("/")
        : null,
      updateBaseline: options.updateBaseline === true,
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
    // Phase 1: classify tests, run baseline + transform + transformed-runtime checks.
    // Tests that pass all pre-decompile checks are collected for batch decompilation.
    const pending = [];

    for (const filePath of tests) {
      const source = readFileSync(filePath, "utf8");
      const relativePath = relative(options.test262Root, filePath).split(sep).join("/");
      const pathClassification = classifyTestPath(filePath);

      if (!pathClassification.runnable) {
        report.totals.skipped += 1;
        report.results.push({
          path: relativePath,
          status: "skipped",
          reason: pathClassification.reason,
        });
        report.totals.processed += 1;
        writeReportOutputs(report, options);
        continue;
      }

      let metadata;
      try {
        metadata = parseTestMetadata(source);
      } catch (error) {
        report.totals.runnable += 1;
        recordResult(
          report,
          failure(relativePath, "harness-configuration", error),
          options,
        );
        continue;
      }
      const classification = classifyTest(filePath, source, metadata);

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
      if (metadata.negative && ["parse", "early"].includes(metadata.negative.phase)) {
        recordResult(
          report,
          verifyParseNegative({
            relativePath,
            source,
            metadata,
            variants,
            timeoutMs: options.caseTimeoutMs,
          }),
          options,
        );
        continue;
      }
      const unsupportedCapability = unsupportedTest262Capability(metadata);
      if (unsupportedCapability) {
        recordResult(
          report,
          unsupported(
            relativePath,
            "runtime-capability",
            new Error(`unsupported Test262 capability: ${unsupportedCapability}`),
            unsupportedCapability,
            { variants: variants.map((variant) => variant.name) },
          ),
          options,
        );
        continue;
      }
      const harnessSource = buildHarnessSource(options.test262Root, metadata);
      const isModule = variants.some((v) => v.module);

      const prepPromise = isModule
        ? prepareModuleTest({
            filePath,
            relativePath,
            harnessSource,
            metadata,
            tmpRoot,
            options,
            knownBlockers,
          })
        : prepareScriptTest({
            filePath,
            relativePath,
            source,
            harnessSource,
            metadata,
            variants,
            options,
            knownBlockers,
          });

      const prep = await withTimeout(prepPromise, options.caseTimeoutMs, relativePath);

      if (prep.result) {
        recordResult(report, prep.result, options);
        continue;
      }

      pending.push(prep);
    }

    // Phase 2: batch-decompile all pending tests in parallel.
    if (pending.length > 0) {
      const wakaruCmd = resolveWakaruCmd();
      const decompileJobs = [];

      for (const entry of pending) {
        if (entry.isModule) {
          for (const [modPath, modSource] of entry.transformedSources) {
            decompileJobs.push({ entry, source: modSource, modulePath: modPath });
          }
        } else {
          decompileJobs.push({ entry, source: entry.transformed });
        }
      }

      await runPool(decompileJobs, async (job) => {
        if (job.entry.decompileError) return;
        try {
          const code = await runWakaruAsync(job.source, {
            level: options.level,
            timeoutMs: options.caseTimeoutMs,
            wakaruCmd,
          });
          if (job.modulePath != null) {
            job.entry.decompiledSources.set(job.modulePath, code);
          } else {
            job.entry.decompiled = code;
          }
        } catch (error) {
          job.entry.decompileError = error;
        }
      });
    }

    // Phase 3: verify decompiled results against the Test262 harness.
    for (const entry of pending) {
      const result = entry.isModule
        ? await verifyModuleTest(entry)
        : await verifyScriptTest(entry);
      recordResult(report, result, options);
    }

    report.complete = true;
    if (options.baseline) {
      report.baselineComparison = applyTest262Baseline(report, {
        path: options.baseline,
        update: options.updateBaseline === true,
      });
    }
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

async function prepareScriptTest({
  filePath,
  relativePath,
  source,
  harnessSource,
  metadata,
  variants,
  options,
  knownBlockers,
}) {
  try {
    for (const variant of variants) {
      const outcome = await executeTestSourceOutcome({
        harnessSource,
        testSource: source,
        filename: `${relativePath}:${variant.name}:original`,
        strict: variant.strict,
        timeoutMs: options.caseTimeoutMs,
        async: metadataIsAsync(metadata),
      });
      assertExpectedOutcome(outcome, metadata.negative, variant.name);
    }
  } catch (error) {
    return {
      result: unsupported(relativePath, "baseline", error, "node-vm-baseline", {
        variants: variants.map((variant) => variant.name),
      }),
    };
  }

  let transformed;
  try {
    transformed = await transformSource(source, options);
  } catch (error) {
    return {
      result: rejected(
        relativePath,
        "transform",
        error,
        knownTransformRejectReason({ path: relativePath, error, variants, knownBlockers }) ??
          "transform-reject",
      ),
    };
  }

  try {
    for (const variant of variants) {
      const outcome = await executeTestSourceOutcome({
        harnessSource,
        testSource: transformed,
        filename: `${relativePath}:${variant.name}:transformed`,
        strict: variant.strict,
        timeoutMs: options.caseTimeoutMs,
        async: metadataIsAsync(metadata),
      });
      assertExpectedOutcome(outcome, metadata.negative, variant.name);
    }
  } catch (error) {
    return {
      result: rejected(
        relativePath,
        "transformed-runtime",
        error,
        knownTransformedRuntimeRejectReason({ path: relativePath, error, variants, knownBlockers }) ??
          "transform-runtime",
        { variants: variants.map((variant) => variant.name) },
      ),
    };
  }

  return {
    isModule: false,
    relativePath,
    variants,
    harnessSource,
    transformed,
    knownBlockers,
    decompiled: null,
    decompileError: null,
    caseTimeoutMs: options.caseTimeoutMs,
    metadata,
  };
}

async function prepareModuleTest({
  filePath,
  relativePath,
  harnessSource,
  metadata,
  tmpRoot,
  options,
  knownBlockers,
}) {
  let originalGraph;
  try {
    originalGraph = readModuleGraph(options.test262Root, filePath, {
      allowMissing: metadata.negative?.phase === "resolution",
    });
  } catch (error) {
    return { result: unsupported(relativePath, "baseline", error, "module-graph-baseline") };
  }

  try {
    const outcome = executeModuleGraphOutcome({
      harnessSource,
      entryPath: relativePath,
      sources: originalGraph.sources,
      tmpRoot,
      phase: "original",
      timeoutMs: options.caseTimeoutMs,
      async: metadataIsAsync(metadata),
    });
    assertExpectedOutcome(outcome, metadata.negative, "module");
  } catch (error) {
    return { result: unsupported(relativePath, "baseline", error, "node-module-baseline") };
  }

  let transformedSources;
  try {
    transformedSources = new Map();
    for (const [path, moduleSource] of originalGraph.sources) {
      transformedSources.set(path, await transformSource(moduleSource, options, { module: true }));
    }
  } catch (error) {
    return {
      result: rejected(
        relativePath,
        "transform",
        error,
        knownTransformRejectReason({
          path: relativePath,
          error,
          variants: [{ name: "module", strict: true, module: true }],
          knownBlockers,
        }) ?? "transform-reject",
      ),
    };
  }

  try {
    const outcome = executeModuleGraphOutcome({
      harnessSource,
      entryPath: relativePath,
      sources: transformedSources,
      tmpRoot,
      phase: "transformed",
      timeoutMs: options.caseTimeoutMs,
      async: metadataIsAsync(metadata),
    });
    assertExpectedOutcome(outcome, metadata.negative, "module");
  } catch (error) {
    return {
      result: rejected(
        relativePath,
        "transformed-runtime",
        error,
        knownTransformedRuntimeRejectReason({
          path: relativePath,
          error,
          variants: [{ name: "module", strict: true, module: true }],
          knownBlockers,
        }) ?? "transform-runtime",
      ),
    };
  }

  return {
    isModule: true,
    relativePath,
    harnessSource,
    transformedSources,
    originalGraphSize: originalGraph.sources.size,
    tmpRoot,
    caseTimeoutMs: options.caseTimeoutMs,
    knownBlockers,
    decompiledSources: new Map(),
    decompileError: null,
    metadata,
  };
}

function verifyParseNegative({ relativePath, source, metadata, variants, timeoutMs }) {
  try {
    for (const variant of variants) {
      const outcome = parseSourceOutcome({
        source,
        filename: `${relativePath}:${variant.name}:original`,
        strict: variant.strict,
        module: variant.module === true,
        timeoutMs,
      });
      assertExpectedOutcome(outcome, metadata.negative, variant.name);
    }
  } catch (error) {
    return unsupported(relativePath, "parser-boundary", error, "node-parse-baseline", {
      variants: variants.map((variant) => variant.name),
    });
  }
  return {
    path: relativePath,
    status: "passed",
    lane: "parser-boundary",
    variants: variants.map((variant) => variant.name),
  };
}

export function parseSourceOutcome({
  source,
  filename,
  strict,
  module,
  timeoutMs,
  spawnSyncImpl = spawnSync,
}) {
  const prepared = strict && !module ? `"use strict";\n${source}` : source;
  if (module) {
    const result = spawnSyncImpl(process.execPath, ["--check", "--input-type=module"], {
      input: prepared,
      encoding: "utf8",
      maxBuffer: 10 * 1024 * 1024,
      timeout: Number.isFinite(timeoutMs) && timeoutMs > 0 ? timeoutMs : undefined,
    });
    if (result.error) {
      throw new Error(`module parse check failed for ${filename}: ${result.error.message}`, {
        cause: result.error,
      });
    }
    if (result.signal) {
      throw new Error(`module parse check terminated by ${result.signal} for ${filename}`);
    }
    if (result.status == null) {
      throw new Error(`module parse check did not report an exit status for ${filename}`);
    }
    if (result.status === 0) {
      return { phase: "success", error: null, errorName: null };
    }
    const diagnostic = (result.stderr || result.stdout || "").trim();
    if (diagnostic.length === 0) {
      throw new Error(
        `module parse check exited ${result.status} without a diagnostic for ${filename}`,
      );
    }
    const errorMatches = [...diagnostic.matchAll(/\b([A-Za-z_$][\w$]*Error)\b/g)];
    const errorName = errorMatches.at(-1)?.[1] ?? "SyntaxError";
    return { phase: "parse", error: new Error(diagnostic), errorName };
  }
  try {
    new vm.Script(prepared, { filename });
    return { phase: "success", error: null, errorName: null };
  } catch (error) {
    return executionOutcome("parse", error);
  }
}

function assertExpectedOutcome(outcome, negative, variant) {
  if (!negative) {
    if (outcome.phase === "success") return;
    throw new Error(
      `${variant}: expected success, got ${outcome.phase} ${outcome.errorName}: ${outcome.error?.message ?? outcome.error}`,
    );
  }
  const expectedPhase = ["parse", "early"].includes(negative.phase)
    ? "parse"
    : negative.phase;
  if (outcome.phase !== expectedPhase || outcome.errorName !== negative.type) {
    throw new Error(
      `${variant}: expected ${negative.phase} ${negative.type}, got ${outcome.phase} ${outcome.errorName ?? "no error"}: ${outcome.error?.message ?? outcome.error ?? "success"}`,
    );
  }
}

function metadataIsAsync(metadata) {
  return metadata?.flags?.includes("async") === true;
}

export function unsupportedTest262Capability(metadata) {
  if (metadata.flags.includes("non-deterministic")) {
    return "flag:non-deterministic";
  }
  if (metadata.flags.includes("CanBlockIsFalse")) {
    return "host:CanBlockIsFalse";
  }
  if (metadata.includes.some((include) => ["agent.js", "atomicsHelper.js"].includes(include))) {
    return "host:$262.agent";
  }
  if (metadata.features.includes("IsHTMLDDA")) {
    return "host:IsHTMLDDA";
  }
  const probes = new Map([
    ["Temporal", () => typeof globalThis.Temporal === "object"],
    ["ShadowRealm", () => typeof globalThis.ShadowRealm === "function"],
    ["Float16Array", () => typeof globalThis.Float16Array === "function"],
    ["RegExp.escape", () => typeof RegExp.escape === "function"],
    ["promise-try", () => typeof Promise.try === "function"],
    ["Math.sumPrecise", () => typeof Math.sumPrecise === "function"],
    ["Error.isError", () => typeof Error.isError === "function"],
    ["Intl.DurationFormat", () => typeof Intl.DurationFormat === "function"],
  ]);
  for (const feature of metadata.features) {
    const probe = probes.get(feature);
    if (probe && !probe()) {
      return `feature:${feature}`;
    }
  }
  return null;
}

async function verifyScriptTest(entry) {
  const {
    relativePath,
    variants,
    harnessSource,
    transformed,
    knownBlockers,
    caseTimeoutMs,
    metadata,
  } = entry;

  if (entry.decompileError) {
    if (isTimeoutError(entry.decompileError)) {
      return rejected(relativePath, "case-timeout", entry.decompileError, "case-timeout");
    }
    const parseUnsupportedReason = knownWakaruParseUnsupportedReason(
      entry.decompileError,
      variants,
      relativePath,
      knownBlockers,
    );
    if (parseUnsupportedReason) {
      return unsupported(relativePath, "wakaru-parse", entry.decompileError, parseUnsupportedReason);
    }
    return failure(relativePath, "wakaru", entry.decompileError, { transformed });
  }

  const decompiled = entry.decompiled;
  try {
    for (const variant of variants) {
      const outcome = await executeTestSourceOutcome({
        harnessSource,
        testSource: decompiled,
        filename: `${relativePath}:${variant.name}:decompiled`,
        strict: variant.strict,
        timeoutMs: caseTimeoutMs,
        async: metadataIsAsync(metadata),
      });
      assertExpectedOutcome(outcome, metadata?.negative, variant.name);
    }
  } catch (error) {
    const decompiledRuntimeReason = knownDecompiledRuntimeRejectReason({
      path: relativePath,
      error,
      decompiled,
      knownBlockers,
    });
    if (decompiledRuntimeReason) {
      return rejected(relativePath, "decompiled-runtime", error, decompiledRuntimeReason, {
        transformed,
        decompiled,
      });
    }
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

async function verifyModuleTest(entry) {
  const {
    relativePath, harnessSource, transformedSources, originalGraphSize,
    tmpRoot, caseTimeoutMs, knownBlockers, decompiledSources, metadata,
  } = entry;

  if (entry.decompileError) {
    if (isTimeoutError(entry.decompileError)) {
      return rejected(relativePath, "case-timeout", entry.decompileError, "case-timeout");
    }
    const parseUnsupportedReason = knownWakaruParseUnsupportedReason(
      entry.decompileError,
      [{ name: "module", strict: true, module: true }],
      relativePath,
      knownBlockers,
    );
    if (parseUnsupportedReason) {
      return unsupported(relativePath, "wakaru-parse", entry.decompileError, parseUnsupportedReason);
    }
    return failure(relativePath, "wakaru", entry.decompileError, {
      transformed: Object.fromEntries(transformedSources),
    });
  }

  try {
    const outcome = executeModuleGraphOutcome({
      harnessSource,
      entryPath: relativePath,
      sources: decompiledSources,
      tmpRoot,
      phase: "decompiled",
      timeoutMs: caseTimeoutMs,
      async: metadataIsAsync(metadata),
    });
    assertExpectedOutcome(outcome, metadata.negative, "module");
  } catch (error) {
    const decompiled = decompiledSources.get(relativePath) ?? "";
    const decompiledRuntimeReason = knownDecompiledRuntimeRejectReason({
      path: relativePath,
      error,
      decompiled,
      knownBlockers,
    });
    if (decompiledRuntimeReason) {
      return rejected(relativePath, "decompiled-runtime", error, decompiledRuntimeReason, {
        transformed: Object.fromEntries(transformedSources),
        decompiled: Object.fromEntries(decompiledSources),
      });
    }
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
    modules: originalGraphSize,
  };
}

function recordResult(report, result, options) {
  report.results.push(result);
  report.totals.processed += 1;
  if (result.status === "passed") {
    report.totals.passed += 1;
  } else if (result.status === "unsupported") {
    report.totals.unsupported += 1;
  } else if (result.status === "rejected") {
    report.totals.rejected += 1;
  } else if (result.status === "failed") {
    report.totals.failed += 1;
  }
  writeReportOutputs(report, options);
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

function unsupported(path, phase, error, reason = null, extra = {}) {
  return {
    path,
    status: "unsupported",
    phase,
    reason,
    error: formatError(error),
    ...extra,
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

export function knownDecompiledRuntimeRejectReason({
  path,
  error,
  decompiled,
  knownBlockers = defaultKnownBlockers(),
}) {
  return classifyKnownBlocker({
    knownBlockers,
    status: "rejected",
    phase: "decompiled-runtime",
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
  validateToolOnce(`babel:${resolve(toolRoot)}`, () => {
    ensureToolPackages(toolRoot, packages);
    try {
      assertBabelUsable(toolRoot);
    } catch {
      rmSync(join(toolRoot, "node_modules"), { recursive: true, force: true });
      rmSync(join(toolRoot, "package-lock.json"), { force: true });
      ensureToolPackages(toolRoot, packages);
      assertBabelUsable(toolRoot);
    }
  });
}

function ensureSwc(toolRoot) {
  validateToolOnce(`swc:${resolve(toolRoot)}`, () => {
    ensureToolPackages(toolRoot, swcPackages);
    try {
      assertSwcUsable(toolRoot);
    } catch {
      rmSync(join(toolRoot, "node_modules"), { recursive: true, force: true });
      rmSync(join(toolRoot, "package-lock.json"), { force: true });
      ensureToolPackages(toolRoot, swcPackages);
      assertSwcUsable(toolRoot);
    }
  });
}

function ensureEsbuild(toolRoot) {
  validateToolOnce(`esbuild:${resolve(toolRoot)}`, () => {
    ensureToolPackages(toolRoot, esbuildPackages);
    try {
      assertEsbuildUsable(toolRoot);
    } catch {
      rmSync(join(toolRoot, "node_modules"), { recursive: true, force: true });
      rmSync(join(toolRoot, "package-lock.json"), { force: true });
      ensureToolPackages(toolRoot, esbuildPackages);
      assertEsbuildUsable(toolRoot);
    }
  });
}

export function createToolValidationCache() {
  const validated = new Set();
  return (key, validate) => {
    if (validated.has(key)) return;
    validate();
    validated.add(key);
  };
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

class UnsupportedHostCapability extends Error {
  constructor(capability) {
    super(`unsupported Test262 host capability: ${capability}`);
    this.name = "UnsupportedHostCapability";
  }
}

function createTestContext({ timeoutMs = 1000, onPrint = () => {} } = {}) {
  const timers = new Set();
  const childRealms = [];
  const trackedSetTimeout = (callback, delay, ...args) => {
    let handle;
    handle = setTimeout(() => {
      timers.delete(handle);
      callback(...args);
    }, delay);
    timers.add(handle);
    return handle;
  };
  const trackedSetInterval = (callback, delay, ...args) => {
    const handle = setInterval(callback, delay, ...args);
    timers.add(handle);
    return handle;
  };
  const context = {
    console,
    setTimeout: trackedSetTimeout,
    clearTimeout(handle) {
      timers.delete(handle);
      clearTimeout(handle);
    },
    setInterval: trackedSetInterval,
    clearInterval(handle) {
      timers.delete(handle);
      clearInterval(handle);
    },
    setImmediate,
    clearImmediate,
    queueMicrotask,
    structuredClone,
  };
  context.print = (...values) => onPrint(values.map(String).join(" "));
  context.globalThis = context;
  let vmContext;
  const supportedHost = {
    global: context,
    evalScript(source) {
      return vm.runInContext(String(source), vmContext, { timeout: timeoutMs });
    },
    createRealm() {
      const realm = createTestContext({ timeoutMs, onPrint });
      childRealms.push(realm);
      return realm.context.$262;
    },
    detachArrayBuffer(buffer) {
      structuredClone(buffer, { transfer: [buffer] });
    },
    gc() {
      if (typeof globalThis.gc !== "function") {
        throw new UnsupportedHostCapability("gc");
      }
      globalThis.gc();
    },
  };
  context.$262 = new Proxy(supportedHost, {
    get(target, property, receiver) {
      if (typeof property !== "string" || property in target) {
        return Reflect.get(target, property, receiver);
      }
      throw new UnsupportedHostCapability(property);
    },
    has(target, property) {
      return typeof property !== "string" || property in target;
    },
  });
  vmContext = vm.createContext(context);
  return {
    context: vmContext,
    cleanup() {
      for (const handle of timers) {
        clearTimeout(handle);
        clearInterval(handle);
      }
      timers.clear();
      for (const realm of childRealms) {
        realm.cleanup();
      }
    },
  };
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
  if (error?.isTimeout) return true;
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
    `- test262Revision: ${report.options.test262Revision ?? "unmanaged"}`,
    `- harnessVersion: ${report.options.harnessVersion ?? "unrecorded"}`,
    `- nodeMajor: ${report.options.nodeMajor ?? Number.parseInt(process.versions.node.split(".")[0], 10)}`,
    `- producerVersion: ${report.options.producer?.version ?? "unrecorded"}`,
    `- producerConfigHash: ${report.options.producer?.configHash ?? "unrecorded"}`,
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

  return `${lines.join("\n").trimEnd()}\n`;
}

export function test262ReportExitCode(report) {
  if (report.baselineComparison) {
    return report.baselineComparison.clean ? 0 : 1;
  }
  return report.totals.failed === 0 ? 0 : 1;
}

export function formatBaselineComparison(comparison) {
  if (!comparison || comparison.clean) {
    return "";
  }
  const lines = [
    "Test262 baseline changed:",
    `  totals changed: ${comparison.totalsChanged}`,
    `  new outcomes: ${comparison.newOutcomes.length}`,
    `  disappeared outcomes: ${comparison.unexpectedPasses.length}`,
  ];
  if (comparison.candidatePath) {
    lines.push(`  candidate: ${comparison.candidatePath}`);
  }
  for (const outcome of comparison.newOutcomes.slice(0, 20)) {
    lines.push(`  + ${outcome.path} [${outcome.status}:${outcome.kind}]`);
  }
  for (const outcome of comparison.unexpectedPasses.slice(0, 20)) {
    lines.push(`  - ${outcome.path} [${outcome.status}:${outcome.kind}]`);
  }
  return lines.join("\n");
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

export function readModuleGraph(test262Root, entryPath, { allowMissing = false } = {}) {
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
      if (allowMissing) {
        return;
      }
      throw new Error(`missing module import: ${normalizedPath}`);
    }

    visiting.add(normalizedPath);
    const source = readFileSync(absolute, "utf8");
    sources.set(normalizedPath, source);
    for (const specifier of collectStaticModuleSpecifiers(source)) {
      if (!specifier.startsWith(".") && !specifier.startsWith("/")) {
        if (allowMissing) {
          continue;
        }
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
  const outcome = executeModuleGraphOutcome({
    harnessSource,
    entryPath,
    sources,
    tmpRoot,
    phase,
    timeoutMs,
  });
  if (outcome.phase !== "success") {
    throw outcome.error;
  }
}

export function executeModuleGraphOutcome({
  harnessSource,
  entryPath,
  sources,
  tmpRoot,
  phase,
  timeoutMs,
  async = false,
}) {
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
  const marker = "__WAKARU_TEST262_OUTCOME__";
  writeFileSync(
    bootstrapPath,
    `const marker = ${JSON.stringify(marker)};\n` +
      `let doneResolve, doneReject;\n` +
      `const donePromise = new Promise((resolve, reject) => { doneResolve = resolve; doneReject = reject; });\n` +
      `globalThis.$DONE = error => error == null ? doneResolve() : doneReject(error);\n` +
      `globalThis.print = message => {\n` +
      `  const text = String(message);\n` +
      `  if (text === "Test262:AsyncTestComplete") doneResolve();\n` +
      `  else if (text.startsWith("Test262:AsyncTestFailure:")) doneReject(new Error(text));\n` +
      `};\n` +
      `globalThis.$262 = {\n` +
      `  global: globalThis,\n` +
      `  evalScript(source) { return (0, eval)(source); },\n` +
      `  createRealm() { return globalThis.$262; },\n` +
      `  detachArrayBuffer(buffer) { structuredClone(buffer, { transfer: [buffer] }); },\n` +
      `  gc() { throw new Error("$262.gc is not available in this runner"); }\n` +
      `};\n` +
      `function classify(error) {\n` +
      `  const message = String(error?.message ?? error);\n` +
      `  if (error?.code === "ERR_MODULE_NOT_FOUND" || /does not provide an export|ambiguous indirect export|requested module/i.test(message)) return "resolution";\n` +
      `  return "runtime";\n` +
      `}\n` +
      `function emit(outcome) { process.stdout.write(marker + JSON.stringify(outcome) + "\\n"); }\n` +
      `try {\n` +
      `  (0, eval)(${JSON.stringify(harnessSource)});\n` +
      `  await import(${JSON.stringify(pathToFileURL(join(graphRoot, entryPath)).href)});\n` +
      `  if (${JSON.stringify(async)}) await donePromise;\n` +
      `  emit({ phase: "success", errorName: null, message: null });\n` +
      `} catch (error) {\n` +
      `  emit({ phase: classify(error), errorName: error?.name ?? error?.constructor?.name ?? typeof error, message: String(error?.message ?? error) });\n` +
      `}\n`,
  );

  const result = spawnForPlatform(process.execPath, [bootstrapPath], {
    cwd: graphRoot,
    encoding: "utf8",
    maxBuffer: 20 * 1024 * 1024,
    timeout: timeoutMs,
  });
  if (result.error) {
    return executionOutcome("runtime", result.error);
  }
  const markerLine = String(result.stdout)
    .split(/\r?\n/)
    .findLast((line) => line.startsWith(marker));
  if (!markerLine) {
    return executionOutcome(
      "runtime",
      new Error(
        `module worker produced no outcome${result.stderr ? `: ${result.stderr.trim()}` : ""}`,
      ),
    );
  }
  const response = JSON.parse(markerLine.slice(marker.length));
  if (response.phase === "success") {
    return { phase: "success", error: null, errorName: null };
  }
  const error = new Error(response.message);
  error.name = response.errorName;
  return { phase: response.phase, error, errorName: response.errorName };
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
      if (report.baselineComparison?.clean === false) {
        console.error(formatBaselineComparison(report.baselineComparison));
      }
      process.exitCode = test262ReportExitCode(report);
    }
  } catch (error) {
    console.error(formatError(error));
    process.exitCode = 1;
  }
}
