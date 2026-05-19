import { LEVELS, type Level } from "../lib/constants";

interface ControlsProps {
  level: Level;
  onLevelChange: (level: Level) => void;
  onRun: () => void;
  isLoading: boolean;
  wasmReady: boolean;
  elapsed: number | null;
}

export function Controls({
  level,
  onLevelChange,
  onRun,
  isLoading,
  wasmReady,
  elapsed,
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
        <button
          className="controls-button"
          onClick={onRun}
          disabled={isLoading || !wasmReady}
        >
          {isLoading ? "Running..." : wasmReady ? "Decompile" : "Loading WASM..."}
        </button>
      </div>
      <div className="controls-right">
        {elapsed !== null && (
          <span className="controls-elapsed">{elapsed.toFixed(0)}ms</span>
        )}
      </div>
    </div>
  );
}
