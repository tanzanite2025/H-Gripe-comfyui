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

export interface PsdTemplateRef {
  /** The psdTemplate node's id. */
  node: string;
  /** The configured template path (may be empty / syntactically invalid). */
  path: string;
}

/**
 * Collect every `psdTemplate` node with a non-empty `.psd` path. These are the
 * paths worth confirming actually exist on disk via the backend before a run.
 */
export function psdTemplatePaths(graph: WorkflowGraph): PsdTemplateRef[] {
  const refs: PsdTemplateRef[] = [];
  for (const n of graph.nodes) {
    if (n.kind !== "psdTemplate") continue;
    const path = String(n.params.path ?? "").trim();
    if (path) refs.push({ node: n.id, path });
  }
  return refs;
}

export interface PsdExportTarget {
  /** The psdExport node's id. */
  node: string;
  /** Path of the psdTemplate feeding this export's `template` input, if any. */
  templatePath: string | null;
  /** The placeholder layer name configured on the export (may be empty). */
  placeholder: string;
}

/**
 * For each `psdExport` node, resolve the template path it will actually use --
 * by walking back along its `template` input edge to the connected
 * `psdTemplate` node's path -- together with its configured placeholder layer
 * name. Pure graph walk so it is unit-testable without the backend; the
 * caller can then confirm the placeholder name exists inside that PSD.
 */
export function psdExportTargets(graph: WorkflowGraph): PsdExportTarget[] {
  const nodeById = new Map(graph.nodes.map((n) => [n.id, n]));
  // Source node id feeding each node's `template` input port.
  const templateSource = new Map<string, string>();
  for (const e of graph.edges) {
    if (e.targetPort === "template") templateSource.set(e.target, e.source);
  }

  const targets: PsdExportTarget[] = [];
  for (const n of graph.nodes) {
    if (n.kind !== "psdExport") continue;
    const placeholder = String(n.params.placeholder ?? "").trim();
    const sourceId = templateSource.get(n.id);
    const source = sourceId ? nodeById.get(sourceId) : undefined;
    const templatePath =
      source && source.kind === "psdTemplate"
        ? String(source.params.path ?? "").trim() || null
        : null;
    targets.push({ node: n.id, templatePath, placeholder });
  }
  return targets;
}
