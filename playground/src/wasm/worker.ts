import initWakaru, { decompile } from "wakaru-wasm";
import * as biomeFormatter from "@wasm-fmt/biome_fmt/vite";
import type { WakaruWarning, WorkerRequest, WorkerResponse } from "./types";

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}


let initialized = false;

self.onmessage = async (event: MessageEvent<WorkerRequest>) => {
  const msg = event.data;

  if (msg.type === "init") {
    try {
      await Promise.all([
        initWakaru(),
        initFormatter(), // Never rejects
      ]);
      initialized = true;
      self.postMessage({ type: "init-done" } satisfies WorkerResponse);
    } catch (e) {
      self.postMessage({
        type: "init-error",
        error: errorMessage(e),
      } satisfies WorkerResponse);
    }
    return;
  }

  if (msg.type === "decompile") {
    if (!initialized) {
      self.postMessage({
        type: "decompile-error",
        id: msg.id,
        error: "WASM not initialized",
      } satisfies WorkerResponse);
      return;
    }
    try {
      const result = decompile(
        msg.source,
        msg.level,
        undefined,
        msg.diagnostics
      );
      const { code, warnings: formatWarnings } = formatDecompiledCode(
        result.code
      );

      self.postMessage({
        type: "decompile-result",
        id: msg.id,
        code,
        warnings: [...result.warnings, ...formatWarnings],
      } satisfies WorkerResponse);
    } catch (e) {
      self.postMessage({
        type: "decompile-error",
        id: msg.id,
        error: errorMessage(e),
      } satisfies WorkerResponse);
    }
  }
};


const FORMAT_FILENAME = "decompiled.jsx";

const { format: formatWithBiome } = biomeFormatter;
const initBiomeFormatter = (
  biomeFormatter as typeof biomeFormatter & {
    default: () => Promise<unknown>;
  }
).default;

function createFormatWarning(message: string): WakaruWarning {
  return {
    filename: FORMAT_FILENAME,
    kind: "format",
    message,
  };
}

type FormatterState =
  | { type: "initializing" }
  | { type: "initialized" }
  | { type: "failed"; error: string };


let formatterState: FormatterState = { type: "initializing" };

async function initFormatter() {
  try {
    await initBiomeFormatter();
    formatterState = { type: "initialized" };
  } catch (e) {
    formatterState = { type: "failed", error: errorMessage(e) };
  }
}

interface FormatDecompiledCodeResult {
  code: string;
  warnings: WakaruWarning[];
}

function formatDecompiledCode(code: string): FormatDecompiledCodeResult {
  if (formatterState.type === "initialized") {
    try {
      return {
        code: formatWithBiome(code, FORMAT_FILENAME),
        warnings: [],
      };
    } catch (e) {
      return {
        code,
        warnings: [
          createFormatWarning(`Biome formatting failed: ${errorMessage(e)}`),
        ],
      };
    }
  }

  if (formatterState.type === "failed") {
    return {
      code,
      warnings: [
        createFormatWarning(
          `Biome formatter failed to initialize: ${formatterState.error}`
        ),
      ],
    };
  }

  return {
    code,
    warnings: [
      createFormatWarning(
        "Biome formatter is still initializing; output was returned unformatted."
      ),
    ],
  };
}