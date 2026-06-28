import type { ParamSpec } from "../graph/nodeSpecs";
import { isTauri, pickFile } from "../bridge/tauri";

interface ParamFieldProps {
  spec: ParamSpec;
  value: unknown;
  onChange: (value: unknown) => void;
  /**
   * When rendered on a node card, inputs must not start a node drag or pan the
   * canvas, so they carry React Flow's `nodrag` / `nowheel` classes.
   */
  compact?: boolean;
}

// Single source of truth for rendering a param control. Used by both the
// Inspector (full form) and the node card (inline editing) so the two never
// drift apart.
export function ParamField({ spec, value, onChange, compact }: ParamFieldProps) {
  const cls = compact ? "nodrag nowheel" : undefined;

  switch (spec.control) {
    case "textarea":
      return (
        <textarea className={cls} value={String(value ?? "")} onChange={(e) => onChange(e.target.value)} />
      );
    case "text":
      return (
        <input className={cls} value={String(value ?? "")} onChange={(e) => onChange(e.target.value)} />
      );
    case "path":
      return (
        <span className="path-row">
          <input className={cls} value={String(value ?? "")} onChange={(e) => onChange(e.target.value)} />
          {isTauri() && (
            <button
              type="button"
              className={compact ? "nodrag" : undefined}
              title="Choose a file…"
              onClick={async () => {
                const picked = await pickFile({
                  title: `Choose ${spec.label}`,
                  filterName: spec.pickerFilterName,
                  extensions: spec.pickerExtensions,
                });
                if (picked) onChange(picked);
              }}
            >
              Browse…
            </button>
          )}
        </span>
      );
    case "number":
      return (
        <input
          className={cls}
          type="number"
          value={String(value ?? 0)}
          min={spec.min}
          max={spec.max}
          step={spec.step}
          onChange={(e) => onChange(Number(e.target.value))}
        />
      );
    case "slider":
      return (
        <span className="slider-row">
          <input
            className={cls}
            type="range"
            value={Number(value ?? spec.min ?? 0)}
            min={spec.min ?? 0}
            max={spec.max ?? 100}
            step={spec.step ?? 1}
            onChange={(e) => onChange(Number(e.target.value))}
          />
          <output>{String(value ?? spec.min ?? 0)}</output>
        </span>
      );
    case "checkbox":
      return (
        <input
          className={cls}
          type="checkbox"
          checked={Boolean(value)}
          onChange={(e) => onChange(e.target.checked)}
        />
      );
    case "select":
      return (
        <select className={cls} value={String(value ?? "")} onChange={(e) => onChange(e.target.value)}>
          {(spec.options ?? []).map((o) => (
            <option key={o} value={o}>
              {o}
            </option>
          ))}
        </select>
      );
    default:
      return null;
  }
}
