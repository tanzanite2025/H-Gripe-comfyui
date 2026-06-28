// Renderer-agnostic graph data model.
//
// This is the durable asset: it does NOT depend on React Flow (or any
// renderer). The editor renders from this model and writes back to it, and the
// DAG runtime executes it. Swapping renderers later (e.g. tldraw) only requires
// a new adapter, not a data migration.

/** Typed port data kinds. `any` is compatible with every other type. */
export type PortDataType =
  | "image"
  | "text"
  | "model"
  | "number"
  | "latent"
  | "any";

export interface PortSpec {
  id: string;
  label: string;
  type: PortDataType;
}

/**
 * A reference to a media file. The node UI shows a backend-generated
 * thumbnail; the original `path` is always the source of truth for
 * execution/export so previews never degrade quality.
 */
export interface MediaRef {
  /** Absolute path to the original file (never read from the on-screen thumb). */
  path: string;
  /** Backend-generated thumbnail path (sized for display). */
  thumbnailPath?: string;
  width?: number;
  height?: number;
  /** Content hash, used to key the thumbnail cache. */
  hash?: string;
  mime?: string;
}

export interface GraphNode {
  id: string;
  kind: string;
  position: { x: number; y: number };
  /** Scalar form values (prompt text, model name, numbers, …). */
  params: Record<string, unknown>;
  /** Media references keyed by port/field id. */
  media?: Record<string, MediaRef>;
}

export interface GraphEdge {
  id: string;
  source: string;
  sourcePort: string;
  target: string;
  targetPort: string;
}

export const GRAPH_VERSION = 1 as const;

export interface WorkflowGraph {
  version: typeof GRAPH_VERSION;
  nodes: GraphNode[];
  edges: GraphEdge[];
}

export function emptyGraph(): WorkflowGraph {
  return { version: GRAPH_VERSION, nodes: [], edges: [] };
}

/** `any` matches anything; otherwise the port types must be equal. */
export function arePortsCompatible(
  source: PortDataType,
  target: PortDataType,
): boolean {
  return source === target || source === "any" || target === "any";
}

export function serializeGraph(graph: WorkflowGraph): string {
  return JSON.stringify(graph, null, 2);
}

export function deserializeGraph(raw: string): WorkflowGraph {
  const parsed = JSON.parse(raw) as Partial<WorkflowGraph>;
  if (parsed.version !== GRAPH_VERSION) {
    throw new Error(
      `unsupported graph version: ${String(parsed.version)} (expected ${GRAPH_VERSION})`,
    );
  }
  if (!Array.isArray(parsed.nodes) || !Array.isArray(parsed.edges)) {
    throw new Error("invalid graph: nodes and edges must be arrays");
  }
  return { version: GRAPH_VERSION, nodes: parsed.nodes, edges: parsed.edges };
}
