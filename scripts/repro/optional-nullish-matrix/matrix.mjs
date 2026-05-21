#!/usr/bin/env node

import { existsSync, mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const tmpRoot = mkdtempSync(join(tmpdir(), "wakaru-optional-nullish-"));
const toolRoot = join(repoRoot, "target", "repro-tools", "optional-nullish");
const showDetails = process.argv.includes("--details");
const rewriteLevel = readOption("--level", "standard");
const failures = [];

if (!["minimal", "standard", "aggressive"].includes(rewriteLevel)) {
  throw new Error(`unsupported --level ${rewriteLevel}`);
}

const snippets = [
  {
    name: "member-chain-nullish",
    source: "const out = obj?.foo?.bar ?? fallback;\n",
    expected: ["?.", "??"],
  },
  {
    name: "mixed-leading-required-members",
    source: "const out = obj.foo?.bar.baz?.qux;\n",
    expected: ["obj.foo?.bar.baz?.qux"],
  },
  {
    name: "mixed-leading-optional-member",
    source: "const out = obj?.foo.bar?.baz.qux;\n",
    expected: ["obj?.foo.bar?.baz.qux"],
  },
  {
    name: "optional-call-nullish",
    source: "const out = obj?.method?.(arg) ?? fallback;\n",
    expected: ["?.", "??"],
  },
  {
    name: "nested-receiver-call",
    source: "const out = obj?.foo?.method?.(arg);\n",
    expected: ["?.method?.("],
  },
  {
    name: "computed-member-nullish",
    source: "const out = obj?.[key]?.value ?? fallback;\n",
    expected: ["?.[", "??"],
  },
  {
    name: "nullish-only",
    source: "const out = value ?? fallback;\n",
    expected: ["??"],
  },
  {
    name: "optional-after-nullish",
    source: "const out = (obj?.foo ?? fallback)?.bar;\n",
    expected: ["??", "?.bar"],
  },
];

const transformers = [
  {
    name: "babel-spec",
    run: (source) =>
      runBabel(source, {
        assumptions: {},
        pluginOptions: {},
      }),
  },
  {
    name: "babel-noDocumentAll",
    run: (source) =>
      runBabel(source, {
        assumptions: { noDocumentAll: true },
        pluginOptions: {},
      }),
  },
  {
    name: "babel-loose",
    run: (source) =>
      runBabel(source, {
        assumptions: {},
        pluginOptions: { loose: true },
      }),
  },
  {
    name: "tsc-es5",
    run: runTsc,
  },
  {
    name: "swc-es5",
    run: runSwc,
  },
  {
    name: "esbuild-es2015",
    run: runEsbuild,
  },
];

try {
  console.log(`# Optional/nullish reproduction matrix`);
  console.log(`# wakaru: ${wakaruDescription()}`);
  console.log(`# level: ${rewriteLevel}`);
  console.log("");
  console.log("| snippet | tool | recovered | notes |");
  console.log("|---|---|---:|---|");

  for (const snippet of snippets) {
    for (const transformer of transformers) {
      const result = runCase(snippet, transformer);
      if (!result.recovered && result.failure) {
        failures.push(result.failure);
      }
      console.log(
        `| ${snippet.name} | ${transformer.name} | ${result.recovered ? "yes" : "no"} | ${escapeCell(
          result.notes,
        )} |`,
      );
    }
  }

  if (showDetails && failures.length > 0) {
    console.log("");
    console.log("## Failure Details");
    for (const failure of failures) {
      console.log("");
      console.log(`### ${failure.snippet} / ${failure.tool}`);
      console.log("");
      console.log("Lowered:");
      console.log("```js");
      console.log(failure.lowered.trim());
      console.log("```");
      console.log("");
      console.log("Wakaru:");
      console.log("```js");
      console.log(failure.recovered.trim());
      console.log("```");
    }
  }
} finally {
  rmSync(tmpRoot, { recursive: true, force: true });
}

function runCase(snippet, transformer) {
  let lowered;
  try {
    lowered = transformer.run(snippet.source);
  } catch (error) {
    return { recovered: false, notes: `transform failed: ${error.message}` };
  }

  let recovered;
  try {
    recovered = runWakaru(lowered, `${snippet.name}-${transformer.name}.js`);
  } catch (error) {
    return { recovered: false, notes: `wakaru failed: ${error.message}` };
  }

  const missing = snippet.expected.filter((needle) => !recovered.includes(needle));
  if (missing.length === 0) {
    return { recovered: true, notes: "expected syntax present" };
  }

  const loweredShape = summarize(lowered);
  const recoveredShape = summarize(recovered);
  return {
    recovered: false,
    notes: `missing ${missing.join(", ")}; lowered: ${loweredShape}; wakaru: ${recoveredShape}`,
    failure: {
      snippet: snippet.name,
      tool: transformer.name,
      lowered,
      recovered,
    },
  };
}

function runBabel(source, options) {
  const toolDir = ensureNodeTool("babel", [
    "@babel/core@7",
    "@babel/plugin-transform-optional-chaining@7",
    "@babel/plugin-transform-nullish-coalescing-operator@7",
  ]);
  const helper = join(toolDir, "babel-transform.cjs");
  writeFileSync(
    helper,
    `
const fs = require("node:fs");
const babel = require("@babel/core");
const optional = require("@babel/plugin-transform-optional-chaining");
const nullish = require("@babel/plugin-transform-nullish-coalescing-operator");
const source = fs.readFileSync(0, "utf8");
const options = JSON.parse(process.env.MATRIX_BABEL_OPTIONS || "{}");
const result = babel.transformSync(source, {
  filename: "input.js",
  babelrc: false,
  configFile: false,
  comments: false,
  compact: false,
  assumptions: options.assumptions || {},
  plugins: [
    [optional, options.pluginOptions || {}],
    [nullish, options.pluginOptions || {}],
  ],
});
process.stdout.write(result.code + "\\n");
`,
  );
  return runChecked("node", [helper], {
    input: source,
    cwd: toolDir,
    env: { MATRIX_BABEL_OPTIONS: JSON.stringify(options) },
  });
}

function runTsc(source) {
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
  },
});
process.stdout.write(result.outputText);
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
  },
  module: { type: "es6" },
});
process.stdout.write(result.code);
`,
  );
  return runChecked("node", [helper], { input: source, cwd: toolDir });
}

function runEsbuild(source) {
  const toolDir = ensureNodeTool("esbuild", ["esbuild@0.25"]);
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
  logLevel: "warning",
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
  const debugBinary = join(repoRoot, "target", "debug", process.platform === "win32" ? "wakaru.exe" : "wakaru");
  return debugBinary;
}

function summarize(code) {
  return code.replaceAll(/\s+/g, " ").trim().slice(0, 160).replaceAll("|", "\\|");
}

function escapeCell(value) {
  return value.replaceAll("|", "\\|").replaceAll("\n", " ");
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
