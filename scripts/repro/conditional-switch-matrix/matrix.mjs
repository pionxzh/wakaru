#!/usr/bin/env node

import { existsSync, mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const tmpRoot = mkdtempSync(join(tmpdir(), "wakaru-conditional-switch-"));
const toolRoot = join(repoRoot, "target", "repro-tools", "conditional-switch");
const showDetails = process.argv.includes("--details");
const rewriteLevel = readOption("--level", "standard");
const failures = [];

if (!["minimal", "standard", "aggressive"].includes(rewriteLevel)) {
  throw new Error(`unsupported --level ${rewriteLevel}`);
}

const snippets = [
  {
    name: "strict-return-chain",
    source:
      'export function f(kind) { return kind === "bar" ? bar() : kind === "baz" ? baz() : kind === "qux" ? qux() : quux(); }\n',
    expected: ["switch(", 'case "bar"', 'case "baz"', 'case "qux"', "default:"],
  },
  {
    name: "strict-statement-chain",
    source:
      'export function f(kind) { kind === "bar" ? bar() : kind === "baz" ? baz() : kind === "qux" ? qux() : quux(); }\n',
    expected: ["switch(", 'case "bar"', 'case "baz"', 'case "qux"', "default:"],
  },
  {
    name: "preserved-switch-return",
    source:
      'export function f(kind) { switch (kind) { case "bar": return bar(); case "baz": return baz(); case "qux": return qux(); default: return quux(); } }\n',
    expected: ["switch(", 'case "bar"', 'case "baz"', 'case "qux"', "default:"],
  },
  {
    name: "loose-return-chain",
    source:
      'export function f(kind) { return "bar" == kind ? bar() : "baz" == kind ? baz() : quux(); }\n',
    expected: ["=="],
    note: "Loose equality is intentionally not a switch-recovery target.",
  },
];

const transformers = [
  {
    name: "terser-5",
    run: runTerser,
  },
  {
    name: "esbuild-0.28",
    run: runEsbuild,
  },
  {
    name: "swc-1",
    run: runSwc,
  },
  {
    name: "swc-1-terser",
    run: (source) => runTerser(runSwc(source)),
  },
];

try {
  console.log("# Conditional switch reproduction matrix");
  console.log(`# wakaru: ${wakaruDescription()}`);
  console.log(`# level: ${rewriteLevel}`);
  console.log("");
  console.log("| snippet | shape | tools | recovered | notes |");
  console.log("|---|---:|---|---:|---|");

  for (const snippet of snippets) {
    const shapes = collectShapes(snippet);
    for (const shape of shapes) {
      const result = runShape(snippet, shape);
      if (!result.recovered && result.failure) {
        failures.push(result.failure);
      }
      console.log(
        `| ${snippet.name} | ${shape.label} | ${escapeCell(shape.tools.join(", "))} | ${
          result.recovered ? "yes" : "no"
        } | ${escapeCell(result.notes)} |`,
      );
    }
  }

  if (showDetails || failures.length > 0) {
    for (const failure of failures) {
      console.log("");
      console.log(`## ${failure.snippet} / ${failure.shape}`);
      console.log(`tools: ${failure.tools.join(", ")}`);
      console.log("");
      console.log("### lowered");
      console.log("```js");
      console.log(failure.lowered.trim());
      console.log("```");
      console.log("");
      console.log("### wakaru");
      console.log("```js");
      console.log(failure.recovered.trim());
      console.log("```");
    }
  }
} finally {
  rmSync(tmpRoot, { recursive: true, force: true });
}

function collectShapes(snippet) {
  const groups = new Map();
  const shapes = [];

  for (const transformer of transformers) {
    let lowered;
    try {
      lowered = transformer.run(snippet.source);
    } catch (error) {
      shapes.push({
        label: "transform-failed",
        tools: [transformer.name],
        transformError: error,
      });
      continue;
    }

    const key = shapeKey(lowered);
    const existing = groups.get(key);
    if (existing) {
      existing.tools.push(transformer.name);
      continue;
    }

    const shape = {
      label: `shape ${groups.size + 1}`,
      tools: [transformer.name],
      lowered,
    };
    groups.set(key, shape);
    shapes.push(shape);
  }

  return shapes;
}

function runShape(snippet, shape) {
  if (shape.transformError) {
    return { recovered: false, notes: `transform failed: ${shape.transformError.message}` };
  }

  let recovered;
  try {
    recovered = runWakaru(shape.lowered, `${snippet.name}-${shape.label.replaceAll(" ", "-")}.js`);
  } catch (error) {
    return { recovered: false, notes: `wakaru failed: ${error.message}` };
  }

  const missing = snippet.expected.filter((needle) => !recovered.includes(needle));
  if (missing.length === 0) {
    return { recovered: true, notes: snippet.note ?? "expected syntax present" };
  }

  return {
    recovered: false,
    notes: `missing ${missing.join(", ")}; lowered: ${summarize(shape.lowered)}; wakaru: ${summarize(
      recovered,
    )}`,
    failure: {
      snippet: snippet.name,
      shape: shape.label,
      tools: shape.tools,
      lowered: shape.lowered,
      recovered,
    },
  };
}

function runTerser(source) {
  const toolDir = ensureNodeTool("terser", ["terser@5"]);
  const helper = join(toolDir, "terser-transform.mjs");
  writeFileSync(
    helper,
    `
import fs from "node:fs";
import { minify } from "terser";
const source = fs.readFileSync(0, "utf8");
const result = await minify(source, {
  module: true,
  compress: { defaults: true, unused: false },
  mangle: false,
  format: { comments: false },
});
process.stdout.write(result.code + "\\n");
`,
  );
  return runChecked("node", [helper], { input: source, cwd: toolDir });
}

function runEsbuild(source) {
  const toolDir = ensureNodeTool("esbuild-0.28", ["esbuild@0.28.0"]);
  const helper = join(toolDir, "esbuild-transform.cjs");
  writeFileSync(
    helper,
    `
const fs = require("node:fs");
const esbuild = require("esbuild");
const source = fs.readFileSync(0, "utf8");
const result = esbuild.transformSync(source, {
  target: "es2015",
  format: "esm",
  loader: "js",
  minify: true,
  logLevel: "warning",
});
process.stdout.write(result.code);
`,
  );
  return runChecked("node", [helper], { input: source, cwd: toolDir });
}

function runSwc(source) {
  const toolDir = ensureNodeTool("swc", ["@swc/core@1"]);
  const helper = join(toolDir, "swc-transform.cjs");
  writeFileSync(
    helper,
    `
const fs = require("node:fs");
const swc = require("@swc/core");
const source = fs.readFileSync(0, "utf8");
const result = swc.transformSync(source, {
  filename: "input.js",
  jsc: {
    target: "es5",
    parser: { syntax: "ecmascript" },
    minify: { compress: true, mangle: true },
  },
  module: { type: "es6" },
  minify: true,
});
process.stdout.write(result.code);
`,
  );
  return runChecked("node", [helper], { input: source, cwd: toolDir });
}

function runWakaru(source, name) {
  const input = join(tmpRoot, name);
  writeFileSync(input, source);
  const configured = process.env.WAKARU;
  if (configured) {
    return runChecked(configured, ["--level", rewriteLevel, input]);
  }

  const debugBinary = join(repoRoot, "target", "debug", process.platform === "win32" ? "wakaru.exe" : "wakaru");
  try {
    return runChecked(debugBinary, ["--level", rewriteLevel, input]);
  } catch {
    return runChecked("cargo", ["run", "-q", "-p", "wakaru-cli", "--", "--level", rewriteLevel, input], {
      cwd: repoRoot,
    });
  }
}

function ensureNodeTool(name, packages) {
  const dir = join(toolRoot, name);
  const marker = join(dir, ".installed");
  if (existsSync(marker)) {
    return dir;
  }

  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, "package.json"), JSON.stringify({ private: true, type: "commonjs" }, null, 2));
  runCommandScript("npm", ["install", "--silent", "--no-audit", "--no-fund", ...packages], { cwd: dir });
  writeFileSync(marker, packages.join("\n"));
  return dir;
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
    maxBuffer: 1024 * 1024 * 20,
    shell: options.shell ?? false,
    env: { ...process.env, ...(options.env ?? {}) },
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    const detail = [result.stderr.trim(), result.stdout.trim()].filter(Boolean).join(" ");
    throw new Error(`${basename(command)} exited ${result.status}: ${detail}`);
  }
  return result.stdout;
}

function wakaruDescription() {
  if (process.env.WAKARU) {
    return process.env.WAKARU;
  }
  return join(repoRoot, "target", "debug", process.platform === "win32" ? "wakaru.exe" : "wakaru");
}

function summarize(code) {
  return code.replaceAll(/\s+/g, " ").trim().slice(0, 160).replaceAll("|", "\\|");
}

function escapeCell(value) {
  return value.replaceAll("|", "\\|").replaceAll("\n", " ");
}

function shapeKey(code) {
  return code.replaceAll("\r\n", "\n").trim();
}

function readOption(name, fallback) {
  const equalsArg = process.argv.find((arg) => arg.startsWith(`${name}=`));
  if (equalsArg) {
    return equalsArg.slice(name.length + 1);
  }
  const index = process.argv.indexOf(name);
  if (index !== -1) {
    return process.argv[index + 1] ?? fallback;
  }
  return fallback;
}
