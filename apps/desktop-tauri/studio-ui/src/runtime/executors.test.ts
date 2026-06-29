import { describe, expect, it } from "vitest";
import { GRAPH_VERSION, type WorkflowGraph } from "../graph/model";
import { runGraph, validateGraph } from "./dag";
import { batchItems, defaultExecutors } from "./executors";

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

describe("batch", () => {
  it("parses non-empty trimmed lines", () => {
    expect(batchItems("a\n  b  \n\n c\n")).toEqual(["a", "b", "c"]);
    expect(batchItems("")).toEqual([]);
    expect(batchItems(undefined)).toEqual([]);
  });

  it("emits the item at the swept index, defaulting to the first", async () => {
    const items = "red fox\nblue jay\ngreen frog";
    expect(await defaultExecutors.batch(ctx("batch", { items }))).toEqual({ item: "red fox" });
    expect(await defaultExecutors.batch(ctx("batch", { items, index: 2 }))).toEqual({ item: "green frog" });
    // Out-of-range index falls back to the first item.
    expect(await defaultExecutors.batch(ctx("batch", { items, index: 9 }))).toEqual({ item: "red fox" });
  });
});

describe("reroute", () => {
  it("forwards its input unchanged (null when nothing is connected)", async () => {
    expect(await defaultExecutors.reroute(ctx("reroute", {}, { in: "/a/b.png" }))).toEqual({
      out: "/a/b.png",
    });
    expect(await defaultExecutors.reroute(ctx("reroute", {}, {}))).toEqual({ out: null });
  });

  it("validates and threads a value through when spliced into a chain", async () => {
    const g: WorkflowGraph = {
      version: GRAPH_VERSION,
      nodes: [
        { id: "prompt-1", kind: "prompt", position: { x: 0, y: 0 }, params: { text: "hi" } },
        { id: "reroute-1", kind: "reroute", position: { x: 0, y: 0 }, params: {} },
        { id: "preview-1", kind: "preview", position: { x: 0, y: 0 }, params: {} },
      ],
      edges: [
        { id: "e1", source: "prompt-1", sourcePort: "text", target: "reroute-1", targetPort: "in" },
        { id: "e2", source: "reroute-1", sourcePort: "out", target: "preview-1", targetPort: "image" },
      ],
    };
    // `any` ports keep the chain type-valid in both directions.
    expect(validateGraph(g)).toEqual([]);
    const { outputs } = await runGraph(g, defaultExecutors);
    expect(outputs.get("preview-1")).toEqual({ image: "hi" });
  });
});

describe("compare source", () => {
  it("compares numerically when both sides are numbers", async () => {
    expect(await defaultExecutors.compare(ctx("compare", { op: ">" }, { a: 5, b: 3 }))).toEqual({ result: 1 });
    expect(await defaultExecutors.compare(ctx("compare", { op: "<=" }, { a: 5, b: 3 }))).toEqual({ result: 0 });
    expect(await defaultExecutors.compare(ctx("compare", { op: "==" }, { a: "2", b: 2 }))).toEqual({ result: 1 });
  });

  it("falls back to string comparison for non-numeric values", async () => {
    expect(await defaultExecutors.compare(ctx("compare", { op: "==" }, { a: "fox", b: "fox" }))).toEqual({
      result: 1,
    });
    expect(await defaultExecutors.compare(ctx("compare", { op: "!=" }, { a: "fox", b: "jay" }))).toEqual({
      result: 1,
    });
  });
});

describe("logic source", () => {
  it("evaluates and/or/xor on truthiness", async () => {
    expect(await defaultExecutors.logic(ctx("logic", { op: "and" }, { a: 1, b: 0 }))).toEqual({ result: 0 });
    expect(await defaultExecutors.logic(ctx("logic", { op: "or" }, { a: 0, b: "x" }))).toEqual({ result: 1 });
    expect(await defaultExecutors.logic(ctx("logic", { op: "xor" }, { a: 1, b: 1 }))).toEqual({ result: 0 });
  });

  it("not uses only a", async () => {
    expect(await defaultExecutors.logic(ctx("logic", { op: "not" }, { a: 0, b: 1 }))).toEqual({ result: 1 });
  });
});

