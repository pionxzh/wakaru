import MonacoEditor from "@monaco-editor/react";

interface OutputViewerProps {
  value: string;
}

export function OutputViewer({ value }: OutputViewerProps) {
  return (
    <div className="editor-pane">
      <div className="editor-pane-label">Output</div>
      <MonacoEditor
        language="javascript"
        theme="vs-dark"
        value={value}
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
