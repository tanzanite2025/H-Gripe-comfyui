import type { WorkflowGraph } from "../graph/model";

export interface PsdWarning {
  node: string;
  message: string;
}

/**
 * Validate a PSD template path. Returns a human warning string, or null when
 * the path looks usable (non-empty and ending in `.psd`).
 */
export function psdTemplatePathWarning(path: string): string | null {
  const p = path.trim();
  if (!p) return "no template path set";
  if (!p.toLowerCase().endsWith(".psd")) return "path is not a .psd file";
  return null;
}

/**
 * Pre-run sanity checks for the PSD node chain. Surfaces problems the executor
 * would otherwise only report mid-run: PSD Template nodes with a missing/invalid
 * path, and PSD Export nodes missing their image / template input connections.
 */
export function validatePsdChain(graph: WorkflowGraph): PsdWarning[] {
  const warnings: PsdWarning[] = [];

  // Map each node to the set of input ports that have an incoming edge.
  const connected = new Map<string, Set<string>>();
  for (const e of graph.edges) {
    const set = connected.get(e.target) ?? new Set<string>();
    set.add(e.targetPort);
    connected.set(e.target, set);
  }

  for (const n of graph.nodes) {
    if (n.kind === "psdTemplate") {
      const warn = psdTemplatePathWarning(String(n.params.path ?? ""));
      if (warn) warnings.push({ node: n.id, message: `PSD Template: ${warn}` });
    } else if (n.kind === "psdExport") {
      const ports = connected.get(n.id) ?? new Set<string>();
      if (!ports.has("image")) warnings.push({ node: n.id, message: "PSD Export: no image connected" });
      if (!ports.has("template")) {
        warnings.push({ node: n.id, message: "PSD Export: no template connected" });
      }
    }
  }
  return warnings;
}
