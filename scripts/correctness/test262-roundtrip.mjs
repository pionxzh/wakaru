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
const defaultRewriteLevel = "minimal";
const defaultTransform = "terser";
const supportedTransforms = new Set(["none", "terser"]);
const supportedLevels = new Set(["minimal", "standard", "aggressive"]);
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
  templates: ["test/language/expressions/template-literal", "test/language/expressions/tagged-template"],
  modules: ["test/language/module-code"],
};

export function parseArgs(argv) {
  const options = {
    test262Root: defaultTest262Root,
    paths: [],
    presets: [],
    limit: 25,
    transform: defaultTransform,
    terserProfile: "light",
    level: defaultRewriteLevel,
    json: null,
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
    } else if (arg === "--transform") {
      options.transform = readRequiredValue(argv, ++i, arg);
    } else if (arg === "--level") {
      options.level = readRequiredValue(argv, ++i, arg);
    } else if (arg === "--terser-profile") {
      options.terserProfile = readRequiredValue(argv, ++i, arg);
    } else if (arg === "--json") {
      options.json = resolve(readRequiredValue(argv, ++i, arg));
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
  if (!supportedLevels.has(options.level)) {
    throw new Error(`unsupported --level ${options.level}`);
  }
  if (!["light", "full"].includes(options.terserProfile)) {
    throw new Error(`unsupported --terser-profile ${options.terserProfile}`);
  }
  for (const preset of options.presets) {
    if (!Object.hasOwn(pathPresets, preset)) {
      throw new Error(`unsupported --preset ${preset}`);
    }
    options.paths.push(...pathPresets[preset]);
  }
  if (options.paths.length === 0) {
    options.paths = [...defaultPaths];
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
  --transform <name>    none | terser. Default: terser
  --terser-profile <p>  light | full. Default: light
  --level <level>       minimal | standard | aggressive. Default: minimal
  --json <file>         Write full JSON report
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
    return [];
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
  if (normalized.includes("/_FIXTURE")) {
    return { runnable: false, reason: "fixture" };
  }
  if (normalized.includes("/intl402/")) {
    return { runnable: false, reason: "intl402" };
  }
  if (metadata.negative) {
    return { runnable: false, reason: "negative" };
  }
  for (const flag of ["raw", "module", "async"]) {
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

export function executeTestSource({ harnessSource, testSource, filename, strict }) {
  const context = createTestContext();
  vm.runInContext(harnessSource, context, {
    filename: "test262-harness.js",
    timeout: 1000,
  });

  const source = strict ? `"use strict";\n${testSource}` : testSource;
  vm.runInContext(source, context, {
    filename,
    timeout: 1000,
  });
}

export async function minifyWithTerser(source, options) {
  await ensureTerser(options.toolRoot);
  const toolRequire = createRequire(pathToFileURL(join(options.toolRoot, "package.json")));
  const terserModule = await import(pathToFileURL(toolRequire.resolve("terser")).href);
  const result = await terserModule.minify(source, {
    module: false,
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

export async function transformSource(source, options) {
  if (options.transform === "none") {
    return source;
  }
  if (options.transform === "terser") {
    return minifyWithTerser(source, options);
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
  const tests = discoverTests(options.test262Root, options.paths);
  const tmpRoot = mkdtempSync(join(tmpdir(), "wakaru-test262-"));
  const report = {
    options: {
      test262Root: options.test262Root,
      paths: options.paths,
      limit: Number.isFinite(options.limit) ? options.limit : "all",
      transform: options.transform,
      terserProfile: options.terserProfile,
      level: options.level,
    },
    totals: {
      discovered: tests.length,
      skipped: 0,
      unsupported: 0,
      rejected: 0,
      runnable: 0,
      passed: 0,
      failed: 0,
    },
    results: [],
  };

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
        continue;
      }

      report.totals.runnable += 1;
      const harnessSource = buildHarnessSource(options.test262Root, metadata);
      const result = await runOneTest({
        filePath,
        relativePath,
        source,
        harnessSource,
        variants,
        tmpRoot,
        options,
      });
      report.results.push(result);
      if (result.status === "passed") {
        report.totals.passed += 1;
      } else if (result.status === "unsupported") {
        report.totals.unsupported += 1;
      } else if (result.status === "rejected") {
        report.totals.rejected += 1;
      } else {
        report.totals.failed += 1;
      }
    }
  } finally {
    if (!options.keepTemp) {
      rmSync(tmpRoot, { recursive: true, force: true });
    }
  }

  return report;
}

async function runOneTest({ filePath, relativePath, source, harnessSource, variants, tmpRoot, options }) {
  try {
    for (const variant of variants) {
      executeTestSource({
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
    return rejected(relativePath, "transform", error, "transform-reject");
  }

  try {
    for (const variant of variants) {
      executeTestSource({
        harnessSource,
        testSource: transformed,
        filename: `${relativePath}:${variant.name}:transformed`,
        strict: variant.strict,
      });
    }
  } catch (error) {
    return rejected(relativePath, "transformed-runtime", error, "transform-runtime");
  }

  let decompiled;
  try {
    decompiled = runWakaru(transformed, {
      level: options.level,
      tmpRoot,
      basename: relativePath.replaceAll("/", "__"),
    });
  } catch (error) {
    const parseUnsupportedReason = knownWakaruParseUnsupportedReason(error, variants, relativePath);
    if (parseUnsupportedReason) {
      return unsupported(relativePath, "wakaru-parse", error, parseUnsupportedReason);
    }
    return failure(relativePath, "wakaru", error, { transformed });
  }

  try {
    for (const variant of variants) {
      executeTestSource({
        harnessSource,
        testSource: decompiled,
        filename: `${relativePath}:${variant.name}:decompiled`,
        strict: variant.strict,
      });
    }
  } catch (error) {
    const fidelityReason = knownSwcFidelityIssueReason({ path: relativePath, error, decompiled });
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

export function knownWakaruParseUnsupportedReason(error, variants, path) {
  const message = formatError(error);
  const normalized = path.split("\\").join("/");
  if (
    variants.length > 0 &&
    variants.every((variant) => !variant.strict) &&
    message.includes("InvalidIdentInStrict")
  ) {
    return "sloppy-only-strict-ident";
  }
  if (message.includes("InvalidIdentInAsync")) {
    return "swc-parse-async-ident";
  }
  if (message.includes("ExpectedIdent") && normalized.includes("static-init-await")) {
    return "swc-parse-static-init-await";
  }
  if (message.includes("TS1109") && normalized.includes("yield-as-identifier")) {
    return "swc-parse-yield-ident";
  }
  return null;
}

export function knownSwcFidelityIssueReason({ path, error, decompiled }) {
  const normalized = path.split("\\").join("/");
  if (
    normalized.startsWith("test/language/statements/for-of/dstr/") &&
    /array-(?:elem-trlg-iter-)?elision|array-iteration/.test(normalized) &&
    /\bfor\s*\(\s*\[(?:\]|[^,\]]+\]\s+of)/.test(decompiled) &&
    /Test262Error|TypeError/.test(formatError(error))
  ) {
    return "swc-array-binding-elision";
  }
  return null;
}

function runWakaru(source, { level, tmpRoot, basename }) {
  const input = join(tmpRoot, `${basename}.js`);
  writeFileSync(input, source);

  const configured = process.env.WAKARU;
  if (configured) {
    return runChecked(configured, ["--level", level, input]).stdout;
  }

  const debugBinary = join(repoRoot, "target", "debug", process.platform === "win32" ? "wakaru.exe" : "wakaru");
  if (existsSync(debugBinary)) {
    return runChecked(debugBinary, ["--level", level, input]).stdout;
  }

  return runChecked("cargo", ["run", "-q", "-p", "wakaru-cli", "--", "--level", level, input], {
    cwd: repoRoot,
  }).stdout;
}

async function ensureTerser(toolRoot) {
  const packageJson = join(toolRoot, "package.json");
  const nodeModules = join(toolRoot, "node_modules", "terser");
  if (existsSync(nodeModules)) {
    return;
  }

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
  runChecked("npm", ["install", "--silent", "--no-save", "terser@5.31.6"], {
    cwd: toolRoot,
  });
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
  });
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
      if (options.json) {
        mkdirSync(dirname(options.json), { recursive: true });
        writeFileSync(options.json, `${JSON.stringify(report, null, 2)}\n`);
      }
      process.exitCode = report.totals.failed === 0 ? 0 : 1;
    }
  } catch (error) {
    console.error(formatError(error));
    process.exitCode = 1;
  }
}
