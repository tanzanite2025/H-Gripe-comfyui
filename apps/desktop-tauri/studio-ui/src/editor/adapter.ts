// Adapter between the React Flow render state and the renderer-agnostic
// WorkflowGraph model. Keeping this conversion in one place is what lets us
// swap renderers later without touching the runtime or serialization.

import type { Edge, Node } from "@xyflow/react";
import type { GraphEdge, GraphNode, WorkflowGraph } from "../graph/model";
import { GRAPH_VERSION } from "../graph/model";
import { defaultParams } from "../graph/nodeSpecs";
import type { HgripeNodeData } from "./HgripeNode";

export function toWorkflowGraph(nodes: Node[], edges: Edge[]): WorkflowGraph {
  const graphNodes: GraphNode[] = nodes.map((n) => {
    const data = n.data as HgripeNodeData;
    return {
      id: n.id,
      kind: data.kind,
      position: { x: n.position.x, y: n.position.y },
      params: data.params ?? {},
    };
  });

  const graphEdges: GraphEdge[] = edges.map((e) => ({
    id: e.id,
    source: e.source,
    sourcePort: e.sourceHandle ?? "",
    target: e.target,
    targetPort: e.targetHandle ?? "",
  }));

  return { version: GRAPH_VERSION, nodes: graphNodes, edges: graphEdges };
}

/**
 * Rebuild React Flow render state from a WorkflowGraph (load / round-trip).
 * Params are merged over the node kind's defaults so graphs saved before a new
 * param was introduced still get sensible values.
 */
export function fromWorkflowGraph(graph: WorkflowGraph): { nodes: Node[]; edges: Edge[] } {
  const nodes: Node[] = graph.nodes.map((n) => {
    const data: HgripeNodeData = {
      kind: n.kind,
      params: { ...defaultParams(n.kind), ...(n.params ?? {}) },
      status: "idle",
    };
    return { id: n.id, type: "hgripe", position: { ...n.position }, data };
  });

  const edges: Edge[] = graph.edges.map((e) => ({
    id: e.id,
    source: e.source,
    sourceHandle: e.sourcePort || null,
    target: e.target,
    targetHandle: e.targetPort || null,
  }));

  return { nodes, edges };
}
