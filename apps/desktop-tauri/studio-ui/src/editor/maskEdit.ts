// Pure edit-state model for the Mask-Edit modal.
//
// The modal owns an `EditState` (the current `EditPaths` plus an undo/redo
// stack) and mutates it only through these pure helpers. Keeping the model
// renderer-agnostic and side-effect-free means it is unit-testable on its own
// and the React component stays a thin view. The committed `EditPaths` is what
// gets written back onto the node's `edit_paths` param; the Rust backend
// rasterises it on run (Phase 1 stores `paths`, applies `brush_strokes` +
// `operations`).

import type { BrushStroke, EditPaths, MaskOperation, PointPrompt } from "../types/production";
import { emptyEditPaths } from "../types/production";

export interface EditState {
  /** The committed edits. */
  current: EditPaths;
  /** Snapshots older than `current`, most-recent last. */
  past: EditPaths[];
  /** Snapshots undone from `current`, most-recent last. */
  future: EditPaths[];
}

const MAX_HISTORY = 100;

export function initEditState(initial?: EditPaths | null): EditState {
  return { current: normalizeEditPaths(initial), past: [], future: [] };
}

/** Coerce an arbitrary stored value into a well-formed `EditPaths`. */
export function normalizeEditPaths(value: unknown): EditPaths {
  if (!value || typeof value !== "object") return emptyEditPaths();
  const v = value as Partial<EditPaths>;
  return {
    version: 1,
    paths: Array.isArray(v.paths) ? v.paths : [],
    brush_strokes: Array.isArray(v.brush_strokes) ? v.brush_strokes : [],
    matte_strokes: Array.isArray(v.matte_strokes) ? v.matte_strokes : [],
    operations: Array.isArray(v.operations) ? v.operations : [],
    points: Array.isArray(v.points) ? v.points.map(normalizePoint).filter((p): p is PointPrompt => p !== null) : [],
  };
}

/**
 * Coerce a stored point into a `PointPrompt`. Accepts the current
 * `{ x, y, label }` shape and the legacy `[x, y]` pair (read as positive), so
 * workflows saved before negative points stay loadable.
 */
function normalizePoint(value: unknown): PointPrompt | null {
  if (Array.isArray(value) && value.length >= 2) {
    const [x, y] = value;
    if (typeof x === "number" && typeof y === "number") return { x, y, label: 1 };
    return null;
  }
  if (value && typeof value === "object") {
    const v = value as { x?: unknown; y?: unknown; label?: unknown };
    if (typeof v.x === "number" && typeof v.y === "number") {
      return { x: v.x, y: v.y, label: v.label === 0 ? 0 : 1 };
    }
  }
  return null;
}

// Commit a new `current`, pushing the previous onto the undo stack and clearing
// the redo stack. The history is capped so a long editing session cannot grow
// unbounded in memory.
function commit(state: EditState, next: EditPaths): EditState {
  const past = [...state.past, state.current];
  if (past.length > MAX_HISTORY) past.shift();
  return { current: next, past, future: [] };
}

export function addBrushStroke(state: EditState, stroke: BrushStroke): EditState {
  if (stroke.points.length === 0) return state;
  return commit(state, {
    ...state.current,
    brush_strokes: [...state.current.brush_strokes, stroke],
  });
}

/** Append a trimap unknown-band stroke (resolved to soft alpha by the matter). */
export function addMatteStroke(state: EditState, stroke: BrushStroke): EditState {
  if (stroke.points.length === 0) return state;
  return commit(state, {
    ...state.current,
    matte_strokes: [...state.current.matte_strokes, stroke],
  });
}

export function addOperation(state: EditState, op: MaskOperation): EditState {
  return commit(state, {
    ...state.current,
    operations: [...state.current.operations, op],
  });
}

/**
 * Append a SAM 2 point prompt (image-space). `label` is `1` for a positive
 * (include) point and `0` for a negative (exclude) point.
 */
export function addPoint(state: EditState, point: PointPrompt): EditState {
  return commit(state, {
    ...state.current,
    points: [...state.current.points, point],
  });
}

/** Drop every edit, recording the wipe as an undoable step. */
export function clearEdits(state: EditState): EditState {
  if (isEmpty(state.current)) return state;
  return commit(state, emptyEditPaths());
}

export function undo(state: EditState): EditState {
  if (state.past.length === 0) return state;
  const past = [...state.past];
  const previous = past.pop()!;
  return { current: previous, past, future: [...state.future, state.current] };
}

export function redo(state: EditState): EditState {
  if (state.future.length === 0) return state;
  const future = [...state.future];
  const next = future.pop()!;
  return { current: next, past: [...state.past, state.current], future };
}

export const canUndo = (state: EditState): boolean => state.past.length > 0;
export const canRedo = (state: EditState): boolean => state.future.length > 0;

export function isEmpty(edits: EditPaths): boolean {
  return (
    edits.paths.length === 0 &&
    edits.brush_strokes.length === 0 &&
    edits.matte_strokes.length === 0 &&
    edits.operations.length === 0 &&
    edits.points.length === 0
  );
}

/** Count of applied edits, for the modal's status line. */
export function editCount(edits: EditPaths): number {
  return (
    edits.paths.length +
    edits.brush_strokes.length +
    edits.matte_strokes.length +
    edits.operations.length +
    edits.points.length
  );
}
