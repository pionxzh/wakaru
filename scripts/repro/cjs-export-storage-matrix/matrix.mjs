#!/usr/bin/env node

// CommonJS export-storage matrix: compile ESM sources to CommonJS with several
// producers, decompile each module back to ESM, and compare runtime behavior.
// See README.md for the producer profiles, verdicts, and options.

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  babelBatch, defaultConcurrency, ensureNodeTool, esbuildBatch, readOption, runPool,
  runWakaruArgsAsync, swcBatch, tscBatch, wakaruCommand,
} from "../lib/runner.mjs";
import { runNodeBatch } from "../lib/tool-process.mjs";
import { cases } from "./cases.mjs";

const BABEL_PROFILE = {
  core: "7.28.5",
  plugin: ["@babel/plugin-transform-modules-commonjs", "7.28.6"],
};

// Each producer compiles every source file (a string) or, when it needs the
// module graph on disk, every file path. Both return Map<input, code | Error>.
const producers = [
  { name: "tsc-5.9-es2020", batch: (sources) => tscBatch(sources, tscOptions("5.9.3", "ES2020")) },
  { name: "tsc-5.9-es5", batch: (sources) => tscBatch(sources, tscOptions("5.9.3", "ES5")) },
  { name: "tsc-4.3-es2015", batch: (sources) => tscBatch(sources, tscOptions("4.3.5", "ES2015")) },
  { name: "tsc-3.9-es5", batch: (sources) => tscBatch(sources, tscOptions("3.9.10", "ES5")) },
  { name: "babel-7.28", batch: (sources) => babelBatch(sources, BABEL_PROFILE) },
  {
    name: "babel-7.28-loose",
    batch: (sources) => babelBatch(sources, BABEL_PROFILE, { pluginOptions: { loose: true } }),
  },
  { name: "swc-1.16-es2020", batch: (sources) => swcBatch(sources, { target: "es2020", moduleType: "commonjs" }) },
  { name: "swc-1.16-es5", batch: (sources) => swcBatch(sources, { target: "es5", moduleType: "commonjs" }) },
  { name: "esbuild-0.28", batch: (sources) => esbuildBatch(sources, { target: "es2020", format: "cjs" }) },
  { name: "rollup-4.63", paths: true, batch: rollupBatch },
  { name: "sucrase-3.35", batch: sucraseBatch },
  // Bundlers compile a whole case into one file, which is unpacked instead of
  // decompiled file by file. The batch takes case directories. webpack 5.108
  // added an array form of `require.d` for `const` exports; 5.107 is the last
  // release that emits only the object form.
  webpackProducer("webpack-5.107-terser", "5.107.2", true),
  webpackProducer("webpack-5.111", "5.111.1", false),
  webpackProducer("webpack-5.111-terser", "5.111.1", true),
];

function webpackProducer(name, version, minimize) {
  return { name, bundle: true, batch: (dirs) => webpackBatch(dirs, name, version, minimize) };
}
// Set by the bundle's stub entry, so the CommonJS driver can reach the
// namespace of `mod.js` without making it the entry module.
const BUNDLE_GLOBAL = "__wakaruMatrixModule";

function tscOptions(version, target) {
  return { version, target, module: "CommonJS", esModuleInterop: true };
}

function rollupBatch(paths) {
  const toolDir = ensureNodeTool("rollup-4.63.5", ["rollup@4.63.5"]);
  const launcher = `
const fs = require("node:fs");
const { rollup } = require("rollup");
const paths = JSON.parse(fs.readFileSync(0, "utf8"));
(async () => {
  const results = [];
  for (const input of paths) {
    try {
      // One module per output file: every other module stays an external
      // require, matching the per-file output of the other producers.
      const bundle = await rollup({ input, external: (id) => id !== input, onwarn() {} });
      const { output } = await bundle.generate({ format: "cjs", exports: "named" });
      results.push({ code: output[0].code });
    } catch (e) { results.push({ error: e.message }); }
  }
  process.stdout.write(JSON.stringify(results));
})();
`;
  return runNodeBatch(launcher, paths, { label: "rollupBatch", format: "commonjs", cwd: toolDir });
}

