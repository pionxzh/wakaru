import { FORMATTERS, LEVELS, type Formatter, type Level } from "../lib/constants";

function ShareIcon() {
  return (
    <svg className="controls-button-icon" viewBox="0 0 24 24" aria-hidden="true">
      <circle cx="18" cy="5" r="3" />
      <circle cx="6" cy="12" r="3" />
      <circle cx="18" cy="19" r="3" />
      <line x1="8.59" x2="15.42" y1="13.51" y2="17.49" />
      <line x1="15.41" x2="8.59" y1="6.51" y2="10.49" />
    </svg>
  );
}

interface ControlsProps {
  level: Level;
  formatter: Formatter;
  onLevelChange: (level: Level) => void;
  onFormatterChange: (formatter: Formatter) => void;
  onRun: () => void;
  onShare: () => void;
  isLoading: boolean;
  wasmReady: boolean;
  elapsed: number | null;
  shareStatus: string | null;
}

export function Controls({
  level,
  formatter,
  onLevelChange,
  onFormatterChange,
  onRun,
  onShare,
  isLoading,
  wasmReady,
  elapsed,
  shareStatus,
}: ControlsProps) {
  return (
    <div className="controls">
      <div className="controls-left">
        <label className="controls-label">
          Level
          <select
            className="controls-select"
            value={level}
            onChange={(e) => onLevelChange(e.target.value as Level)}
          >
            {LEVELS.map((l) => (
              <option key={l.value} value={l.value}>
                {l.label}
              </option>
            ))}
          </select>
        </label>
        <label className="controls-label">
          Formatter
          <select
            className="controls-select"
            value={formatter}
            onChange={(e) => onFormatterChange(e.target.value as Formatter)}
          >
            {FORMATTERS.map((f) => (
              <option key={f.value} value={f.value}>
                {f.label}
              </option>
            ))}
          </select>
        </label>
        <button
          className="controls-button"
          onClick={onRun}
          disabled={isLoading || !wasmReady}
        >
          {isLoading ? "Running..." : wasmReady ? "Decompile" : "Loading WASM..."}
        </button>
        <span className="controls-separator" aria-hidden="true" />
        <button className="controls-button controls-button-secondary" onClick={onShare}>
          <ShareIcon />
          Share
        </button>
      </div>
      <div className="controls-right">
        {shareStatus && (
          <span className="controls-share-status" role="status">
            {shareStatus}
          </span>
        )}
        {elapsed !== null && (
          <span className="controls-elapsed">{elapsed.toFixed(0)}ms</span>
        )}
      </div>
    </div>
  );
}
