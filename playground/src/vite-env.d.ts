/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_WAKARU_VERSION: string;
  readonly VITE_WAKARU_GIT_HASH: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}

declare module "wakaru-wasm" {
  export default function init(
    input?: RequestInfo | URL | Response | BufferSource | WebAssembly.Module
  ): Promise<void>;

  export function decompile(
    source: string,
    level?: string | null,
    sourcemap?: Uint8Array | null,
    diagnostics?: boolean | null,
    formatter?: boolean | null
  ): WakaruDecompileResult;

  export function unpack(
    source: string,
    level?: string | null,
    heuristicSplit?: boolean | null,
    diagnostics?: boolean | null,
    formatter?: boolean | null
  ): WakaruUnpackResult;

  export function ruleNames(): string[];

  export interface WakaruDecompileResult {
    code: string;
    warnings: WakaruWarning[];
  }

  export interface WakaruUnpackResult {
    modules: WakaruModule[];
    warnings: WakaruWarning[];
  }

  export interface WakaruModule {
    filename: string;
    code: string;
  }

  export interface WakaruWarning {
    filename: string;
    kind: string;
    message: string;
  }
}
