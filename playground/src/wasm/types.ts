export type WorkerRequest =
  | { type: "init" }
  | {
      type: "decompile";
      id: number;
      source: string;
      level: string;
      formatter: boolean;
      diagnostics: boolean;
      emitSourceMap: boolean;
      vueSfc: boolean;
    };

export type WorkerResponse =
  | { type: "init-done" }
  | { type: "init-error"; error: string }
  | {
      type: "decompile-result";
      id: number;
      code: string;
      sourceMap?: string;
      vueSfc?: string;
      warnings: WakaruWarning[];
    }
  | { type: "decompile-error"; id: number; error: string };

export interface WakaruWarning {
  filename: string;
  kind: string;
  message: string;
}

export interface DecompileResult {
  code: string;
  sourceMap?: string;
  vueSfc?: string;
  warnings: WakaruWarning[];
}
