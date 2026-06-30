import { describe, expect, it } from "vitest";
import { EXEC_LANES, isHeavyLane, isPreviewLane, type ExecLane } from "./execLanes";

describe("exec lanes", () => {
  it("orders lanes cheapest → heaviest", () => {
    expect(EXEC_LANES).toEqual(["interactive", "preview", "render"]);
  });

  it("treats only render as the heavy (GPU-queued) lane", () => {
    const heavy = EXEC_LANES.filter(isHeavyLane);
    expect(heavy).toEqual(["render"]);
  });

  it("treats only preview as the single-slot preview lane", () => {
    const preview = EXEC_LANES.filter(isPreviewLane);
    expect(preview).toEqual(["preview"]);
  });

  it("partitions every lane into exactly one predicate", () => {
    for (const lane of EXEC_LANES as ExecLane[]) {
      const matches = [isHeavyLane(lane), isPreviewLane(lane), lane === "interactive"].filter(
        Boolean,
      );
      expect(matches.length, lane).toBe(1);
    }
  });
});