describe("if gate", () => {
  it("emits value only on the selected branch (param fallback)", async () => {
    expect(await defaultExecutors.if(ctx("if", { cond: "true" }, { value: "x" }))).toEqual({ true: "x" });
    expect(await defaultExecutors.if(ctx("if", { cond: "false" }, { value: "x" }))).toEqual({ false: "x" });
  });

  it("prefers the wired cond input (truthiness) over the param", async () => {
    expect(await defaultExecutors.if(ctx("if", { cond: "true" }, { value: "x", cond: 0 }))).toEqual({
      false: "x",
    });
    expect(await defaultExecutors.if(ctx("if", { cond: "false" }, { value: "x", cond: 1 }))).toEqual({
      true: "x",
    });
  });
});

describe("switch router", () => {
  it("routes to the port matching index, else default", async () => {
    expect(await defaultExecutors.switch(ctx("switch", { index: 1 }, { value: "v" }))).toEqual({ "1": "v" });
    expect(await defaultExecutors.switch(ctx("switch", { index: 9 }, { value: "v" }))).toEqual({
      default: "v",
    });
  });

  it("prefers the wired index input over the param", async () => {
    expect(await defaultExecutors.switch(ctx("switch", { index: 0 }, { value: "v", index: 2 }))).toEqual({
      "2": "v",
    });
  });
});

describe("generate", () => {
  // Outside Tauri, runTaskJson echoes the task back as output_json.task, so we
  // can assert how the executor composed the broker task.
  it("lifts provider/operation/credentials to top level, forwards the rest as params", async () => {
    const out = await defaultExecutors.generate(
      ctx(
        "generate",
        {
          provider: "openai",
          operation: "image.edit",
          credentials_ref: "openai-key",
          model: "gpt-image-1",
          steps: 20,
        },
        { prompt: "a fox" },
      ),
    );
    const task = (out.result as { output_json: { task: Record<string, unknown> } }).output_json.task;
    expect(task.provider).toBe("openai");
    expect(task.operation).toBe("image.edit");
    expect(task.credentials_ref).toBe("openai-key");
    expect(task.params).toEqual({ model: "gpt-image-1", steps: 20 });
    expect((task.params as Record<string, unknown>).credentials_ref).toBeUndefined();
  });

  it("uses null credentials when none is set", async () => {
    const out = await defaultExecutors.generate(ctx("generate", { provider: "mock", credentials_ref: "" }));
    const task = (out.result as { output_json: { task: Record<string, unknown> } }).output_json.task;
    expect(task.credentials_ref).toBeNull();
  });
});

