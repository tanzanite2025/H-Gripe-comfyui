// Adapter between the React Flow render state and the renderer-agnostic
// WorkflowGraph model. Keeping this conversion in one place is what lets us
// swap renderers later without touching the runtime or serialization.

import type { Edge, Node } from "@xyflow/react";
import type { GraphEdge, GraphNode, WorkflowGraph } from "../graph/model";
import { GRAPH_VERSION } from "../graph/model";
import { defaultParams } from "../graph/nodeSpecs";
import { GROUP_KIND, DEFAULT_GROUP_WIDTH, DEFAULT_GROUP_HEIGHT, orderNodes } from "./grouping";
import type { HgripeNodeData } from "./HgripeNode";

export function toWorkflowGraph(nodes: Node[], edges: Edge[]): WorkflowGraph {
  const graphNodes: GraphNode[] = nodes.map((n) => {
    const data = n.data as HgripeNodeData;
    const node: GraphNode = {
      id: n.id,
      kind: data.kind,
      position: { x: n.position.x, y: n.position.y },
      params: data.params ?? {},
    };
    if (n.parentId) node.parentId = n.parentId;
    // Persist the frame size for group containers (regular nodes auto-size).
    if (data.kind === GROUP_KIND) {
      node.width = n.width ?? n.measured?.width ?? DEFAULT_GROUP_WIDTH;
      node.height = n.height ?? n.measured?.height ?? DEFAULT_GROUP_HEIGHT;
    }
    return node;
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
    if (n.kind === GROUP_KIND) {
      const node: Node = {
        id: n.id,
        type: "group",
        position: { ...n.position },
        width: n.width ?? DEFAULT_GROUP_WIDTH,
        height: n.height ?? DEFAULT_GROUP_HEIGHT,
        data: { kind: GROUP_KIND, params: { label: n.params?.label ?? "Group" }, status: "idle" },
      };
      if (n.parentId) node.parentId = n.parentId;
      return node;
    }
    const data: HgripeNodeData = {
      kind: n.kind,
      params: { ...defaultParams(n.kind), ...(n.params ?? {}) },
      status: "idle",
    };
    const node: Node = { id: n.id, type: "hgripe", position: { ...n.position }, data };
    if (n.parentId) node.parentId = n.parentId;
    return node;
  });

  const edges: Edge[] = graph.edges.map((e) => ({
    id: e.id,
    source: e.source,
    sourceHandle: e.sourcePort || null,
    target: e.target,
    targetHandle: e.targetPort || null,
  }));

  // Group frames must precede their children for React Flow.
  return { nodes: orderNodes(nodes), edges };
}
