// Renderer-agnostic undo/redo stack for the node graph. Kept as a plain
// factory (no React) so the logic is unit-testable on its own; `useHistory`
// wraps it for the editor.

import type { Edge, Node } from "@xyflow/react";

export interface GraphSnapshot {
  nodes: Node[];
  edges: Edge[];
}

export interface HistoryStack {
  /** Record `current` as a restore point. Clears the redo stack. */
  push(current: GraphSnapshot): void;
  /** Move back one step: returns the state to restore (and stashes `current`
   *  for redo), or `null` when there is nothing to undo. */
  undo(current: GraphSnapshot): GraphSnapshot | null;
  /** Move forward one step, or `null` when there is nothing to redo. */
  redo(current: GraphSnapshot): GraphSnapshot | null;
  canUndo(): boolean;
  canRedo(): boolean;
  /** Drop all history (e.g. after a load/clear that should not be undoable). */
  clear(): void;
}

export function createHistoryStack(limit = 100): HistoryStack {
  let past: GraphSnapshot[] = [];
  let future: GraphSnapshot[] = [];

  return {
    push(current) {
      past.push(current);
      if (past.length > limit) past.shift();
      future = [];
    },
    undo(current) {
      const prev = past.pop();
      if (!prev) return null;
      future.push(current);
      return prev;
    },
    redo(current) {
      const next = future.pop();
      if (!next) return null;
      past.push(current);
      return next;
    },
    canUndo() {
      return past.length > 0;
    },
    canRedo() {
      return future.length > 0;
    },
    clear() {
      past = [];
      future = [];
    },
  };
}
