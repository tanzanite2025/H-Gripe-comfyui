import type { Node } from "@xyflow/react";

import type { HgripeNodeData } from "./HgripeNode";
import { nodeSpec } from "../graph/nodeSpecs";

export interface NodeMatch {
  id: string;
  /** Human title from the node spec (falls back to the kind). */
  title: string;
  kind: string;
}

/** Title shown for a node in search results. */
function nodeTitle(kind: string): string {
  try {
    return nodeSpec(kind).title;
  } catch {
    return kind;
  }
}

/**
 * Case-insensitive substring search over the graph's nodes, matching against
 * the node id, its kind, and its spec title. Returns newest matches capped at
 * `limit`. An empty/blank query returns no results.
 */
export function searchNodes(nodes: Node[], query: string, limit = 20): NodeMatch[] {
  const q = query.trim().toLowerCase();
  if (!q) return [];
  const out: NodeMatch[] = [];
  for (const n of nodes) {
    if (n.type === "group") continue;
    const kind = (n.data as HgripeNodeData).kind;
    const title = nodeTitle(kind);
    if (`${n.id} ${kind} ${title}`.toLowerCase().includes(q)) {
      out.push({ id: n.id, title, kind });
      if (out.length >= limit) break;
    }
  }
  return out;
}
