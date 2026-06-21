import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { editor as MonacoEditorNS } from "monaco-editor";
import { Header } from "./components/Header";
import { Controls } from "./components/Controls";
import { Editor } from "./components/Editor";
import type { EditorDecoration } from "./components/Editor";
import { OutputViewer } from "./components/OutputViewer";
import { SplitLayout } from "./components/SplitLayout";
import { WarningsPanel } from "./components/WarningsPanel";
import { WasmBridge } from "./wasm/bridge";
import type { WakaruWarning } from "./wasm/types";
import type { Level } from "./lib/constants";
import { DEFAULT_EXAMPLE } from "./lib/examples";
import { createShareUrl, readShareState, SHARE_LIMIT_MESSAGE } from "./lib/share";
import { parseMappings, lineColorClass, lineColorActiveClass, generateMappingCSS, LINE_COLORS_RGB } from "./lib/sourcemap";
import type { MappingData } from "./lib/sourcemap";

const WAKARU_VERSION = import.meta.env.VITE_WAKARU_VERSION;
const WAKARU_GIT_HASH = import.meta.env.VITE_WAKARU_GIT_HASH;
const VERSION_LABEL = `v${WAKARU_VERSION}+${WAKARU_GIT_HASH}`;
const INITIAL_SHARE_STATE = readShareState();
const INITIAL_AUTO_RUN_DELAY_MS = 80;
const MIN_AUTO_RUN_DELAY_MS = 60;
const MAX_AUTO_RUN_DELAY_MS = 300;
const RUNNING_STATUS_DELAY_MS = 180;

function getAutoRunDelay(elapsed: number) {
  return Math.round(
    Math.min(
      MAX_AUTO_RUN_DELAY_MS,
      Math.max(MIN_AUTO_RUN_DELAY_MS, elapsed * 0.5)
    )
  );
}

