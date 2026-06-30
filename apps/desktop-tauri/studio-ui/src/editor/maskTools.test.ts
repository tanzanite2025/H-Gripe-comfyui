import { describe, expect, it } from "vitest";
import {
  DEFAULT_TOOL_ID,
  MASK_TOOLS,
  PLANNED_TOOLS,
  READY_TOOLS,
  maskTool,
} from "./maskTools";

describe("mask tool registry", () => {
  it("has unique ids", () => {
    const ids = MASK_TOOLS.map((t) => t.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("keeps pen / lasso / matting planned (greyed) per the contract", () => {
    for (const id of ["pen", "lasso", "matting"]) {
      expect(maskTool(id)?.status, id).toBe("planned");
    }
  });

  it("ships brush / eraser / wand / morphology as ready", () => {
    for (const id of ["brush", "eraser", "wand", "rect", "ellipse", "invert", "fill_holes", "smooth", "grow", "shrink", "feather"]) {
      expect(maskTool(id)?.status, id).toBe("ready");
    }
  });

  it("partitions ready vs planned and orders ready first", () => {
    expect(READY_TOOLS.every((t) => t.status === "ready")).toBe(true);
    expect(PLANNED_TOOLS.every((t) => t.status === "planned")).toBe(true);
    expect(READY_TOOLS.length + PLANNED_TOOLS.length).toBe(MASK_TOOLS.length);
    const firstPlanned = MASK_TOOLS.findIndex((t) => t.status === "planned");
    const lastReady = MASK_TOOLS.map((t) => t.status).lastIndexOf("ready");
    expect(lastReady).toBeLessThan(firstPlanned);
  });

  it("the default tool is ready and selectable", () => {
    expect(maskTool(DEFAULT_TOOL_ID)?.status).toBe("ready");
    expect(DEFAULT_TOOL_ID).toBe("brush");
  });

  it("paint tools carry an add/subtract mode", () => {
    expect(maskTool("brush")?.mode).toBe("add");
    expect(maskTool("eraser")?.mode).toBe("subtract");
  });
});
