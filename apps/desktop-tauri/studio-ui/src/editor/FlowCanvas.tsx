import { useCallback, useMemo } from "react";
import {
  ReactFlow,
  Background,
  Controls,
  MiniMap,
  addEdge,
  type Connection,
  type Edge,
  type Node,
  type OnConnect,
  type OnNodesChange,
  type OnEdgesChange,
  type IsValidConnection,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";

import { HgripeNode } from "./HgripeNode";
import { nodeSpec } from "../graph/nodeSpecs";
import { arePortsCompatible } from "../graph/model";
import { toWorkflowGraph } from "./adapter";
import { wouldCreateCycle } from "../runtime/dag";

interface FlowCanvasProps {
  nodes: Node[];
  edges: Edge[];
  onNodesChange: OnNodesChange;
  onEdgesChange: OnEdgesChange;
  setEdges: React.Dispatch<React.SetStateAction<Edge[]>>;
  onSelect: (nodeId: string | null) => void;
}

export function FlowCanvas({
  nodes,
  edges,
  onNodesChange,
  onEdgesChange,
  setEdges,
  onSelect,
}: FlowCanvasProps) {
  // Declared once so React does not re-create the map each render.
  const nodeTypes = useMemo(() => ({ hgripe: HgripeNode }), []);

  const portType = useCallback(
    (nodeId: string | null, handleId: string | null | undefined, dir: "in" | "out") => {
      const node = nodes.find((n) => n.id === nodeId);
      if (!node) return undefined;
      const spec = nodeSpec((node.data as { kind: string }).kind);
      const ports = dir === "in" ? spec.inputs : spec.outputs;
      return ports.find((p) => p.id === handleId)?.type;
    },
    [nodes],
  );

  // Typed-port + acyclic connection validation.
  const isValidConnection: IsValidConnection = useCallback(
    (c: Connection | Edge) => {
      const sourceType = portType(c.source, c.sourceHandle, "out");
      const targetType = portType(c.target, c.targetHandle, "in");
      if (!sourceType || !targetType) return false;
      if (!arePortsCompatible(sourceType, targetType)) return false;
      if (c.source && c.target && wouldCreateCycle(toWorkflowGraph(nodes, edges), c.source, c.target)) {
        return false;
      }
      return true;
    },
    [nodes, edges, portType],
  );

  const onConnect: OnConnect = useCallback(
    (params) => setEdges((eds) => addEdge(params, eds)),
    [setEdges],
  );

  return (
    <ReactFlow
      nodes={nodes}
      edges={edges}
      nodeTypes={nodeTypes}
      onNodesChange={onNodesChange}
      onEdgesChange={onEdgesChange}
      onConnect={onConnect}
      isValidConnection={isValidConnection}
      onSelectionChange={({ nodes: sel }) => onSelect(sel[0]?.id ?? null)}
      onlyRenderVisibleElements
      fitView
    >
      <Background />
      <MiniMap pannable zoomable />
      <Controls />
    </ReactFlow>
  );
}
