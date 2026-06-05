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
    name: "logical-and-negated-member-call",
    source: "const hidden = !settings?.role?.blocks?.hideList.includes(blockName);\n",
    expected: ["!settings?.role?.blocks?.hideList.includes(blockName)"],
  },
  {
    name: "logical-and-duplicated-access",
    source:
      'if (root?.[key]?.group?.items && root?.[key]?.group?.items.length) {\n  sink("clear");\n} else {\n  sink("fill");\n}\n',
    expected: ["root?.[key]?.group?.items && root?.[key]?.group?.items.length"],
  },
  {
    name: "logical-and-prefix-loose-comparisons",
    source:
      'if (item?.meta?.enabled && item.kind != "alpha" && item.kind != "beta" && item.kind != "gamma") {\n  sink(item);\n}\n',
    expected: ['item?.meta?.enabled && item.kind != "alpha"'],
  },
  {
    name: "logical-and-prefix-strict-comparisons",
    source:
      'if (item?.meta?.enabled && item.kind !== "alpha" && item.kind !== "beta" && item.kind !== "gamma") {\n  sink(item);\n}\n',
    expected: ['item?.meta?.enabled && item.kind !== "alpha"'],
  },
  {
    name: "logical-and-prefix-includes-condition",
    source:
      'const blockedKinds = ["alpha", "beta", "gamma"];\nif (item?.meta?.enabled && !blockedKinds.includes(item.kind)) {\n  sink(item);\n}\n',
    expected: ["item?.meta?.enabled && !blockedKinds.includes(item.kind)"],
  },
  {
    name: "nullish-only",
    source: "const out = value ?? fallback;\n",
    expected: ["??"],
  },
  {
    name: "nullish-with-optional-middle",
    source: "const out = foo ?? bar?.prop ?? baz;\n",
    expected: ["foo ??", "bar?.prop ?? baz"],
  },
  {
    name: "optional-after-nullish",
    source: "const out = (obj?.foo ?? fallback)?.bar;\n",
    expected: ["??", "?.bar"],
  },
];

const babelProfiles = [
  {
    name: "babel-7.8",
    core: "7.8.7",
    optionalPlugin: ["@babel/plugin-proposal-optional-chaining", "7.8.3"],
    nullishPlugin: ["@babel/plugin-proposal-nullish-coalescing-operator", "7.8.3"],
    modes: ["spec", "loose"],
  },
  {
    name: "babel-7.13",
    core: "7.13.16",
    optionalPlugin: ["@babel/plugin-proposal-optional-chaining", "7.13.12"],
    nullishPlugin: ["@babel/plugin-proposal-nullish-coalescing-operator", "7.13.8"],
    modes: ["spec", "noDocumentAll", "loose"],
  },
  {
    name: "babel-7.28",
    core: "7.28.5",
    optionalPlugin: ["@babel/plugin-transform-optional-chaining", "7.28.5"],
    nullishPlugin: ["@babel/plugin-transform-nullish-coalescing-operator", "7.28.6"],
    modes: ["spec", "noDocumentAll", "loose"],
  },
  {
    name: "babel-8-rc",
    core: "8.0.0-rc.5",
    optionalPlugin: ["@babel/plugin-transform-optional-chaining", "8.0.0-rc.5"],
    nullishPlugin: ["@babel/plugin-transform-nullish-coalescing-operator", "8.0.0-rc.5"],
    modes: ["spec", "noDocumentAll", "loose"],
  },
];

const transformers = [
  ...babelProfiles.flatMap((profile) =>
    profile.modes.flatMap((mode) => [
      {
        name: `${profile.name}-${mode}`,
        run: (source) => runBabel(source, profile, babelModeOptions(mode)),
      },
      {
        name: `${profile.name}-${mode}-terser`,
        run: (source) => runTerser(runBabel(source, profile, babelModeOptions(mode))),
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
  console.log(`# Optional/nullish reproduction matrix`);
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
        `| ${snippet.name} | ${shape.label} | ${escapeCell(shape.tools.join(", "))} | ${escapeCell(
          result.status,
        )} | ${escapeCell(
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
    return {
      status: "no",
      recovered: false,
      notes: `transform failed: ${shape.transformError.message}`,
    };
  }

  let recovered;
  try {
    recovered = runWakaru(shape.lowered, `${snippet.name}-${shape.label.replaceAll(" ", "-")}.js`);
  } catch (error) {
    return { status: "no", recovered: false, notes: `wakaru failed: ${error.message}` };
  }

  const missing = snippet.expected.filter((needle) => !recovered.includes(needle));
  if (missing.length === 0) {
    return { status: "yes", recovered: true, notes: "expected syntax present" };
  }

  if (isExpectedLevelGate(snippet, shape)) {
    return {
      status: "gated",
      recovered: false,
      notes: `gated at ${rewriteLevel}; Babel loose repeated-property optional calls require aggressive`,
    };
  }

  const loweredShape = summarize(shape.lowered);
  const recoveredShape = summarize(recovered);
  return {
    status: "no",
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

function isExpectedLevelGate(snippet, shape) {
  if (rewriteLevel !== "standard") {
    return false;
  }
  if (!["optional-call-nullish", "nested-receiver-call"].includes(snippet.name)) {
    return false;
  }
  return shape.tools.some((tool) => tool.includes("-loose"));
}

function babelModeOptions(mode) {
  switch (mode) {
    case "spec":
      return { assumptions: {}, pluginOptions: {} };
    case "noDocumentAll":
      return { assumptions: { noDocumentAll: true }, pluginOptions: {} };
    case "loose":
      return { assumptions: {}, pluginOptions: { loose: true } };
    default:
      throw new Error(`unsupported Babel mode ${mode}`);
  }
}

function runBabel(source, profile, options) {
  const [optionalName, optionalVersion] = profile.optionalPlugin;
  const [nullishName, nullishVersion] = profile.nullishPlugin;
  const toolDir = ensureNodeTool(`babel-${profile.core}`, [
    `@babel/core@${profile.core}`,
    `${optionalName}@${optionalVersion}`,
    `${nullishName}@${nullishVersion}`,
  ]);
  const helper = join(toolDir, "babel-transform.mjs");
  writeFileSync(
    helper,
    `
import fs from "node:fs";

const babelModule = await import("@babel/core");
const optionalModule = await import(${JSON.stringify(optionalName)});
const nullishModule = await import(${JSON.stringify(nullishName)});
const babel = babelModule.default ?? babelModule;
const optional = optionalModule.default ?? optionalModule;
const nullish = nullishModule.default ?? nullishModule;
const source = fs.readFileSync(0, "utf8");
const options = JSON.parse(process.env.MATRIX_BABEL_OPTIONS || "{}");
const transformOptions = {
  filename: "input.js",
  babelrc: false,
  configFile: false,
  comments: false,
  compact: false,
  plugins: [
    [optional, options.pluginOptions || {}],
    [nullish, options.pluginOptions || {}],
  ],
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
