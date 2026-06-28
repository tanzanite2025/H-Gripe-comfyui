// DAG runtime skeleton.
//
// Renderer-agnostic: it consumes a WorkflowGraph and a registry of executors
// keyed by node kind. It resolves dependencies, runs independent branches in
// parallel (level by level), threads each node's outputs to downstream inputs,
// and caches results by a stable signature so unchanged nodes are not re-run.
//
// It can run headless (no UI), which is the whole point of keeping it separate
// from the renderer — the same graph could later run from a CRON job or webhook.

import { arePortsCompatible, type GraphEdge, type WorkflowGraph } from "../graph/model";
import { nodeSpec } from "../graph/nodeSpecs";

export type NodeStatus = "idle" | "queued" | "running" | "succeeded" | "failed" | "cached";

export interface NodeExecutionContext {
  nodeId: string;
  kind: string;
  params: Record<string, unknown>;
  /** Upstream values keyed by this node's input port id. */
  inputs: Record<string, unknown>;
}

/** Returns output values keyed by output port id. */
export type NodeExecutor = (
  ctx: NodeExecutionContext,
) => Promise<Record<string, unknown>>;

export type ExecutorRegistry = Record<string, NodeExecutor>;

export interface RunObserver {
  onStatus?: (nodeId: string, status: NodeStatus) => void;
}

export interface ValidationIssue {
  severity: "error" | "warning";
  code: string;
  message: string;
  edgeId?: string;
  nodeId?: string;
}

interface Adjacency {
  /** node id -> downstream node ids */
  out: Map<string, Set<string>>;
  /** node id -> upstream edge list */
  incoming: Map<string, GraphEdge[]>;
  indegree: Map<string, number>;
}

function buildAdjacency(graph: WorkflowGraph): Adjacency {
  const out = new Map<string, Set<string>>();
  const incoming = new Map<string, GraphEdge[]>();
  const indegree = new Map<string, number>();

  for (const node of graph.nodes) {
    out.set(node.id, new Set());
    incoming.set(node.id, []);
    indegree.set(node.id, 0);
  }
  for (const edge of graph.edges) {
    if (!out.has(edge.source) || !indegree.has(edge.target)) continue;
    const downstream = out.get(edge.source)!;
    if (!downstream.has(edge.target)) {
      downstream.add(edge.target);
      indegree.set(edge.target, (indegree.get(edge.target) ?? 0) + 1);
    }
    incoming.get(edge.target)!.push(edge);
  }
  return { out, incoming, indegree };
}

/**
 * Kahn's algorithm, grouped into levels. Each level is a set of nodes with no
 * remaining unsatisfied dependencies and can be executed in parallel.
 * Throws if the graph contains a cycle.
 */
export function topoLevels(graph: WorkflowGraph): string[][] {
  const { out, indegree } = buildAdjacency(graph);
  const degree = new Map(indegree);
  let frontier = graph.nodes.filter((n) => (degree.get(n.id) ?? 0) === 0).map((n) => n.id);
  const levels: string[][] = [];
  let visited = 0;

  while (frontier.length > 0) {
    levels.push(frontier);
    visited += frontier.length;
    const next: string[] = [];
    for (const id of frontier) {
      for (const down of out.get(id) ?? []) {
        const d = (degree.get(down) ?? 0) - 1;
        degree.set(down, d);
        if (d === 0) next.push(down);
      }
    }
    frontier = next;
  }

  if (visited !== graph.nodes.length) {
    throw new Error("graph contains a cycle");
  }
  return levels;
}

/** Would adding source->target create a cycle? */
export function wouldCreateCycle(
  graph: WorkflowGraph,
  source: string,
  target: string,
): boolean {
  if (source === target) return true;
  const { out } = buildAdjacency(graph);
  // DFS from target; if we reach source, the new edge closes a loop.
  const stack = [target];
  const seen = new Set<string>();
  while (stack.length) {
    const cur = stack.pop()!;
    if (cur === source) return true;
    if (seen.has(cur)) continue;
    seen.add(cur);
    for (const down of out.get(cur) ?? []) stack.push(down);
  }
  return false;
}

