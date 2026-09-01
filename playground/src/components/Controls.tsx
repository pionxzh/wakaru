import { LEVELS, type Level } from "../lib/constants";
import { EXAMPLES } from "../lib/examples";
import {
  PRODUCERS,
  type PlaygroundMode,
  type Producer,
} from "../lib/roundTrip";

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
  mode: PlaygroundMode;
  producer: Producer;
  level: Level;
  formatter: boolean;
  formatterDisabled: boolean;
  mapping: boolean;
  vueSfc: boolean;
  onModeChange: (mode: PlaygroundMode) => void;
  onProducerChange: (producer: Producer) => void;
  onLevelChange: (level: Level) => void;
  onFormatterChange: (formatter: boolean) => void;
  onMappingChange: (mapping: boolean) => void;
  onVueSfcChange: (vueSfc: boolean) => void;
  onLoadExample: (source: string) => void;
  onShare: () => void;
  isLoading: boolean;
  wasmReady: boolean;
  elapsed: number | null;
  shareStatus: string | null;
  coveragePct: number | null;
}

export function Controls({
  mode,
  producer,
  level,
  formatter,
  formatterDisabled,
  mapping,
  vueSfc,
  onModeChange,
  onProducerChange,
  onLevelChange,
  onFormatterChange,
  onMappingChange,
  onVueSfcChange,
  onLoadExample,
  onShare,
  isLoading,
  wasmReady,
  elapsed,
  shareStatus,
  coveragePct,
}: ControlsProps) {
  return (
    <div className="controls">
      <div className="controls-left">
        <div className="controls-mode" role="group" aria-label="Playground mode">
          <button
            className="controls-mode-button"
            type="button"
            aria-pressed={mode === "decompile"}
            onClick={() => onModeChange("decompile")}
          >
            Decompile
          </button>
          <button
            className="controls-mode-button"
            type="button"
            aria-pressed={mode === "roundtrip"}
            onClick={() => onModeChange("roundtrip")}
          >
            Compile &amp; Restore
          </button>
        </div>
        {mode === "decompile" && (
          <label className="controls-label">
            Example
            <select
              className="controls-select"
              value=""
              onChange={(event) => {
                const example = EXAMPLES.find(
                  ({ value }) => value === event.target.value
                );
                if (example) onLoadExample(example.source);
              }}
            >
              <option value="" disabled>
                Load…
              </option>
              {EXAMPLES.map(({ value, label }) => (
                <option key={value} value={value}>{label}</option>
              ))}
            </select>
          </label>
        )}
        {mode === "roundtrip" && (
          <label className="controls-label">
            Producer
            <select
              className="controls-select"
              value={producer}
              onChange={(event) => onProducerChange(event.target.value as Producer)}
            >
              {PRODUCERS.map(({ value, label }) => (
                <option key={value} value={value}>{label}</option>
              ))}
            </select>
          </label>
        )}
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
          <button
            className="controls-switch"
            type="button"
            role="switch"
            aria-checked={formatter}
            disabled={formatterDisabled}
            onClick={() => onFormatterChange(!formatter)}
          >
            <span className="controls-switch-thumb" />
          </button>
        </label>
        <label className="controls-label">
          Mapping
          <button
            className="controls-switch"
            type="button"
            role="switch"
            aria-checked={mapping}
            onClick={() => onMappingChange(!mapping)}
          >
            <span className="controls-switch-thumb" />
          </button>
        </label>
        <label
          className="controls-label controls-vue-label"
          title="Best-effort Vue 3 SFC recovery from generated render JavaScript"
        >
          <span className="controls-vue-copy">
            <span className="controls-experimental">Experimental</span>
            <span>Vue SFC</span>
          </span>
          <button
            className="controls-switch"
            type="button"
            role="switch"
            aria-checked={vueSfc}
            onClick={() => onVueSfcChange(!vueSfc)}
          >
            <span className="controls-switch-thumb" />
          </button>
        </label>
        {mapping && coveragePct !== null && (
          <span className="controls-elapsed">{coveragePct}% mapped</span>
        )}
        <span className="controls-separator" aria-hidden="true" />
        <button className="controls-button controls-button-secondary" onClick={onShare}>
          <ShareIcon />
          Share
        </button>
      </div>
      <div className="controls-right">
        {!wasmReady && <span className="controls-share-status">Loading WASM...</span>}
        {wasmReady && isLoading && <span className="controls-share-status">Running...</span>}
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
