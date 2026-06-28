import { describe, expect, it } from "vitest";
import { defaultExecutors } from "./executors";

function ctx(kind: string, params: Record<string, unknown>, inputs: Record<string, unknown> = {}) {
  return { nodeId: `${kind}-1`, kind, params, inputs };
}

describe("source executors", () => {
  it("imageSource emits its path as an image (or null when empty)", async () => {
    expect(await defaultExecutors.imageSource(ctx("imageSource", { path: "/a/b.png" }))).toEqual({
      image: "/a/b.png",
    });
    expect(await defaultExecutors.imageSource(ctx("imageSource", { path: "" }))).toEqual({ image: null });
  });

  it("number coerces its param to a number", async () => {
    expect(await defaultExecutors.number(ctx("number", { value: "42" }))).toEqual({ value: 42 });
  });

  it("psdTemplate emits its path as a template", async () => {
    expect(await defaultExecutors.psdTemplate(ctx("psdTemplate", { path: "/t.psd" }))).toEqual({
      template: "/t.psd",
    });
  });
});

describe("save sink", () => {
  it("collects the incoming image/template and filename", async () => {
    const out = await defaultExecutors.save(
      ctx("save", { filename: "fox.png" }, { image: "/out/x.png", template: "/t.psd" }),
    );
    expect(out).toEqual({ image: "/out/x.png", template: "/t.psd", filename: "fox.png" });
  });
});
