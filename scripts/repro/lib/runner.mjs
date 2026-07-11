import { existsSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { cpus, tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const execHarnessPath = fileURLToPath(new URL("./exec-harness.mjs", import.meta.url));

// Decompile results keyed by `${level}\0${source}`, populated in parallel before
// the (synchronous) comparison loop so each shape's wakaru run is amortized.
const decompileCache = new Map();
const decompileKey = (level, source, wakaruArgs = []) => `${level}\0${wakaruArgs.join("\0")}\0${source}`;
const refreshedNodeTools = new Set();

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
    wakaruArgs = [],
  } = config;
  const showDetails = process.argv.includes("--details");
  const jsonMode = process.argv.includes("--json");
  const clusterMode = process.argv.includes("--cluster");
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
      dumpSingleShape(filteredSnippets, transformers, tmpRoot, rewriteLevel, dumpShape, dumpTool, wakaruArgs);
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
    await decompileAll(allLowered, rewriteLevel, wakaruArgs);

    // Let the matrix prewarm any comparison-time work (e.g. structural
    // normalization of recovered output) concurrently, before the synchronous
    // comparison loop reads it from cache.
    if (prewarm) {
      const rows = [];
      for (const snippet of filteredSnippets) {
        for (const shape of shapesBySnippet.get(snippet)) {
          if (shape.transformError) continue;
          const recovered = decompileCache.get(decompileKey(rewriteLevel, shape.lowered, wakaruArgs));
          rows.push({ snippet, shape, recovered: recovered instanceof Error ? null : recovered });
        }
      }
      await prewarm(rows);
    }

    const execVerdicts = await runExecutionChecks(
      filteredSnippets,
      shapesBySnippet,
      rewriteLevel,
      wakaruArgs,
    );

    for (const snippet of filteredSnippets) {
      for (const shape of shapesBySnippet.get(snippet)) {
        const result = runShape(
          snippet,
          shape,
          tmpRoot,
          rewriteLevel,
          expectedNeedles,
          validateRecovered,
          wakaruArgs,
          execVerdicts,
        );
        if (!result.recovered && result.failure) {
          failures.push(result.failure);
        }
        const status = result.status ?? (result.recovered ? "yes" : "no");
        if (status === "yes") countYes++;
        else if (status === "no") countNo++;
        else countError++;
        rows.push({
          snippet: snippet.name,
          shape: shape.label,
          tools: shape.tools,
          status,
          notes: result.notes,
          // Full diagnostic payload so failure triage is `jq` over data, not
          // regex over the truncated markdown note.
          lowered: result.lowered,
          recovered: result.code,
          missing: result.missing,
          leaked: result.leaked && result.leaked.length ? result.leaked : undefined,
        });
      }
    }

    if (clusterMode) {
      await printFailureClusters(name, rewriteLevel, rows);
      return;
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

function dumpSingleShape(snippets, transformers, tmpRoot, rewriteLevel, snippetName, toolHint, wakaruArgs = []) {
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
        wakaruArgs,
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

function runShape(
  snippet,
  shape,
  tmpRoot,
  rewriteLevel,
  expectedNeedles,
  validateRecovered,
  wakaruArgs,
  execVerdicts = new Map(),
) {
  if (shape.transformError) {
    return { recovered: false, status: "transform-failed", notes: shape.transformError.message };
  }

  let recovered;
  try {
    recovered = runWakaru(shape.lowered, `${snippet.name}-${shape.label.replaceAll(" ", "-")}.js`, tmpRoot, rewriteLevel, wakaruArgs);
  } catch (error) {
    return { recovered: false, status: "wakaru-failed", notes: error.message, lowered: shape.lowered };
  }

  // Substring/structural checks accept a *shape*; the execution verdict then
  // rejects recoveries whose observable behavior diverged from the lowered
  // program (wrong declaration kind, extra evaluation, stale key, …).
  const applyExecVerdict = (success) => {
    const verdict = execVerdicts.get(execKey(snippet, shape));
    if (!verdict || verdict.status === "equivalent") {
      const note = verdict ? `${success.notes}; execution-equivalent` : success.notes;
      return { ...success, notes: note };
    }
    if (verdict.status === "skipped") {
      return { ...success, notes: `${success.notes}; exec skipped (${verdict.reason})` };
    }
    return {
      recovered: false,
      notes: `behavior diverged: ${verdict.reason}`,
      code: recovered,
      lowered: shape.lowered,
      missing: [],
      leaked: [],
      failure: { snippet: snippet.name, shape: shape.label, tools: shape.tools, lowered: shape.lowered, recovered },
    };
  };

  const leaked = (snippet.rejected ?? []).filter((needle) => recovered.includes(needle));
  const customResult = validateRecovered?.({
    snippet,
    shape,
    recovered,
    rewriteLevel,
    tmpRoot,
    runWakaru,
  });
  if (customResult?.recovered && leaked.length === 0) {
    return applyExecVerdict({ ...customResult, code: recovered, lowered: shape.lowered });
  }
  if (customResult && customResult.recovered === false) {
    return {
      code: recovered,
      lowered: shape.lowered,
      failure: { snippet: snippet.name, shape: shape.label, tools: shape.tools, lowered: shape.lowered, recovered },
      ...customResult,
    };
  }

  const missingGroups = expectedNeedleGroups(snippet, expectedNeedles).map((needles) =>
    needles.filter((needle) => !recovered.includes(needle)),
  );
  const missing = missingGroups.reduce((best, next) => (next.length < best.length ? next : best));
  if (missingGroups.some((group) => group.length === 0) && leaked.length === 0) {
    return applyExecVerdict({
      recovered: true,
      notes: "expected syntax present",
      code: recovered,
      lowered: shape.lowered,
    });
  }

  const loweredShape = summarize(shape.lowered);
  const recoveredShape = summarize(recovered);
  const detail = {
    code: recovered,
    lowered: shape.lowered,
    missing,
    leaked,
    failure: { snippet: snippet.name, shape: shape.label, tools: shape.tools, lowered: shape.lowered, recovered },
  };
  if (missing.length === 0 && leaked.length > 0) {
    return { recovered: false, notes: `leaked ${leaked.join(", ")}; lowered: ${loweredShape}; wakaru: ${recoveredShape}`, ...detail };
  }
  return { recovered: false, notes: `missing ${missing.join(", ")}; lowered: ${loweredShape}; wakaru: ${recoveredShape}`, ...detail };
}

// --cluster: group failing shapes by the structure of their recovered output
// (alpha-renamed canonical form) so "30 identical state machines" collapse to a
// single cluster, separating real distinct failures from repeats. This is the
// built-in version of the ad-hoc dedupe scripts used during rule development.
async function printFailureClusters(name, rewriteLevel, rows) {
  const failing = rows.filter((row) => row.status !== "yes");
  const distinctCode = [...new Set(failing.filter((r) => r.status === "no" && r.recovered).map((r) => r.recovered))];
  const normalized = new Map();
  await runPool(distinctCode, async (code) => {
    let key;
    try {
      key = (await runWakaruArgsAsync(["debug", "normalize", "--rename", "-"], { input: code })).trim();
    } catch {
      key = "";
    }
    // Fall back to whitespace-collapsed text if the output doesn't parse.
    normalized.set(code, key || code.replace(/\s+/g, " ").trim());
  });

  const clusters = new Map();
  for (const row of failing) {
    let key;
    if (row.status !== "no") key = `<${row.status}>`;
    else if (row.recovered) key = normalized.get(row.recovered);
    else key = "<no-output>";
    let cluster = clusters.get(key);
    if (!cluster) {
      cluster = { key, representative: row, rows: [] };
      clusters.set(key, cluster);
    }
    cluster.rows.push(row);
  }

  const sorted = [...clusters.values()].sort((a, b) => b.rows.length - a.rows.length);
  const shapeCount = failing.reduce((sum, row) => sum + row.tools.length, 0);
  console.log(`# ${name} failure clusters`);
  console.log(`# level: ${rewriteLevel}`);
  console.log(`# ${failing.length} failing shapes (${shapeCount} tool variants) in ${sorted.length} clusters`);
  console.log("");

  sorted.forEach((cluster, index) => {
    const rep = cluster.representative;
    console.log(`## cluster ${index + 1} — ${cluster.rows.length} shape${cluster.rows.length === 1 ? "" : "s"}`);
    if (cluster.key.startsWith("<")) {
      console.log(`kind: ${cluster.key}`);
    } else {
      console.log(`representative: ${rep.snippet} / ${rep.shape} (${rep.tools[0]})`);
    }
    const sample = rep.recovered ?? rep.notes ?? "";
    const lines = sample.split("\n");
    console.log("```js");
    console.log(lines.slice(0, 12).join("\n").trim());
    if (lines.length > 12) console.log("// …");
    console.log("```");
    const bySnippet = new Map();
    for (const row of cluster.rows) {
      if (!bySnippet.has(row.snippet)) bySnippet.set(row.snippet, []);
      bySnippet.get(row.snippet).push(row.shape);
    }
    console.log("members:");
    for (const [snippet, shapes] of bySnippet) {
      console.log(`  ${snippet}: ${shapes.join(", ")}`);
    }
    console.log("");
  });
}

function runWakaru(source, name, tmpRoot, rewriteLevel, wakaruArgs = []) {
  const cached = decompileCache.get(decompileKey(rewriteLevel, source, wakaruArgs));
  if (cached !== undefined) {
    if (cached instanceof Error) throw cached;
    return cached;
  }
  // Uncached fallback (e.g. the synchronous --dump path).
  const input = join(tmpRoot, name);
  writeFileSync(input, source);
  return runWakaruArgs(["--level", rewriteLevel, ...wakaruArgs, input]);
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
async function decompileAll(sources, rewriteLevel, wakaruArgs = []) {
  const { command, prefix } = resolveWakaruCmd();
  const pending = [...new Set(sources)].filter(
    (source) => !decompileCache.has(decompileKey(rewriteLevel, source, wakaruArgs)),
  );
  await runPool(pending, async (source) => {
    const key = decompileKey(rewriteLevel, source, wakaruArgs);
    try {
      decompileCache.set(key, await spawnCapture(command, [...prefix, "--level", rewriteLevel, ...wakaruArgs, "-"], source));
    } catch (error) {
      decompileCache.set(key, error instanceof Error ? error : new Error(String(error)));
    }
  });
}

// ── Execution-equivalence checks ──────────────────────────────
//
// Rows opt in with `execute: true` or `execute: { env, returns }`:
// `env` binds JSON values as globals, `returns` makes named stubs return a
// given JSON value (fresh clone per call); every other free identifier is
// auto-stubbed as a deterministic recording function. The lowered program and
// wakaru's recovery run in isolated `node:vm` contexts (see exec-harness.mjs)
// and must produce the same effect log.

function execKey(snippet, shape) {
  return `${snippet.name}\0${shape.lowered}`;
}

async function runExecutionChecks(snippets, shapesBySnippet, rewriteLevel, wakaruArgs) {
  const verdicts = new Map();
  const jobs = [];
  for (const snippet of snippets) {
    if (!snippet.execute) continue;
    const spec = snippet.execute === true ? {} : snippet.execute;
    for (const shape of shapesBySnippet.get(snippet)) {
      if (shape.transformError) continue;
      const recovered = decompileCache.get(decompileKey(rewriteLevel, shape.lowered, wakaruArgs));
      if (typeof recovered !== "string") continue;
      jobs.push({ key: execKey(snippet, shape), spec, lowered: shape.lowered, recovered });
    }
  }
  await runPool(jobs, async (job) => {
    try {
      const raw = await spawnCapture("node", [execHarnessPath], JSON.stringify({
        env: job.spec.env ?? {},
        returns: job.spec.returns ?? {},
        programs: [job.lowered, job.recovered],
      }));
      const { status, reason } = JSON.parse(raw);
      verdicts.set(job.key, { status, reason });
    } catch (error) {
      verdicts.set(job.key, { status: "skipped", reason: `harness failed: ${error.message}` });
    }
  });
  return verdicts;
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
  const refresh = process.env.WAKARU_REPRO_REFRESH_TOOLS === "1" && !refreshedNodeTools.has(dir);
  if (!refresh && existsSync(marker) && readFileSync(marker, "utf8") === markerText) {
    return dir;
  }
  if (refresh) {
    refreshedNodeTools.add(dir);
  }
  rmSync(dir, { recursive: true, force: true });
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
  const minify = options.minify ?? false;
  const externalHelpers = options.externalHelpers ?? false;
  const toolDir = ensureNodeTool("swc", ["@swc/core@1"]);
  const variant = minify ? "minify" : externalHelpers ? "external" : "base";
  const helper = join(toolDir, variant === "base" ? "swc-batch.cjs" : `swc-${variant}-batch.cjs`);
  const jscExtra =
    (externalHelpers ? ", externalHelpers: true" : "") +
    (minify ? ", minify: { compress: true, mangle: true }" : "");
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
      jsc: { target, parser: { syntax: "ecmascript" }${jscExtra} },
      module: { type: "es6" },
      minify: ${minify},
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
  const minify = options.minify ?? false;
  const toolDir = ensureNodeTool("esbuild-0.28", ["esbuild@0.28.0"]);
  const helper = join(toolDir, minify ? "esbuild-minify-batch.cjs" : "esbuild-batch.cjs");
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
      loader: "js", target, format: "esm", minify: ${minify}, logLevel: "warning",
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

// The lowerer tail shared by most matrices: tsc + swc + esbuild, each in its
// three Terser variants, plus a source-through-Terser pair. Spread it into a
// matrix's transformer array and append any special-case lowerers explicitly:
//
//   const transformers = [
//     ...babelProfiles.flatMap(...),       // feature-specific Babel block
//     ...standardLowerers(allSources),     // the boring 80% overlap
//   ];
//
// Overrides cover the small variations seen in practice: `esbuildTarget`
// (es2015/es2017/es5), `includeSource`, and `tsc`/`swc`/`esbuild` to swap in a
// matrix's own batch (e.g. a TypeScript-flavored swc).
export function standardLowerers(allSources, options = {}) {
  const {
    esbuildTarget = "es2015",
    includeSource = true,
    swcExternalHelpers = false,
    tsc = (sources) => tscBatch(sources),
    swc = (sources) => swcBatch(sources),
    esbuild = (sources) => esbuildBatch(sources, { target: esbuildTarget }),
  } = options;
  const variants = [
    ...withTerserVariants("tsc-es5", allSources, batchRunner(() => tsc(allSources))),
    ...withTerserVariants("swc-es5", allSources, batchRunner(() => swc(allSources))),
  ];
  // swc with externalHelpers emits `@swc/helpers` *imports* instead of inline
  // helper definitions — a distinct shape for helper-import recovery. Only worth
  // enabling for features swc actually lowers via a helper (spread family).
  if (swcExternalHelpers) {
    variants.push(
      ...withTerserVariants(
        "swc-es5-external",
        allSources,
        batchRunner(() => swcBatch(allSources, { externalHelpers: true })),
      ),
    );
  }
  variants.push(
    ...withTerserVariants(`esbuild-${esbuildTarget}`, allSources, batchRunner(() => esbuild(allSources))),
  );
  if (includeSource) {
    variants.push(...withTerserVariants("source", allSources, (source) => source, { includeRaw: false }));
  }
  return variants;
}
