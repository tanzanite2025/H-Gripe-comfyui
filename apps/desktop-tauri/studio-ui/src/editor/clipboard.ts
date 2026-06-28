// Copy/paste helpers for the node graph. Pure functions so they are
// unit-testable without a renderer.

import type { Edge, Node } from "@xyflow/react";
import type { HgripeNodeData } from "./HgripeNode";

export interface Clip {
  nodes: Node[];
  edges: Edge[];
}

/**
 * Build a clip from a selection: the selected nodes plus only the edges whose
 * both endpoints are in the selection (so pasted nodes keep their internal
 * wiring but drop dangling connections to nodes left behind).
 */
export function clipFromSelection(nodes: Node[], edges: Edge[]): Clip {
  const selected = nodes.filter((n) => n.selected);
  const ids = new Set(selected.map((n) => n.id));
  const internal = edges.filter((e) => ids.has(e.source) && ids.has(e.target));
  return { nodes: selected, edges: internal };
}

/**
 * Produce pasteable nodes/edges from a clip: fresh ids, offset positions,
 * internal edges remapped to the new ids, run state reset, everything marked
 * selected (callers should deselect the originals).
 */
export function buildPaste(
  clip: Clip,
  offset: { x: number; y: number },
  newId: (kind: string) => string,
): Clip {
  const idMap = new Map<string, string>();
  const nodes = clip.nodes.map((n) => {
    const d = n.data as HgripeNodeData;
    const id = newId(d.kind);
    idMap.set(n.id, id);
    return {
      ...n,
      id,
      position: { x: n.position.x + offset.x, y: n.position.y + offset.y },
      selected: true,
      // Clone params so editing the paste never mutates the source node, and
      // drop transient run state / thumbnails.
      data: { kind: d.kind, params: { ...d.params }, status: "idle" } as HgripeNodeData,
    } as Node;
  });
  const edges = clip.edges
    .filter((e) => idMap.has(e.source) && idMap.has(e.target))
    .map((e, i) => ({
      ...e,
      id: `e-${idMap.get(e.source)}-${idMap.get(e.target)}-${i}`,
      source: idMap.get(e.source) as string,
      target: idMap.get(e.target) as string,
      selected: true,
    }));
  return { nodes, edges };
}
