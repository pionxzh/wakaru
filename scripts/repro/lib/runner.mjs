import { existsSync, mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { cpus, tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(fileURLToPath(new URL("../../..", import.meta.url)));

// Decompile results keyed by `${level}\0${source}`, populated in parallel before
// the (synchronous) comparison loop so each shape's wakaru run is amortized.
const decompileCache = new Map();
const decompileKey = (level, source) => `${level}\0${source}`;

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

// Sync-callable entry point. The body is async (it decompiles all shapes in
// parallel before comparing), so errors are caught here to keep call sites
// simple: `runMatrix({...})` as the last statement of a matrix needs no await.
export function runMatrix(config) {
  return runMatrixAsync(config).catch((error) => {
    console.error(error?.stack ?? String(error));
    process.exitCode = 1;
  });
}

async function runMatrixAsync(config) {
  const {
    name,
    snippets,
    transformers,
    expectedNeedles = defaultExpectedNeedles,
    validateRecovered,
    prewarm,
  } = config;
  const showDetails = process.argv.includes("--details");
  const jsonMode = process.argv.includes("--json");
  const rewriteLevel = readOption("--level", "standard");
  if (!["minimal", "standard", "aggressive"].includes(rewriteLevel)) {
    throw new Error(`unsupported --level ${rewriteLevel}`);
  }

  const snippetFilter = readOption("--snippet");
  const dumpShape = readOption("--dump");

  const filteredSnippets = snippetFilter
    ? snippets.filter((s) => s.name === snippetFilter || s.name.includes(snippetFilter))
    : snippets;

  if (filteredSnippets.length === 0) {
    const available = snippets.map((s) => s.name).join(", ");
    throw new Error(`no snippets match --snippet ${snippetFilter} (available: ${available})`);
  }

  const tmpRoot = mkdtempSync(join(tmpdir(), `wakaru-${name}-`));
  const failures = [];
  const rows = [];
  let countYes = 0;
  let countNo = 0;
  let countError = 0;

  try {
    // Prewarm all transformer batches concurrently before any shape collection
    // (collectShapes calls transformer.run synchronously). The Babel/tsc/swc/
    // esbuild batch processes are the dominant fixed cost; running them in
    // parallel rather than lazily-serially is the main speedup. Sync custom
    // batches are unaffected (they still trigger on first lookup).
    await runPool(
      transformers.filter((transformer) => typeof transformer.run?.prewarm === "function"),
      (transformer) => transformer.run.prewarm(),
    );

    // --dump <snippet> <tool>: print full lowered + recovered for one shape
    if (dumpShape) {
      const dumpTool = process.argv[process.argv.indexOf("--dump") + 2] ?? "";
      dumpSingleShape(filteredSnippets, transformers, tmpRoot, rewriteLevel, dumpShape, dumpTool);
      return;
    }

    // Collect every shape up front, then decompile all of them in parallel so
    // the comparison loop below reads from the cache instead of spawning serially.
    const shapesBySnippet = new Map();
    const allLowered = [];
    for (const snippet of filteredSnippets) {
      const shapes = collectShapes(snippet, transformers);
      shapesBySnippet.set(snippet, shapes);
      for (const shape of shapes) {
        if (!shape.transformError) allLowered.push(shape.lowered);
      }
    }
    await decompileAll(allLowered, rewriteLevel);

    // Let the matrix prewarm any comparison-time work (e.g. structural
    // normalization of recovered output) concurrently, before the synchronous
    // comparison loop reads it from cache.
    if (prewarm) {
      const rows = [];
      for (const snippet of filteredSnippets) {
        for (const shape of shapesBySnippet.get(snippet)) {
          if (shape.transformError) continue;
          const recovered = decompileCache.get(decompileKey(rewriteLevel, shape.lowered));
          rows.push({ snippet, shape, recovered: recovered instanceof Error ? null : recovered });
        }
      }
      await prewarm(rows);
    }

    for (const snippet of filteredSnippets) {
      for (const shape of shapesBySnippet.get(snippet)) {
        const result = runShape(snippet, shape, tmpRoot, rewriteLevel, expectedNeedles, validateRecovered);
        if (!result.recovered && result.failure) {
          failures.push(result.failure);
        }
        const status = result.status ?? (result.recovered ? "yes" : "no");
        if (status === "yes") countYes++;
        else if (status === "no") countNo++;
        else countError++;
        rows.push({ snippet: snippet.name, shape: shape.label, tools: shape.tools, status, notes: result.notes });
      }
    }

    if (jsonMode) {
      const total = countYes + countNo;
      console.log(JSON.stringify(
        {
          name,
          level: rewriteLevel,
          summary: { yes: countYes, no: countNo, error: countError, pct: total > 0 ? +((countYes / total) * 100).toFixed(1) : 0 },
          rows,
        },
        null,
        2,
      ));
      return;
    }

    console.log(`# ${name} reproduction matrix`);
    console.log(`# wakaru: ${wakaruDescription()}`);
    console.log(`# level: ${rewriteLevel}`);
    if (snippetFilter) console.log(`# filter: ${snippetFilter}`);
    console.log("");
    console.log("| snippet | shape | tools | status | notes |");
    console.log("|---|---:|---|---:|---|");
    for (const row of rows) {
      console.log(
        `| ${row.snippet} | ${row.shape} | ${escapeCell(row.tools.join(", "))} | ${row.status} | ${escapeCell(row.notes)} |`,
      );
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

    const total = countYes + countNo;
    const pct = total > 0 ? ((countYes / total) * 100).toFixed(1) : "0.0";
    console.log("");
    console.log(
      `# ${countYes} yes / ${countNo} no` +
        (countError > 0 ? ` / ${countError} error` : "") +
        ` (${pct}%)`,
    );
  } finally {
    rmSync(tmpRoot, { recursive: true, force: true });
  }
}

function dumpSingleShape(snippets, transformers, tmpRoot, rewriteLevel, snippetName, toolHint) {
  const snippet = snippets.find((s) => s.name === snippetName || s.name.includes(snippetName));
  if (!snippet) {
    const available = snippets.map((s) => s.name).join(", ");
    throw new Error(`no snippet matches "${snippetName}" (available: ${available})`);
  }

  const shapes = collectShapes(snippet, transformers);
  const matching = toolHint
    ? shapes.filter((s) => s.tools.some((t) => t.includes(toolHint)))
    : shapes;

  if (matching.length === 0) {
    const available = shapes.flatMap((s) => s.tools).join(", ");
    throw new Error(`no shape matches tool "${toolHint}" for ${snippet.name} (available: ${available})`);
  }

  for (const shape of matching) {
    console.log(`=== ${snippet.name} / ${shape.label} ===`);
    console.log(`Tools: ${shape.tools.join(", ")}`);
    if (shape.transformError) {
      console.log(`Transform error: ${shape.transformError.message}`);
      continue;
    }
    console.log("");
    console.log("--- lowered ---");
    console.log(shape.lowered);
    console.log("--- wakaru ---");
    try {
      const recovered = runWakaru(
        shape.lowered,
        `${snippet.name}-dump.js`,
        tmpRoot,
        rewriteLevel,
      );
      console.log(recovered);
    } catch (error) {
      console.log(`Wakaru error: ${error.message}`);
    }
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
    return { recovered: false, status: "transform-failed", notes: shape.transformError.message };
  }

  let recovered;
  try {
    recovered = runWakaru(shape.lowered, `${snippet.name}-${shape.label.replaceAll(" ", "-")}.js`, tmpRoot, rewriteLevel);
  } catch (error) {
    return { recovered: false, status: "wakaru-failed", notes: error.message };
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
  const cached = decompileCache.get(decompileKey(rewriteLevel, source));
  if (cached !== undefined) {
    if (cached instanceof Error) throw cached;
    return cached;
  }
  // Uncached fallback (e.g. the synchronous --dump path).
  const input = join(tmpRoot, name);
  writeFileSync(input, source);
  return runWakaruArgs(["--level", rewriteLevel, input]);
}

function defaultConcurrency() {
  return Math.max(1, Math.min(16, (cpus().length || 4) - 2));
}

// Run `worker` over `items` with a bounded number of concurrent invocations.
export async function runPool(items, worker, concurrency = defaultConcurrency()) {
  let cursor = 0;
  const run = async () => {
    while (cursor < items.length) {
      const index = cursor++;
      await worker(items[index], index);
    }
  };
  await Promise.all(Array.from({ length: Math.min(concurrency, items.length) }, run));
}

// Decompile every (unique) source concurrently and fill decompileCache, so the
// comparison loop runs against in-memory results instead of serial spawns.
async function decompileAll(sources, rewriteLevel) {
  const { command, prefix } = resolveWakaruCmd();
  const pending = [...new Set(sources)].filter(
    (source) => !decompileCache.has(decompileKey(rewriteLevel, source)),
  );
  await runPool(pending, async (source) => {
    const key = decompileKey(rewriteLevel, source);
    try {
      decompileCache.set(key, await spawnCapture(command, [...prefix, "--level", rewriteLevel, "-"], source));
    } catch (error) {
      decompileCache.set(key, error instanceof Error ? error : new Error(String(error)));
    }
  });
}

function spawnCapture(command, args, input) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(command, args, { cwd: repoRoot });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => (stdout += chunk));
    child.stderr.on("data", (chunk) => (stderr += chunk));
    child.on("error", reject);
    child.on("close", (code) =>
      code === 0
        ? resolvePromise(stdout)
        : reject(new Error(`${basename(command)} exited ${code}: ${stderr.trim() || stdout.trim()}`)),
    );
    child.stdin.end(input);
  });
}