// Bundle each case with `mod.js` as an ordinary module, not the entry: a stub
// entry imports its namespace and stores it on a global, which the drivers
// read from the bundle and from the unpacked entry alike. Webpack then emits
// `mod.js` as a module factory with `require.d` export getters, the shape the
// unpacker hands to `UnEsm`. Module concatenation is off so every source file
// stays its own module, and one chunk keeps `import()` targets in the bundle.
function webpackBatch(dirs, name, version, minimize) {
  const toolDir = ensureNodeTool(`webpack-${version}`, [`webpack@${version}`]);
  const launcher = `
const fs = require("node:fs");
const path = require("node:path");
const webpack = require("webpack");
const dirs = JSON.parse(fs.readFileSync(0, "utf8"));
const minimize = ${JSON.stringify(minimize)};
function bundle(dir) {
  const work = path.join(dir, "..", ${JSON.stringify(`${name}-build`)});
  const srcDir = path.join(work, "src");
  fs.mkdirSync(srcDir, { recursive: true });
  fs.writeFileSync(
    path.join(srcDir, "entry.js"),
    "import * as m from " + JSON.stringify(path.join(dir, "mod.js")) + "; globalThis.${BUNDLE_GLOBAL} = m;\\n",
  );
  return new Promise((resolve) => {
    webpack({
      mode: "production",
      context: srcDir,
      entry: "./entry.js",
      target: "node",
      output: { path: path.join(work, "dist"), filename: "bundle.js" },
      optimization: { concatenateModules: false, minimize },
      plugins: [new webpack.optimize.LimitChunkCountPlugin({ maxChunks: 1 })],
      devtool: false,
    }, (err, stats) => {
      if (err || stats.hasErrors()) {
        return resolve({ error: String(err ?? stats.toString({ all: false, errors: true })).split("\\n")[0] });
      }
      resolve({ code: fs.readFileSync(path.join(work, "dist", "bundle.js"), "utf8") });
    });
  });
}
(async () => {
  const results = [];
  for (const dir of dirs) {
    try { results.push(await bundle(dir)); } catch (e) { results.push({ error: e.message }); }
  }
  process.stdout.write(JSON.stringify(results));
})();
`;
  return runNodeBatch(launcher, dirs, { label: `webpackBatch ${name}`, format: "commonjs", cwd: toolDir });
}

function sucraseBatch(sources) {
  const toolDir = ensureNodeTool("sucrase-3.35.1", ["sucrase@3.35.1"]);
  const launcher = `
const fs = require("node:fs");
const { transform } = require("sucrase");
const sources = JSON.parse(fs.readFileSync(0, "utf8"));
process.stdout.write(JSON.stringify(sources.map((source) => {
  try { return { code: transform(source, { transforms: ["imports"] }).code }; }
  catch (e) { return { error: e.message }; }
})));
`;
  return runNodeBatch(launcher, sources, { label: "sucraseBatch", format: "commonjs", cwd: toolDir });
}

const DRIVER_PRELUDE =
  'const __log = []; const log = (v) => __log.push(v === undefined ? "<undefined>" : JSON.parse(JSON.stringify(v)));';

function writeDriver(dir, body, importLine) {
  writeFileSync(
    join(dir, "driver.mjs"),
    `${importLine}\n${DRIVER_PRELUDE}\n` +
      `try { ${body} } catch (e) { __log.push("THROW " + e.constructor.name + ": " + e.message); }\n` +
      "console.log(JSON.stringify(__log));\n",
  );
}

function runDriver(dir) {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, [join(dir, "driver.mjs")], { cwd: dir, timeout: 20_000 });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => (stdout += chunk));
    child.stderr.on("data", (chunk) => (stderr += chunk));
    child.on("close", (status) => {
      if (status === 0) return resolve(stdout.trim());
      // A load-time failure (link error, top-level throw) never reaches the
      // driver's try block; report the first error line instead.
      const line = stderr.split("\n").find((l) => /Error/.test(l)) ?? stderr.slice(0, 200);
      resolve(`CRASH ${line.trim()}`);
    });
  });
}

function writeModuleDir(dir, type, files) {
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, "package.json"), JSON.stringify({ type }));
  for (const [name, code] of Object.entries(files)) writeFileSync(join(dir, name), code);
}

