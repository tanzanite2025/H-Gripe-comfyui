import { useCallback, useMemo } from "react";
import {
  ReactFlow,
  Background,
  Controls,
  MiniMap,
  addEdge,
  useReactFlow,
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
import { GroupNode } from "./GroupNode";
import { HelperLines } from "./HelperLines";
import { DND_NODE_KIND } from "./Palette";
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
  /** Create a node of `kind` at a flow-space position. */
  onAddNode: (kind: string, position: { x: number; y: number }) => void;
  /** Called right before a new edge is created, so the host can snapshot. */
  onBeforeConnect?: () => void;
  /** Called after a node finishes dragging, so the host can (re)assign groups. */
  onNodeDragStop?: (node: Node) => void;
  /** Snap node positions to a grid while dragging. */
  snapToGrid?: boolean;
  /** Alignment guide lines (flow-space coords) to draw, if any. */
  helperLines?: { horizontal?: number; vertical?: number };
}

const SNAP_GRID: [number, number] = [16, 16];

export function FlowCanvas({
  nodes,
  edges,
  onNodesChange,
  onEdgesChange,
  setEdges,
  onSelect,
  onAddNode,
  onBeforeConnect,
  onNodeDragStop,
  snapToGrid = false,
  helperLines,
}: FlowCanvasProps) {
  // Declared once so React does not re-create the map each render.
  const nodeTypes = useMemo(() => ({ hgripe: HgripeNode, group: GroupNode }), []);
  const { screenToFlowPosition } = useReactFlow();

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
    (params) => {
      onBeforeConnect?.();
      setEdges((eds) => addEdge(params, eds));
    },
    [setEdges, onBeforeConnect],
  );

  const onDragOver = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    e.dataTransfer.dropEffect = "move";
  }, []);

  const onDrop = useCallback(
    (e: React.DragEvent) => {
      e.preventDefault();
      const kind = e.dataTransfer.getData(DND_NODE_KIND);
      if (!kind) return;
      const position = screenToFlowPosition({ x: e.clientX, y: e.clientY });
      onAddNode(kind, position);
    },
    [screenToFlowPosition, onAddNode],
  );

  return (
    <ReactFlow
      nodes={nodes}
      edges={edges}
      nodeTypes={nodeTypes}
      onNodesChange={onNodesChange}
      onEdgesChange={onEdgesChange}
      onConnect={onConnect}
      onNodeDragStop={(_, node) => onNodeDragStop?.(node)}
      snapToGrid={snapToGrid}
      snapGrid={SNAP_GRID}
      isValidConnection={isValidConnection}
      onSelectionChange={({ nodes: sel }) => onSelect(sel[0]?.id ?? null)}
      onDragOver={onDragOver}
      onDrop={onDrop}
      onlyRenderVisibleElements
      deleteKeyCode={["Backspace", "Delete"]}
      fitView
    >
      <Background />
      <MiniMap pannable zoomable />
      <Controls />
      <HelperLines horizontal={helperLines?.horizontal} vertical={helperLines?.vertical} />
    </ReactFlow>
  );
}
