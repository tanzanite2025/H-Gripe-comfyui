import { useEffect, useState } from "react";
import { generateThumbnail } from "../bridge/tauri";

// Large image extensions we know how to display. Anything else falls back to a
// "open externally" hint rather than trying to decode it in the webview.
const IMAGE_RE = /\.(png|jpe?g|webp|gif|bmp|tiff?)$/i;

function basename(p: string): string {
  const parts = p.split(/[/\\]/);
  return parts[parts.length - 1] || p;
}

interface MediaViewerProps {
  path: string;
  onClose: () => void;
}

// Full-resolution media viewer (modal overlay). Big previews live here — never
// inside the node card — so the canvas stays light. We still go through the
// backend thumbnail command (at a large size) rather than decoding the raw
// original in the webview; the original path stays the source of truth and is
// shown for copy / external open.
export function MediaViewer({ path, onClose }: MediaViewerProps) {
  const [src, setSrc] = useState<string | null>(null);
  const [dims, setDims] = useState<{ w: number; h: number } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [actualSize, setActualSize] = useState(false);
  const isImage = IMAGE_RE.test(path);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  useEffect(() => {
    if (!isImage) return;
    let cancelled = false;
    setSrc(null);
    setError(null);
    // Request a large, crisp preview (capped) — high quality without loading
    // the raw original into the webview.
    generateThumbnail({ path, size: 1280 })
      .then((t) => {
        if (cancelled) return;
        if (t.data_url) {
          setSrc(t.data_url);
          setDims({ w: t.width, h: t.height });
        } else {
          // Browser preview (backend mocked) returns an empty data URL.
          setError("preview unavailable (backend mocked)");
        }
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [path, isImage]);

  return (
    <div className="media-viewer-backdrop" onClick={onClose}>
      <div className="media-viewer" onClick={(e) => e.stopPropagation()}>
        <div className="media-viewer-bar">
          <span className="media-viewer-name" title={path}>
            {basename(path)}
            {dims ? <span className="muted"> · {dims.w}×{dims.h}</span> : null}
          </span>
          <div className="media-viewer-actions">
            {isImage && src ? (
              <button onClick={() => setActualSize((v) => !v)}>
                {actualSize ? "Fit" : "100%"}
              </button>
            ) : null}
            <button onClick={onClose} title="Close (Esc)">
              ✕
            </button>
          </div>
        </div>
        <div className={`media-viewer-stage ${actualSize ? "actual" : "fit"}`}>
          {!isImage ? (
            <p className="muted">No inline preview for this file type. Original path:</p>
          ) : error ? (
            <p className="muted">{error}</p>
          ) : src ? (
            <img className="media-viewer-img" src={src} alt={basename(path)} />
          ) : (
            <p className="muted">loading…</p>
          )}
        </div>
        <code className="media-viewer-path">{path}</code>
      </div>
    </div>
  );
}
