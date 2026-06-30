// Mask-Edit tool registry (Subject Mask card).
//
// The Mask-Edit modal renders its toolbar from this registry rather than
// hard-coding buttons, so Phase 1 can ship the morphology/brush set while
// pen/lasso/matting stay visibly stubbed. A `planned` tool renders greyed
// ("coming soon") and is not selectable. This mirrors the frozen contract in
// `docs/cards/subject-mask-matte.md` (§ "Mask-Edit tool registry").

export type ToolStatus = "ready" | "planned";

/** How a tool behaves on the canvas, which drives pointer handling. */
export type ToolKind =
  // Freehand paint that records a `brush_strokes` entry.
  | "paint"
  // Single click that records an `operations` entry seeded at the click point.
  | "click"
  // Single click that records a SAM 2 point prompt (`points` entry).
  | "point"
  // Freehand paint that records a `matte_strokes` entry: the trimap unknown
  // band the matter resolves into soft alpha (hair / fur / glass).
  | "matte"
  // Whole-mask operation with no canvas interaction (records an `operations` entry).
  | "global"
  // Drag a marquee that records an `operations` entry with a rect region.
  | "marquee"
  // Phase 3+: places vector anchor points (stored, not rasterised in Phase 1).
  | "path";

export interface MaskTool {
  id: string;
  /** Short label shown on the toolbar button. */
  label: string;
  /** `ready` tools are interactive; `planned` render greyed and disabled. */
  status: ToolStatus;
  kind: ToolKind;
  /** `add` builds the mask up, `subtract` cuts it away (paint/marquee tools). */
  mode?: "add" | "subtract";
  /** One-line tooltip describing the Phase 1 behaviour. */
  hint: string;
}

// Order here is the toolbar order. Keep `ready` tools first, `planned` last,
// matching the contract table.
export const MASK_TOOLS: readonly MaskTool[] = [
  { id: "brush", label: "Brush", status: "ready", kind: "paint", mode: "add", hint: "Paint mask in." },
  { id: "eraser", label: "Eraser", status: "ready", kind: "paint", mode: "subtract", hint: "Paint mask out." },
  { id: "point", label: "Point (SAM 2)", status: "ready", kind: "point", hint: "Left-click the subject to include, right-click to exclude — SAM 2 segments from your points (auto modes)." },
  { id: "wand", label: "Wand", status: "ready", kind: "click", hint: "Flood-fill a region by colour similarity (wand_tolerance)." },
  { id: "rect", label: "Rect", status: "ready", kind: "marquee", mode: "add", hint: "Marquee add a rectangle." },
  { id: "ellipse", label: "Ellipse", status: "ready", kind: "marquee", mode: "add", hint: "Marquee add an ellipse." },
  { id: "invert", label: "Invert", status: "ready", kind: "global", hint: "Invert the whole mask." },
  { id: "fill_holes", label: "Fill holes", status: "ready", kind: "global", hint: "Close interior holes." },
  { id: "smooth", label: "Smooth", status: "ready", kind: "global", hint: "Morphological open/close." },
  { id: "grow", label: "Grow", status: "ready", kind: "global", hint: "Dilate the mask by N px." },
  { id: "shrink", label: "Shrink", status: "ready", kind: "global", hint: "Erode the mask by N px." },
  { id: "feather", label: "Feather", status: "ready", kind: "global", hint: "Gaussian-feather the mask edge." },
  { id: "matting", label: "Matting", status: "ready", kind: "matte", hint: "Paint the trimap unknown band over hair / fur / glass — the matter resolves it into soft alpha." },
  { id: "pen", label: "Pen", status: "planned", kind: "path", hint: "Phase 3 — bezier path, rasterised + boolean-combined." },
  { id: "lasso", label: "Lasso", status: "planned", kind: "path", hint: "Phase 3 — freehand path selection." },
] as const;

export const READY_TOOLS = MASK_TOOLS.filter((t) => t.status === "ready");
export const PLANNED_TOOLS = MASK_TOOLS.filter((t) => t.status === "planned");

export function maskTool(id: string): MaskTool | undefined {
  return MASK_TOOLS.find((t) => t.id === id);
}

/** First selectable (ready) tool — the modal's default. */
export const DEFAULT_TOOL_ID = READY_TOOLS[0]?.id ?? "brush";
