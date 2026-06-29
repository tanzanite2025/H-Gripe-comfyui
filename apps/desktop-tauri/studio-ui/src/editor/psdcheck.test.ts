import { describe, expect, it } from "vitest";

import { GRAPH_VERSION, type WorkflowGraph } from "../graph/model";
import {
  psdExportTargets,
  psdTemplatePathWarning,
  psdTemplatePaths,
  validatePsdChain,
} from "./psdcheck";

const node = (id: string, kind: string, params: Record<string, unknown> = {}) => ({
  id,
  kind,
  position: { x: 0, y: 0 },
  params,
});
const edge = (id: string, source: string, sourcePort: string, target: string, targetPort: string) => ({
  id,
  source,
  sourcePort,
  target,
  targetPort,
});

const graph = (nodes: WorkflowGraph["nodes"], edges: WorkflowGraph["edges"]): WorkflowGraph => ({
  version: GRAPH_VERSION,
  nodes,
  edges,
});

describe("psdTemplatePathWarning", () => {
  it("warns on an empty path", () => {
    expect(psdTemplatePathWarning("   ")).toBe("no template path set");
  });
  it("warns when the extension is not .psd", () => {
    expect(psdTemplatePathWarning("/x/template.png")).toBe("path is not a .psd file");
  });
  it("accepts a .psd path (case-insensitive)", () => {
    expect(psdTemplatePathWarning("/x/Template.PSD")).toBeNull();
  });
});

describe("validatePsdChain", () => {
  it("flags a template node with a bad path", () => {
    const w = validatePsdChain(graph([node("t", "psdTemplate", { path: "" })], []));
    expect(w).toEqual([{ node: "t", message: "PSD Template: no template path set" }]);
  });

  it("flags a psdExport missing both inputs", () => {
    const w = validatePsdChain(graph([node("e", "psdExport")], []));
    expect(w.map((x) => x.message)).toEqual([
      "PSD Export: no image connected",
      "PSD Export: no template connected",
    ]);
  });

  it("passes a fully connected chain", () => {
    const nodes = [
      node("t", "psdTemplate", { path: "/x/a.psd" }),
      node("g", "generate"),
      node("e", "psdExport"),
    ];
    const edges = [
      edge("e1", "g", "image", "e", "image"),
      edge("e2", "t", "template", "e", "template"),
    ];
    expect(validatePsdChain(graph(nodes, edges))).toEqual([]);
  });
});

describe("psdTemplatePaths", () => {
  it("collects only psdTemplate nodes with a non-empty path", () => {
    const nodes = [
      node("t1", "psdTemplate", { path: "/x/a.psd" }),
      node("t2", "psdTemplate", { path: "  " }),
      node("g", "generate", { path: "/x/ignored.psd" }),
    ];
    expect(psdTemplatePaths(graph(nodes, []))).toEqual([{ node: "t1", path: "/x/a.psd" }]);
  });
});

describe("psdExportTargets", () => {
  it("resolves the connected template path and placeholder for each export", () => {
    const nodes = [
      node("t", "psdTemplate", { path: "/x/a.psd" }),
      node("e", "psdExport", { placeholder: "main" }),
    ];
    const edges = [edge("e2", "t", "template", "e", "template")];
    expect(psdExportTargets(graph(nodes, edges))).toEqual([
      { node: "e", templatePath: "/x/a.psd", placeholder: "main" },
    ]);
  });

  it("reports a null template path when nothing feeds the template input", () => {
    const nodes = [node("e", "psdExport", { placeholder: "main" })];
    expect(psdExportTargets(graph(nodes, []))).toEqual([
      { node: "e", templatePath: null, placeholder: "main" },
    ]);
  });

  it("ignores a template input not fed by a psdTemplate node", () => {
    const nodes = [node("g", "generate"), node("e", "psdExport", { placeholder: "main" })];
    const edges = [edge("e1", "g", "image", "e", "template")];
    expect(psdExportTargets(graph(nodes, edges))).toEqual([
      { node: "e", templatePath: null, placeholder: "main" },
    ]);
  });
});
