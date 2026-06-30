import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { generateThumbnail } from "../bridge/tauri";
import { useT } from "../i18n";

// Logical fallback size when the connected image has no decodable thumbnail
// (browser preview mocks the backend). The crop box is recorded in this pixel
// space and the backend crops the real image against it on run.
const DEFAULT_W = 960;
const DEFAULT_H = 640;

const ASPECTS = ["free", "1:1", "4:3", "3:2", "16:9", "2:3", "3:4", "9:16"] as const;

/** A crop box in image pixels. */
export interface CropBox {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface CropCommit {
  mode: "manual" | "auto_subject";
  /** `[x, y, w, h]` in image pixels; omitted for auto (computed by the backend). */
  cropBox: [number, number, number, number] | null;
  aspect: string;
  marginPct: number;
}

interface CropEditModalProps {
  title: string;
  imagePath?: string | null;
  initialMode: "manual" | "auto_subject";
  initialBox: [number, number, number, number] | null;
  initialAspect: string;
  initialMargin: number;
  onCommit: (commit: CropCommit) => void;
  onClose: () => void;
}

type DragKind = "draw" | "move" | "nw" | "ne" | "sw" | "se";

function aspectRatio(aspect: string): number | null {
  if (aspect === "free") return null;
  const [a, b] = aspect.split(":");
  const an = Number(a);
  const bn = Number(b);
  return an > 0 && bn > 0 ? an / bn : null;
}

function clampBox(box: CropBox, w: number, h: number): CropBox {
  const bw = Math.min(Math.max(1, Math.round(box.w)), w);
  const bh = Math.min(Math.max(1, Math.round(box.h)), h);
  const bx = Math.min(Math.max(0, Math.round(box.x)), w - bw);
  const by = Math.min(Math.max(0, Math.round(box.y)), h - bh);
  return { x: bx, y: by, w: bw, h: bh };
}

/** A centred box covering ~80% of the image, used as the default manual box. */
function defaultBox(w: number, h: number): CropBox {
  const bw = Math.round(w * 0.8);
  const bh = Math.round(h * 0.8);
  return { x: Math.round((w - bw) / 2), y: Math.round((h - bh) / 2), w: bw, h: bh };
}

export function CropEditModal({
  title,
  imagePath,
  initialMode,
  initialBox,
  initialAspect,
  initialMargin,
  onCommit,
  onClose,
}: CropEditModalProps) {
  const t = useT();
  const [underlay, setUnderlay] = useState<string | null>(null);
  const [dims, setDims] = useState<{ w: number; h: number }>({ w: DEFAULT_W, h: DEFAULT_H });
  const [mode, setMode] = useState<"manual" | "auto_subject">(initialMode);
  const [aspect, setAspect] = useState<string>(initialAspect);
  const [margin, setMargin] = useState<number>(initialMargin);
  const [box, setBox] = useState<CropBox>(() =>
    initialBox ? { x: initialBox[0], y: initialBox[1], w: initialBox[2], h: initialBox[3] } : defaultBox(DEFAULT_W, DEFAULT_H),
  );
  // Whether the box came from the user (vs the default seeded for fresh dims).
  const boxTouched = useRef<boolean>(initialBox != null);

  const stageRef = useRef<HTMLDivElement | null>(null);
  const drag = useRef<{ kind: DragKind; startX: number; startY: number; origin: CropBox } | null>(null);

  // Best-effort underlay + true image dimensions. Empty in browser preview.
  useEffect(() => {
    if (!imagePath) return;
    let cancelled = false;
    generateThumbnail({ path: imagePath, size: 1280 })
      .then((thumb) => {
        if (cancelled) return;
        if (thumb.data_url) setUnderlay(thumb.data_url);
        if (thumb.width && thumb.height) {
          const w = thumb.width;
          const h = thumb.height;
          setDims({ w, h });
          if (!boxTouched.current) setBox(defaultBox(w, h));
        }
      })
      .catch(() => {
        /* keep the fallback dims + box */
      });
    return () => {
      cancelled = true;
    };
  }, [imagePath]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const ratio = useMemo(() => aspectRatio(aspect), [aspect]);

  // Apply the locked aspect ratio to a box, keeping width and deriving height.
  const applyAspect = useCallback(
    (b: CropBox): CropBox => {
      if (ratio == null) return b;
      return { ...b, h: Math.round(b.w / ratio) };
    },
    [ratio],
  );

  // Map a pointer event to image-pixel coordinates.
  const toImage = useCallback(
    (e: React.PointerEvent): [number, number] => {
      const stage = stageRef.current;
      if (!stage) return [0, 0];
      const rect = stage.getBoundingClientRect();
      const x = ((e.clientX - rect.left) / rect.width) * dims.w;
      const y = ((e.clientY - rect.top) / rect.height) * dims.h;
      return [x, y];
    },
    [dims.w, dims.h],
  );

  const onPointerDown = (kind: DragKind) => (e: React.PointerEvent) => {
    if (mode !== "manual") return;
    e.stopPropagation();
    (e.target as Element).setPointerCapture?.(e.pointerId);
    const [px, py] = toImage(e);
    boxTouched.current = true;
    if (kind === "draw") {
      const seed: CropBox = { x: Math.round(px), y: Math.round(py), w: 1, h: 1 };
      setBox(seed);
      drag.current = { kind: "se", startX: px, startY: py, origin: seed };
    } else {
      drag.current = { kind, startX: px, startY: py, origin: box };
    }
  };

  const onPointerMove = (e: React.PointerEvent) => {
    const d = drag.current;
    if (!d || mode !== "manual") return;
    const [px, py] = toImage(e);
    const dx = px - d.startX;
    const dy = py - d.startY;
    const o = d.origin;
    let next: CropBox = o;
    if (d.kind === "move") {
      next = { ...o, x: o.x + dx, y: o.y + dy };
    } else {
      // Resize from a corner: derive the new rect from the fixed opposite corner.
      let left = o.x;
      let top = o.y;
      let right = o.x + o.w;
      let bottom = o.y + o.h;
      if (d.kind === "nw") {
        left = o.x + dx;
        top = o.y + dy;
      } else if (d.kind === "ne") {
        right = o.x + o.w + dx;
        top = o.y + dy;
      } else if (d.kind === "sw") {
        left = o.x + dx;
        bottom = o.y + o.h + dy;
      } else if (d.kind === "se") {
        right = o.x + o.w + dx;
        bottom = o.y + o.h + dy;
      }
      next = {
        x: Math.min(left, right),
        y: Math.min(top, bottom),
        w: Math.abs(right - left),
        h: Math.abs(bottom - top),
      };
      next = applyAspect(next);
    }
    setBox(clampBox(next, dims.w, dims.h));
  };

  const onPointerUp = (e: React.PointerEvent) => {
    if (drag.current) {
      (e.target as Element).releasePointerCapture?.(e.pointerId);
      drag.current = null;
    }
  };

  const display = clampBox(box, dims.w, dims.h);
  // Box rect as percentages of the stage, so it tracks the letterboxed image.
  const pct = {
    left: `${(display.x / dims.w) * 100}%`,
    top: `${(display.y / dims.h) * 100}%`,
    width: `${(display.w / dims.w) * 100}%`,
    height: `${(display.h / dims.h) * 100}%`,
  };

  const handleApply = () => {
    if (mode === "auto_subject") {
      onCommit({ mode, cropBox: null, aspect, marginPct: margin });
    } else {
      const b = clampBox(box, dims.w, dims.h);
      onCommit({ mode, cropBox: [b.x, b.y, b.w, b.h], aspect, marginPct: margin });
    }
    onClose();
  };

  return (
    <div className="media-viewer-backdrop" onClick={onClose}>
      <div className="media-viewer crop-edit" onClick={(e) => e.stopPropagation()}>
        <div className="media-viewer-bar">
          <span className="media-viewer-name" title={title}>
            {title} <span className="muted">· {t("crop.title")}</span>
          </span>
          <div className="media-viewer-actions">
            <button className="primary" onClick={handleApply} title={t("crop.applyTitle")}>
              {t("crop.apply")}
            </button>
            <button onClick={onClose} title={t("crop.closeTitle")}>
              ✕
            </button>
          </div>
        </div>

        <div className="mask-edit-body">
          <div className="crop-edit-stage-wrap">
            <div
              ref={stageRef}
              className={`crop-edit-stage${mode === "auto_subject" ? " auto" : ""}`}
              style={{ aspectRatio: `${dims.w} / ${dims.h}` }}
              onPointerDown={onPointerDown("draw")}
              onPointerMove={onPointerMove}
              onPointerUp={onPointerUp}
              onPointerLeave={onPointerUp}
            >
              {underlay ? (
                <img className="crop-edit-img" src={underlay} alt="preview" draggable={false} />
              ) : (
                <div className="crop-edit-img placeholder" />
              )}
              {mode === "manual" ? (
                <div className="crop-box" style={pct} onPointerDown={onPointerDown("move")}>
                  <span className="crop-handle nw" onPointerDown={onPointerDown("nw")} />
                  <span className="crop-handle ne" onPointerDown={onPointerDown("ne")} />
                  <span className="crop-handle sw" onPointerDown={onPointerDown("sw")} />
                  <span className="crop-handle se" onPointerDown={onPointerDown("se")} />
                </div>
              ) : null}
            </div>
            <small className="muted">
              {mode === "manual" ? t("crop.drawHint") : t("crop.autoHint")}
            </small>
          </div>

          <div className="mask-edit-controls">
            <div className="field">
              <span>{t("crop.title")}</span>
              <div className="crop-mode-row">
                <button
                  className={mode === "manual" ? "active" : ""}
                  title={t("crop.modeManualTitle")}
                  onClick={() => setMode("manual")}
                >
                  {t("crop.modeManual")}
                </button>
                <button
                  className={mode === "auto_subject" ? "active" : ""}
                  title={t("crop.modeAutoTitle")}
                  onClick={() => setMode("auto_subject")}
                >
                  {t("crop.modeAuto")}
                </button>
              </div>
            </div>

            <label className="field">
              <span>{t("crop.aspect")}</span>
              <select value={aspect} onChange={(e) => setAspect(e.target.value)}>
                {ASPECTS.map((a) => (
                  <option key={a} value={a}>
                    {a === "free" ? t("crop.aspectFree") : a}
                  </option>
                ))}
              </select>
            </label>

            {mode === "auto_subject" ? (
              <label className="field">
                <span>{t("crop.margin")}</span>
                <span className="slider-row">
                  <input
                    type="range"
                    min={0}
                    max={100}
                    value={margin}
                    onChange={(e) => setMargin(Number(e.target.value))}
                  />
                  <output>{margin}</output>
                </span>
              </label>
            ) : (
              <>
                <div className="field">
                  <span>{t("crop.boxLabel")}</span>
                  <small className="muted">
                    {display.x},{display.y} · {display.w}×{display.h}
                  </small>
                </div>
                <button onClick={() => setBox(defaultBox(dims.w, dims.h))}>{t("crop.reset")}</button>
              </>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
