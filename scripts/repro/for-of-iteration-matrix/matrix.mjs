#!/usr/bin/env node

import { runNodeBatchSync } from "../lib/tool-process.mjs";

import {
  runMatrix, batchRunner, tscBatch, swcBatch,
  esbuildBatch, withTerserVariants, ensureNodeTool,
} from "../lib/runner.mjs";
import { mangleValidator } from "../lib/compare.mjs";

const snippets = [
  {
    name: "array-basic",
    source: "export function f(items) { for (const item of items) { use(item); } }\n",
    expected: ["for (const item of items)", "use(item)"],
  },
  {
    name: "array-reassign",
    source: "export function f(items) { for (let item of items) { item = normalize(item); use(item); } }\n",
    expected: ["for (let item of items)", "item = normalize(item)", "use(item)"],
  },
  {
    name: "destructuring-pair",
    source: "export function f(entries) { for (const [key, value] of entries) { use(key, value); } }\n",
    expected: ["for (const [key, value] of entries)", "use(key, value)"],
  },
  {
    name: "destructuring-control-flow",
    source:
      "export function f(entries) { for (const [key, value] of entries) { if (value == null) continue; if (key === \"stop\") break; use(key, value); } }\n",
    expected: [
      "for (const [key, value] of entries)",
      "if (value == null)",
      "continue",
      'if (key === "stop")',
      "break",
      "use(key, value)",
    ],
    expectedAny: [
      [
        "for (const [key, value] of entries)",
        "if (value == null)",
        "continue",
        'if (key === "stop")',
        "break",
        "use(key, value)",
      ],
      [
        "for (const [key, value] of entries)",
        "if (value != null)",
        'if (key === "stop")',
        "break",
        "use(key, value)",
      ],
      ["for (const [", "!= null", "break", "use("],
    ],
    acceptForms: [
      `
export function f(entries) {
  for (const [key, value] of entries) {
    if (value != null) {
      if (key === "stop") break;
      use(key, value);
    }
  }
}
`,
    ],
  },
];

const allSources = snippets.map((s) => s.source);

// Custom tsc batch with downlevelIteration option
function tscDownlevelBatch(sources, downlevelIteration) {
  const toolDir = ensureNodeTool("typescript-5.9.3", ["typescript@5.9.3"]);
  const helperSource = `
const fs = require("node:fs");
const ts = require("typescript");
const downlevelIteration = process.env.MATRIX_DOWNLEVEL === "true";
const sources = JSON.parse(fs.readFileSync(0, "utf8"));
const results = sources.map(source => {
  try {
    return { code: ts.transpileModule(source, {
      compilerOptions: {
        target: ts.ScriptTarget.ES5,
        module: ts.ModuleKind.ESNext,
        downlevelIteration,
      },
    }).outputText };
  } catch (e) { return { error: e.message }; }
});
process.stdout.write(JSON.stringify(results));
`;
  return runNodeBatchSync(helperSource, sources, {
    label: "tsc-downlevel-batch.cjs",
    format: "commonjs",
    cwd: toolDir,
    env: { MATRIX_DOWNLEVEL: String(downlevelIteration) },
  });
}

// Custom babel batch with for-of + destructuring plugins
function babelForOfBatch(sources, mode) {
  const packages = [
    "@babel/core@7.28.5",
    "@babel/plugin-transform-for-of@7.27.1",
    "@babel/plugin-transform-destructuring@7.28.5",
  ];
  const toolDir = ensureNodeTool("babel-7.28-for-of-destructuring", packages);
  const helperSource = `
import fs from "node:fs";
const babelModule = await import("@babel/core");
const forOfModule = await import("@babel/plugin-transform-for-of");
const destructuringModule = await import("@babel/plugin-transform-destructuring");
const babel = babelModule.default ?? babelModule;
const forOf = forOfModule.default ?? forOfModule;
const destructuring = destructuringModule.default ?? destructuringModule;
const mode = process.env.MATRIX_BABEL_MODE;
const assumptions = mode === "iterableIsArray" ? { iterableIsArray: true } : {};
const pluginOptions = mode === "loose" ? { loose: true } : {};
const sources = JSON.parse(fs.readFileSync(0, "utf8"));
const results = sources.map(source => {
  try {
    return { code: babel.transformSync(source, {
      filename: "input.js", babelrc: false, configFile: false, comments: false, compact: false,
      assumptions,
      plugins: [[forOf, pluginOptions], [destructuring, {}]],
    }).code };
  } catch (e) { return { error: e.message }; }
});
process.stdout.write(JSON.stringify(results));
`;
  return runNodeBatchSync(helperSource, sources, {
    label: "babel-for-of-batch.mjs",
    cwd: toolDir,
    env: { MATRIX_BABEL_MODE: mode },
  });
}

const transformers = [
  ...withTerserVariants(
    "tsc-es5-downlevel-false",
    allSources,
    batchRunner(() => tscDownlevelBatch(allSources, false)),
  ),
  ...withTerserVariants(
    "tsc-es5-downlevel-true",
    allSources,
    batchRunner(() => tscDownlevelBatch(allSources, true)),
  ),
  ...["spec", "loose", "iterableIsArray"].flatMap((mode) =>
    withTerserVariants(
      `babel-7.28-${mode}`,
      allSources,
      batchRunner(() => babelForOfBatch(allSources, mode)),
    ),
  ),
  ...withTerserVariants("swc-es5", allSources, batchRunner(() => swcBatch(allSources))),
  ...withTerserVariants("esbuild-es2015", allSources, batchRunner(() => esbuildBatch(allSources))),
  ...withTerserVariants("source", allSources, (source) => source, { includeRaw: false }),
];

runMatrix({
  name: "for-of-iteration",
  snippets,
  transformers,
  ...mangleValidator(),
});
