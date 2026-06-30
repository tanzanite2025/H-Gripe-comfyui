import { useEffect, useState } from "react";
import { generateThumbnail } from "../bridge/tauri";

// Shared "review gate" modal.
//
// Deliberately NOT Subject-Mask-specific: it is a generic, reusable surface you
// can drop after ANY stage to eyeball the current image / mask / result and
// decide whether to proceed. It exposes an optional `Edit` action that the
// caller wires to a heavier editor (e.g. the Mask-Edit modal) — the preview
// itself stays read-only and cheap. See docs/cards/subject-mask-matte.md
// (§ "Responsibility split").
//
// Like MediaViewer, it shows a backend-generated thumbnail (sized up) rather
// than decoding the raw original in the webview, so the canvas/media discipline
// is preserved. In browser preview the backend is mocked and returns an empty
// data URL, so we degrade to a path-only card.

const IMAGE_RE = /\.(png|jpe?g|webp|gif|bmp|tiff?)$/i;

function basename(p: string): string {
  const parts = p.split(/[/\\]/);
  return parts[parts.length - 1] || p;
}

interface PreviewLayer {
  label: string;
  path: string | null | undefined;
}

interface PreviewModalProps {
  title: string;
  /** Layers to flip between (e.g. image / mask / cutout). Blank paths are kept
   * so the gate can still say "not produced yet". */
  layers: PreviewLayer[];
  /** Optional caption under the bar (e.g. mask coverage / mode). */
  caption?: string;
  /** When set, an `Edit` button is shown that opens the heavier editor. */
  onEdit?: () => void;
  onClose: () => void;
}

function PreviewImage({ path }: { path: string }) {
  const [src, setSrc] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setSrc(null);
    setError(null);
    generateThumbnail({ path, size: 1280 })
      .then((t) => {
        if (cancelled) return;
        if (t.data_url) setSrc(t.data_url);
        else setError("preview unavailable (backend mocked)");
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [path]);

  if (error) return <p className="muted">{error}</p>;
  if (!src) return <p className="muted">loading…</p>;
  return <img className="media-viewer-img" src={src} alt={basename(path)} />;
}

export function PreviewModal({ title, layers, caption, onEdit, onClose }: PreviewModalProps) {
  // Default to the first layer that actually has a path, else the first layer.
  const firstReady = Math.max(0, layers.findIndex((l) => !!l.path));
  const [active, setActive] = useState(firstReady === -1 ? 0 : firstReady);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const layer = layers[active];
  const path = layer?.path ?? null;
  const isImage = path ? IMAGE_RE.test(path) : false;

  return (
    <div className="media-viewer-backdrop" onClick={onClose}>
      <div className="media-viewer preview-modal" onClick={(e) => e.stopPropagation()}>
        <div className="media-viewer-bar">
          <span className="media-viewer-name" title={title}>
            {title}
            {caption ? <span className="muted"> · {caption}</span> : null}
          </span>
          <div className="media-viewer-actions">
            {layers.length > 1 &&
              layers.map((l, i) => (
                <button
                  key={l.label}
                  className={i === active ? "active" : ""}
                  disabled={!l.path}
                  title={l.path ? l.label : `${l.label} (not produced yet)`}
                  onClick={() => setActive(i)}
                >
                  {l.label}
                </button>
              ))}
            {onEdit ? (
              <button className="primary" onClick={onEdit} title="Open the mask editor">
                Edit
              </button>
            ) : null}
            <button onClick={onClose} title="Close (Esc)">
              ✕
            </button>
          </div>
        </div>
        <div className="media-viewer-stage fit">
          {!path ? (
            <p className="muted">No “{layer?.label}” produced yet — run the node to generate it.</p>
          ) : !isImage ? (
            <p className="muted">No inline preview for this file type.</p>
          ) : (
            <PreviewImage path={path} />
          )}
        </div>
        {path ? <code className="media-viewer-path">{path}</code> : null}
      </div>
    </div>
  );
}
