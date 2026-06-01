#!/usr/bin/env node

import { existsSync, mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const tmpRoot = mkdtempSync(join(tmpdir(), "wakaru-swc-minifier-"));
const toolRoot = join(repoRoot, "target", "repro-tools", "swc-minifier");
const showDetails = process.argv.includes("--details");
const rewriteLevel = readOption("--level", "standard");
const failures = [];

if (!["minimal", "standard", "aggressive"].includes(rewriteLevel)) {
  throw new Error(`unsupported --level ${rewriteLevel}`);
}

const profiles = [
  {
    name: "sequences",
    options: {
      compress: { defaults: false, sequences: true },
      mangle: false,
    },
  },
  {
    name: "booleans",
    options: {
      compress: { defaults: false, booleans: true },
      mangle: false,
    },
  },
  {
    name: "evaluate",
    options: {
      compress: { defaults: false, evaluate: true },
      mangle: false,
    },
  },
  {
    name: "inline-iife",
    options: {
      compress: { defaults: false, inline: 3, reduce_vars: true, reduce_funcs: true },
      mangle: false,
    },
  },
  {
    name: "mangle",
    options: {
      compress: false,
      mangle: { toplevel: true },
    },
  },
  {
    name: "all",
    options: {
      compress: true,
      mangle: true,
    },
  },
];

const snippets = [
  {
    name: "sequence-before-if",
    bucket: "sequences",
    source: `
function run(y) {
  x = 5;
  if (y) z();
}
run(input);
`,
    expectedAny: [
      ["x = 5;", "if (y)"],
      ["x = 5;", "if (input_1)"],
      ["x = 5;", "if (input)"],
    ],
  },
  {
    name: "sequence-before-return",
    bucket: "sequences",
    source: `
function run() {
  side();
  return value;
}
console.log(run());
`,
    expected: ["side();", "return value;"],
  },
  {
    name: "sequence-before-for",
    bucket: "sequences",
    source: `
function run() {
  setup();
  for (; i < n; i++) work(i);
}
run();
`,
    expected: ["setup();", "for(; i < n; i++)"],
  },
  {
    name: "sequence-before-throw",
    bucket: "sequences",
    source: `
function run() {
  log();
  throw error;
}
try {
  run();
} catch (caught) {
  handle(caught);
}
`,
    expected: ["log();", "throw error;"],
  },
  {
    name: "boolean-literals",
    bucket: "booleans",
    source: `
const yes = true;
const no = false;
console.log(yes, no);
`,
    expected: ["true", "false"],
  },
  {
    name: "negated-condition",
    bucket: "booleans",
    source: `
function run(flag) {
  if (!flag) disabled();
}
run(input);
`,
    expectedAny: [["if (!flag)"], ["if (!input)"]],
  },
  {
    name: "boolean-return",
    bucket: "booleans",
    source: `
function run(flag) {
  return flag ? true : false;
}
console.log(run(input));
`,
    expectedAny: [["return !!flag;"], ["!!input"]],
  },
  {
    name: "double-negation",
    bucket: "booleans",
    source: `
const out = !!value;
console.log(out);
`,
    expected: ["!!value"],
  },
  {
    name: "undefined-infinity",
    bucket: "evaluate",
    source: `
const missing = undefined;
const forever = Infinity;
console.log(missing, forever);
`,
    expected: ["undefined", "Infinity"],
  },
  {
    name: "numeric-fold",
    bucket: "evaluate",
    source: `
const total = 1 + 2 * 3;
console.log(total);
`,
    expectedAny: [["const total = 7;"], ["console.log(7)"]],
  },
  {
    name: "string-constant-access",
    bucket: "evaluate",
    source: `
const letter = "abc".charAt(1);
console.log(letter);
`,
    expected: ["const letter = \"b\";"],
    informational: true,
  },
  {
    name: "array-constant-access",
    bucket: "evaluate",
    source: `
const item = [1, 2, 3][1];
console.log(item);
`,
    expectedAny: [["const item = 2;"], ["console.log(2)"]],
    informational: true,
  },
  {
    name: "arrow-iife-arg",
    bucket: "inline-iife",
    source: `
const out = ((value) => value + 1)(input);
console.log(out);
`,
    expected: ["input + 1"],
    informational: true,
  },
  {
    name: "function-iife-arg",
    bucket: "inline-iife",
    source: `
const out = (function (value) {
  return value + 1;
})(input);
console.log(out);
`,
    expected: ["input + 1"],
    informational: true,
  },
  {
    name: "single-use-temp-alias",
    bucket: "inline-iife",
    source: `
function run(input) {
  const alias = input.value;
  return alias;
}
console.log(run(input));
`,
    expectedAny: [["return input1.value;"], ["input.value"]],
  },
  {
    name: "callback-wrapper",
    bucket: "inline-iife",
    source: `
const wrapped = function (value) {
  return handler(value);
};
wrapped(input);
`,
    expected: ["handler(input)"],
    informational: true,
  },
  {
    name: "react-hook-tuple",
    bucket: "mangle",
    source: `
import { useState } from "react";
export function Counter() {
  const [count, setCount] = useState(0);
  return setCount(count + 1);
}
`,
    expected: ["useState", "setT"],
  },
  {
    name: "member-init-name",
    bucket: "mangle",
    source: `
const logger = services.logger;
logger.info("ready");
`,
    expected: ["logger.info"],
  },
  {
    name: "symbol-for-name",
    bucket: "mangle",
    source: `
const token = Symbol.for("wakaru.token");
console.log(token);
`,
    expected: ["token"],
  },
  {
    name: "component-value-position",
    bucket: "mangle",
    source: `
const UserCard = registry.UserCard;
export const view = UserCard(props);
`,
    expected: ["UserCard"],
  },
];

try {
  console.log("# SWC minifier reproduction matrix");
  console.log(`# wakaru: ${wakaruDescription()}`);
  console.log(`# level: ${rewriteLevel}`);
  console.log("");
  console.log("| bucket | snippet | shape | tools | recovered | notes |");
  console.log("|---|---|---:|---|---:|---|");

  for (const snippet of snippets) {
    const shapes = collectShapes(snippet);
    for (const shape of shapes) {
      const result = runShape(snippet, shape);
      if (!result.recovered && result.failure) {
        failures.push(result.failure);
      }
      console.log(
        `| ${snippet.bucket} | ${snippet.name} | ${shape.label} | ${escapeCell(
          shape.tools.join(", "),
        )} | ${escapeCell(result.status)} | ${escapeCell(result.notes)} |`,
      );
    }
  }

  if (showDetails && failures.length > 0) {
    console.log("");
    console.log("## Miss Details");
    for (const failure of failures) {
      console.log("");
      console.log(`### ${failure.bucket} / ${failure.snippet} / ${failure.shape}`);
      console.log("");
      console.log(`Tools: ${failure.tools.join(", ")}`);
      console.log("");
      console.log("SWC options:");
      console.log("");
      console.log("```json");
      console.log(JSON.stringify(failure.options, null, 2));
      console.log("```");
      console.log("");
      console.log("Original:");
      console.log("");
      console.log("```js");
      console.log(failure.source.trim());
      console.log("```");
      console.log("");
      console.log("Lowered:");
      console.log("");
      console.log("```js");
      console.log(failure.lowered.trim());
      console.log("```");
      console.log("");
      console.log("Wakaru:");
      console.log("");
      console.log("```js");
      console.log(failure.recoveredCode.trim());
      console.log("```");
      console.log("");
      console.log(`Missing expectations: ${failure.missing.join(", ")}`);
    }
  }

  if (failures.some((failure) => failure.executionError)) {
    process.exitCode = 1;
  }
} finally {
  rmSync(tmpRoot, { recursive: true, force: true });
}

function collectShapes(snippet) {
  const groups = new Map();
  const shapes = [];

  for (const profile of profilesFor(snippet)) {
    let lowered;
    try {
      lowered = runSwcMinify(snippet.source, profile.options);
    } catch (error) {
      shapes.push({
        label: `error ${shapes.length + 1}`,
        tools: [profile.name],
        profile,
        transformError: error,
      });
      continue;
    }

    const key = shapeKey(lowered);
    if (groups.has(key)) {
      groups.get(key).tools.push(profile.name);
      continue;
    }

    const shape = {
      label: `shape ${groups.size + 1}`,
      tools: [profile.name],
      profile,
      lowered,
    };
    groups.set(key, shape);
    shapes.push(shape);
  }

  return shapes;
}

function profilesFor(snippet) {
  const bucketProfiles = profiles.filter((profile) => profile.name === snippet.bucket);
  const allProfile = profiles.find((profile) => profile.name === "all");
  return [...bucketProfiles, allProfile].filter(Boolean);
}

function runShape(snippet, shape) {
  if (shape.transformError) {
    return {
      recovered: false,
      status: "transform-error",
      notes: shape.transformError.message,
      failure: {
        bucket: snippet.bucket,
        snippet: snippet.name,
        shape: shape.label,
        tools: shape.tools,
        options: shape.profile.options,
        source: snippet.source,
        lowered: "",
        recoveredCode: "",
        missing: ["transform failed"],
        informational: false,
        executionError: true,
      },
    };
  }

  let recoveredCode;
  try {
    recoveredCode = runWakaru(shape.lowered, `${snippet.name}-${shape.label.replaceAll(" ", "-")}.js`);
  } catch (error) {
    return {
      recovered: false,
      status: "wakaru-error",
      notes: error.message,
      failure: {
        bucket: snippet.bucket,
        snippet: snippet.name,
        shape: shape.label,
        tools: shape.tools,
        options: shape.profile.options,
        source: snippet.source,
        lowered: shape.lowered,
        recoveredCode: "",
        missing: ["wakaru execution failed"],
        informational: Boolean(snippet.informational),
        executionError: true,
      },
    };
  }

  const missing = bestMissingExpectation(snippet, recoveredCode);
  if (missing.length === 0) {
    return {
      recovered: true,
      status: snippet.informational ? "info-ok" : "yes",
      notes: summarize(shape.lowered),
    };
  }

  return {
    recovered: false,
    status: snippet.informational ? "info-miss" : "no",
    notes: `missing ${missing.map((token) => JSON.stringify(token)).join(", ")}`,
    failure: {
      bucket: snippet.bucket,
      snippet: snippet.name,
      shape: shape.label,
      tools: shape.tools,
      options: shape.profile.options,
      source: snippet.source,
      lowered: shape.lowered,
      recoveredCode,
      missing,
      informational: Boolean(snippet.informational),
    },
  };
}

function bestMissingExpectation(snippet, recoveredCode) {
  const groups = snippet.expectedAny ?? [snippet.expected];
  return groups
    .map((group) => group.filter((token) => !recoveredCode.includes(token)))
    .sort((left, right) => left.length - right.length)[0];
}

function runSwcMinify(source, options) {
  const toolDir = ensureNodeTool("swc", ["@swc/core@1"]);
  const helper = join(toolDir, "swc-minify.cjs");
  writeFileSync(
    helper,
    `
const fs = require("node:fs");
const swc = require("@swc/core");
const source = fs.readFileSync(0, "utf8");
const options = JSON.parse(process.env.SWC_MINIFY_OPTIONS);
const result = swc.minifySync(source, {
  ...options,
  format: {
    ascii_only: true,
    comments: false,
  },
  module: true,
});
process.stdout.write(result.code + "\\n");
`,
  );
  return runChecked("node", [helper], {
    input: source,
    cwd: toolDir,
    env: { SWC_MINIFY_OPTIONS: JSON.stringify(options) },
  });
}

function runWakaru(source, name) {
  const input = join(tmpRoot, name);
  writeFileSync(input, source);

  const configured = process.env.WAKARU;
  if (configured) {
    return runChecked(configured, ["--level", rewriteLevel, input]);
  }

  const debugBinary = join(
    repoRoot,
    "target",
    "debug",
    process.platform === "win32" ? "wakaru.exe" : "wakaru",
  );
  if (existsSync(debugBinary)) {
    return runChecked(debugBinary, ["--level", rewriteLevel, input]);
  }

  return runChecked("cargo", ["run", "-q", "-p", "wakaru-cli", "--", "--level", rewriteLevel, input], {
    cwd: repoRoot,
  });
}

function wakaruDescription() {
  const configured = process.env.WAKARU;
  if (configured) {
    return configured;
  }
  const debugBinary = join(
    repoRoot,
    "target",
    "debug",
    process.platform === "win32" ? "wakaru.exe" : "wakaru",
  );
  if (existsSync(debugBinary)) {
    return debugBinary;
  }
  return "cargo run -q -p wakaru-cli --";
}

function ensureNodeTool(name, packages) {
  const toolDir = join(toolRoot, name);
  const packageJson = join(toolDir, "package.json");
  const nodeModules = join(toolDir, "node_modules");

  mkdirSync(toolDir, { recursive: true });

  const expected = new Set(packages.map(packageName));
  const installed = packages.every((pkg) => existsSync(join(nodeModules, packageName(pkg))));
  if (existsSync(packageJson) && installed) {
    return toolDir;
  }

  rmSync(nodeModules, { recursive: true, force: true });
  rmSync(join(toolDir, "package-lock.json"), { force: true });
  writeFileSync(packageJson, JSON.stringify({ private: true, type: "commonjs" }, null, 2));
  runCommandScript("npm", ["install", "--silent", "--no-audit", "--no-fund", ...packages], {
    cwd: toolDir,
  });

  for (const pkg of expected) {
    if (!existsSync(join(nodeModules, pkg))) {
      throw new Error(`failed to install ${pkg}`);
    }
  }

  return toolDir;
}

function packageName(spec) {
  if (spec.startsWith("@")) {
    const versionIndex = spec.indexOf("@", 1);
    return versionIndex === -1 ? spec : spec.slice(0, versionIndex);
  }
  const versionIndex = spec.indexOf("@");
  return versionIndex === -1 ? spec : spec.slice(0, versionIndex);
}

function runCommandScript(command, args, options = {}) {
  if (process.platform !== "win32") {
    return runChecked(command, args, options);
  }
  return runChecked("cmd.exe", ["/d", "/s", "/c", `${command}.cmd`, ...args], options);
}

function runChecked(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: options.cwd ?? repoRoot,
    input: options.input,
    encoding: "utf8",
    maxBuffer: 20 * 1024 * 1024,
    env: {
      ...process.env,
      ...(options.env ?? {}),
    },
  });

  if (result.error) {
    throw result.error;
  }

  if (result.status !== 0) {
    const details = [result.stderr, result.stdout].filter(Boolean).join("\n").trim();
    throw new Error(`${command} ${args.join(" ")} failed${details ? `\n${details}` : ""}`);
  }

  return result.stdout;
}

function summarize(code) {
  const singleLine = code.replaceAll("\r\n", "\n").replace(/\s+/g, " ").trim();
  return singleLine.length <= 120 ? singleLine : `${singleLine.slice(0, 117)}...`;
}

function escapeCell(value) {
  return String(value).replaceAll("|", "\\|").replaceAll("\n", "<br>");
}

function shapeKey(code) {
  return code.replaceAll("\r\n", "\n").trim();
}

function readOption(name, fallback) {
  const index = process.argv.indexOf(name);
  if (index === -1) {
    return fallback;
  }
  const value = process.argv[index + 1];
  if (!value || value.startsWith("--")) {
    throw new Error(`${name} requires a value`);
  }
  return value;
}
