#!/usr/bin/env node

import {
  runMatrix, batchRunner, swcBatch, esbuildBatch,
  terserBatch, withTerserVariants, ensureNodeTool,
} from "../lib/runner.mjs";
import { join, resolve } from "node:path";
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

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

const allSources = snippets.map((s) => s.source);

// Custom esbuild batch with minify: true
const repoRoot = resolve(fileURLToPath(new URL("../../..", import.meta.url)));
function esbuildMinifyBatch(sources) {
  const toolDir = ensureNodeTool("esbuild-0.28", ["esbuild@0.28.0"]);
  const helper = join(toolDir, "esbuild-minify-batch.cjs");
  writeFileSync(
    helper,
    `
const fs = require("node:fs");
const esbuild = require("esbuild");
const sources = JSON.parse(fs.readFileSync(0, "utf8"));
const results = sources.map(source => {
  try {
    return { code: esbuild.transformSync(source, {
      target: "es2015", format: "esm", loader: "js", minify: true, logLevel: "warning",
    }).code };
  } catch (e) { return { error: e.message }; }
});
process.stdout.write(JSON.stringify(results));
`,
  );
  const result = spawnSync("node", [helper], {
    cwd: toolDir,
    input: JSON.stringify(sources),
    encoding: "utf8",
    maxBuffer: 1024 * 1024 * 50,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`esbuild batch exited ${result.status}: ${result.stderr}`);
  const outputs = JSON.parse(result.stdout);
  const map = new Map();
  for (let i = 0; i < sources.length; i++) {
    map.set(sources[i], outputs[i].error ? new Error(outputs[i].error) : outputs[i].code);
  }
  return map;
}

// Custom SWC batch with minify enabled
function swcMinifyBatch(sources) {
  const toolDir = ensureNodeTool("swc", ["@swc/core@1"]);
  const helper = join(toolDir, "swc-minify-batch.cjs");
  writeFileSync(
    helper,
    `
const fs = require("node:fs");
const swc = require("@swc/core");
const sources = JSON.parse(fs.readFileSync(0, "utf8"));
const results = sources.map(source => {
  try {
    return { code: swc.transformSync(source, {
      filename: "input.js",
      jsc: {
        target: "es5",
        parser: { syntax: "ecmascript" },
        minify: { compress: true, mangle: true },
      },
      module: { type: "es6" },
      minify: true,
    }).code };
  } catch (e) { return { error: e.message }; }
});
process.stdout.write(JSON.stringify(results));
`,
  );
  const result = spawnSync("node", [helper], {
    cwd: toolDir,
    input: JSON.stringify(sources),
    encoding: "utf8",
    maxBuffer: 1024 * 1024 * 50,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`swc batch exited ${result.status}: ${result.stderr}`);
  const outputs = JSON.parse(result.stdout);
  const map = new Map();
  for (let i = 0; i < sources.length; i++) {
    map.set(sources[i], outputs[i].error ? new Error(outputs[i].error) : outputs[i].code);
  }
  return map;
}

const transformers = [
  ...withTerserVariants(
    "terser-5",
    allSources,
    batchRunner(() => terserBatch(allSources)),
    { includeRaw: false },
  ),
  ...withTerserVariants(
    "esbuild-0.28",
    allSources,
    batchRunner(() => esbuildMinifyBatch(allSources)),
  ),
  ...withTerserVariants(
    "swc-1",
    allSources,
    batchRunner(() => swcMinifyBatch(allSources)),
  ),
];

runMatrix({
  name: "conditional-switch",
  snippets,
  transformers,
});
