#!/usr/bin/env node

import { existsSync, mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const tmpRoot = mkdtempSync(join(tmpdir(), "wakaru-async-await-"));
const toolRoot = join(repoRoot, "target", "repro-tools", "async-await");
const showDetails = process.argv.includes("--details");
const rewriteLevel = readOption("--level", "standard");
const failures = [];

if (!["minimal", "standard", "aggressive"].includes(rewriteLevel)) {
  throw new Error(`unsupported --level ${rewriteLevel}`);
}

const snippets = [
  {
    name: "async-simple-await",
    source: "async function load_user(app_id) {\n  await fetch_user(app_id);\n}\n",
    expected: ["async function load_user(app_id)", "await fetch_user(app_id)"],
  },
  {
    name: "async-return-value",
    source:
      "async function load_user(app_id) {\n  const response = await fetch_user(app_id);\n  const data = await response.json();\n  return data;\n}\n",
    expected: ["async function load_user(app_id)", "await fetch_user(app_id)", "await response.json()", "return data"],
  },
  {
    name: "async-try-catch",
    source:
      "async function load_user(app_id) {\n  try {\n    return await fetch_user(app_id);\n  } catch (error) {\n    return fallback_user(error);\n  }\n}\n",
    expected: ["async function load_user(app_id)", "try", "return await fetch_user(app_id)", "catch"],
  },
  {
    name: "async-try-finally-await",
    source:
      "async function save_record(record) {\n  const lock = await acquire_lock(record.id);\n  try {\n    const payload = await prepare_record(record);\n    return await commit_record(payload);\n  } finally {\n    await lock.release();\n  }\n}\n",
    expected: [
      "async function save_record(record)",
      "await acquire_lock(record.id)",
      "try",
      "await prepare_record(record)",
      "return await commit_record(payload)",
      "finally",
      "await lock.release()",
    ],
  },
  {
    name: "async-loop-try-catch",
    source:
      "async function collect_enabled(items) {\n  const output = [];\n  for (let index = 0; index < items.length; index++) {\n    const item = items[index];\n    if (!item.enabled) {\n      continue;\n    }\n    try {\n      output.push(await fetch_item(item.id));\n    } catch (error) {\n      output.push(await recover_item(item, error));\n    }\n  }\n  return output;\n}\n",
    expected: [
      "async function collect_enabled(items)",
      "for (let index = 0",
      "const item = items[index]",
      "continue",
      "try",
      "await fetch_item(item.id)",
      "catch",
      "await recover_item(item, error)",
      "return output",
    ],
  },
  {
    name: "async-destructuring-default-await",
    source:
      "async function normalize_user(input) {\n  const source = input == null ? await load_user() : input;\n  const { id, profile: { name } = {}, tags: [primary, , backup] = [] } = source;\n  const resolved_backup = backup == null ? await load_backup(id) : backup;\n  const meta = await load_meta(id);\n  return { id, name, primary, backup: resolved_backup, meta };\n}\n",
    expected: [
      "async function normalize_user(input)",
      "const source = input == null ? await load_user() : input",
      "profile: { name }",
      "tags: [primary, , backup]",
      "await load_backup(id)",
      "await load_meta(id)",
      "return {",
      "backup: resolved_backup",
    ],
  },
  {
    name: "async-arrow",
    source: "const load_user = async (app_id) => await fetch_user(app_id);\nuse(load_user);\n",
    expected: ["async (app_id)", "await fetch_user(app_id)"],
  },
  {
    name: "async-arrow-nested-awaits",
    source:
      "const run_pipeline = async (source) => {\n  const steps = await load_steps(source);\n  return steps.map(async (step) => await step.run(source));\n};\nuse(run_pipeline);\n",
    expected: [
      "const run_pipeline = async (source)",
      "await load_steps(source)",
      "steps.map(async (step)",
      "await step.run(source)",
    ],
  },
  {
    name: "async-arrow-object-rest",
    source:
      "const load_user = async (config) => {\n  const source = config == null ? await load_config() : config;\n  const { id, token, ...options } = source;\n  const session = await open_session(token);\n  return await fetch_user(id, { ...options, session });\n};\nuse(load_user);\n",
    expected: [
      "const load_user = async (config)",
      "{ id, token, ...options }",
      "const source = config == null ? await load_config() : config",
      "await load_config()",
      "await open_session(token)",
      "return await fetch_user(id, {",
      "...options",
    ],
  },
  {
    name: "generator-basic",
    source: "function* read_items(items) {\n  yield first_item(items);\n  yield second_item(items);\n}\n",
    expected: ["function* read_items(items)", "yield first_item(items)", "yield second_item(items)"],
  },
  {
    name: "generator-try-finally-delegate",
    source:
      "function* read_all(source) {\n  try {\n    yield start_read(source);\n    yield* read_chunks(source);\n    return yield finish_read(source);\n  } finally {\n    yield close_reader(source);\n  }\n}\n",
    expected: [
      "function* read_all(source)",
      "try",
      "yield start_read(source)",
      "yield* read_chunks(source)",
      "return yield finish_read(source)",
      "finally",
      "yield close_reader(source)",
    ],
  },
];

const babelProfiles = [
  {
    name: "babel-7.8",
    core: "7.8.7",
    asyncPlugin: ["@babel/plugin-transform-async-to-generator", "7.8.3"],
    regeneratorPlugin: ["@babel/plugin-transform-regenerator", "7.8.7"],
  },
  {
    name: "babel-7.13",
    core: "7.13.16",
    asyncPlugin: ["@babel/plugin-transform-async-to-generator", "7.13.0"],
    regeneratorPlugin: ["@babel/plugin-transform-regenerator", "7.13.15"],
  },
  {
    name: "babel-7.28",
    core: "7.28.5",
    asyncPlugin: ["@babel/plugin-transform-async-to-generator", "7.28.6"],
    regeneratorPlugin: ["@babel/plugin-transform-regenerator", "7.28.4"],
  },
  {
    name: "babel-8-rc",
    core: "8.0.0-rc.5",
    asyncPlugin: ["@babel/plugin-transform-async-to-generator", "8.0.0-rc.5"],
    regeneratorPlugin: ["@babel/plugin-transform-regenerator", "8.0.0-rc.5"],
  },
];

const transformers = [
  ...babelProfiles.flatMap((profile) =>
    ["async-generator", "regenerator"].flatMap((mode) => [
      {
        name: `${profile.name}-${mode}`,
        run: (source) => runBabel(source, profile, mode),
      },
      {
        name: `${profile.name}-${mode}-terser`,
        run: (source) => runTerser(runBabel(source, profile, mode)),
      },
    ]),
  ),
  {
    name: "tsc-es5",
    run: runTsc,
  },
  {
    name: "tsc-es5-terser",
    run: (source) => runTerser(runTsc(source)),
  },
  {
    name: "swc-es5",
    run: runSwc,
  },
  {
    name: "swc-es5-terser",
    run: (source) => runTerser(runSwc(source)),
  },
  {
    name: "esbuild-es2015",
    run: runEsbuild,
  },
  {
    name: "esbuild-es2015-terser",
    run: (source) => runTerser(runEsbuild(source)),
  },
  {
    name: "terser-5",
    run: runTerser,
  },
];

try {
  console.log("# Async/await reproduction matrix");
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

function runBabel(source, profile, mode) {
  const [asyncName, asyncVersion] = profile.asyncPlugin;
  const [regeneratorName, regeneratorVersion] = profile.regeneratorPlugin;
  const packages = [`@babel/core@${profile.core}`, `${asyncName}@${asyncVersion}`];
  if (mode === "regenerator") {
    packages.push(`${regeneratorName}@${regeneratorVersion}`);
  }
  const toolDir = ensureNodeTool(`babel-${profile.core}-${mode}`, packages);
  const helper = join(toolDir, "babel-transform.mjs");
  writeFileSync(
    helper,
    `
import fs from "node:fs";

const babelModule = await import("@babel/core");
const asyncModule = await import(${JSON.stringify(asyncName)});
const babel = babelModule.default ?? babelModule;
const asyncToGenerator = asyncModule.default ?? asyncModule;
const source = fs.readFileSync(0, "utf8");
const mode = process.env.MATRIX_BABEL_MODE;
const plugins = [asyncToGenerator];
if (mode === "regenerator") {
  const regeneratorModule = await import(${JSON.stringify(regeneratorName)});
  plugins.push(regeneratorModule.default ?? regeneratorModule);
}
const result = babel.transformSync(source, {
  filename: "input.js",
  babelrc: false,
  configFile: false,
  comments: false,
  compact: false,
  plugins,
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
