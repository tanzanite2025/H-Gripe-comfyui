import { describe, expect, it } from "vitest";
import type { Node } from "@xyflow/react";
import {
  absolutePosition,
  detachChildren,
  findContainingGroup,
  isGroupNode,
  makeGroupNode,
  orderNodes,
  reparentNode,
} from "./grouping";

function hg(id: string, x: number, y: number, parentId?: string): Node {
  const n: Node = {
    id,
    type: "hgripe",
    position: { x, y },
    width: 100,
    height: 60,
    data: { kind: "prompt", params: {}, status: "idle" },
  };
  if (parentId) n.parentId = parentId;
  return n;
}

describe("grouping geometry", () => {
  it("identifies group nodes", () => {
    expect(isGroupNode(makeGroupNode("g1", 0, 0))).toBe(true);
    expect(isGroupNode(hg("n1", 0, 0))).toBe(false);
  });

  it("resolves absolute position through a parent", () => {
    const group = makeGroupNode("g1", 200, 100);
    const child = hg("n1", 20, 10, "g1");
    const byId = new Map([group, child].map((n) => [n.id, n]));
    expect(absolutePosition(child, byId)).toEqual({ x: 220, y: 110 });
    expect(absolutePosition(group, byId)).toEqual({ x: 200, y: 100 });
  });

  it("finds the group containing a node's center", () => {
    const group = makeGroupNode("g1", 0, 0); // 320x240
    const inside = hg("n1", 50, 50); // center 100,80 inside
    const outside = hg("n2", 400, 400);
    const nodes = [group, inside, outside];
    expect(findContainingGroup("n1", nodes)).toBe("g1");
    expect(findContainingGroup("n2", nodes)).toBeNull();
  });

  it("never assigns a group to another group", () => {
    const big = { ...makeGroupNode("g1", 0, 0), width: 600, height: 600 } as Node;
    const small = makeGroupNode("g2", 50, 50);
    expect(findContainingGroup("g2", [big, small])).toBeNull();
  });
});

describe("reparentNode", () => {
  it("attaches a node and converts to relative coords without visual jump", () => {
    const group = makeGroupNode("g1", 200, 100);
    const child = hg("n1", 220, 110);
    const out = reparentNode([group, child], "n1", "g1");
    const moved = out.find((n) => n.id === "n1")!;
    expect(moved.parentId).toBe("g1");
    expect(moved.position).toEqual({ x: 20, y: 10 });
    // absolute position is preserved
    const byId = new Map(out.map((n) => [n.id, n]));
    expect(absolutePosition(moved, byId)).toEqual({ x: 220, y: 110 });
  });

  it("detaches a node and converts back to absolute coords", () => {
    const group = makeGroupNode("g1", 200, 100);
    const child = hg("n1", 20, 10, "g1");
    const out = reparentNode([group, child], "n1", null);
    const moved = out.find((n) => n.id === "n1")!;
    expect(moved.parentId).toBeUndefined();
    expect(moved.position).toEqual({ x: 220, y: 110 });
  });

  it("is a no-op when parent is unchanged", () => {
    const group = makeGroupNode("g1", 200, 100);
    const child = hg("n1", 20, 10, "g1");
    const input = [group, child];
    expect(reparentNode(input, "n1", "g1")).toBe(input);
  });

  it("round-trips in then out preserving absolute position", () => {
    const group = makeGroupNode("g1", 200, 100);
    const child = hg("n1", 220, 110);
    const into = reparentNode([group, child], "n1", "g1");
    const out = reparentNode(into, "n1", null);
    const moved = out.find((n) => n.id === "n1")!;
    expect(moved.parentId).toBeUndefined();
    expect(moved.position).toEqual({ x: 220, y: 110 });
  });
});

describe("orderNodes", () => {
  it("places group frames before roots before children", () => {
    const group = makeGroupNode("g1", 0, 0);
    const root = hg("r1", 500, 0);
    const child = hg("c1", 10, 10, "g1");
    const ordered = orderNodes([child, root, group]);
    expect(ordered.map((n) => n.id)).toEqual(["g1", "r1", "c1"]);
  });
});

describe("detachChildren", () => {
  it("frees a deleted group's members back to absolute coords", () => {
    const group = makeGroupNode("g1", 200, 100);
    const child = hg("n1", 20, 10, "g1");
    const out = detachChildren([group, child], new Set(["g1"]));
    const freed = out.find((n) => n.id === "n1")!;
    expect(freed.parentId).toBeUndefined();
    expect(freed.position).toEqual({ x: 220, y: 110 });
  });
});
