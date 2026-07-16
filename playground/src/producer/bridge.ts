import type { Producer } from "../lib/roundTrip";
import type { ProducerWorkerRequest, ProducerWorkerResponse } from "./types";

export class ProducerBridge {
  private worker: Worker;
  private pendingRequests = new Map<
    number,
    {
      resolve: (code: string) => void;
      reject: (error: Error) => void;
    }
  >();
  private nextId = 0;

  constructor() {
    this.worker = new Worker(new URL("./worker.ts", import.meta.url), {
      type: "module",
    });
    this.worker.onmessage = this.handleMessage.bind(this);
  }

  private handleMessage(event: MessageEvent<ProducerWorkerResponse>) {
    const message = event.data;
    const pending = this.pendingRequests.get(message.id);
    if (!pending) return;

    this.pendingRequests.delete(message.id);
    if (message.type === "compile-result") {
      pending.resolve(message.code);
    } else {
      pending.reject(new Error(message.error));
    }
  }

  compile(source: string, producer: Producer): Promise<string> {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pendingRequests.set(id, { resolve, reject });
      this.worker.postMessage({
        type: "compile",
        id,
        source,
        producer,
      } satisfies ProducerWorkerRequest);
    });
  }

  terminate() {
    this.worker.terminate();
    for (const { reject } of this.pendingRequests.values()) {
      reject(new Error("Producer worker terminated"));
    }
    this.pendingRequests.clear();
  }
}
