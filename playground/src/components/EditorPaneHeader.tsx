import type { ReactNode } from "react";

interface EditorPaneHeaderProps {
  children: ReactNode;
  className?: string;
}

export function EditorPaneHeader({
  children,
  className,
}: EditorPaneHeaderProps) {
  return (
    <div className={`editor-pane-header${className ? ` ${className}` : ""}`}>
      {children}
    </div>
  );
}
