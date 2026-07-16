import { Allotment } from "allotment";
import "allotment/dist/style.css";
import { Children, type ReactNode } from "react";

interface SplitLayoutProps {
  children: ReactNode;
}

export function SplitLayout({ children }: SplitLayoutProps) {
  const panes = Children.toArray(children);

  if (panes.length === 3) {
    return (
      <div className="editor-area editor-area-roundtrip">
        <Allotment vertical defaultSizes={[50, 50]}>
          <Allotment.Pane>
            <div className="roundtrip-top-row">
              <Allotment defaultSizes={[50, 50]}>
                <Allotment.Pane>{panes[0]}</Allotment.Pane>
                <Allotment.Pane>{panes[1]}</Allotment.Pane>
              </Allotment>
            </div>
          </Allotment.Pane>
          <Allotment.Pane>
            <div className="roundtrip-restored-row">{panes[2]}</div>
          </Allotment.Pane>
        </Allotment>
      </div>
    );
  }

  return (
    <div className="editor-area">
      <Allotment key={panes.length} defaultSizes={[50, 50]}>
        {panes.map((pane, index) => (
          <Allotment.Pane key={index}>{pane}</Allotment.Pane>
        ))}
      </Allotment>
    </div>
  );
}
