#!/usr/bin/env node
// Generates the payload with Closure Compiler, then packages those chunks in
// the public Closure Library ModuleManager response contract.
// Generated outputs are checked in, so Rust tests do not require Node.js.

import { spawnSync } from "node:child_process";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const cwd = dirname(fileURLToPath(import.meta.url));
const compilerVersion = "20260629.0.0";
const outputPath = join(cwd, "dist/compiler-chunks/bundle.js");
const check = process.argv.includes("--check");

const chunks = [
  { name: "base", inputs: 1, dependencies: [] },
  { name: "chunk_final", inputs: 1, dependencies: ["base"] },
  { name: "chunk_alpha", inputs: 1, dependencies: ["base"] },
  { name: "empty_one", inputs: 0, dependencies: ["base"] },
  { name: "empty_two", inputs: 0, dependencies: ["base"] },
  { name: "empty_three", inputs: 0, dependencies: ["base"] },
  { name: "chunk_beta", inputs: 1, dependencies: ["chunk_alpha"] },
  { name: "empty_four", inputs: 0, dependencies: ["base"] },
  { name: "empty_five", inputs: 0, dependencies: ["base"] },
];
const loadingIds = [
  "chunk_alpha",
  "empty_one",
  "empty_two",
  "empty_three",
  "chunk_beta",
  "empty_four",
  "empty_five",
  "chunk_final",
];

function run(command, args) {
  const result =
    process.platform === "win32"
      ? spawnSync([command, ...args].join(" "), {
          cwd,
          shell: true,
          stdio: "inherit",
        })
      : spawnSync(command, args, { cwd, stdio: "inherit" });

  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}

function chunkFlag(chunk) {
  const dependencies = chunk.dependencies.length
    ? `:${chunk.dependencies.join(",")}`
    : "";
  return `${chunk.name}:${chunk.inputs}${dependencies}`;
}

function encodeGraph(compilerGraph) {
  const graph = compilerGraph.filter(({ name }) => name !== "$weak$");
  const indexes = new Map(graph.map(({ name }, index) => [name, index]));

  return graph
    .map(({ name, dependencies }) => {
      const encodedDependencies = dependencies.map((dependency) => {
        const index = indexes.get(dependency);
        if (index === undefined) {
          throw new Error(`unknown chunk dependency: ${dependency}`);
        }
        return index.toString(36);
      });
      return encodedDependencies.length
        ? `${name}:${encodedDependencies.join(",")}`
        : name;
    })
    .join("/");
}

function guardedSegment(name, payload) {
  return [
    `/*_M:${name}*/`,
    "try{",
    name === "base" ? "" : `_.beginModule(${JSON.stringify(name)});`,
    payload.trim(),
    name === "base" ? "" : "_.endModule();",
    "}catch(e){_._DumpException(e)}",
  ]
    .filter(Boolean)
    .join("\n");
}

function assembleBundle(compilerDir) {
  const compilerGraph = JSON.parse(
    readFileSync(join(compilerDir, "chunks.json"), "utf8"),
  );
  const graph = encodeGraph(compilerGraph);
  const compilerChunks = new Map(
    chunks.map(({ name }) => [
      name,
      readFileSync(join(compilerDir, `${name}.js`), "utf8"),
    ]),
  );

  const runtime = [
    "_._DumpException=function(error){throw error};",
    "_.beginModule=function(id){_.activeModule=id};",
    "_.endModule=function(){_.activeModule=null};",
    `_._ModuleManager_initialize(${JSON.stringify(graph)},${JSON.stringify(loadingIds)});`,
  ].join("\n");
  const basePayload = `${runtime}\n${compilerChunks.get("base")}`;
  const segments = [guardedSegment("base", basePayload)];

  for (const name of loadingIds) {
    const payload = compilerChunks.get(name);
    if (payload === undefined) {
      throw new Error(`missing compiled chunk: ${name}`);
    }
    // Closure Compiler emits empty files for zero-input chunks. ModuleManager
    // treats these as synthetic modules, whose marker has no loader overhead.
    segments.push(payload.trim() ? guardedSegment(name, payload) : `/*_M:${name}*/`);
  }

  return [
    `/* Generated with google-closure-compiler@${compilerVersion}; see generate.mjs. */`,
    "/* _GlobalPrefix_ */",
    '"use strict";/*_JS*/',
    "this.default_ClosureProducer=this.default_ClosureProducer||{};",
    "(function(_){var window=this;",
    ...segments,
    "}).call(this,this.default_ClosureProducer);",
    "",
  ].join("\n");
}

const temporaryRoot = mkdtempSync(join(tmpdir(), "wakaru-closure-producer-"));
const compilerDir = join(temporaryRoot, "compiler");
mkdirSync(compilerDir, { recursive: true });

try {
  console.log(`=== Closure Compiler ${compilerVersion} ===`);
  run("npx", [
    "--no-install",
    "google-closure-compiler",
    "--js=src/base.js",
    "--js=src/chunk_final.js",
    "--js=src/chunk_alpha.js",
    "--js=src/chunk_beta.js",
    ...chunks.map((chunk) => `--chunk=${chunkFlag(chunk)}`),
    `--chunk_output_path_prefix=${compilerDir}/`,
    `--output_chunk_dependencies=${compilerDir}/chunks.json`,
    "--chunk_output_type=GLOBAL_NAMESPACE",
    "--compilation_level=ADVANCED",
    "--language_in=ECMASCRIPT_NEXT",
    "--language_out=ECMASCRIPT5",
    "--warning_level=QUIET",
  ]);

  const generated = assembleBundle(compilerDir);
  if (check) {
    const checkedIn = readFileSync(outputPath, "utf8");
    if (checkedIn !== generated) {
      console.error("Generated Closure fixture is stale. Run npm run generate.");
      process.exitCode = 1;
    } else {
      console.log("Checked-in fixture is current.");
    }
  } else {
    mkdirSync(dirname(outputPath), { recursive: true });
    writeFileSync(outputPath, generated);
    console.log("Wrote dist/compiler-chunks/bundle.js");
  }
} finally {
  rmSync(temporaryRoot, { recursive: true, force: true });
}
