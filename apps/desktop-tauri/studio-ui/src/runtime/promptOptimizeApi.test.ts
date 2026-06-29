import { beforeEach, describe, expect, it, vi } from "vitest";

// Mock the Tauri bridge so we can drive the broker result for the api path
// (the default browser mock echoes the task with no `text`, which only
// exercises the raw-fallback branch — covered in executors.test.ts).
const runTaskJson = vi.fn();
vi.mock("../bridge/tauri", () => ({
  runTaskJson: (...args: unknown[]) => runTaskJson(...args),
  composePsd: vi.fn(),
  getOutputDir: vi.fn(),
}));

const { defaultExecutors } = await import("./executors");

function ctx(params: Record<string, unknown>, inputs: Record<string, unknown> = {}) {
  return { nodeId: "promptOptimize-1", kind: "promptOptimize", params, inputs };
}

describe("promptOptimize api mode (mocked broker)", () => {
  beforeEach(() => runTaskJson.mockReset());

  it("returns the trimmed optimized text when the broker succeeds", async () => {
    runTaskJson.mockResolvedValue({
      status: "succeeded",
      output_json: { text: "  a vivid red fox, cinematic lighting  " },
    });
    const out = await defaultExecutors.promptOptimize(ctx({ text: "fox", mode: "api" }));
    expect((out as { text: string }).text).toBe("a vivid red fox, cinematic lighting");
  });

  it("falls back to the raw prompt when the broker returns no usable text", async () => {
    runTaskJson.mockResolvedValue({ status: "succeeded", output_json: { text: "   " } });
    const out = await defaultExecutors.promptOptimize(ctx({ text: "fox", mode: "api" }));
    expect((out as { text: string }).text).toBe("fox");
  });

  it("throws with the broker error message when the task fails", async () => {
    runTaskJson.mockResolvedValue({
      status: "failed",
      error: { message: "no credentials configured" },
    });
    await expect(
      defaultExecutors.promptOptimize(ctx({ text: "fox", mode: "api" })),
    ).rejects.toThrow(/no credentials configured/);
  });
});