/** Static validation: port type compatibility + cycle freedom. */
export function validateGraph(graph: WorkflowGraph): ValidationIssue[] {
  const issues: ValidationIssue[] = [];
  const byId = new Map(graph.nodes.map((n) => [n.id, n]));

  for (const edge of graph.edges) {
    const src = byId.get(edge.source);
    const dst = byId.get(edge.target);
    if (!src || !dst) {
      issues.push({ severity: "error", code: "dangling_edge", message: "edge references a missing node", edgeId: edge.id });
      continue;
    }
    const srcPort = nodeSpec(src.kind).outputs.find((p) => p.id === edge.sourcePort);
    const dstPort = nodeSpec(dst.kind).inputs.find((p) => p.id === edge.targetPort);
    if (!srcPort || !dstPort) {
      issues.push({ severity: "error", code: "unknown_port", message: "edge references an unknown port", edgeId: edge.id });
      continue;
    }
    if (!arePortsCompatible(srcPort.type, dstPort.type)) {
      issues.push({
        severity: "error",
        code: "type_mismatch",
        message: `cannot connect ${srcPort.type} -> ${dstPort.type}`,
        edgeId: edge.id,
      });
    }
  }

  try {
    topoLevels(graph);
  } catch {
    issues.push({ severity: "error", code: "cycle", message: "graph contains a cycle" });
  }
  return issues;
}

function signature(
  kind: string,
  params: Record<string, unknown>,
  inputs: Record<string, unknown>,
): string {
  return JSON.stringify({ kind, params, inputs });
}

export interface RunResult {
  outputs: Map<string, Record<string, unknown>>;
  statuses: Map<string, NodeStatus>;
}

/**
 * Execute the graph. Independent branches in the same level run concurrently.
 * Results are memoized by signature for the duration of the run so a node fed
 * identical params+inputs is reported as `cached` rather than re-executed.
 */
export async function runGraph(
  graph: WorkflowGraph,
  registry: ExecutorRegistry,
  observer: RunObserver = {},
  /**
   * Per-node param overrides merged on top of `node.params` for this run only
   * (used by batch fan-out to sweep one node across a list without mutating the
   * graph). Overrides are part of the cache signature, so each sweep value is a
   * distinct cache entry.
   */
  paramOverrides: Map<string, Record<string, unknown>> = new Map(),
): Promise<RunResult> {
  const issues = validateGraph(graph);
  const firstError = issues.find((i) => i.severity === "error");
  if (firstError) throw new Error(`invalid graph: ${firstError.message}`);

  const { incoming } = buildAdjacency(graph);
  const byId = new Map(graph.nodes.map((n) => [n.id, n]));
  const outputs = new Map<string, Record<string, unknown>>();
  const statuses = new Map<string, NodeStatus>();
  const cache = new Map<string, Record<string, unknown>>();

  const setStatus = (id: string, s: NodeStatus) => {
    statuses.set(id, s);
    observer.onStatus?.(id, s);
  };
  for (const n of graph.nodes) setStatus(n.id, "queued");

  for (const level of topoLevels(graph)) {
    await Promise.all(
      level.map(async (id) => {
        const node = byId.get(id)!;
        const inputs: Record<string, unknown> = {};
        for (const edge of incoming.get(id) ?? []) {
          const upstream = outputs.get(edge.source);
          if (upstream && edge.sourcePort in upstream) {
            inputs[edge.targetPort] = upstream[edge.sourcePort];
          }
        }

        const override = paramOverrides.get(id);
        const params = override ? { ...node.params, ...override } : node.params;
        const sig = signature(node.kind, params, inputs);
        const cached = cache.get(sig);
        if (cached) {
          outputs.set(id, cached);
          setStatus(id, "cached");
          return;
        }

        const executor = registry[node.kind];
        if (!executor) {
          setStatus(id, "failed");
          throw new Error(`no executor registered for node kind: ${node.kind}`);
        }

        setStatus(id, "running");
        try {
          const result = await executor({ nodeId: id, kind: node.kind, params, inputs });
          outputs.set(id, result);
          cache.set(sig, result);
          setStatus(id, "succeeded");
        } catch (err) {
          setStatus(id, "failed");
          throw err;
        }
      }),
    );
  }

  return { outputs, statuses };
}