const LEFTOVER = /\b(?:exports|module\.exports|require)\b/;
function leftoverLines(file, code) {
  return code
    .split("\n")
    .filter((line) => LEFTOVER.test(line))
    .map((line) => `${file}: ${line.trim()}`);
}

// A bundle row runs the bundle as CommonJS, unpacks it, and runs the unpacked
// entry, which stores the namespace of the recovered `mod.js` on the global.
async function runBundleRow(row, caseName, testCase, producer, outputs) {
  const result = outputs instanceof Error ? outputs : outputs.get(join(root, caseName, "esm"));
  if (result instanceof Error) {
    row.verdict = "compile-error";
    row.detail = result.message.split("\n")[0];
    return;
  }
  const code = result;
  const base = join(root, caseName, producer.name);
  const cjsDir = join(base, "cjs");
  writeModuleDir(cjsDir, "commonjs", { "bundle.js": code });
  writeDriver(
    cjsDir,
    testCase.driver,
    'import { createRequire } from "node:module"; createRequire(import.meta.url)("./bundle.js");' +
      ` const m = globalThis.${BUNDLE_GLOBAL};`,
  );
  row.commonjs = await runDriver(cjsDir);
  if (row.commonjs !== row.expected) {
    row.verdict = "producer-diverges";
    return;
  }
  if (explain) row.storage = { "bundle.js": ["not reported for unpacked bundles"] };

  // Unpack refuses a non-empty output directory, so package.json comes after.
  const recoveredDir = join(base, "recovered");
  row.leftovers = [];
  row.unrecovered = [];
  let report;
  try {
    report = JSON.parse(await runWakaruArgsAsync([join(cjsDir, "bundle.js"), "--unpack", "-o", recoveredDir, "--json"]));
  } catch (error) {
    row.verdict = "wakaru-error";
    row.detail = String(error.message ?? error).slice(0, 300);
    return;
  }
  writeModuleDir(recoveredDir, "module", {});
  for (const warning of report.warnings ?? []) {
    if (warning.kind === "commonjs_export_unrecovered") row.unrecovered.push(warning.message);
  }
  const files = (report.modules ?? []).map((module) => module.filename);
  if (!files.includes("entry.js")) {
    row.verdict = "wakaru-error";
    row.detail = `unpack emitted no entry.js (${files.join(", ")})`;
    return;
  }
  for (const file of files) {
    const path = join(recoveredDir, file);
    if (existsSync(path)) row.leftovers.push(...leftoverLines(file, readFileSync(path, "utf8")));
  }
  writeDriver(recoveredDir, testCase.driver, `import "./entry.js"; const m = globalThis.${BUNDLE_GLOBAL};`);
  row.recovered = await runDriver(recoveredDir);
  row.verdict = row.recovered === row.expected ? "ok" : "wrong";
}

const caseFilter = readOption("--case", null);
const producerFilter = readOption("--producer", null);
const asJson = process.argv.includes("--json");
const showDetails = process.argv.includes("--details");
const keep = process.argv.includes("--keep");
const explain = process.argv.includes("--explain");

const selectedCases = Object.entries(cases).filter(([name]) => !caseFilter || name.includes(caseFilter));
const selectedProducers = producers.filter((p) => !producerFilter || p.name.includes(producerFilter));
const root = mkdtempSync(join(tmpdir(), "wakaru-cjs-export-storage-"));

