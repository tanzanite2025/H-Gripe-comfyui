import { useState } from "react";
import { getOutputDir, listPsdOutputs, type PsdOutput } from "../bridge/tauri";

interface OutputPickerProps {
  /**
   * `template` picks the `.psd` path; `image` picks the preview PNG (falling
   * back to the `.psd` path when no preview exists).
   */
  kind: "template" | "image";
  onPick: (path: string) => void;
}

// Browse the configured output directory's `.psd` outputs (via the same backend
// commands as PSD Studio) and drop a path into a node's `path` param.
export function OutputPicker({ kind, onPick }: OutputPickerProps) {
  const [items, setItems] = useState<PsdOutput[] | null>(null);
  const [open, setOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = async () => {
    setError(null);
    try {
      const dir = await getOutputDir();
      setItems(await listPsdOutputs(dir));
      setOpen(true);
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="output-picker">
      <button type="button" onClick={() => (open ? setOpen(false) : void load())}>
        {open ? "Hide PSD outputs" : "Pick from PSD outputs"}
      </button>
      {error && <small className="hint">outputs unavailable: {error}</small>}
      {open && items && (
        <div className="output-list">
          {items.length === 0 && <small className="hint">no PSD outputs found</small>}
          {items.map((o) => {
            const path = kind === "template" ? o.psd_path : (o.preview_path ?? o.psd_path);
            const noPreview = kind === "image" && !o.preview_path;
            return (
              <button
                key={o.psd_path}
                type="button"
                className="output-item"
                title={path}
                onClick={() => {
                  onPick(path);
                  setOpen(false);
                }}
              >
                {o.name}
                {noPreview ? " (no preview)" : ""}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