export function App() {
  const [source, setSource] = useState(INITIAL_SHARE_STATE?.source ?? DEFAULT_EXAMPLE);
  const [output, setOutput] = useState("");
  const [warnings, setWarnings] = useState<WakaruWarning[]>([]);
  const [level, setLevel] = useState<Level>(INITIAL_SHARE_STATE?.level ?? "standard");
  const [formatter, setFormatter] = useState(
    INITIAL_SHARE_STATE?.formatter ?? true
  );
  const [isLoading, setIsLoading] = useState(false);
  const [showRunningStatus, setShowRunningStatus] = useState(false);
  const [wasmReady, setWasmReady] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [elapsed, setElapsed] = useState<number | null>(null);
  const [shareStatus, setShareStatus] = useState<string | null>(null);
  const [mappingEnabled, setMappingEnabled] = useState(false);
  const [sourceMapJson, setSourceMapJson] = useState<string | undefined>();
  const [hoveredOutputLine, setHoveredOutputLine] = useState<number | null>(null);
  const [hoveredInputLine, setHoveredInputLine] = useState<number | null>(null);
  const inputEditorRef = useRef<MonacoEditorNS.IStandaloneCodeEditor | null>(null);
  const outputEditorRef = useRef<MonacoEditorNS.IStandaloneCodeEditor | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const editorWrapRef = useRef<HTMLDivElement | null>(null);
  const bridgeRef = useRef<WasmBridge | null>(null);
  const activeRunRef = useRef(false);
  const autoRunDelayRef = useRef(INITIAL_AUTO_RUN_DELAY_MS);
  const inputVersionRef = useRef(0);
  const latestInputRef = useRef({ source, level, formatter });
  const shareStatusTimeoutRef = useRef<number | null>(null);

  const runDecompile = useCallback(async () => {
    if (!bridgeRef.current || activeRunRef.current) return;
    activeRunRef.current = true;
    const input = latestInputRef.current;
    const inputVersion = inputVersionRef.current;
    setIsLoading(true);
    setError(null);
    const start = performance.now();
    try {
      const result = await bridgeRef.current.decompile(
        input.source,
        input.level,
        input.formatter,
        true,
        true
      );
      if (inputVersion !== inputVersionRef.current) return;
      const duration = performance.now() - start;
      autoRunDelayRef.current = getAutoRunDelay(duration);
      setOutput(result.code);
      setSourceMapJson(result.sourceMap);
      setWarnings(result.warnings);
      setElapsed(duration);
    } catch (e) {
      if (inputVersion !== inputVersionRef.current) return;
      autoRunDelayRef.current = getAutoRunDelay(performance.now() - start);
      setError(e instanceof Error ? e.message : String(e));
      setOutput("");
      setWarnings([]);
    } finally {
      activeRunRef.current = false;
      if (inputVersion !== inputVersionRef.current) {
        void runDecompile();
      } else {
        setIsLoading(false);
      }
    }
  }, []);

  useEffect(() => {
    const style = document.createElement("style");
    style.textContent = generateMappingCSS();
    document.head.appendChild(style);
    return () => { document.head.removeChild(style); };
  }, []);

  useEffect(() => {
    const bridge = new WasmBridge();
    bridgeRef.current = bridge;
    bridge
      .waitForInit()
      .then(() => {
        setWasmReady(true);
      })
      .catch((e) => setError(e.message));
    return () => bridge.terminate();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    latestInputRef.current = { source, level, formatter };
    inputVersionRef.current += 1;
  }, [source, level, formatter]);

  useEffect(() => {
    if (!wasmReady) return;
    if (activeRunRef.current) return;
    const timeoutId = window.setTimeout(() => {
      void runDecompile();
    }, autoRunDelayRef.current);
    return () => window.clearTimeout(timeoutId);
  }, [source, level, formatter, wasmReady, runDecompile]);

  useEffect(() => {
    if (!isLoading) {
      setShowRunningStatus(false);
      return;
    }

    const timeoutId = window.setTimeout(() => {
      setShowRunningStatus(true);
    }, RUNNING_STATUS_DELAY_MS);
    return () => window.clearTimeout(timeoutId);
  }, [isLoading]);

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

  const mappingData: MappingData | null = useMemo(() => {
    if (!sourceMapJson || !output) return null;
    try {
      return parseMappings(sourceMapJson, output);
    } catch {
      return null;
    }
  }, [sourceMapJson, output]);

  // Determine which output line is "active" (hovered directly, or via input hover)
  const activeOutputLine = useMemo(() => {
    if (!mappingEnabled || !mappingData) return null;
    if (hoveredOutputLine !== null) return hoveredOutputLine;
    if (hoveredInputLine !== null) {
      const region = mappingData.regions.find(r => r.srcLine === hoveredInputLine);
      return region ? region.genLine : null;
    }
    return null;
  }, [mappingEnabled, mappingData, hoveredOutputLine, hoveredInputLine]);

  const outputDecorations: EditorDecoration[] = useMemo(() => {
    if (!mappingEnabled || !mappingData) return [];
    // One whole-line decoration per output line that has any mapping tokens.
    const seen = new Map<number, number>();
    for (const r of mappingData.regions) {
      if (!seen.has(r.genLine)) seen.set(r.genLine, r.colorIndex);
    }
    return Array.from(seen.entries()).map(([line, colorIndex]) => ({
      line,
      startCol: 0,
      endCol: 0,
      className: activeOutputLine === line
        ? lineColorActiveClass(colorIndex)
        : lineColorClass(colorIndex),
      wholeLine: true,
    }));
  }, [mappingEnabled, mappingData, activeOutputLine]);

  const inputDecorations: EditorDecoration[] = useMemo(() => {
    if (!mappingEnabled || !mappingData) return [];
    return mappingData.regions.map((r) => ({
      line: r.srcLine,
      startCol: r.srcStartCol,
      endCol: r.srcEndCol,
      className: activeOutputLine === r.genLine
        ? lineColorActiveClass(r.colorIndex)
        : lineColorClass(r.colorIndex),
    }));
  }, [mappingEnabled, mappingData, activeOutputLine]);

  const hoverClearTimer = useRef<number | null>(null);

  const handleOutputHover = useCallback((line: number | null) => {
    if (!mappingEnabled) return;
    if (hoverClearTimer.current !== null) {
      window.clearTimeout(hoverClearTimer.current);
      hoverClearTimer.current = null;
    }
    if (line !== null) {
      setHoveredOutputLine((prev) => (prev === line ? prev : line));
      setHoveredInputLine((prev) => (prev === null ? prev : null));
    } else {
      hoverClearTimer.current = window.setTimeout(() => {
        setHoveredOutputLine(null);
        hoverClearTimer.current = null;
      }, 60);
    }
  }, [mappingEnabled]);

  const handleInputHover = useCallback((line: number | null) => {
    if (!mappingEnabled) return;
    if (hoverClearTimer.current !== null) {
      window.clearTimeout(hoverClearTimer.current);
      hoverClearTimer.current = null;
    }
    if (line !== null) {
      setHoveredInputLine((prev) => (prev === line ? prev : line));
      setHoveredOutputLine((prev) => (prev === null ? prev : null));
    } else {
      hoverClearTimer.current = window.setTimeout(() => {
        setHoveredInputLine(null);
        hoverClearTimer.current = null;
      }, 60);
    }
  }, [mappingEnabled]);

  // Find the source line for the active output line (for arrow drawing)
  const activeSrcLine = useMemo(() => {
    if (activeOutputLine === null || !mappingData) return null;
    const region = mappingData.regions.find(r => r.genLine === activeOutputLine);
    return region ? region.srcLine : null;
  }, [activeOutputLine, mappingData]);

  // Draw connector arrow between panes
  useLayoutEffect(() => {
    const canvas = canvasRef.current;
    const wrap = editorWrapRef.current;
    const inputEd = inputEditorRef.current;
    const outputEd = outputEditorRef.current;
    if (!canvas || !wrap) return;

    const dpr = window.devicePixelRatio || 1;
    canvas.width = wrap.offsetWidth * dpr;
    canvas.height = wrap.offsetHeight * dpr;
    canvas.style.width = wrap.offsetWidth + "px";
    canvas.style.height = wrap.offsetHeight + "px";
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, canvas.width, canvas.height);

    if (activeOutputLine === null || activeSrcLine === null || !inputEd || !outputEd || !mappingData) return;

    const region = mappingData.regions.find(r => r.genLine === activeOutputLine);
    if (!region) return;
    const color = LINE_COLORS_RGB[region.colorIndex % LINE_COLORS_RGB.length];

    const wrapRect = wrap.getBoundingClientRect();
    const inputDom = inputEd.getDomNode();
    const outputDom = outputEd.getDomNode();
    if (!inputDom || !outputDom) return;

    const inputRect = inputDom.getBoundingClientRect();
    const outputRect = outputDom.getBoundingClientRect();

    // Find the span boundaries for the active mapping
    const activeRegions = mappingData.regions.filter(r => r.genLine === activeOutputLine);
    const maxSrcEndCol = Math.max(...activeRegions.map(r => r.srcEndCol));

    // Input side: arrow lands at the right edge of the colored span
    const srcPos = inputEd.getScrolledVisiblePosition({ lineNumber: activeSrcLine + 1, column: maxSrcEndCol + 1 });
    // Output side: arrow starts at the left edge of the line content
    const outputLayout = outputEd.getLayoutInfo();
    const genPos = outputEd.getScrolledVisiblePosition({ lineNumber: activeOutputLine + 1, column: 1 });
    if (!srcPos || !genPos) return;

    const gap = 6;
    const srcX = inputRect.left - wrapRect.left + srcPos.left + gap;
    const srcY = inputRect.top - wrapRect.top + srcPos.top + srcPos.height / 2;
    const genX = outputRect.left - wrapRect.left + outputLayout.contentLeft - gap;
    const genY = outputRect.top - wrapRect.top + genPos.top + genPos.height / 2;
    const midX = (srcX + genX) / 2;

    ctx.beginPath();
    ctx.moveTo(genX, genY);
    ctx.bezierCurveTo(midX, genY, midX, srcY, srcX, srcY);
    ctx.strokeStyle = `rgba(${color},0.7)`;
    ctx.lineWidth = 2;
    ctx.stroke();

    // Arrow pointing left (toward input)
    ctx.beginPath();
    ctx.moveTo(srcX, srcY);
    ctx.lineTo(srcX + 5, srcY - 4);
    ctx.lineTo(srcX + 5, srcY + 4);
    ctx.closePath();
    ctx.fillStyle = `rgba(${color},0.7)`;
    ctx.fill();

    // Dot on output side
    ctx.beginPath();
    ctx.arc(genX, genY, 3, 0, Math.PI * 2);
    ctx.fillStyle = `rgba(${color},0.7)`;
    ctx.fill();
  }, [activeOutputLine, activeSrcLine, mappingData]);

  return (
    <div className="app">
      <Header
        version={WAKARU_VERSION}
        gitHash={WAKARU_GIT_HASH}
      />
      <Controls
        level={level}
        formatter={formatter}
        mapping={mappingEnabled}
        onLevelChange={setLevel}
        onFormatterChange={setFormatter}
        onMappingChange={setMappingEnabled}
        onShare={handleShare}
        isLoading={showRunningStatus}
        wasmReady={wasmReady}
        elapsed={elapsed}
        shareStatus={shareStatus}
        coveragePct={mappingData?.coveragePct ?? null}
      />
      <div ref={editorWrapRef} style={{ position: "relative", flex: 1, minHeight: 0, display: "flex", flexDirection: "column" }}>
        <canvas
          ref={canvasRef}
          style={{ position: "absolute", top: 0, left: 0, width: "100%", height: "100%", pointerEvents: "none", zIndex: 10 }}
        />
        <SplitLayout>
          <Editor
            value={source}
            onChange={setSource}
            decorations={inputDecorations}
            onHoverLine={handleInputHover}
            onEditorReady={(ed) => { inputEditorRef.current = ed; }}
          />
          <OutputViewer
            value={output}
            decorations={outputDecorations}
            onHoverLine={handleOutputHover}
            onEditorReady={(ed) => { outputEditorRef.current = ed; }}
          />
        </SplitLayout>
      </div>
      <WarningsPanel warnings={warnings} error={error} />
    </div>
  );
}
