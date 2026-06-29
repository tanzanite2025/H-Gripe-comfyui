import { describe, expect, it } from "vitest";

import { baseName } from "./ProjectPanel";

describe("baseName", () => {
  it("returns the last segment of a POSIX path", () => {
    expect(baseName("/home/user/workflows/poster.workflow.json")).toBe(
      "poster.workflow.json",
    );
  });

  it("returns the last segment of a Windows path", () => {
    expect(baseName("C:\\Users\\me\\studio\\banner.json")).toBe("banner.json");
  });

  it("handles mixed separators", () => {
    expect(baseName("C:/Users/me\\studio/scene.json")).toBe("scene.json");
  });

  it("returns the input when there is no separator", () => {
    expect(baseName("workflow.json")).toBe("workflow.json");
  });

  it("falls back to the full string for a trailing separator", () => {
    expect(baseName("/tmp/")).toBe("/tmp/");
  });
});
