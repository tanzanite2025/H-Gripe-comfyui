// Thin React wrapper around the pure `HistoryStack`. Exposes stable
// callbacks plus `canUndo`/`canRedo` flags that re-render the toolbar.

import { useCallback, useReducer, useRef } from "react";
import type { Edge, Node } from "@xyflow/react";
import { createHistoryStack, type GraphSnapshot } from "./history";

interface UseHistoryArgs {
  nodes: Node[];
  edges: Edge[];
  setNodes: (nodes: Node[]) => void;
  setEdges: (edges: Edge[]) => void;
  limit?: number;
}

export interface History {
  /** Capture the current graph as a restore point. Call *before* mutating. */
  takeSnapshot: () => void;
  undo: () => void;
  redo: () => void;
  canUndo: boolean;
  canRedo: boolean;
}

export function useHistory({ nodes, edges, setNodes, setEdges, limit }: UseHistoryArgs): History {
  const stack = useRef(createHistoryStack(limit));
  // Latest graph in a ref so callbacks stay stable but read fresh state.
  const latest = useRef<GraphSnapshot>({ nodes, edges });
  latest.current = { nodes, edges };
  const [, force] = useReducer((x: number) => x + 1, 0);

  const current = (): GraphSnapshot => ({
    nodes: [...latest.current.nodes],
    edges: [...latest.current.edges],
  });

  const takeSnapshot = useCallback(() => {
    stack.current.push(current());
    force();
  }, []);

  const undo = useCallback(() => {
    const prev = stack.current.undo(current());
    if (prev) {
      setNodes(prev.nodes);
      setEdges(prev.edges);
    }
    force();
  }, [setNodes, setEdges]);

  const redo = useCallback(() => {
    const next = stack.current.redo(current());
    if (next) {
      setNodes(next.nodes);
      setEdges(next.edges);
    }
    force();
  }, [setNodes, setEdges]);

  return {
    takeSnapshot,
    undo,
    redo,
    canUndo: stack.current.canUndo(),
    canRedo: stack.current.canRedo(),
  };
}
