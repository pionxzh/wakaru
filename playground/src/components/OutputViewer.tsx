import MonacoEditor, { type OnMount } from "@monaco-editor/react";
import { useCallback, useEffect, useRef } from "react";
import type { editor as MonacoEditorNS } from "monaco-editor";
import type { EditorDecoration } from "./Editor";

interface OutputViewerProps {
  value: string;
  decorations?: EditorDecoration[];
  onHoverLine?: (line: number | null) => void;
  onEditorReady?: (editor: MonacoEditorNS.IStandaloneCodeEditor) => void;
}

export function OutputViewer({ value, decorations, onHoverLine, onEditorReady }: OutputViewerProps) {
  const editorRef = useRef<MonacoEditorNS.IStandaloneCodeEditor | null>(null);
  const decorationIds = useRef<string[]>([]);
  const hoverRef = useRef(onHoverLine);
  hoverRef.current = onHoverLine;

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
    if (!decorations || decorations.length === 0) {
      decorationIds.current = editor.deltaDecorations(decorationIds.current, []);
      return;
    }
    const monacoDecorations: MonacoEditorNS.IModelDeltaDecoration[] = decorations.map((d) => ({
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
  }, [decorations]);

  return (
    <div className="editor-pane">
      <div className="editor-pane-label">Output</div>
      <MonacoEditor
        language="javascript"
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
