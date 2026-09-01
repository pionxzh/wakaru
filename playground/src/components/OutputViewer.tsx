import MonacoEditor, { DiffEditor, type OnMount } from "@monaco-editor/react";
import { useCallback, useEffect, useRef } from "react";
import type { editor as MonacoEditorNS } from "monaco-editor";
import type { EditorDecoration } from "./Editor";
import type { OutputPaneView } from "../lib/outputPane";
import { EditorPaneHeader } from "./EditorPaneHeader";

interface OutputViewerProps {
  javascriptValue: string;
  javascriptLabel?: string;
  vueSfcEnabled: boolean;
  vueSfc: string | null;
  diffAvailable: boolean;
  diffOriginal: string;
  view: OutputPaneView;
  onViewChange: (view: OutputPaneView) => void;
  isLoading: boolean;
  decorations?: EditorDecoration[];
  onHoverLine?: (line: number | null) => void;
  onEditorReady?: (editor: MonacoEditorNS.IStandaloneCodeEditor) => void;
}

export function OutputViewer({
  javascriptValue,
  javascriptLabel = "JavaScript",
  vueSfcEnabled,
  vueSfc,
  diffAvailable,
  diffOriginal,
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
  const activeView = view;
  const activeDecorations = activeView === "javascript" ? decorations : [];
  const value = activeView === "vue" ? vueSfc ?? "" : javascriptValue;

  const handleMount: OnMount = useCallback((editor) => {
    editorRef.current = editor;
    decorationIds.current = [];
    onEditorReady?.(editor);
    editor.onMouseMove((e) => {
      const line = e.target.position?.lineNumber ?? null;
      hoverRef.current?.(line !== null ? line - 1 : null);
    });
    editor.onMouseLeave(() => hoverRef.current?.(null));
  }, []);

  useEffect(() => {
    const editor = editorRef.current;
    if (!editor || editor.getModel() === null) return;
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
      <EditorPaneHeader>
        <div className="output-tabs" role="tablist" aria-label="Output format">
          <button
            className="output-tab"
            type="button"
            role="tab"
            aria-selected={activeView === "javascript"}
            onClick={() => onViewChange("javascript")}
          >
            {javascriptLabel}
          </button>
          {diffAvailable && (
            <button
              className="output-tab"
              type="button"
              role="tab"
              aria-selected={activeView === "diff"}
              title="Diff against the original source"
              onClick={() => onViewChange("diff")}
            >
              Diff
            </button>
          )}
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
      </EditorPaneHeader>
      {activeView === "diff" ? (
        <DiffEditor
          original={diffOriginal}
          modified={javascriptValue}
          language="javascript"
          theme="vs-dark"
          options={{
            readOnly: true,
            renderSideBySide: false,
            minimap: { enabled: false },
            fontSize: 14,
            scrollBeyondLastLine: false,
            wordWrap: "on",
            automaticLayout: true,
          }}
        />
      ) : (
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
      )}
    </div>
  );
}
