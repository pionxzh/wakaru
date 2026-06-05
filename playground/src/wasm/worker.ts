import init, { decompile } from "wakaru-wasm";
import type { WorkerRequest, WorkerResponse } from "./types";

let initialized = false;

self.onmessage = async (event: MessageEvent<WorkerRequest>) => {
  const msg = event.data;

  if (msg.type === "init") {
    try {
      await init();
      initialized = true;
      self.postMessage({ type: "init-done" } satisfies WorkerResponse);
    } catch (e) {
      self.postMessage({
        type: "init-error",
        error: e instanceof Error ? e.message : String(e),
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
        msg.diagnostics,
        msg.formatter
      );
      self.postMessage({
        type: "decompile-result",
        id: msg.id,
        code: result.code,
        warnings: result.warnings,
      } satisfies WorkerResponse);
    } catch (e) {
      self.postMessage({
        type: "decompile-error",
        id: msg.id,
        error: e instanceof Error ? e.message : String(e),
      } satisfies WorkerResponse);
    }
  }
};
