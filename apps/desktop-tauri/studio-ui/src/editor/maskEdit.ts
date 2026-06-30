// Pure edit-state model for the Mask-Edit modal.
//
// The modal owns an `EditState` (the current `EditPaths` plus an undo/redo
// stack) and mutates it only through these pure helpers. Keeping the model
// renderer-agnostic and side-effect-free means it is unit-testable on its own
// and the React component stays a thin view. The committed `EditPaths` is what
// gets written back onto the node's `edit_paths` param; the Rust backend
// rasterises it on run (Phase 1 stores `paths`, applies `brush_strokes` +
// `operations`).

import type { BrushStroke, EditPaths, MaskOperation } from "../types/production";
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
    operations: Array.isArray(v.operations) ? v.operations : [],
    points: Array.isArray(v.points) ? v.points : [],
  };
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

export function addOperation(state: EditState, op: MaskOperation): EditState {
  return commit(state, {
    ...state.current,
    operations: [...state.current.operations, op],
  });
}

/** Append a positive SAM 2 point prompt (image-space `[x, y]`). */
export function addPoint(state: EditState, point: [number, number]): EditState {
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
    edits.operations.length === 0 &&
    edits.points.length === 0
  );
}

/** Count of applied edits, for the modal's status line. */
export function editCount(edits: EditPaths): number {
  return (
    edits.paths.length +
    edits.brush_strokes.length +
    edits.operations.length +
    edits.points.length
  );
}
