#!/usr/bin/env node

import { existsSync, mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const tmpRoot = mkdtempSync(join(tmpdir(), "wakaru-template-literal-"));
const toolRoot = join(repoRoot, "target", "repro-tools", "template-literal");
const showDetails = process.argv.includes("--details");
const rewriteLevel = readOption("--level", "standard");
const failures = [];

if (!["minimal", "standard", "aggressive"].includes(rewriteLevel)) {
  throw new Error(`unsupported --level ${rewriteLevel}`);
}

const snippets = [
  {
    name: "template-basic",
    source: "var out = `Hello ${name}!`;\nuse(out);\n",
    expected: ["`Hello ${name}!`"],
  },
  {
    name: "template-multiple-expressions",
    source: "var out = `${greeting}, ${user.name}! ${count} items`;\nuse(out);\n",
    expected: ["`${greeting}, ${user.name}! ${count} items`"],
  },
  {
    name: "template-expression-start-end",
    source: "var out = `${prefix}/users/${id}`;\nuse(out);\n",
    expected: ["`${prefix}/users/${id}`"],
  },
  {
    name: "template-escaped-newline",
    source: "var out = `line 1\\n${value}\\t${tail}`;\nuse(out);\n",
    expected: ["`line 1\\n${value}\\t${tail}`"],
  },
  {
    name: "template-nested-expression",
    source: "var out = `status: ${ok ? `ok ${name}` : \"bad\"}`;\nuse(out);\n",
    expected: ["`status: ${", "`ok ${name}`"],
  },
  {
    name: "tagged-basic",
    source: "var out = tag`hello ${name}`;\nuse(out);\n",
    expected: ["tag`hello ${name}`"],
  },
  {
    name: "tagged-raw-cooked",
    source: "var out = tag`line\\n${value}\\u{1f600}`;\nuse(out);\n",
    expected: ["tag`line\\n${value}\\u{1f600}`"],
  },
  {
    name: "tagged-member",
    source: "var out = css.div`color: ${color}; margin: ${space}px;`;\nuse(out);\n",
    expected: ["css.div`color: ${color}; margin: ${space}px;`"],
  },
];

const babelProfiles = [
  {
    name: "babel-7.8",
    core: "7.8.7",
    plugin: ["@babel/plugin-transform-template-literals", "7.8.3"],
    modes: ["spec", "loose"],
  },
  {
    name: "babel-7.13",
    core: "7.13.16",
    plugin: ["@babel/plugin-transform-template-literals", "7.13.0"],
    modes: ["spec", "loose", "mutableTemplateObject"],
  },
  {
    name: "babel-7.28",
    core: "7.28.5",
    plugin: ["@babel/plugin-transform-template-literals", "7.27.1"],
    modes: ["spec", "loose", "mutableTemplateObject"],
  },
  {
    name: "babel-8-rc",
    core: "8.0.0-rc.5",
    plugin: ["@babel/plugin-transform-template-literals", "8.0.0-rc.5"],
    modes: ["spec", "loose", "mutableTemplateObject"],
  },
];

const transformers = [
  ...babelProfiles.flatMap((profile) =>
    profile.modes.map((mode) => ({
      name: `${profile.name}-${mode}`,
      run: (source) => runBabel(source, profile, babelModeOptions(mode)),
    })),
  ),
  {
    name: "tsc-es5",
    run: runTsc,
  },
  {
    name: "swc-es5",
    run: runSwc,
  },
  {
    name: "esbuild-es5",
    run: runEsbuild,
  },
];

try {
  console.log("# Template literal reproduction matrix");
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

  if (showDetails && failures.length > 0) {
    console.log("");
    console.log("## Failure Details");
    for (const failure of failures) {
      console.log("");
      console.log(`### ${failure.snippet} / ${failure.shape}`);
      console.log("");
      console.log(`Tools: ${failure.tools.join(", ")}`);
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
    return { recovered: true, notes: "expected syntax present" };
  }

  const loweredShape = summarize(shape.lowered);
  const recoveredShape = summarize(recovered);
  return {
    recovered: false,
    notes: `missing ${missing.join(", ")}; lowered: ${loweredShape}; wakaru: ${recoveredShape}`,
    failure: {
      snippet: snippet.name,
      shape: shape.label,
      tools: shape.tools,
      lowered: shape.lowered,
      recovered,
    },
  };
}

function babelModeOptions(mode) {
  switch (mode) {
    case "spec":
      return { assumptions: {}, pluginOptions: {} };
    case "loose":
      return { assumptions: {}, pluginOptions: { loose: true } };
    case "mutableTemplateObject":
      return { assumptions: { mutableTemplateObject: true }, pluginOptions: {} };
    default:
      throw new Error(`unsupported Babel mode ${mode}`);
  }
}

function runBabel(source, profile, options) {
  const [pluginName, pluginVersion] = profile.plugin;
  const toolDir = ensureNodeTool(`babel-${profile.core}`, [
    `@babel/core@${profile.core}`,
    `${pluginName}@${pluginVersion}`,
  ]);
  const helper = join(toolDir, "babel-transform.mjs");
  writeFileSync(
    helper,
    `
import fs from "node:fs";

const babelModule = await import("@babel/core");
const pluginModule = await import(${JSON.stringify(pluginName)});
const babel = babelModule.default ?? babelModule;
const templateLiterals = pluginModule.default ?? pluginModule;
const source = fs.readFileSync(0, "utf8");
const options = JSON.parse(process.env.MATRIX_BABEL_OPTIONS || "{}");
const transformOptions = {
  filename: "input.js",
  babelrc: false,
  configFile: false,
  comments: false,
  compact: false,
  plugins: [[templateLiterals, options.pluginOptions || {}]],
};
if (options.assumptions && Object.keys(options.assumptions).length > 0) {
  transformOptions.assumptions = options.assumptions;
}
const result = babel.transformSync(source, transformOptions);
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
  const toolDir = ensureNodeTool("esbuild-0.28", ["esbuild@0.28.0"]);
  const helper = join(toolDir, "esbuild-transform.cjs");
  writeFileSync(
    helper,
    `
const fs = require("node:fs");
const esbuild = require("esbuild");
const source = fs.readFileSync(0, "utf8");
const result = esbuild.transformSync(source, {
  target: "es5",
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
  const markerText = packages.join("\n");
  if (existsSync(marker)) {
    return dir;
  }

  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, "package.json"), JSON.stringify({ private: true, type: "commonjs" }, null, 2));
  runCommandScript("npm", ["install", "--silent", "--no-audit", "--no-fund", ...packages], { cwd: dir });
  writeFileSync(marker, markerText);
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
