import type {
  DecompileResult,
  WorkerRequest,
  WorkerResponse,
} from "./types";

export class WasmBridge {
  private worker: Worker;
  private pendingRequests = new Map<
    number,
    {
      resolve: (result: DecompileResult) => void;
      reject: (error: Error) => void;
    }
  >();
  private nextId = 0;
  private initPromise: Promise<void>;
  private resolveInit!: () => void;
  private rejectInit!: (e: Error) => void;

  constructor() {
    this.worker = new Worker(new URL("./worker.ts", import.meta.url), {
      type: "module",
    });
    this.worker.onmessage = this.handleMessage.bind(this);

    this.initPromise = new Promise((resolve, reject) => {
      this.resolveInit = resolve;
      this.rejectInit = reject;
    });
    this.worker.postMessage({ type: "init" } satisfies WorkerRequest);
  }

  private handleMessage(event: MessageEvent<WorkerResponse>) {
    const msg = event.data;
    switch (msg.type) {
      case "init-done":
        this.resolveInit();
        break;
      case "init-error":
        this.rejectInit(new Error(msg.error));
        break;
      case "decompile-result": {
        const pending = this.pendingRequests.get(msg.id);
        if (pending) {
          this.pendingRequests.delete(msg.id);
          pending.resolve({ code: msg.code, sourceMap: msg.sourceMap, warnings: msg.warnings });
        }
        break;
      }
      case "decompile-error": {
        const pending = this.pendingRequests.get(msg.id);
        if (pending) {
          this.pendingRequests.delete(msg.id);
          pending.reject(new Error(msg.error));
        }
        break;
      }
    }
  }

  async waitForInit(): Promise<void> {
    return this.initPromise;
  }

  async decompile(
    source: string,
    level: string,
    formatter: boolean,
    diagnostics = true,
    emitSourceMap = false
  ): Promise<DecompileResult> {
    await this.initPromise;
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pendingRequests.set(id, { resolve, reject });
      this.worker.postMessage({
        type: "decompile",
        id,
        source,
        level,
        formatter,
        diagnostics,
        emitSourceMap,
      } satisfies WorkerRequest);
    });
  }

  terminate() {
    this.worker.terminate();
  }
}
