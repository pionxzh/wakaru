import { existsSync, mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(fileURLToPath(new URL("../../..", import.meta.url)));

export function readOption(name, fallback) {
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

export function runMatrix(config) {
  const {
    name,
    snippets,
    transformers,
    expectedNeedles = defaultExpectedNeedles,
    validateRecovered,
  } = config;
  const showDetails = process.argv.includes("--details");
  const rewriteLevel = readOption("--level", "standard");
  if (!["minimal", "standard", "aggressive"].includes(rewriteLevel)) {
    throw new Error(`unsupported --level ${rewriteLevel}`);
  }

  const tmpRoot = mkdtempSync(join(tmpdir(), `wakaru-${name}-`));
  const failures = [];

  try {
    console.log(`# ${name} reproduction matrix`);
    console.log(`# wakaru: ${wakaruDescription()}`);
    console.log(`# level: ${rewriteLevel}`);
    console.log("");
    console.log("| snippet | shape | tools | recovered | notes |");
    console.log("|---|---:|---|---:|---|");

    for (const snippet of snippets) {
      const shapes = collectShapes(snippet, transformers);
      for (const shape of shapes) {
        const result = runShape(snippet, shape, tmpRoot, rewriteLevel, expectedNeedles, validateRecovered);
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
}

function defaultExpectedNeedles(snippet) {
  return Array.isArray(snippet.expected) ? snippet.expected : [snippet.expected];
}

function expectedNeedleGroups(snippet, expectedNeedles) {
  if (snippet.expectedAny) {
    return snippet.expectedAny.map((group) => (Array.isArray(group) ? group : [group]));
  }
  return [expectedNeedles(snippet)];
}

function collectShapes(snippet, transformers) {
  const groups = new Map();
  const shapes = [];

  for (const transformer of [...transformers, ...(snippet.extraTransformers ?? [])]) {
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

function runShape(snippet, shape, tmpRoot, rewriteLevel, expectedNeedles, validateRecovered) {
  if (shape.transformError) {
    return { recovered: false, notes: `transform failed: ${shape.transformError.message}` };
  }

  let recovered;
  try {
    recovered = runWakaru(shape.lowered, `${snippet.name}-${shape.label.replaceAll(" ", "-")}.js`, tmpRoot, rewriteLevel);
  } catch (error) {
    return { recovered: false, notes: `wakaru failed: ${error.message}` };
  }

  const missingGroups = expectedNeedleGroups(snippet, expectedNeedles).map((needles) =>
    needles.filter((needle) => !recovered.includes(needle)),
  );
  const missing = missingGroups.reduce((best, next) => (next.length < best.length ? next : best));
  const leaked = (snippet.rejected ?? []).filter((needle) => recovered.includes(needle));
  if (missingGroups.some((group) => group.length === 0) && leaked.length === 0) {
    return { recovered: true, notes: "expected syntax present" };
  }

  const customResult = validateRecovered?.({
    snippet,
    shape,
    recovered,
    rewriteLevel,
    tmpRoot,
    runWakaru,
  });
  if (customResult?.recovered && leaked.length === 0) {
    return customResult;
  }

  const loweredShape = summarize(shape.lowered);
  const recoveredShape = summarize(recovered);
  if (missing.length === 0 && leaked.length > 0) {
    return {
      recovered: false,
      notes: `leaked ${leaked.join(", ")}; lowered: ${loweredShape}; wakaru: ${recoveredShape}`,
      failure: { snippet: snippet.name, shape: shape.label, tools: shape.tools, lowered: shape.lowered, recovered },
    };
  }
  return {
    recovered: false,
    notes: `missing ${missing.join(", ")}; lowered: ${loweredShape}; wakaru: ${recoveredShape}`,
    failure: { snippet: snippet.name, shape: shape.label, tools: shape.tools, lowered: shape.lowered, recovered },
  };
}

function runWakaru(source, name, tmpRoot, rewriteLevel) {
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
    return runChecked("cargo", ["run", "-q", "-p", "wakaru-cli", "--", "--level", rewriteLevel, input], { cwd: repoRoot });
  }
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

// ── Batched tool runners ──────────────────────────────────────

export function batchRunner(lazyBatch) {
  let cache;
  return (source) => {
    if (!cache) cache = lazyBatch();
    const result = cache.get(source);
    if (result === undefined) throw new Error("source not in batch");
    if (result instanceof Error) throw result;
    return result;
  };
}

function runBatchHelper(command, args, sources, options = {}) {
  const result = spawnSync(command, args, {
    cwd: options.cwd ?? repoRoot,
    input: JSON.stringify(sources),
    encoding: "utf8",
    maxBuffer: 1024 * 1024 * 50,
    shell: options.shell ?? false,
    env: { ...process.env, ...(options.env ?? {}) },
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    const detail = [result.stderr.trim(), result.stdout.trim()].filter(Boolean).join(" ");
    throw new Error(`${basename(command)} batch exited ${result.status}: ${detail}`);
  }
  const outputs = JSON.parse(result.stdout);
  const map = new Map();
  for (let i = 0; i < sources.length; i++) {
    map.set(sources[i], outputs[i].error ? new Error(outputs[i].error) : outputs[i].code);
  }
  return map;
}

export function ensureNodeTool(name, packages) {
  const toolRoot = join(repoRoot, "target", "repro-tools");
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
  if (result.error) throw result.error;
  if (result.status !== 0) {
    const detail = [result.stderr.trim(), result.stdout.trim()].filter(Boolean).join(" ");
    throw new Error(`${basename(command)} exited ${result.status}: ${detail}`);
  }
  return result.stdout;
}

// ── Batched standard tool runners ─────────────────────────────

export function babelBatch(sources, profile, babelOptions = {}) {
  const pluginName = profile.plugin[0];
  const pluginVersion = profile.plugin[1];
  const packages = [`@babel/core@${profile.core}`, `${pluginName}@${pluginVersion}`];
  const toolDir = ensureNodeTool(`babel-${profile.core}`, packages);
  const helper = join(toolDir, "babel-batch.mjs");
  writeFileSync(
    helper,
    `
import fs from "node:fs";
const babelModule = await import("@babel/core");
const pluginModule = await import(${JSON.stringify(pluginName)});
const babel = babelModule.default ?? babelModule;
const plugin = pluginModule.default ?? pluginModule;
const babelOptions = JSON.parse(process.env.MATRIX_BABEL_OPTIONS || "{}");
const sources = JSON.parse(fs.readFileSync(0, "utf8"));
const results = sources.map(source => {
  try {
    const transformOptions = {
      filename: "input.js", babelrc: false, configFile: false, comments: false, compact: false,
      plugins: [[plugin, babelOptions.pluginOptions || {}]],
    };
    if (babelOptions.assumptions && Object.keys(babelOptions.assumptions).length > 0) {
      transformOptions.assumptions = babelOptions.assumptions;
    }
    return { code: babel.transformSync(source, transformOptions).code };
  } catch (e) { return { error: e.message }; }
});
process.stdout.write(JSON.stringify(results));
`,
  );
  return runBatchHelper("node", [helper], sources, {
    cwd: toolDir,
    env: { MATRIX_BABEL_OPTIONS: JSON.stringify(babelOptions) },
  });
}

export function babelMultiPluginBatch(sources, profile, plugins, env = {}) {
  const packages = [`@babel/core@${profile.core}`, ...plugins.map(([name, ver]) => `${name}@${ver}`)];
  const toolKey = `babel-${profile.core}-${plugins.map(([n]) => n.split("/").pop()).join("-")}`;
  const toolDir = ensureNodeTool(toolKey, packages);
  const helper = join(toolDir, "babel-multi-batch.mjs");
  const pluginImports = plugins.map(([name], i) => `const p${i} = (await import(${JSON.stringify(name)})).default ?? (await import(${JSON.stringify(name)}));`).join("\n");
  const pluginList = plugins.map((_, i) => `p${i}`).join(", ");
  writeFileSync(
    helper,
    `
import fs from "node:fs";
const babelModule = await import("@babel/core");
const babel = babelModule.default ?? babelModule;
${pluginImports}
const plugins = [${pluginList}];
const sources = JSON.parse(fs.readFileSync(0, "utf8"));
const results = sources.map(source => {
  try {
    return { code: babel.transformSync(source, {
      filename: "input.js", babelrc: false, configFile: false, comments: false, compact: false, plugins,
    }).code };
  } catch (e) { return { error: e.message }; }
});
process.stdout.write(JSON.stringify(results));
`,
  );
  return runBatchHelper("node", [helper], sources, { cwd: toolDir, env });
}

export function babelPresetEnvBatch(sources, options = {}) {
  const coreVersion = options.core ?? "7.29.7";
  const presetVersion = options.preset ?? "7.29.7";
  const targets = options.targets ?? { ie: "11" };
  const toolDir = ensureNodeTool(`babel-${coreVersion}-preset-env`, [
    `@babel/core@${coreVersion}`,
    `@babel/preset-env@${presetVersion}`,
  ]);
  const helper = join(toolDir, "babel-preset-env-batch.mjs");
  writeFileSync(
    helper,
    `
import fs from "node:fs";
const babelModule = await import("@babel/core");
const presetEnvModule = await import("@babel/preset-env");
const babel = babelModule.default ?? babelModule;
const presetEnv = presetEnvModule.default ?? presetEnvModule;
const targets = JSON.parse(process.env.MATRIX_TARGETS || "{}");
const sources = JSON.parse(fs.readFileSync(0, "utf8"));
const results = sources.map(source => {
  try {
    return { code: babel.transformSync(source, {
      filename: "input.js", babelrc: false, configFile: false, comments: false, compact: false,
      presets: [[presetEnv, { targets }]],
    }).code };
  } catch (e) { return { error: e.message }; }
});
process.stdout.write(JSON.stringify(results));
`,
  );
  return runBatchHelper("node", [helper], sources, {
    cwd: toolDir,
    env: { MATRIX_TARGETS: JSON.stringify(targets) },
  });
}

export function tscBatch(sources, options = {}) {
  const target = options.target ?? "ES5";
  const toolDir = ensureNodeTool("typescript", ["typescript@5"]);
  const helper = join(toolDir, "tsc-batch.cjs");
  writeFileSync(
    helper,
    `
const fs = require("node:fs");
const ts = require("typescript");
const target = process.env.MATRIX_TSC_TARGET || "ES5";
const sources = JSON.parse(fs.readFileSync(0, "utf8"));
const results = sources.map(source => {
  try {
    return { code: ts.transpileModule(source, {
      compilerOptions: { target: ts.ScriptTarget[target], module: ts.ModuleKind.ESNext },
    }).outputText };
  } catch (e) { return { error: e.message }; }
});
process.stdout.write(JSON.stringify(results));
`,
  );
  return runBatchHelper("node", [helper], sources, {
    cwd: toolDir,
    env: { MATRIX_TSC_TARGET: target },
  });
}

export function swcBatch(sources, options = {}) {
  const target = options.target ?? "es5";
  const toolDir = ensureNodeTool("swc", ["@swc/core@1"]);
  const helper = join(toolDir, "swc-batch.cjs");
  writeFileSync(
    helper,
    `
const fs = require("node:fs");
const swc = require("@swc/core");
const target = process.env.MATRIX_SWC_TARGET || "es5";
const sources = JSON.parse(fs.readFileSync(0, "utf8"));
const results = sources.map(source => {
  try {
    return { code: swc.transformSync(source, {
      filename: "input.js",
      jsc: { target, parser: { syntax: "ecmascript" } },
      module: { type: "es6" },
    }).code };
  } catch (e) { return { error: e.message }; }
});
process.stdout.write(JSON.stringify(results));
`,
  );
  return runBatchHelper("node", [helper], sources, {
    cwd: toolDir,
    env: { MATRIX_SWC_TARGET: target },
  });
}

export function esbuildBatch(sources, options = {}) {
  const target = options.target ?? "es2015";
  const toolDir = ensureNodeTool("esbuild-0.28", ["esbuild@0.28.0"]);
  const helper = join(toolDir, "esbuild-batch.cjs");
  writeFileSync(
    helper,
    `
const fs = require("node:fs");
const esbuild = require("esbuild");
const target = process.env.MATRIX_ESBUILD_TARGET || "es2015";
const sources = JSON.parse(fs.readFileSync(0, "utf8"));
const results = sources.map(source => {
  try {
    return { code: esbuild.transformSync(source, {
      loader: "js", target, format: "esm", logLevel: "warning",
    }).code };
  } catch (e) { return { error: e.message }; }
});
process.stdout.write(JSON.stringify(results));
`,
  );
  return runBatchHelper("node", [helper], sources, {
    cwd: toolDir,
    env: { MATRIX_ESBUILD_TARGET: target },
  });
}

export function terserBatch(sources, options = {}) {
  const mangle = options.mangle ?? false;
  const toolDir = ensureNodeTool("terser", ["terser@5"]);
  const suffix = mangle ? "mangle-batch" : "batch";
  const helper = join(toolDir, `terser-${suffix}.mjs`);
  writeFileSync(
    helper,
    `
import fs from "node:fs";
import { minify } from "terser";
const mangle = ${mangle};
const sources = JSON.parse(fs.readFileSync(0, "utf8"));
const results = [];
for (const source of sources) {
  try {
    const result = await minify(source, {
      module: true,
      compress: { defaults: true, unused: false },
      mangle,
      format: { comments: false },
    });
    results.push({ code: result.code });
  } catch (e) { results.push({ error: e.message }); }
}
process.stdout.write(JSON.stringify(results));
`,
  );
  return runBatchHelper("node", [helper], sources, { cwd: toolDir });
}

export function withTerserVariants(name, allSources, runRaw, options = {}) {
  const terserCompressCache = batchRunner(() => {
    const rawOutputs = allSources.map((s) => { try { return runRaw(s); } catch { return null; } });
    const valid = rawOutputs.filter((r) => r !== null);
    if (valid.length === 0) return new Map();
    const batchResult = terserBatch(valid, { mangle: false });
    const map = new Map();
    for (let i = 0; i < allSources.length; i++) {
      if (rawOutputs[i] !== null) map.set(allSources[i], batchResult.get(rawOutputs[i]));
    }
    return map;
  });
  const terserMangleCache = batchRunner(() => {
    const rawOutputs = allSources.map((s) => { try { return runRaw(s); } catch { return null; } });
    const valid = rawOutputs.filter((r) => r !== null);
    if (valid.length === 0) return new Map();
    const batchResult = terserBatch(valid, { mangle: true });
    const map = new Map();
    for (let i = 0; i < allSources.length; i++) {
      if (rawOutputs[i] !== null) map.set(allSources[i], batchResult.get(rawOutputs[i]));
    }
    return map;
  });
  const variants = [
    { name, run: runRaw },
    { name: `${name}-terser-compress`, run: terserCompressCache },
    { name: `${name}-terser-compress-mangle`, run: terserMangleCache },
  ];
  if (options.includeRaw === false) {
    return variants.slice(1);
  }
  return variants;
}
