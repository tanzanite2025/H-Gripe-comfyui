import { describe, expect, it } from "vitest";
import type { Edge, Node } from "@xyflow/react";
import { buildPaste, clipFromSelection } from "./clipboard";

function node(id: string, kind: string, selected: boolean, params: Record<string, unknown> = {}): Node {
  return {
    id,
    type: "hgripe",
    position: { x: 100, y: 200 },
    selected,
    data: { kind, params, status: "idle" },
  } as Node;
}

describe("clipFromSelection", () => {
  it("keeps selected nodes and only edges internal to the selection", () => {
    const nodes = [node("a", "prompt", true), node("b", "generate", true), node("c", "preview", false)];
    const edges: Edge[] = [
      { id: "e1", source: "a", target: "b" }, // both selected → kept
      { id: "e2", source: "b", target: "c" }, // c not selected → dropped
    ];
    const clip = clipFromSelection(nodes, edges);
    expect(clip.nodes.map((n) => n.id)).toEqual(["a", "b"]);
    expect(clip.edges.map((e) => e.id)).toEqual(["e1"]);
  });

  it("returns an empty clip when nothing is selected", () => {
    const clip = clipFromSelection([node("a", "prompt", false)], []);
    expect(clip.nodes).toEqual([]);
  });
});

describe("buildPaste", () => {
  it("assigns new ids, offsets positions, remaps internal edges, and selects", () => {
    const clip = {
      nodes: [node("a", "prompt", true, { text: "hi" }), node("b", "generate", true)],
      edges: [{ id: "e1", source: "a", target: "b" }] as Edge[],
    };
    let seq = 0;
    const out = buildPaste(clip, { x: 40, y: 40 }, (kind) => `${kind}-new-${seq++}`);

    expect(out.nodes.map((n) => n.id)).toEqual(["prompt-new-0", "generate-new-1"]);
    // Position offset applied.
    expect(out.nodes[0].position).toEqual({ x: 140, y: 240 });
    // Edge remapped to the new ids.
    expect(out.edges[0].source).toBe("prompt-new-0");
    expect(out.edges[0].target).toBe("generate-new-1");
    // Everything selected.
    expect(out.nodes.every((n) => n.selected)).toBe(true);
    expect(out.edges.every((e) => e.selected)).toBe(true);
  });

  it("clones params so editing a paste does not mutate the source", () => {
    const src = node("a", "prompt", true, { text: "original" });
    const out = buildPaste({ nodes: [src], edges: [] }, { x: 0, y: 0 }, (k) => `${k}-1`);
    (out.nodes[0].data as { params: Record<string, unknown> }).params.text = "edited";
    expect((src.data as { params: Record<string, unknown> }).params.text).toBe("original");
  });

  it("drops edges that dangle outside the clip", () => {
    const clip = {
      nodes: [node("a", "prompt", true)],
      edges: [{ id: "e1", source: "a", target: "z" }] as Edge[],
    };
    const out = buildPaste(clip, { x: 0, y: 0 }, (k) => `${k}-1`);
    expect(out.edges).toEqual([]);
  });
});