// Resolve how to invoke the wakaru CLI once: $WAKARU override → debug build →
// `cargo run` fallback. Returns the command plus any argument prefix.
function resolveWakaruCmd() {
  if (process.env.WAKARU) {
    return { command: process.env.WAKARU, prefix: [] };
  }
  const debugBinary = join(repoRoot, "target", "debug", process.platform === "win32" ? "wakaru.exe" : "wakaru");
  if (existsSync(debugBinary)) {
    return { command: debugBinary, prefix: [] };
  }
  return { command: "cargo", prefix: ["run", "-q", "-p", "wakaru-cli", "--"] };
}

// Invoke the wakaru CLI synchronously with arbitrary args. `options.input` is
// piped to stdin (use the `-` arg to make wakaru read it).
export function runWakaruArgs(args, options = {}) {
  const runOptions = options.input !== undefined ? { input: options.input } : {};
  const { command, prefix } = resolveWakaruCmd();
  return runChecked(command, [...prefix, ...args], { ...runOptions, cwd: repoRoot });
}

// Async counterpart of runWakaruArgs, for concurrent prewarming via runPool.
export async function runWakaruArgsAsync(args, options = {}) {
  const { command, prefix } = resolveWakaruCmd();
  return spawnCapture(command, [...prefix, ...args], options.input);
}