describe("promptOptimize", () => {
  it("off mode passes the param text through unchanged", async () => {
    expect(
      await defaultExecutors.promptOptimize(ctx("promptOptimize", { text: "a fox", mode: "off" })),
    ).toEqual({ text: "a fox" });
  });

  it("a connected text input overrides the param", async () => {
    expect(
      await defaultExecutors.promptOptimize(
        ctx("promptOptimize", { text: "param", mode: "off" }, { text: "wired" }),
      ),
    ).toEqual({ text: "wired" });
  });

  it("local mode applies the rule-based preset transform", async () => {
    expect(
      await defaultExecutors.promptOptimize(
        ctx("promptOptimize", { text: "a cat, a cat", mode: "local", preset: "detailed" }),
      ),
    ).toEqual({
      text: "a cat, highly detailed, intricate, ultra quality, masterpiece",
    });
  });

  it("api mode builds a text.generate task and falls back to raw when no text is returned", async () => {
    const out = await defaultExecutors.promptOptimize(
      ctx("promptOptimize", {
        text: "a fox",
        mode: "api",
        provider: "openai_compatible",
        model: "gpt-4o-mini",
        instruction: "make it better",
        credentials_ref: "key-1",
      }),
    );
    // Outside Tauri, runTaskJson echoes the task in output_json.task with no
    // `text`, so the executor falls back to the raw prompt.
    expect((out as { text: string }).text).toBe("a fox");
    const task = (out.result as { output_json: { task: Record<string, unknown> } }).output_json.task;
    expect(task.operation).toBe("text.generate");
    expect(task.provider).toBe("openai_compatible");
    expect(task.credentials_ref).toBe("key-1");
    expect(task.inputs).toEqual({ prompt: "a fox" });
    expect(task.params).toEqual({ model: "gpt-4o-mini", system_prompt: "make it better" });
  });

  it("api mode short-circuits empty input without calling the broker", async () => {
    expect(
      await defaultExecutors.promptOptimize(ctx("promptOptimize", { text: "   ", mode: "api" })),
    ).toEqual({ text: "   " });
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

describe("psdExport sink", () => {
  // Outside Tauri, composePsd returns a mocked succeeded result built from the
  // request, so we can assert how the executor mapped node params to paths.
  it("composes the image into the template and returns export paths", async () => {
    const out = await defaultExecutors.psdExport(
      ctx(
        "psdExport",
        { filename: "poster", output_dir: "/out", smart_object_mode: "replace_content" },
        { image: "/gen/x.png", template: "/t.psd" },
      ),
    );
    expect(out).toEqual({
      psdPath: "/out/poster.psd",
      previewPath: "/out/poster_preview.png",
      metadataPath: "/out/poster_metadata.json",
      placeholderKind: "smartobject",
      smartObjectMode: "replace_content",
    });
  });

  it("falls back to the configured output dir when none is set", async () => {
    const out = (await defaultExecutors.psdExport(
      ctx("psdExport", { filename: "final" }, { image: "/gen/x.png", template: "/t.psd" }),
    )) as { psdPath: string };
    // getOutputDir's browser mock resolves to /mock/outputs.
    expect(out.psdPath).toBe("/mock/outputs/final.psd");
  });

  it("requires both an image and a template input", async () => {
    await expect(
      defaultExecutors.psdExport(ctx("psdExport", {}, { template: "/t.psd" })),
    ).rejects.toThrow(/image input/);
    await expect(
      defaultExecutors.psdExport(ctx("psdExport", {}, { image: "/gen/x.png" })),
    ).rejects.toThrow(/template input/);
  });
});

describe("psdContextAnalyze source", () => {
  // Outside Tauri, analyzePsdContext returns a mocked VisualContext, so we can
  // assert how the executor flattens it onto the node's output ports.
  it("analyzes a connected template and exposes the flat output ports", async () => {
    const out = (await defaultExecutors.psdContextAnalyze(
      ctx("psdContextAnalyze", { output_dir: "/out" }, { template: "/t.psd" }),
    )) as Record<string, unknown>;
    expect(out.prompt_suffix).toBe((out.visual_context as { prompt_suffix: string }).prompt_suffix);
    expect(out.background_image).toBe("/out/template_background.png");
    expect(out.placeholder_mask).toBe("/out/template_placeholder_mask.png");
    expect(out.placeholder_bounds).toEqual({ x: 320, y: 180, width: 1024, height: 1400 });
  });

  it("falls back to the psd_path param when no template is connected", async () => {
    const out = (await defaultExecutors.psdContextAnalyze(
      ctx("psdContextAnalyze", { psd_path: "/p.psd" }),
    )) as Record<string, unknown>;
    // getOutputDir's browser mock resolves to /mock/outputs.
    expect(out.background_image).toBe("/mock/outputs/template_background.png");
  });

  it("requires a template input or psd_path param", async () => {
    await expect(
      defaultExecutors.psdContextAnalyze(ctx("psdContextAnalyze", {})),
    ).rejects.toThrow(/PSD template/);
  });
});
