import type { WakaruWarning } from "../wasm/types";

interface WarningsPanelProps {
  warnings: WakaruWarning[];
  error: string | null;
}

export function WarningsPanel({ warnings, error }: WarningsPanelProps) {
  if (!error && warnings.length === 0) return null;

  return (
    <div className="warnings-panel">
      {error && <div className="warnings-error">{error}</div>}
      {warnings.length > 0 && (
        <details open>
          <summary className="warnings-summary">
            {warnings.length} warning{warnings.length !== 1 ? "s" : ""}
          </summary>
          <ul className="warnings-list">
            {warnings.map((w, i) => (
              <li key={i} className="warnings-item">
                <span className="warnings-kind">[{w.kind}]</span>{" "}
                <span className="warnings-message">{w.message}</span>
              </li>
            ))}
          </ul>
        </details>
      )}
    </div>
  );
}
