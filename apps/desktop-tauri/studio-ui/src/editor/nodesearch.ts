import type { Node } from "@xyflow/react";

import type { HgripeNodeData } from "./HgripeNode";
import { nodeSpec } from "../graph/nodeSpecs";
import { NODE_ZH } from "../graph/nodeSpecsI18n";
import type { Lang } from "../i18n";

export interface NodeMatch {
  id: string;
  /** Human title from the node spec (falls back to the kind). */
  title: string;
  kind: string;
}

/** English title shown for a node in search results (falls back to the kind). */
function nodeTitle(kind: string): string {
  try {
    return nodeSpec(kind).title;
  } catch {
    return kind;
  }
}

/**
 * Case-insensitive substring search over the graph's nodes, matching against
 * the node id, its kind, and its spec title (in both languages so results are
 * found regardless of the active UI language). Returns newest matches capped at
 * `limit`. An empty/blank query returns no results.
 */
export function searchNodes(nodes: Node[], query: string, lang: Lang = "en", limit = 20): NodeMatch[] {
  const q = query.trim().toLowerCase();
  if (!q) return [];
  const out: NodeMatch[] = [];
  for (const n of nodes) {
    if (n.type === "group") continue;
    const kind = (n.data as HgripeNodeData).kind;
    const enTitle = nodeTitle(kind);
    const zhTitle = NODE_ZH[kind]?.title ?? enTitle;
    const title = lang === "zh" ? zhTitle : enTitle;
    if (`${n.id} ${kind} ${enTitle} ${zhTitle}`.toLowerCase().includes(q)) {
      out.push({ id: n.id, title, kind });
      if (out.length >= limit) break;
    }
  }
  return out;
}