try {
  // Original ESM: write every case and record the expected behavior.
  const expected = new Map();
  await runPool(selectedCases, async ([caseName, testCase]) => {
    const dir = join(root, caseName, "esm");
    writeModuleDir(dir, "module", testCase.files);
    writeDriver(dir, testCase.driver, 'import * as m from "./mod.js";');
    expected.set(caseName, await runDriver(dir));
  });

  // Compile every source once per producer.
  const compiled = new Map();
  await Promise.all(
    selectedProducers.map(async (producer) => {
      const inputs = [];
      for (const [caseName, testCase] of selectedCases) {
        if (producer.bundle) {
          inputs.push(join(root, caseName, "esm"));
          continue;
        }
        for (const [file, code] of Object.entries(testCase.files)) {
          inputs.push(producer.paths ? join(root, caseName, "esm", file) : code);
        }
      }
      const unique = [...new Set(inputs)];
      try {
        compiled.set(producer.name, await producer.batch(unique));
      } catch (error) {
        compiled.set(producer.name, error instanceof Error ? error : new Error(String(error)));
      }
    }),
  );

  const jobs = [];
  for (const [caseName, testCase] of selectedCases) {
    for (const producer of selectedProducers) jobs.push({ caseName, testCase, producer });
  }
  const rows = [];
  await runPool(
    jobs,
    async ({ caseName, testCase, producer }) => {
      const row = { case: caseName, producer: producer.name, expected: expected.get(caseName) };
      rows.push(row);
      const outputs = compiled.get(producer.name);
      if (producer.bundle) {
        await runBundleRow(row, caseName, testCase, producer, outputs);
        return;
      }
      const cjsFiles = {};
      for (const [file, code] of Object.entries(testCase.files)) {
        const key = producer.paths ? join(root, caseName, "esm", file) : code;
        const result = outputs instanceof Error ? outputs : outputs.get(key);
        if (result instanceof Error) {
          row.verdict = "compile-error";
          row.detail = result.message.split("\n")[0];
          return;
        }
        cjsFiles[file] = result;
      }

      const base = join(root, caseName, producer.name);
      const cjsDir = join(base, "cjs");
      writeModuleDir(cjsDir, "commonjs", cjsFiles);
      writeDriver(
        cjsDir,
        testCase.driver,
        'import { createRequire } from "node:module"; const m = createRequire(import.meta.url)("./mod.js");',
      );
      row.commonjs = await runDriver(cjsDir);
      // A producer whose CommonJS already behaves differently from the ESM
      // source (including output that does not parse) cannot be recovered to
      // the source; keep it out of the score.
      if (row.commonjs !== row.expected) {
        row.verdict = "producer-diverges";
        return;
      }

      if (explain) {
        row.storage = {};
        for (const file of Object.keys(cjsFiles)) {
          try {
            const report = JSON.parse(await runWakaruArgsAsync(["debug", "cjs-exports", join(cjsDir, file), "--json"]));
            row.storage[file] = report.gate
              ? [`gate: ${report.gate}`]
              : report.exports.map(
                  (e) => `${e.name}=${e.storage}${e.binding ? `(${e.binding})` : ""}` +
                    e.rejected.map((r) => ` [${r}]`).join(""),
                );
          } catch (error) {
            row.storage[file] = [`error: ${String(error.message ?? error).slice(0, 200)}`];
          }
        }
      }

      const recoveredDir = join(base, "recovered");
      writeModuleDir(recoveredDir, "module", {});
      row.leftovers = [];
      row.unrecovered = [];
      for (const file of Object.keys(cjsFiles)) {
        const output = join(recoveredDir, file);
        try {
          const report = JSON.parse(await runWakaruArgsAsync([join(cjsDir, file), "-o", output, "--json"]));
          for (const warning of report.warnings ?? []) {
            if (warning.kind === "commonjs_export_unrecovered") row.unrecovered.push(`${file}: ${warning.message}`);
          }
        } catch (error) {
          row.verdict = "wakaru-error";
          row.detail = String(error.message ?? error).slice(0, 300);
          return;
        }
        if (existsSync(output)) row.leftovers.push(...leftoverLines(file, readFileSync(output, "utf8")));
      }
      writeDriver(recoveredDir, testCase.driver, 'import * as m from "./mod.js";');
      row.recovered = await runDriver(recoveredDir);
      row.verdict = row.recovered === row.expected ? "ok" : "wrong";
    },
    defaultConcurrency(),
  );

  const order = (row) => [
    selectedCases.findIndex(([name]) => name === row.case),
    selectedProducers.findIndex((p) => p.name === row.producer),
  ];
  rows.sort((a, b) => {
    const [ca, pa] = order(a);
    const [cb, pb] = order(b);
    return ca - cb || pa - pb;
  });

  const summary = {};
  for (const producer of selectedProducers) summary[producer.name] = { ok: 0, wrong: 0, excluded: 0 };
  for (const row of rows) {
    const entry = summary[row.producer];
    if (row.verdict === "ok") entry.ok++;
    else if (row.verdict === "wrong" || row.verdict === "wakaru-error") entry.wrong++;
    else entry.excluded++;
  }

  if (asJson) {
    // `summary` and each row's `status` use the shape every matrix reports to
    // collect-stats.mjs: a wrong row or a wakaru failure is `no`, a producer
    // whose CommonJS diverges from the source is informational, and a
    // producer that cannot compile the case is an error outside the score.
    const status = (verdict) => {
      if (verdict === "ok") return "yes";
      if (verdict === "wrong" || verdict === "wakaru-error") return "no";
      if (verdict === "producer-diverges") return "info-producer-diverges";
      return verdict;
    };
    const jsonRows = rows.map((row) => ({
      ...row,
      snippet: row.case,
      tools: [row.producer],
      status: status(row.verdict),
      notes: row.detail,
    }));
    const count = (predicate) => jsonRows.filter((row) => predicate(row.status)).length;
    const yes = count((s) => s === "yes");
    const no = count((s) => s === "no");
    const info = count((s) => s.startsWith("info-"));
    const error = jsonRows.length - yes - no - info;
    const pct = yes + no > 0 ? +((yes / (yes + no)) * 100).toFixed(1) : 0;
    console.log(JSON.stringify(
      { name: "cjs-export-storage", summary: { yes, no, error, info, pct }, producers: summary, rows: jsonRows },
      null,
      2,
    ));
  } else {
    const label = { ok: "ok", wrong: "**no**", "wakaru-error": "**err**", "producer-diverges": "p≠", "compile-error": "c-err" };
    console.log("# CommonJS export-storage matrix");
    console.log(`# wakaru: ${wakaruCommand()}`);
    console.log("");
    console.log(`| case | ${selectedProducers.map((p) => p.name).join(" | ")} |`);
    console.log(`|---|${selectedProducers.map(() => "---").join("|")}|`);
    for (const [caseName] of selectedCases) {
      const cells = selectedProducers.map((p) => {
        const row = rows.find((r) => r.case === caseName && r.producer === p.name);
        return label[row.verdict] ?? row.verdict;
      });
      console.log(`| ${caseName} | ${cells.join(" | ")} |`);
    }
    console.log("");
    console.log("| producer | ok | no | excluded |");
    console.log("|---|---:|---:|---:|");
    let ok = 0;
    let scored = 0;
    for (const [name, entry] of Object.entries(summary)) {
      console.log(`| ${name} | ${entry.ok} | ${entry.wrong} | ${entry.excluded} |`);
      ok += entry.ok;
      scored += entry.ok + entry.wrong;
    }
    console.log("");
    console.log(`Behavior preserved: ${ok}/${scored} (excluded: producer diverges from ESM or failed to compile)`);
    const warned = (verdict) => {
      const matching = rows.filter((r) => r.verdict === verdict);
      return `${matching.filter((r) => r.unrecovered?.length).length} of ${matching.length} ${verdict}`;
    };
    console.log(`commonjs_export_unrecovered warning on: ${warned("wrong")} rows, ${warned("ok")} rows`);
    if (showDetails) {
      for (const row of rows.filter((r) => r.verdict !== "ok")) {
        console.log("");
        console.log(`## ${row.case} / ${row.producer}: ${row.verdict}`);
        if (row.detail) console.log(`  detail:    ${row.detail}`);
        console.log(`  esm:       ${row.expected}`);
        if (row.commonjs !== undefined) console.log(`  commonjs:  ${row.commonjs}`);
        if (row.recovered !== undefined) console.log(`  recovered: ${row.recovered}`);
        for (const line of row.leftovers ?? []) console.log(`  leftover:  ${line}`);
        for (const line of row.unrecovered ?? []) console.log(`  warning:   ${line}`);
      }
    }
    if (explain) {
      for (const row of rows.filter((r) => r.storage)) {
        console.log("");
        console.log(`## storage ${row.case} / ${row.producer}: ${row.verdict}`);
        for (const [file, lines] of Object.entries(row.storage)) {
          for (const line of lines) console.log(`  ${file}: ${line}`);
        }
      }
    }
  }
  if (keep) console.error(`kept work directory: ${root}`);
} finally {
  if (!keep) rmSync(root, { recursive: true, force: true });
}
