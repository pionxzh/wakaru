import MonacoEditor, { type OnMount } from "@monaco-editor/react";
import { useCallback, useEffect, useRef } from "react";
import type { editor as MonacoEditorNS } from "monaco-editor";
import type { EditorDecoration } from "./Editor";
import { EditorPaneHeader } from "./EditorPaneHeader";

interface ReadonlyEditorProps {
  value: string;
  label: string;
  status?: string;
  busy?: boolean;
  decorations?: EditorDecoration[];
  onHoverLine?: (line: number | null) => void;
  onEditorReady?: (editor: MonacoEditorNS.IStandaloneCodeEditor) => void;
}

export function ReadonlyEditor({
  value,
  label,
  status,
  busy = false,
  decorations,
  onHoverLine,
  onEditorReady,
}: ReadonlyEditorProps) {
  const editorRef = useRef<MonacoEditorNS.IStandaloneCodeEditor | null>(null);
  const decorationIds = useRef<string[]>([]);
  const hoverRef = useRef(onHoverLine);
  hoverRef.current = onHoverLine;

  const handleMount: OnMount = useCallback((editor) => {
    editorRef.current = editor;
    onEditorReady?.(editor);
    editor.onMouseMove((event) => {
      const line = event.target.position?.lineNumber ?? null;
      hoverRef.current?.(line !== null ? line - 1 : null);
    });
    editor.onMouseLeave(() => hoverRef.current?.(null));
  }, []);

  useEffect(() => {
    const editor = editorRef.current;
    if (!editor) return;
    const nextDecorations: MonacoEditorNS.IModelDeltaDecoration[] = (decorations ?? []).map(
      (decoration) => ({
        range: {
          startLineNumber: decoration.line + 1,
          startColumn: decoration.startCol + 1,
          endLineNumber: decoration.line + 1,
          endColumn: decoration.endCol + 1,
        },
        options: decoration.wholeLine
          ? { className: decoration.className, isWholeLine: true }
          : { inlineClassName: decoration.className },
      })
    );
    decorationIds.current = editor.deltaDecorations(
      decorationIds.current,
      nextDecorations
    );
  }, [decorations]);

  return (
    <div className="editor-pane">
      <EditorPaneHeader>
        <span className="editor-pane-title">{label}</span>
        {status && (
          <span
            className={`output-status${busy ? " output-status-busy" : ""}`}
            title={status}
          >
            {status}
          </span>
        )}
      </EditorPaneHeader>
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
