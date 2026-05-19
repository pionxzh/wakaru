import { Allotment } from "allotment";
import "allotment/dist/style.css";
import type { ReactNode } from "react";

interface SplitLayoutProps {
  children: [ReactNode, ReactNode];
}

export function SplitLayout({ children }: SplitLayoutProps) {
  return (
    <div className="editor-area">
      <Allotment defaultSizes={[50, 50]}>
        <Allotment.Pane>{children[0]}</Allotment.Pane>
        <Allotment.Pane>{children[1]}</Allotment.Pane>
      </Allotment>
    </div>
  );
}
