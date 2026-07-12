import MonacoEditor, { type OnMount } from "@monaco-editor/react";
import { useCallback, useEffect, useRef } from "react";
import type { editor as MonacoEditorNS } from "monaco-editor";
import type { EditorDecoration } from "./Editor";
import type { OutputView } from "../lib/vuePreview";

interface OutputViewerProps {
  javascriptValue: string;
  vueSfcEnabled: boolean;
  vueSfc: string | null;
  view: OutputView;
  onViewChange: (view: OutputView) => void;
  isLoading: boolean;
  decorations?: EditorDecoration[];
  onHoverLine?: (line: number | null) => void;
  onEditorReady?: (editor: MonacoEditorNS.IStandaloneCodeEditor) => void;
}

export function OutputViewer({
  javascriptValue,
  vueSfcEnabled,
  vueSfc,
  view,
  onViewChange,
  isLoading,
  decorations,
  onHoverLine,
  onEditorReady,
}: OutputViewerProps) {
  const editorRef = useRef<MonacoEditorNS.IStandaloneCodeEditor | null>(null);
  const decorationIds = useRef<string[]>([]);
  const hoverRef = useRef(onHoverLine);
  hoverRef.current = onHoverLine;
  const activeView = view === "vue" && vueSfc ? "vue" : "javascript";
  const activeDecorations = activeView === "javascript" ? decorations : [];
  const value = activeView === "vue" ? vueSfc ?? "" : javascriptValue;

  const handleMount: OnMount = useCallback((editor) => {
    editorRef.current = editor;
    onEditorReady?.(editor);
    editor.onMouseMove((e) => {
      const line = e.target.position?.lineNumber ?? null;
      hoverRef.current?.(line !== null ? line - 1 : null);
    });
    editor.onMouseLeave(() => hoverRef.current?.(null));
  }, []);

  useEffect(() => {
    const editor = editorRef.current;
    if (!editor) return;
    if (!activeDecorations || activeDecorations.length === 0) {
      decorationIds.current = editor.deltaDecorations(decorationIds.current, []);
      return;
    }
    const monacoDecorations: MonacoEditorNS.IModelDeltaDecoration[] = activeDecorations.map((d) => ({
      range: {
        startLineNumber: d.line + 1,
        startColumn: d.startCol + 1,
        endLineNumber: d.line + 1,
        endColumn: d.endCol + 1,
      },
      options: d.wholeLine
        ? { className: d.className, isWholeLine: true }
        : { inlineClassName: d.className },
    }));
    decorationIds.current = editor.deltaDecorations(decorationIds.current, monacoDecorations);
  }, [activeDecorations]);

  return (
    <div className="editor-pane">
      <div className="editor-pane-header">
        <div className="output-tabs" role="tablist" aria-label="Output format">
          <button
            className="output-tab"
            type="button"
            role="tab"
            aria-selected={activeView === "javascript"}
            onClick={() => onViewChange("javascript")}
          >
            JavaScript
          </button>
          {vueSfcEnabled && (
            <button
              className="output-tab"
              type="button"
              role="tab"
              aria-selected={activeView === "vue"}
              disabled={!vueSfc}
              onClick={() => onViewChange("vue")}
            >
              Vue SFC
            </button>
          )}
        </div>
        {vueSfcEnabled && (
          <span className={`output-status${vueSfc ? " output-status-success" : ""}`}>
            {vueSfc ? "Experimental" : isLoading ? "Checking…" : "Not recovered"}
          </span>
        )}
      </div>
      <MonacoEditor
        language={activeView === "vue" ? "html" : "javascript"}
        theme="vs-dark"
        value={value}
        onMount={handleMount}
        options={{
          readOnly: true,
          minimap: { enabled: false },
          fontSize: 14,
          scrollBeyondLastLine: false,
          wordWrap: "on",
          automaticLayout: true,
          padding: { top: 12 },
        }}
      />
    </div>
  );
}
