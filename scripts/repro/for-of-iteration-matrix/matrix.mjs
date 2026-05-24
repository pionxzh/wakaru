#!/usr/bin/env node

import { existsSync, mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const tmpRoot = mkdtempSync(join(tmpdir(), "wakaru-for-of-iteration-"));
const toolRoot = join(repoRoot, "target", "repro-tools", "for-of-iteration");
const showDetails = process.argv.includes("--details");
const rewriteLevel = readOption("--level", "standard");
const failures = [];

if (!["minimal", "standard", "aggressive"].includes(rewriteLevel)) {
  throw new Error(`unsupported --level ${rewriteLevel}`);
}

const snippets = [
  {
    name: "array-basic",
    source: "export function f(items) { for (const item of items) { use(item); } }\n",
    expected: ["for (const item of items)", "use(item)"],
    shouldRecover: true,
  },
  {
    name: "array-reassign",
    source: "export function f(items) { for (let item of items) { item = normalize(item); use(item); } }\n",
    expected: ["for (let item of items)", "item = normalize(item)", "use(item)"],
    shouldRecover: true,
  },
  {
    name: "destructuring-pair",
    source: "export function f(entries) { for (const [key, value] of entries) { use(key, value); } }\n",
    expected: ["for (const [key, value] of entries)", "use(key, value)"],
    shouldRecover: true,
  },
];

const transformers = [
  {
    name: "tsc-es5-downlevel-false",
    run: (source) => runTsc(source, false),
    shouldRecover: true,
  },
  {
    name: "tsc-es5-downlevel-false-terser",
    run: (source) => runTerser(runTsc(source, false)),
    shouldRecover: true,
  },
  {
    name: "tsc-es5-downlevel-true",
    run: (source) => runTsc(source, true),
    shouldRecover: true,
  },
  {
    name: "tsc-es5-downlevel-true-terser",
    run: (source) => runTerser(runTsc(source, true)),
    shouldRecover: true,
  },
  {
    name: "babel-7.28-spec",
    run: (source) => runBabel(source, "spec"),
    shouldRecover: true,
  },
  {
    name: "babel-7.28-spec-terser",
    run: (source) => runTerser(runBabel(source, "spec")),
    shouldRecover: true,
  },
  {
    name: "babel-7.28-loose",
    run: (source) => runBabel(source, "loose"),
    shouldRecover: true,
  },
  {
    name: "babel-7.28-loose-terser",
    run: (source) => runTerser(runBabel(source, "loose")),
    shouldRecover: true,
  },
  {
    name: "babel-7.28-iterableIsArray",
    run: (source) => runBabel(source, "iterableIsArray"),
    shouldRecover: true,
  },
  {
    name: "babel-7.28-iterableIsArray-terser",
    run: (source) => runTerser(runBabel(source, "iterableIsArray")),
    shouldRecover: true,
  },
  {
    name: "swc-es5",
    run: runSwc,
    shouldRecover: true,
  },
  {
    name: "swc-es5-terser",
    run: (source) => runTerser(runSwc(source)),
    shouldRecover: true,
  },
  {
    name: "esbuild-es2015",
    run: runEsbuild,
    shouldRecover: true,
    note: "esbuild preserves for-of at ES2015; ES5 for-of lowering is not supported.",
  },
  {
    name: "esbuild-es2015-terser",
    run: (source) => runTerser(runEsbuild(source)),
    shouldRecover: true,
    note: "esbuild preserves for-of at ES2015; ES5 for-of lowering is not supported.",
  },
  {
    name: "terser-5",
    run: runTerser,
    shouldRecover: true,
  },
];

try {
  console.log("# For-of iteration reproduction matrix");
  console.log(`# wakaru: ${wakaruDescription()}`);
  console.log(`# level: ${rewriteLevel}`);
  console.log("");
  console.log("| snippet | shape | tools | recovered | notes |");
  console.log("|---|---:|---|---:|---|");

  for (const snippet of snippets) {
    const shapes = collectShapes(snippet);
    for (const shape of shapes) {
      const result = runShape(snippet, shape);
      if (!result.ok && result.failure) {
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
      console.log(failure.recoveredCode.trim());
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
      existing.shouldRecover ||= transformer.shouldRecover;
      if (transformer.note) {
        existing.notes.push(transformer.note);
      }
      continue;
    }

    const shape = {
      label: `shape ${groups.size + 1}`,
      tools: [transformer.name],
      lowered,
      shouldRecover: transformer.shouldRecover,
      notes: transformer.note ? [transformer.note] : [],
    };
    groups.set(key, shape);
    shapes.push(shape);
  }

  return shapes;
}

function runShape(snippet, shape) {
  if (shape.transformError) {
    return {
      ok: false,
      recovered: false,
      notes: `transform failed: ${shape.transformError.message}`,
    };
  }

  let recoveredCode;
  try {
    recoveredCode = runWakaru(shape.lowered, `${snippet.name}-${shape.label.replaceAll(" ", "-")}.js`);
  } catch (error) {
    return { ok: false, recovered: false, notes: `wakaru failed: ${error.message}` };
  }

  const missing = snippet.expected.filter((needle) => !recoveredCode.includes(needle));
  const recovered = missing.length === 0;
  const shouldRecover = snippet.shouldRecover && shape.shouldRecover;

  if (shouldRecover && recovered) {
    return { ok: true, recovered, notes: "expected for-of syntax present" };
  }
  if (!shouldRecover && !recovered) {
    return {
      ok: true,
      recovered,
      notes: shape.notes.length > 0 ? shape.notes.join("; ") : "not an indexed loop target",
    };
  }
  if (!shouldRecover && recovered) {
    return { ok: true, recovered, notes: "recovered beyond current expectation" };
  }

  return {
    ok: false,
    recovered,
    notes: `missing ${missing.join(", ")}; lowered: ${summarize(shape.lowered)}; wakaru: ${summarize(
      recoveredCode,
    )}`,
    failure: {
      snippet: snippet.name,
      shape: shape.label,
      tools: shape.tools,
      lowered: shape.lowered,
      recoveredCode,
    },
  };
}

function runTsc(source, downlevelIteration) {
  const toolDir = ensureNodeTool("typescript", ["typescript@5"]);
  const helper = join(toolDir, "tsc-transform.cjs");
  writeFileSync(
    helper,
    `
const fs = require("node:fs");
const ts = require("typescript");
const source = fs.readFileSync(0, "utf8");
const result = ts.transpileModule(source, {
  compilerOptions: {
    target: ts.ScriptTarget.ES5,
    module: ts.ModuleKind.ESNext,
    downlevelIteration: ${JSON.stringify(downlevelIteration)},
  },
});
process.stdout.write(result.outputText);
`,
  );
  return runChecked("node", [helper], { input: source, cwd: toolDir });
}

function runBabel(source, mode) {
  const toolDir = ensureNodeTool("babel-7.28", [
    "@babel/core@7.28.5",
    "@babel/plugin-transform-for-of@7.27.1",
    "@babel/plugin-transform-destructuring@7.28.5",
  ]);
  const helper = join(toolDir, "babel-transform.mjs");
  writeFileSync(
    helper,
    `
import fs from "node:fs";
const babelModule = await import("@babel/core");
const forOfModule = await import("@babel/plugin-transform-for-of");
const destructuringModule = await import("@babel/plugin-transform-destructuring");
const babel = babelModule.default ?? babelModule;
const forOf = forOfModule.default ?? forOfModule;
const destructuring = destructuringModule.default ?? destructuringModule;
const source = fs.readFileSync(0, "utf8");
const mode = process.env.MATRIX_BABEL_MODE;
const assumptions = mode === "iterableIsArray" ? { iterableIsArray: true } : {};
const pluginOptions = mode === "loose" ? { loose: true } : {};
const result = babel.transformSync(source, {
  filename: "input.js",
  babelrc: false,
  configFile: false,
  comments: false,
  compact: false,
  assumptions,
  plugins: [[forOf, pluginOptions], [destructuring, {}]],
});
process.stdout.write(result.code + "\\n");
`,
  );
  return runChecked("node", [helper], {
    input: source,
    cwd: toolDir,
    env: { MATRIX_BABEL_MODE: mode },
  });
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
  },
  module: { type: "es6" },
});
process.stdout.write(result.code);
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
  loader: "js",
  target: "es2015",
  format: "esm",
});
process.stdout.write(result.code);
`,
  );
  return runChecked("node", [helper], { input: source, cwd: toolDir });
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
