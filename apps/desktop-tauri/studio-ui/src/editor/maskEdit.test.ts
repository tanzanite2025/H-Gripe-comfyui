import { describe, expect, it } from "vitest";
import {
  addBrushStroke,
  addMatteStroke,
  addOperation,
  addPoint,
  canRedo,
  canUndo,
  clearEdits,
  editCount,
  initEditState,
  isEmpty,
  normalizeEditPaths,
  redo,
  undo,
} from "./maskEdit";
import type { BrushStroke } from "../types/production";

const stroke = (id: string): BrushStroke => ({
  id,
  mode: "add",
  radius: 12,
  points: [
    [0, 0],
    [4, 4],
  ],
});

describe("maskEdit normalizeEditPaths", () => {
  it("returns an empty, well-formed EditPaths for junk input", () => {
    for (const bad of [null, undefined, 42, "x", {}]) {
      const e = normalizeEditPaths(bad);
      expect(e.version).toBe(1);
      expect(e.paths).toEqual([]);
      expect(e.brush_strokes).toEqual([]);
      expect(e.operations).toEqual([]);
    }
  });

  it("preserves existing arrays", () => {
    const e = normalizeEditPaths({ version: 1, brush_strokes: [stroke("s1")], paths: [], operations: [] });
    expect(e.brush_strokes).toHaveLength(1);
  });
});

describe("maskEdit reducer-style helpers", () => {
  it("records brush strokes and operations and counts them", () => {
    let s = initEditState();
    s = addBrushStroke(s, stroke("s1"));
    s = addOperation(s, { type: "feather", amount: 3 });
    expect(s.current.brush_strokes).toHaveLength(1);
    expect(s.current.operations).toHaveLength(1);
    expect(editCount(s.current)).toBe(2);
    expect(isEmpty(s.current)).toBe(false);
  });

  it("records trimap matting-band strokes and counts them", () => {
    let s = initEditState();
    s = addMatteStroke(s, stroke("m1"));
    expect(s.current.matte_strokes).toHaveLength(1);
    expect(s.current.brush_strokes).toHaveLength(0);
    expect(editCount(s.current)).toBe(1);
    expect(isEmpty(s.current)).toBe(false);
    s = undo(s);
    expect(s.current.matte_strokes).toHaveLength(0);
  });

  it("records SAM 2 point prompts and counts them", () => {
    let s = initEditState();
    s = addPoint(s, [120, 80]);
    s = addPoint(s, [200, 150]);
    expect(s.current.points).toEqual([
      [120, 80],
      [200, 150],
    ]);
    expect(editCount(s.current)).toBe(2);
    expect(isEmpty(s.current)).toBe(false);
    s = undo(s);
    expect(s.current.points).toEqual([[120, 80]]);
  });

  it("ignores empty strokes", () => {
    let s = initEditState();
    s = addBrushStroke(s, { id: "x", mode: "add", radius: 4, points: [] });
    expect(s.current.brush_strokes).toHaveLength(0);
  });

  it("undo/redo walks the history and toggles availability", () => {
    let s = initEditState();
    expect(canUndo(s)).toBe(false);
    expect(canRedo(s)).toBe(false);

    s = addBrushStroke(s, stroke("s1"));
    s = addBrushStroke(s, stroke("s2"));
    expect(editCount(s.current)).toBe(2);
    expect(canUndo(s)).toBe(true);

    s = undo(s);
    expect(editCount(s.current)).toBe(1);
    expect(canRedo(s)).toBe(true);

    s = redo(s);
    expect(editCount(s.current)).toBe(2);
    expect(canRedo(s)).toBe(false);
  });

  it("a new edit after undo clears the redo branch", () => {
    let s = initEditState();
    s = addBrushStroke(s, stroke("s1"));
    s = undo(s);
    expect(canRedo(s)).toBe(true);
    s = addOperation(s, { type: "invert" });
    expect(canRedo(s)).toBe(false);
    expect(s.current.operations).toHaveLength(1);
  });

  it("clear is undoable and a no-op when already empty", () => {
    let s = initEditState();
    expect(clearEdits(s)).toBe(s); // no-op, same reference
    s = addBrushStroke(s, stroke("s1"));
    s = clearEdits(s);
    expect(isEmpty(s.current)).toBe(true);
    s = undo(s);
    expect(editCount(s.current)).toBe(1);
  });

  it("seeds from an initial EditPaths", () => {
    const s = initEditState({ version: 1, paths: [], brush_strokes: [stroke("s0")], matte_strokes: [], operations: [], points: [] });
    expect(editCount(s.current)).toBe(1);
    expect(canUndo(s)).toBe(false);
  });
});
