// Adapter between the React Flow render state and the renderer-agnostic
// WorkflowGraph model. Keeping this conversion in one place is what lets us
// swap renderers later without touching the runtime or serialization.

import type { Edge, Node } from "@xyflow/react";
import type { GraphEdge, GraphNode, WorkflowGraph } from "../graph/model";
import { GRAPH_VERSION } from "../graph/model";
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
