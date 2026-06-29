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

  it("accepts an optional refined mask and a production metadata object", async () => {
    // The refined mask + metadata object flow through to compose_psd; the
    // browser mock ignores them, so we just assert the export still succeeds.
    const out = (await defaultExecutors.psdExport(
      ctx(
        "psdExport",
        { filename: "poster", output_dir: "/out" },
        {
          image: "/gen/x.png",
          template: "/t.psd",
          mask: "/gen/x_mask.png",
          metadata: { workflow_id: "wf-1", source_psd: "/t.psd" },
        },
      ),
    )) as { psdPath: string };
    expect(out.psdPath).toBe("/out/poster.psd");
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

describe("matchLightColor", () => {
  // Outside Tauri, matchLightColor returns a mocked ColorMatchResult, so we can
  // assert how the executor flattens it onto the node's output ports.
  it("matches a connected image and exposes the flat output ports", async () => {
    const out = (await defaultExecutors.matchLightColor(
      ctx(
        "matchLightColor",
        { mode: "color_transfer", strength: 0.7, output_dir: "/out", output_name: "hero" },
        { image: "/subject.png", background: "/bg.png" },
      ),
    )) as Record<string, unknown>;
    expect(out.matched_image).toBe("/out/hero.png");
    expect(typeof out.prompt_suffix).toBe("string");
    const report = out.match_report as { applied: boolean; mode: string; strength: number };
    expect(report.applied).toBe(true);
    expect(report.mode).toBe("color_transfer");
    expect(report.strength).toBe(0.7);
  });

  it("prefers the upstream visual_context prompt suffix", async () => {
    const out = (await defaultExecutors.matchLightColor(
      ctx(
        "matchLightColor",
        { mode: "prompt_only" },
        { image: "/subject.png", visual_context: { prompt_suffix: "studio rim light, 6000k" } },
      ),
    )) as Record<string, unknown>;
    expect(out.prompt_suffix).toBe("studio rim light, 6000k");
    // prompt_only does not touch pixels.
    expect((out.match_report as { applied: boolean }).applied).toBe(false);
  });

  it("requires a connected image input", async () => {
    await expect(
      defaultExecutors.matchLightColor(ctx("matchLightColor", {})),
    ).rejects.toThrow(/connected image/);
  });
});

describe("refineMaskEdge", () => {
  // Outside Tauri, refineMaskEdge returns a mocked RefineEdgeResult, so we can
  // assert how the executor flattens it onto the node's output ports.
  it("refines a connected image and exposes the flat output ports", async () => {
    const out = (await defaultExecutors.refineMaskEdge(
      ctx(
        "refineMaskEdge",
        { preset: "clean", output_dir: "/out", output_name: "hero" },
        { image: "/subject.png", background: "/bg.png" },
      ),
    )) as Record<string, unknown>;
    expect(out.refined_image).toBe("/out/hero.png");
    expect(out.refined_mask).toBe("/out/hero_mask.png");
    const report = out.edge_report as { preset: string; source_mask: string; background_applied: boolean };
    expect(report.preset).toBe("clean");
    // No explicit mask wired, so the image's own alpha is used.
    expect(report.source_mask).toBe("alpha");
    expect(report.background_applied).toBe(true);
  });

  it("honours custom-preset parameters and a connected mask", async () => {
    const out = (await defaultExecutors.refineMaskEdge(
      ctx(
        "refineMaskEdge",
        { preset: "custom", erode_px: 3, feather_px: 10, edge_decontaminate: false },
        { image: "/subject.png", mask: "/matte.png" },
      ),
    )) as Record<string, unknown>;
    const report = out.edge_report as {
      erode_px: number;
      feather_px: number;
      edge_decontaminate: boolean;
      source_mask: string;
      background_applied: boolean;
    };
    expect(report.erode_px).toBe(3);
    expect(report.feather_px).toBe(10);
    expect(report.edge_decontaminate).toBe(false);
    expect(report.source_mask).toBe("explicit");
    // No background wired, so nothing is blended into the edge band.
    expect(report.background_applied).toBe(false);
  });

  it("requires a connected image input", async () => {
    await expect(
      defaultExecutors.refineMaskEdge(ctx("refineMaskEdge", {})),
    ).rejects.toThrow(/connected image/);
  });
});

describe("imageEnhance", () => {
  // Outside Tauri, enhanceImage returns a mocked EnhanceImageResult, so we can
  // assert how the executor resolves the target size and flattens the report.
  it("derives the scale from connected placeholder bounds", async () => {
    const out = (await defaultExecutors.imageEnhance(
      ctx(
        "imageEnhance",
        { mode: "print_ready", output_dir: "/out", output_name: "hero" },
        { image: "/subject.png", target_bounds: { x: 0, y: 0, width: 2048, height: 2800 } },
      ),
    )) as Record<string, unknown>;
    expect(out.enhanced_image).toBe("/out/hero.png");
    // Mock source is 512x700; covering 2048x2800 needs a 4x upscale.
    expect(out.scale_factor).toBe(4);
    const report = out.enhance_report as {
      mode: string;
      target_size: [number, number] | null;
      clamped: boolean;
    };
    expect(report.mode).toBe("print_ready");
    expect(report.target_size).toEqual([2048, 2800]);
    expect(report.clamped).toBe(false);
  });

  it("falls back to the preset scale with no target and honours preserve_text_logo", async () => {
    const out = (await defaultExecutors.imageEnhance(
      ctx(
        "imageEnhance",
        { mode: "texture_rebuild", output_dir: "/out", output_name: "hero", preserve_text_logo: true },
        { image: "/subject.png" },
      ),
    )) as Record<string, unknown>;
    expect(out.scale_factor).toBe(2);
    const report = out.enhance_report as { target_size: unknown; texture_strength: number };
    expect(report.target_size).toBeNull();
    // texture_rebuild's 0.7 texture is capped to 0.4 when text/logo is protected.
    expect(report.texture_strength).toBe(0.4);
  });

  it("clamps the scale to honour max_pixels", async () => {
    const out = (await defaultExecutors.imageEnhance(
      ctx(
        "imageEnhance",
        { mode: "conservative", target_width: 5120, target_height: 7000, max_pixels: 1_000_000, output_dir: "/out" },
        { image: "/subject.png" },
      ),
    )) as Record<string, unknown>;
    const report = out.enhance_report as { clamped: boolean; output_size: [number, number] };
    expect(report.clamped).toBe(true);
    expect(report.output_size[0] * report.output_size[1]).toBeLessThanOrEqual(1_000_000);
  });

  it("requires a connected image input", async () => {
    await expect(
      defaultExecutors.imageEnhance(ctx("imageEnhance", {})),
    ).rejects.toThrow(/connected image/);
  });
});

describe("detailWatchdog", () => {
  // Outside Tauri, detectQualityIssues returns a mocked DetectQualityResult, so
  // we assert how the executor resolves targets, derives the report, and
  // reports skipped (CPU-unsupported) watch targets.
  it("flags low resolution + colour mismatch and fails on large bounds with a far background", async () => {
    const out = (await defaultExecutors.detailWatchdog(
      ctx(
        "detailWatchdog",
        { mode: "strict", output_dir: "/out", output_name: "iss" },
        {
          image: "/cand.png",
          visual_context: {
            background: { mean_color: [20, 20, 20] },
            placeholder: { bounds: { x: 0, y: 0, width: 2048, height: 2800 } },
          },
        },
      ),
    )) as Record<string, unknown>;
    expect(out.fixed_image).toBe("/cand.png");
    const report = out.quality_report as { status: string; issues: { type: string }[] };
    expect(report.status).toBe("failed");
    const types = report.issues.map((i) => i.type).sort();
    expect(types).toEqual(["color_mismatch", "low_resolution"]);
    expect(out.issue_masks).toBe("/out/iss.png");
  });

  it("passes (no issues, no overlay) when in-bounds with a near background", async () => {
    const out = (await defaultExecutors.detailWatchdog(
      ctx(
        "detailWatchdog",
        { mode: "lenient" },
        {
          image: "/cand.png",
          target_bounds: { x: 0, y: 0, width: 400, height: 500 },
          visual_context: { background: { mean_color: [182, 172, 158] } },
        },
      ),
    )) as Record<string, unknown>;
    const report = out.quality_report as { status: string; issues: unknown[] };
    expect(report.status).toBe("passed");
    expect(report.issues).toEqual([]);
    expect(out.issue_masks).toBeNull();
  });

  it("reports CPU-unsupported watch targets as skipped", async () => {
    const out = (await defaultExecutors.detailWatchdog(
      ctx(
        "detailWatchdog",
        { mode: "balanced", watch_targets: "face,hands,logo" },
        { image: "/cand.png" },
      ),
    )) as Record<string, unknown>;
    const wr = out.watchdog_report as { watch_targets: string[]; skipped_targets: string[] };
    expect(wr.watch_targets).toEqual(["face", "hands", "logo"]);
    expect(wr.skipped_targets).toEqual(["hands", "logo"]);
  });

  it("requires a connected image input", async () => {
    await expect(
      defaultExecutors.detailWatchdog(ctx("detailWatchdog", {})),
    ).rejects.toThrow(/connected image/);
  });
});
