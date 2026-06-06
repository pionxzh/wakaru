import { useCallback, useEffect, useRef, useState } from "react";
import { Header } from "./components/Header";
import { Controls } from "./components/Controls";
import { Editor } from "./components/Editor";
import { OutputViewer } from "./components/OutputViewer";
import { SplitLayout } from "./components/SplitLayout";
import { WarningsPanel } from "./components/WarningsPanel";
import { WasmBridge } from "./wasm/bridge";
import type { WakaruWarning } from "./wasm/types";
import type { Level } from "./lib/constants";
import { DEFAULT_EXAMPLE } from "./lib/examples";
import { createShareUrl, readShareState, SHARE_LIMIT_MESSAGE } from "./lib/share";

const WAKARU_VERSION = import.meta.env.VITE_WAKARU_VERSION;
const WAKARU_GIT_HASH = import.meta.env.VITE_WAKARU_GIT_HASH;
const VERSION_LABEL = `v${WAKARU_VERSION}+${WAKARU_GIT_HASH}`;
const INITIAL_SHARE_STATE = readShareState();

export function App() {
  const [source, setSource] = useState(INITIAL_SHARE_STATE?.source ?? DEFAULT_EXAMPLE);
  const [output, setOutput] = useState("");
  const [warnings, setWarnings] = useState<WakaruWarning[]>([]);
  const [level, setLevel] = useState<Level>(INITIAL_SHARE_STATE?.level ?? "standard");
  const [formatter, setFormatter] = useState(
    INITIAL_SHARE_STATE?.formatter ?? true
  );
  const [isLoading, setIsLoading] = useState(false);
  const [wasmReady, setWasmReady] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [elapsed, setElapsed] = useState<number | null>(null);
  const [shareStatus, setShareStatus] = useState<string | null>(null);
  const bridgeRef = useRef<WasmBridge | null>(null);
  const shareStatusTimeoutRef = useRef<number | null>(null);

  const runDecompile = useCallback(async (src: string, lvl: string, fmt: boolean) => {
    if (!bridgeRef.current) return;
    setIsLoading(true);
    setError(null);
    const start = performance.now();
    try {
      const result = await bridgeRef.current.decompile(src, lvl, fmt, true);
      setOutput(result.code);
      setWarnings(result.warnings);
      setElapsed(performance.now() - start);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setOutput("");
      setWarnings([]);
    } finally {
      setIsLoading(false);
    }
  }, []);

  useEffect(() => {
    const bridge = new WasmBridge();
    bridgeRef.current = bridge;
    bridge
      .waitForInit()
      .then(() => {
        setWasmReady(true);
        runDecompile(source, level, formatter);
      })
      .catch((e) => setError(e.message));
    return () => bridge.terminate();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const handleRun = useCallback(async () => {
    if (!bridgeRef.current || !wasmReady) return;
    runDecompile(source, level, formatter);
  }, [source, level, formatter, wasmReady, runDecompile]);

  const showShareStatus = useCallback((message: string) => {
    if (shareStatusTimeoutRef.current !== null) {
      window.clearTimeout(shareStatusTimeoutRef.current);
    }
    setShareStatus(message);
    shareStatusTimeoutRef.current = window.setTimeout(() => {
      setShareStatus(null);
      shareStatusTimeoutRef.current = null;
    }, 2400);
  }, []);

  const handleShare = useCallback(async () => {
    let shareUrl: string;
    try {
      shareUrl = createShareUrl({
        source,
        level,
        formatter,
        version: VERSION_LABEL,
      });
    } catch (e) {
      const cleanUrl = new URL(window.location.href);
      cleanUrl.hash = "";
      window.history.replaceState(null, "", cleanUrl.toString());
      showShareStatus(e instanceof Error ? e.message : SHARE_LIMIT_MESSAGE);
      return;
    }

    window.history.replaceState(null, "", shareUrl);

    try {
      await navigator.clipboard.writeText(shareUrl);
      showShareStatus("Copied");
    } catch {
      showShareStatus("URL updated");
    }
  }, [formatter, level, showShareStatus, source]);

  useEffect(() => {
    return () => {
      if (shareStatusTimeoutRef.current !== null) {
        window.clearTimeout(shareStatusTimeoutRef.current);
      }
    };
  }, []);

  return (
    <div className="app">
      <Header
        version={WAKARU_VERSION}
        gitHash={WAKARU_GIT_HASH}
      />
      <Controls
        level={level}
        formatter={formatter}
        onLevelChange={setLevel}
        onFormatterChange={setFormatter}
        onRun={handleRun}
        onShare={handleShare}
        isLoading={isLoading}
        wasmReady={wasmReady}
        elapsed={elapsed}
        shareStatus={shareStatus}
      />
      <SplitLayout>
        <Editor value={source} onChange={setSource} />
        <OutputViewer value={output} />
      </SplitLayout>
      <WarningsPanel warnings={warnings} error={error} />
    </div>
  );
}
