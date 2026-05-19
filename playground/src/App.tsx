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

export function App() {
  const [source, setSource] = useState(DEFAULT_EXAMPLE);
  const [output, setOutput] = useState("");
  const [warnings, setWarnings] = useState<WakaruWarning[]>([]);
  const [level, setLevel] = useState<Level>("standard");
  const [isLoading, setIsLoading] = useState(false);
  const [wasmReady, setWasmReady] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [elapsed, setElapsed] = useState<number | null>(null);
  const bridgeRef = useRef<WasmBridge | null>(null);

  const runDecompile = useCallback(async (src: string, lvl: string) => {
    if (!bridgeRef.current) return;
    setIsLoading(true);
    setError(null);
    const start = performance.now();
    try {
      const result = await bridgeRef.current.decompile(src, lvl, true);
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
        runDecompile(source, level);
      })
      .catch((e) => setError(e.message));
    return () => bridge.terminate();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const handleRun = useCallback(async () => {
    if (!bridgeRef.current || !wasmReady) return;
    runDecompile(source, level);
  }, [source, level, wasmReady, runDecompile]);

  return (
    <div className="app">
      <Header />
      <Controls
        level={level}
        onLevelChange={setLevel}
        onRun={handleRun}
        isLoading={isLoading}
        wasmReady={wasmReady}
        elapsed={elapsed}
      />
      <SplitLayout>
        <Editor value={source} onChange={setSource} />
        <OutputViewer value={output} />
      </SplitLayout>
      <WarningsPanel warnings={warnings} error={error} />
    </div>
  );
}
