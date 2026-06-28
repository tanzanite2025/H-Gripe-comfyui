import { describe, expect, it } from "vitest";
import type { Edge, Node } from "@xyflow/react";
import { createHistoryStack, type GraphSnapshot } from "./history";

function snap(label: string): GraphSnapshot {
  return { nodes: [{ id: label, position: { x: 0, y: 0 }, data: {} } as Node], edges: [] as Edge[] };
}

describe("createHistoryStack", () => {
  it("undo returns the prior snapshot and redo restores the later one", () => {
    const h = createHistoryStack();
    expect(h.canUndo()).toBe(false);
    expect(h.canRedo()).toBe(false);

    // Edit A -> B: snapshot A before applying B.
    h.push(snap("A"));
    expect(h.canUndo()).toBe(true);

    // Undo from current B → restores A, stashes B for redo.
    const back = h.undo(snap("B"));
    expect(back?.nodes[0].id).toBe("A");
    expect(h.canUndo()).toBe(false);
    expect(h.canRedo()).toBe(true);

    // Redo from current A → restores B.
    const fwd = h.redo(snap("A"));
    expect(fwd?.nodes[0].id).toBe("B");
    expect(h.canRedo()).toBe(false);
    expect(h.canUndo()).toBe(true);
  });

  it("a new push after undo clears the redo stack", () => {
    const h = createHistoryStack();
    h.push(snap("A"));
    h.undo(snap("B"));
    expect(h.canRedo()).toBe(true);
    h.push(snap("C"));
    expect(h.canRedo()).toBe(false);
  });

  it("undo/redo on an empty stack return null", () => {
    const h = createHistoryStack();
    expect(h.undo(snap("X"))).toBeNull();
    expect(h.redo(snap("X"))).toBeNull();
  });

  it("respects the snapshot limit, dropping the oldest", () => {
    const h = createHistoryStack(2);
    h.push(snap("A"));
    h.push(snap("B"));
    h.push(snap("C")); // drops A
    expect(h.undo(snap("D"))?.nodes[0].id).toBe("C");
    expect(h.undo(snap("C"))?.nodes[0].id).toBe("B");
    expect(h.canUndo()).toBe(false); // A was dropped
  });

  it("clear() drops all history", () => {
    const h = createHistoryStack();
    h.push(snap("A"));
    h.undo(snap("B"));
    h.clear();
    expect(h.canUndo()).toBe(false);
    expect(h.canRedo()).toBe(false);
  });
});
