import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from "react";
import { generateThumbnail } from "../bridge/tauri";
import {
  MASK_TOOLS,
  maskTool,
  DEFAULT_TOOL_ID,
  type MaskTool,
} from "./maskTools";
import {
  addBrushStroke,
  addOperation,
  canRedo,
  canUndo,
  clearEdits,
  editCount,
  initEditState,
  redo,
  undo,
  type EditState,
} from "./maskEdit";
import type { BrushStroke, EditPaths, MaskOperation } from "../types/production";

// Default logical canvas size when no backing image is available (browser
// preview mocks the backend, so the connected image often has no decodable
// thumbnail). Edits are recorded in this pixel space and the backend rasterises
// them against the real image on run.
const DEFAULT_W = 960;
const DEFAULT_H = 640;

type Action =
  | { type: "stroke"; stroke: BrushStroke }
  | { type: "op"; op: MaskOperation }
  | { type: "undo" }
  | { type: "redo" }
  | { type: "clear" };

function reducer(state: EditState, action: Action): EditState {
  switch (action.type) {
    case "stroke":
      return addBrushStroke(state, action.stroke);
    case "op":
      return addOperation(state, action.op);
    case "undo":
      return undo(state);
    case "redo":
      return redo(state);
    case "clear":
      return clearEdits(state);
  }
}

interface MaskEditModalProps {
  title: string;
  /** Backing image path (best-effort underlay); may be missing in preview. */
  imagePath?: string | null;
  initial: EditPaths | null;
  /** Magic-wand colour tolerance from the node's param. */
  wandTolerance: number;
  onCommit: (edits: EditPaths) => void;
  onClose: () => void;
}

let strokeSeq = 0;
const nextId = (prefix: string) => `${prefix}_${Date.now()}_${strokeSeq++}`;