function wakaruDescription() {
  return resolveWakaruCmd().command;
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

// Wrap a lazy batch (`() => Map` or `() => Promise<Map>`) into a memoized
// source→result lookup. `lookup.prewarm()` runs the batch once, concurrently
// with others, and is the way to use an async batch: after prewarming, the
// synchronous `lookup(source)` reads the resolved result. A *synchronous* batch
// still works without prewarming (it is triggered on first lookup), preserving
// the old behavior. A batch that fails is remembered and re-thrown on lookup,
// so the caller (collectShapes) records a transform-failed shape — exactly as
// before, rather than aborting the whole run.
export function batchRunner(lazyBatch) {
  let resolved;
  let pending;
  let failure;
  const lookup = (source) => {
    if (failure) throw failure;
    if (!resolved) {
      const value = lazyBatch();
      if (value && typeof value.then === "function") {
        throw new Error("async batch used before prewarm(); call lookup.prewarm() first");
      }
      resolved = value;
    }
    const result = resolved.get(source);
    if (result === undefined) throw new Error("source not in batch");
    if (result instanceof Error) throw result;
    return result;
  };
  lookup.prewarm = async () => {
    if (resolved || failure) return;
    if (!pending) pending = Promise.resolve().then(lazyBatch);
    try {
      resolved = await pending;
    } catch (error) {
      failure = error instanceof Error ? error : new Error(String(error));
    }
  };
  return lookup;
}

// Spawns the tool process without blocking the event loop, so multiple batches
// run concurrently under runPool. Pipes JSON sources on stdin, parses the JSON
// results array on stdout into a source→(code|Error) map.
function runBatchHelperAsync(command, args, sources, options = {}) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(command, args, {
      cwd: options.cwd ?? repoRoot,
      shell: options.shell ?? false,
      env: { ...process.env, ...(options.env ?? {}) },
    });
    const stdout = [];
    let stderr = "";
    child.stdout.on("data", (chunk) => stdout.push(chunk));
    child.stderr.on("data", (chunk) => (stderr += chunk));
    child.on("error", reject);
    child.on("close", (code) => {
      if (code !== 0) {
        const detail = [stderr.trim(), Buffer.concat(stdout).toString().trim()].filter(Boolean).join(" ");
        reject(new Error(`${basename(command)} batch exited ${code}: ${detail}`));
        return;
      }
      try {
        const outputs = JSON.parse(Buffer.concat(stdout).toString());
        const map = new Map();
        for (let i = 0; i < sources.length; i++) {
          map.set(sources[i], outputs[i].error ? new Error(outputs[i].error) : outputs[i].code);
        }
        resolvePromise(map);
      } catch (error) {
        reject(error);
      }
    });
    child.stdin.end(JSON.stringify(sources));
  });
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
  return runBatchHelperAsync("node", [helper], sources, {
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
  return runBatchHelperAsync("node", [helper], sources, { cwd: toolDir, env });
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
  return runBatchHelperAsync("node", [helper], sources, {
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
  return runBatchHelperAsync("node", [helper], sources, {
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
  return runBatchHelperAsync("node", [helper], sources, {
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
  return runBatchHelperAsync("node", [helper], sources, {
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
  return runBatchHelperAsync("node", [helper], sources, { cwd: toolDir });
}

export function withTerserVariants(name, allSources, runRaw, options = {}) {
  // The terser variants run on top of `runRaw`'s output, so each first ensures
  // the raw batch is prewarmed (idempotent/shared), then minifies. Returning an
  // async lazy batch lets all of these prewarm concurrently with everything else.
  const terserVariant = (mangle) =>
    batchRunner(async () => {
      await runRaw.prewarm?.();
      const rawOutputs = allSources.map((s) => {
        try {
          return runRaw(s);
        } catch {
          return null;
        }
      });
      const valid = rawOutputs.filter((r) => r !== null);
      if (valid.length === 0) return new Map();
      const batchResult = await terserBatch(valid, { mangle });
      const map = new Map();
      for (let i = 0; i < allSources.length; i++) {
        if (rawOutputs[i] !== null) map.set(allSources[i], batchResult.get(rawOutputs[i]));
      }
      return map;
    });
  const terserCompressCache = terserVariant(false);
  const terserMangleCache = terserVariant(true);
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
