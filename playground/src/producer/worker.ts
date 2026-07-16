/// <reference lib="webworker" />

import type { Producer } from "../lib/roundTrip";
import type { ProducerWorkerRequest, ProducerWorkerResponse } from "./types";

let swcReady: Promise<typeof import("@swc/wasm-web")> | null = null;
let esbuildReady: Promise<typeof import("esbuild-wasm")> | null = null;

async function compileWithBabel(source: string): Promise<string> {
  const Babel = await import("@babel/standalone");
  const result = Babel.transform(source, {
    filename: "round-trip.jsx",
    sourceType: "module",
    presets: [
      ["env", { targets: { chrome: "58" }, modules: false }],
      ["react", { runtime: "classic" }],
    ],
    comments: false,
    compact: false,
  });
  return result.code ?? "";
}

async function loadSwc() {
  if (!swcReady) {
    swcReady = Promise.all([
      import("@swc/wasm-web"),
      import("@swc/wasm-web/wasm_bg.wasm?url"),
    ]).then(async ([swc, { default: moduleOrPath }]) => {
      await swc.default({ module_or_path: moduleOrPath });
      return swc;
    });
  }
  return swcReady;
}

async function compileWithSwc(source: string): Promise<string> {
  const swc = await loadSwc();
  const result = await swc.transform(source, {
    filename: "round-trip.jsx",
    jsc: {
      parser: { syntax: "ecmascript", jsx: true },
      target: "es2017",
      transform: { react: { runtime: "classic" } },
    },
    module: { type: "es6" },
    sourceMaps: false,
  });
  return result.code;
}

async function loadEsbuild() {
  if (!esbuildReady) {
    esbuildReady = Promise.all([
      import("esbuild-wasm"),
      import("esbuild-wasm/esbuild.wasm?url"),
    ]).then(async ([esbuild, { default: wasmURL }]) => {
      await esbuild.initialize({ wasmURL, worker: false });
      return esbuild;
    });
  }
  return esbuildReady;
}

async function compileWithEsbuild(source: string): Promise<string> {
  const esbuild = await loadEsbuild();
  const result = await esbuild.transform(source, {
    loader: "jsx",
    target: "es2017",
    format: "esm",
    jsx: "transform",
    legalComments: "none",
  });
  return result.code;
}

async function compile(source: string, producer: Producer): Promise<string> {
  switch (producer) {
    case "babel":
      return compileWithBabel(source);
    case "swc":
      return compileWithSwc(source);
    case "esbuild":
      return compileWithEsbuild(source);
  }
}

self.onmessage = async (event: MessageEvent<ProducerWorkerRequest>) => {
  const message = event.data;
  try {
    const code = await compile(message.source, message.producer);
    self.postMessage({
      type: "compile-result",
      id: message.id,
      code,
    } satisfies ProducerWorkerResponse);
  } catch (error) {
    self.postMessage({
      type: "compile-error",
      id: message.id,
      error: error instanceof Error ? error.message : String(error),
    } satisfies ProducerWorkerResponse);
  }
};