export function MaskEditModal({
  title,
  imagePath,
  initial,
  wandTolerance,
  onCommit,
  onClose,
}: MaskEditModalProps) {
  const [state, dispatch] = useReducer(reducer, initial, initEditState);
  const [toolId, setToolId] = useState<string>(DEFAULT_TOOL_ID);
  const [brushSize, setBrushSize] = useState(24);
  const [amount, setAmount] = useState(4);
  const [tolerance, setTolerance] = useState(wandTolerance);
  const [overlayOnly, setOverlayOnly] = useState(false);

  const [underlay, setUnderlay] = useState<string | null>(null);
  const [dims, setDims] = useState<{ w: number; h: number }>({ w: DEFAULT_W, h: DEFAULT_H });

  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  // In-progress freehand stroke (image-space points), null when not drawing.
  const drawing = useRef<{ points: [number, number][] } | null>(null);
  const marquee = useRef<{ start: [number, number]; end: [number, number] } | null>(null);
  const [, forceRedraw] = useState(0);

  const tool = maskTool(toolId) ?? MASK_TOOLS[0];

  // Best-effort underlay: a large thumbnail of the connected image. Empty in
  // browser preview (mocked backend) — we then draw a checkerboard so the user
  // can still paint in the correct pixel space.
  useEffect(() => {
    if (!imagePath) return;
    let cancelled = false;
    generateThumbnail({ path: imagePath, size: 1280 })
      .then((t) => {
        if (cancelled) return;
        if (t.data_url) setUnderlay(t.data_url);
        if (t.width && t.height) setDims({ w: t.width, h: t.height });
      })
      .catch(() => {
        /* keep checkerboard */
      });
    return () => {
      cancelled = true;
    };
  }, [imagePath]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
      else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "z" && !e.shiftKey) {
        e.preventDefault();
        dispatch({ type: "undo" });
      } else if ((e.ctrlKey || e.metaKey) && (e.key.toLowerCase() === "y" || (e.key.toLowerCase() === "z" && e.shiftKey))) {
        e.preventDefault();
        dispatch({ type: "redo" });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  // Map a pointer event to image-pixel coordinates.
  const toImage = useCallback(
    (e: React.PointerEvent): [number, number] => {
      const canvas = canvasRef.current;
      if (!canvas) return [0, 0];
      const rect = canvas.getBoundingClientRect();
      const x = ((e.clientX - rect.left) / rect.width) * dims.w;
      const y = ((e.clientY - rect.top) / rect.height) * dims.h;
      return [Math.round(x), Math.round(y)];
    },
    [dims.w, dims.h],
  );

  // Redraw the overlay: underlay (optional), committed brush strokes, and the
  // in-progress stroke/marquee.
  const redraw = useCallback(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    canvas.width = dims.w;
    canvas.height = dims.h;
    ctx.clearRect(0, 0, dims.w, dims.h);

    if (!overlayOnly && underlay) {
      const img = new Image();
      img.src = underlay;
      try {
        ctx.globalAlpha = 0.85;
        ctx.drawImage(img, 0, 0, dims.w, dims.h);
        ctx.globalAlpha = 1;
      } catch {
        /* image may not be ready synchronously; the strokes still render */
      }
    } else if (overlayOnly) {
      // Transparency preview: dark backdrop so the mask reads clearly.
      ctx.fillStyle = "#0c0e14";
      ctx.fillRect(0, 0, dims.w, dims.h);
    }

    const paintStroke = (s: { mode: string; radius: number; points: [number, number][] }) => {
      ctx.strokeStyle = s.mode === "subtract" ? "rgba(244,98,98,0.55)" : "rgba(86,168,255,0.55)";
      ctx.fillStyle = ctx.strokeStyle;
      ctx.lineWidth = s.radius * 2;
      ctx.lineCap = "round";
      ctx.lineJoin = "round";
      if (s.points.length === 1) {
        const [x, y] = s.points[0];
        ctx.beginPath();
        ctx.arc(x, y, s.radius, 0, Math.PI * 2);
        ctx.fill();
        return;
      }
      ctx.beginPath();
      s.points.forEach(([x, y], i) => (i === 0 ? ctx.moveTo(x, y) : ctx.lineTo(x, y)));
      ctx.stroke();
    };

    state.current.brush_strokes.forEach(paintStroke);
    const live = drawing.current;
    if (live) paintStroke({ mode: tool.mode ?? "add", radius: brushSize, points: live.points });

    const mq = marquee.current;
    if (mq) {
      const [x1, y1] = mq.start;
      const [x2, y2] = mq.end;
      ctx.strokeStyle = "rgba(86,168,255,0.9)";
      ctx.lineWidth = 1.5;
      ctx.setLineDash([6, 4]);
      if (tool.id === "ellipse") {
        ctx.beginPath();
        ctx.ellipse((x1 + x2) / 2, (y1 + y2) / 2, Math.abs(x2 - x1) / 2, Math.abs(y2 - y1) / 2, 0, 0, Math.PI * 2);
        ctx.stroke();
      } else {
        ctx.strokeRect(Math.min(x1, x2), Math.min(y1, y2), Math.abs(x2 - x1), Math.abs(y2 - y1));
      }
      ctx.setLineDash([]);
    }
  }, [dims.w, dims.h, underlay, overlayOnly, state.current.brush_strokes, tool.mode, tool.id, brushSize]);

  useEffect(() => {
    redraw();
  }, [redraw]);

  const onPointerDown = (e: React.PointerEvent) => {
    if (tool.status !== "ready") return;
    (e.target as Element).setPointerCapture?.(e.pointerId);
    const pt = toImage(e);
    if (tool.kind === "paint") {
      drawing.current = { points: [pt] };
      forceRedraw((n) => n + 1);
    } else if (tool.kind === "marquee") {
      marquee.current = { start: pt, end: pt };
      forceRedraw((n) => n + 1);
    } else if (tool.kind === "click") {
      // Magic-wand: record a seeded flood-fill op for the backend.
      dispatch({ type: "op", op: { type: "wand", amount: tolerance, region: pt } });
    }
  };

  const onPointerMove = (e: React.PointerEvent) => {
    if (drawing.current) {
      drawing.current.points.push(toImage(e));
      redraw();
    } else if (marquee.current) {
      marquee.current.end = toImage(e);
      redraw();
    }
  };

  const onPointerUp = () => {
    if (drawing.current) {
      const stroke: BrushStroke = {
        id: nextId("stroke"),
        mode: tool.mode ?? "add",
        radius: brushSize,
        points: drawing.current.points,
      };
      drawing.current = null;
      dispatch({ type: "stroke", stroke });
    } else if (marquee.current) {
      const { start, end } = marquee.current;
      marquee.current = null;
      const region = [Math.min(start[0], end[0]), Math.min(start[1], end[1]), Math.max(start[0], end[0]), Math.max(start[1], end[1])];
      if (region[2] - region[0] > 1 && region[3] - region[1] > 1) {
        dispatch({ type: "op", op: { type: tool.id, region } });
      }
      forceRedraw((n) => n + 1);
    }
  };

  // Clicking a tool: `global` tools are immediate actions (no canvas mode);
  // paint/click/marquee tools become the active mode; `planned` tools are inert.
  const onToolClick = (t: MaskTool) => {
    if (t.status !== "ready") return;
    if (t.kind === "global") {
      const needsAmount = t.id === "grow" || t.id === "shrink" || t.id === "feather" || t.id === "smooth";
      dispatch({ type: "op", op: needsAmount ? { type: t.id, amount } : { type: t.id } });
      return;
    }
    setToolId(t.id);
  };

  const count = editCount(state.current);
  const ops = state.current.operations;
  const showAmount = useMemo(
    () => tool.kind === "global" || ["grow", "shrink", "feather", "smooth"].includes(toolId),
    [tool.kind, toolId],
  );

  return (
    <div className="media-viewer-backdrop" onClick={onClose}>
      <div className="media-viewer mask-edit" onClick={(e) => e.stopPropagation()}>
        <div className="media-viewer-bar">
          <span className="media-viewer-name" title={title}>
            {title} <span className="muted">· mask editor</span>
          </span>
          <div className="media-viewer-actions">
            <button disabled={!canUndo(state)} onClick={() => dispatch({ type: "undo" })} title="Undo (Ctrl+Z)">
              ↶ Undo
            </button>
            <button disabled={!canRedo(state)} onClick={() => dispatch({ type: "redo" })} title="Redo (Ctrl+Y)">
              ↷ Redo
            </button>
            <button disabled={count === 0} onClick={() => dispatch({ type: "clear" })} title="Discard all edits">
              Clear
            </button>
            <button className={overlayOnly ? "active" : ""} onClick={() => setOverlayOnly((v) => !v)} title="Toggle transparency preview">
              {overlayOnly ? "Show image" : "Mask only"}
            </button>
            <button className="primary" onClick={() => { onCommit(state.current); onClose(); }} title="Apply edits to the node">
              Apply
            </button>
            <button onClick={onClose} title="Close without applying (Esc)">
              ✕
            </button>
          </div>
        </div>

        <div className="mask-edit-body">
          <div className="mask-edit-tools">
            {MASK_TOOLS.map((t) => (
              <button
                key={t.id}
                className={`mask-tool ${t.status === "planned" ? "planned" : ""} ${toolId === t.id && t.kind !== "global" ? "active" : ""}`}
                disabled={t.status === "planned"}
                title={t.status === "planned" ? `${t.hint} (coming soon)` : t.hint}
                onClick={() => onToolClick(t)}
              >
                {t.label}
                {t.status === "planned" ? <em className="soon">soon</em> : null}
              </button>
            ))}
          </div>

          <div className="mask-edit-stage">
            <canvas
              ref={canvasRef}
              className="mask-edit-canvas"
              style={{ aspectRatio: `${dims.w} / ${dims.h}` }}
              onPointerDown={onPointerDown}
              onPointerMove={onPointerMove}
              onPointerUp={onPointerUp}
              onPointerLeave={onPointerUp}
            />
          </div>

          <div className="mask-edit-controls">
            <label className="field">
              <span>Brush size</span>
              <span className="slider-row">
                <input type="range" min={1} max={96} value={brushSize} onChange={(e) => setBrushSize(Number(e.target.value))} />
                <output>{brushSize}</output>
              </span>
            </label>
            {showAmount ? (
              <label className="field">
                <span>Amount (px)</span>
                <span className="slider-row">
                  <input type="range" min={0} max={16} value={amount} onChange={(e) => setAmount(Number(e.target.value))} />
                  <output>{amount}</output>
                </span>
              </label>
            ) : null}
            {tool.id === "wand" ? (
              <label className="field">
                <span>Wand tolerance</span>
                <span className="slider-row">
                  <input type="range" min={0} max={255} value={tolerance} onChange={(e) => setTolerance(Number(e.target.value))} />
                  <output>{tolerance}</output>
                </span>
              </label>
            ) : null}

            <div className="field">
              <span>Queued ops ({ops.length})</span>
              <div className="mask-op-list">
                {ops.length === 0 ? (
                  <small className="muted">none — paint or pick a tool</small>
                ) : (
                  ops.map((op, i) => (
                    <span key={i} className="mask-op-chip">
                      {op.type}
                      {op.amount != null ? ` ${op.amount}` : ""}
                    </span>
                  ))
                )}
              </div>
            </div>

            <small className="muted mask-edit-note">
              Edits ({count}) are recorded as <code>edit_paths</code> and rasterised by the
              backend on run; pen/lasso/matting are planned (greyed).
            </small>
          </div>
        </div>
      </div>
    </div>
  );
}
