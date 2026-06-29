import { describe, expect, it } from "vitest";
import type { Node, NodePositionChange } from "@xyflow/react";
import { getHelperLines } from "./helperLines";

function node(id: string, x: number, y: number, parentId?: string, width = 100): Node {
  return {
    id,
    position: { x, y },
    data: {},
    measured: { width, height: 40 },
    ...(parentId ? { parentId } : {}),
  } as Node;
}

function drag(id: string, x: number, y: number): NodePositionChange {
  return { id, type: "position", position: { x, y }, dragging: true };
}

describe("getHelperLines", () => {
  const others = [node("b", 200, 200)];

  it("snaps left edge and reports a vertical guide when within distance", () => {
    // a.left = 204 is 4px from b.left = 200 (< default 6)
    const r = getHelperLines(drag("a", 204, 50), [node("a", 204, 50), ...others]);
    expect(r.snapPosition.x).toBe(200);
    expect(r.vertical).toBe(200);
  });

  it("snaps top edge and reports a horizontal guide when within distance", () => {
    const r = getHelperLines(drag("a", 500, 203), [node("a", 500, 203), ...others]);
    expect(r.snapPosition.y).toBe(200);
    expect(r.horizontal).toBe(200);
  });

  it("does not snap when no edge is close enough", () => {
    const r = getHelperLines(drag("a", 500, 500), [node("a", 500, 500), ...others]);
    expect(r.snapPosition.x).toBeUndefined();
    expect(r.snapPosition.y).toBeUndefined();
    expect(r.vertical).toBeUndefined();
    expect(r.horizontal).toBeUndefined();
  });

  it("aligns the dragged node's right edge to another node's right edge", () => {
    // a is narrower (60) so only its right edge is near b.right (300); its left
    // (242) is far from b.left (200), isolating right-edge alignment.
    const a = node("a", 242, 50, undefined, 60); // right = 302, 2px from 300
    const r = getHelperLines(drag("a", 242, 50), [a, ...others]);
    expect(r.snapPosition.x).toBe(240); // 300 - 60
    expect(r.vertical).toBe(300);
  });

  it("ignores nodes in a different parent frame", () => {
    const r = getHelperLines(drag("a", 204, 50), [node("a", 204, 50), node("b", 200, 200, "g1")]);
    expect(r.snapPosition.x).toBeUndefined();
  });
});
