#!/usr/bin/env node

import {
  runMatrix, batchRunner, swcBatch, esbuildBatch,
  terserBatch, withTerserVariants,
} from "../lib/runner.mjs";

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
    batchRunner(() => esbuildBatch(allSources, { minify: true })),
  ),
  ...withTerserVariants(
    "swc-1",
    allSources,
    batchRunner(() => swcBatch(allSources, { minify: true })),
  ),
];

runMatrix({
  name: "conditional-switch",
  snippets,
  transformers,
});
