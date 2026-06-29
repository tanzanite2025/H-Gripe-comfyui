// Pure structural diff between two workflow graphs, used to compare a saved
// snapshot against the current graph. Renderer-agnostic and side-effect free so
// it can be unit tested without React / localStorage.

import type { GraphEdge, WorkflowGraph } from "../graph/model";

export interface NodeRef {
  id: string;
  kind: string;
}

export interface NodeChange extends NodeRef {
  /** Param keys whose value differs between the two graphs (sorted). */
  params: string[];
  /** Set when the node kind itself changed. */
  kindChanged?: boolean;
}

export interface GraphDiff {
  addedNodes: NodeRef[];
  removedNodes: NodeRef[];
  changedNodes: NodeChange[];
  addedEdges: string[];
  removedEdges: string[];
}

/** Stable, position-independent description of an edge. */
function edgeKey(e: GraphEdge): string {
  return `${e.source}:${e.sourcePort} → ${e.target}:${e.targetPort}`;
}

/** Param keys whose JSON-serialised value differs between two param maps. */
function changedParamKeys(
  a: Record<string, unknown>,
  b: Record<string, unknown>,
): string[] {
  const keys = new Set([...Object.keys(a), ...Object.keys(b)]);
  const out: string[] = [];
  for (const k of keys) {
    if (JSON.stringify(a[k]) !== JSON.stringify(b[k])) out.push(k);
  }
  return out.sort();
}

/**
 * Diff `base` (e.g. a snapshot) against `curr` (e.g. the live graph). Nodes are
 * matched by id; edges by their source/target endpoints (ignoring edge ids,
 * which are regenerated on restore).
 */
export function diffGraphs(base: WorkflowGraph, curr: WorkflowGraph): GraphDiff {
  const baseNodes = new Map(base.nodes.map((n) => [n.id, n]));
  const currNodes = new Map(curr.nodes.map((n) => [n.id, n]));

  const addedNodes: NodeRef[] = [];
  const removedNodes: NodeRef[] = [];
  const changedNodes: NodeChange[] = [];

  for (const [id, n] of currNodes) {
    if (!baseNodes.has(id)) addedNodes.push({ id, kind: n.kind });
  }
  for (const [id, n] of baseNodes) {
    if (!currNodes.has(id)) removedNodes.push({ id, kind: n.kind });
  }
  for (const [id, cn] of currNodes) {
    const bn = baseNodes.get(id);
    if (!bn) continue;
    const params = changedParamKeys(bn.params, cn.params);
    const kindChanged = bn.kind !== cn.kind;
    if (kindChanged || params.length > 0) {
      changedNodes.push({ id, kind: cn.kind, params, ...(kindChanged ? { kindChanged } : {}) });
    }
  }

  const baseEdges = new Set(base.edges.map(edgeKey));
  const currEdges = new Set(curr.edges.map(edgeKey));
  const addedEdges = [...currEdges].filter((k) => !baseEdges.has(k));
  const removedEdges = [...baseEdges].filter((k) => !currEdges.has(k));

  return { addedNodes, removedNodes, changedNodes, addedEdges, removedEdges };
}

/** True when the two graphs are structurally identical. */
export function isEmptyDiff(d: GraphDiff): boolean {
  return (
    d.addedNodes.length === 0 &&
    d.removedNodes.length === 0 &&
    d.changedNodes.length === 0 &&
    d.addedEdges.length === 0 &&
    d.removedEdges.length === 0
  );
}
