// Default executor registry: maps node kinds to runtime behaviour.
//
// The `generate` node composes an ApiTask and runs it through the existing
// H-Gripe broker (`run_task_json`). Source nodes (`prompt`, `imageSource`,
// `psdTemplate`, `number`) are pure value providers; `preview` / `save` are
// sinks. This wires the renderer-agnostic DAG runtime to real backend
// capability.

import { analyzePsdContext, composePsd, detectQualityIssues, enhanceImage, getOutputDir, matchLightColor, refineMaskEdge, runTaskJson } from "../bridge/tauri";
import type { Bounds, VisualContext } from "../types/production";
import type { ExecutorRegistry } from "./dag";
import {
  optimizePromptLocally,
  promptOptimizeProviderSupported,
  type LocalPreset,
} from "./promptOptimize";

// Params that are not forwarded into the broker task's `params` map; they are
// top-level task fields instead.
const GENERATE_RESERVED = new Set(["provider", "operation", "credentials_ref"]);

/** Non-empty, trimmed lines of a batch node's `items` param. */
export function batchItems(items: unknown): string[] {
  return String(items ?? "")
    .split("\n")
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

export const defaultExecutors: ExecutorRegistry = {
  prompt: async (ctx) => ({ text: String(ctx.params.text ?? "") }),

  // Initial text node with optional prompt optimisation. A connected `text`
  // input overrides the param. `off` passes through, `local` applies the
  // model-free preset transform, `api` rewrites via an LLM provider profile.
  promptOptimize: async (ctx) => {
    const raw =
      "text" in ctx.inputs
        ? String(ctx.inputs.text ?? "")
        : String(ctx.params.text ?? "");
    const mode = String(ctx.params.mode ?? "off");

    if (mode === "local") {
      const preset = String(ctx.params.preset ?? "cleanup") as LocalPreset;
      return { text: optimizePromptLocally(raw, preset) };
    }

    if (mode === "api") {
      if (!raw.trim()) return { text: raw };
      const provider = String(ctx.params.provider ?? "openai_compatible") || "openai_compatible";
      if (!promptOptimizeProviderSupported(provider)) {
        throw new Error(
          `Provider "${provider}" can't optimize prompts (no text.generate support). ` +
            `Pick an OpenAI-compatible chat profile, or switch mode to "local"/"off".`,
        );
      }
      const params: Record<string, unknown> = {};
      const model = String(ctx.params.model ?? "").trim();
      if (model) params.model = model;
      const instruction = String(ctx.params.instruction ?? "").trim();
      if (instruction) params.system_prompt = instruction;
      // Optional sampling controls (forwarded to the chat call when set).
      for (const key of ["temperature", "max_tokens", "seed"] as const) {
        const num = Number(ctx.params[key]);
        if (ctx.params[key] !== undefined && ctx.params[key] !== "" && Number.isFinite(num)) {
          params[key] = num;
        }
      }

      const task = {
        id: `studio-${ctx.nodeId}-${Date.now()}`,
        provider,
        operation: "text.generate",
        inputs: { prompt: raw },
        params,
        credentials_ref: String(ctx.params.credentials_ref ?? "") || null,
        output_type: "text",
        // Cache identical optimisations (same text+instruction+model+sampling)
        // so re-runs don't re-bill the LLM; the broker derives the key.
        cache_policy: { enabled: true, ttl_seconds: null, key: null },
        retry_policy: { max_attempts: 1, backoff_ms: 200, timeout_ms: 60000 },
      };

      const result = await runTaskJson(task);
      if (result.status === "failed") {
        throw new Error(result.error?.message ?? "prompt optimization failed");
      }
      const optimized = (result.output_json as { text?: unknown } | null)?.text;
      const text = typeof optimized === "string" ? optimized.trim() : "";
      return { text: text || raw, result };
    }

    return { text: raw };
  },

  // Emits a single item from the list. A normal run emits index 0; batch
  // fan-out sweeps `index` via runGraph's paramOverrides.
  batch: async (ctx) => {
    const items = batchItems(ctx.params.items);
    const index = Number(ctx.params.index ?? 0);
    return { item: items[index] ?? items[0] ?? "" };
  },

  imageSource: async (ctx) => ({ image: String(ctx.params.path ?? "") || null }),

  psdTemplate: async (ctx) => ({ template: String(ctx.params.path ?? "") || null }),

  number: async (ctx) => ({ value: Number(ctx.params.value ?? 0) }),

  // Pass-through relay: forwards whatever arrives on `in` to `out` unchanged.
  reroute: async (ctx) => ({ out: ctx.inputs.in ?? null }),

  // Group container is purely organisational: no ports, no work at run time.
  group: async () => ({}),

  // Comparison source: emits 1/0 from comparing two values. Numeric comparison
  // when both sides parse as numbers, else lexicographic string comparison.
  compare: async (ctx) => {
    const a = ctx.inputs.a;
    const b = ctx.inputs.b;
    const an = Number(a);
    const bn = Number(b);
    const numeric =
      a !== "" && a != null && b !== "" && b != null && !Number.isNaN(an) && !Number.isNaN(bn);
    const sa = String(a ?? "");
    const sb = String(b ?? "");
    const op = String(ctx.params.op ?? "==");
    let res: boolean;
    switch (op) {
      case "==":
        res = numeric ? an === bn : sa === sb;
        break;
      case "!=":
        res = numeric ? an !== bn : sa !== sb;
        break;
      case ">":
        res = numeric ? an > bn : sa > sb;
        break;
      case ">=":
        res = numeric ? an >= bn : sa >= sb;
        break;
      case "<":
        res = numeric ? an < bn : sa < sb;
        break;
      case "<=":
        res = numeric ? an <= bn : sa <= sb;
        break;
      default:
        res = false;
    }
    return { result: res ? 1 : 0 };
  },

  // Boolean logic source: emits 1/0 from the truthiness of its inputs. `not`
  // negates only `a`.
  logic: async (ctx) => {
    const a = !!ctx.inputs.a;
    const b = !!ctx.inputs.b;
    const op = String(ctx.params.op ?? "and");
    let res: boolean;
    switch (op) {
      case "and":
        res = a && b;
        break;
      case "or":
        res = a || b;
        break;
      case "xor":
        res = a !== b;
        break;
      case "not":
        res = !a;
        break;
      default:
        res = false;
    }
    return { result: res ? 1 : 0 };
  },

  // Conditional gate. Emits `value` on exactly one output port; the other port
  // gets nothing, which prunes that branch (its subtree is skipped). The wired
  // `cond` input (truthiness) wins over the param fallback.
  if: async (ctx) => {
    const active =
      "cond" in ctx.inputs ? !!ctx.inputs.cond : String(ctx.params.cond ?? "true") === "true";
    const value = ctx.inputs.value ?? null;
    return active ? { true: value } : { false: value };
  },

  // Multi-way router. Emits `value` on the port matching `index` (0/1/2), else
  // on `default`; all other ports stay empty so their branches are pruned.
  switch: async (ctx) => {
    const idx = "index" in ctx.inputs ? Number(ctx.inputs.index) : Number(ctx.params.index ?? 0);
    const port = idx === 0 ? "0" : idx === 1 ? "1" : idx === 2 ? "2" : "default";
    return { [port]: ctx.inputs.value ?? null };
  },

  generate: async (ctx) => {
    const prompt = (ctx.inputs.prompt as string | undefined) ?? "";
    const reference = ctx.inputs.reference as string | undefined;
    const seedInput = ctx.inputs.seed as number | undefined;

    const inputs: Record<string, unknown> = {};
    if (prompt) inputs.prompt = prompt;
    if (reference) inputs.image_path = reference;

    // Forward every non-reserved, non-empty param into the broker task params.
    // A connected `seed` input overrides the param of the same name.
    const params: Record<string, unknown> = {};
    for (const [key, value] of Object.entries(ctx.params)) {
      if (GENERATE_RESERVED.has(key)) continue;
      if (value === "" || value === null || value === undefined) continue;
      params[key] = value;
    }
    if (seedInput !== undefined) params.seed = seedInput;

    const credentialsRef = String(ctx.params.credentials_ref ?? "") || null;
    const task = {
      id: `studio-${ctx.nodeId}-${Date.now()}`,
      provider: String(ctx.params.provider ?? "mock"),
      operation: String(ctx.params.operation ?? "image.generate"),
      inputs,
      params,
      credentials_ref: credentialsRef,
      output_type: "image",
      cache_policy: { enabled: false, ttl_seconds: null, key: null },
      retry_policy: { max_attempts: 1, backoff_ms: 200, timeout_ms: 60000 },
    };

    const result = await runTaskJson(task);
    if (result.status === "failed") {
      throw new Error(result.error?.message ?? "generation failed");
    }
    const image = result.output_files?.[0]?.path;
    return { image: image ?? null, result };
  },

  preview: async (ctx) => ({ image: ctx.inputs.image ?? null }),

  save: async (ctx) => ({
    image: ctx.inputs.image ?? null,
    template: ctx.inputs.template ?? null,
    filename: String(ctx.params.filename ?? "output.png"),
  }),

  // Reads a PSD template (connected `template` input, else the `psd_path`
  // param) into a structured VisualContext via the backend
  // `analyze_psd_context` command, exposing the context plus its flat output
  // ports (prompt suffix, background preview, placeholder mask + bounds) for
  // downstream production nodes.
  psdContextAnalyze: async (ctx) => {
    const template =
      (ctx.inputs.template as string | undefined) ??
      (String(ctx.params.psd_path ?? "").trim() || null);
    if (!template) {
      throw new Error(
        "PSD Context Analyze needs a PSD template (connect a PSD Template node or set psd_path)",
      );
    }

    const outputDir = String(ctx.params.output_dir ?? "").trim() || (await getOutputDir());
    const references = String(ctx.params.reference_layers ?? "")
      .split("\n")
      .map((s) => s.trim())
      .filter((s) => s.length > 0);

    const context = await analyzePsdContext({
      template,
      backgroundLayer: String(ctx.params.background_layer ?? "").trim() || undefined,
      targetPlaceholder: String(ctx.params.target_placeholder ?? "").trim() || undefined,
      referenceLayers: references.length > 0 ? references : undefined,
      outputDir: outputDir || undefined,
    });
    return {
      visual_context: context,
      prompt_suffix: context.prompt_suffix,
      background_image: context.background.image_path,
      placeholder_mask: context.placeholder.mask_path,
      placeholder_bounds: context.placeholder.bounds,
    };
  },

  // Nudges the upstream subject image's light & colour toward the PSD
  // background (Reinhard Lab transfer / histogram match, sparing brand colours)
  // via the backend `match_light_color` command, exposing the matched image,
  // the match report, and a prompt suffix.
  matchLightColor: async (ctx) => {
    const image = (ctx.inputs.image as string | undefined) ?? null;
    if (!image) throw new Error("Light & Color Match needs a connected image input");

    const outputDir = String(ctx.params.output_dir ?? "").trim() || (await getOutputDir());
    const result = await matchLightColor({
      image,
      background: (ctx.inputs.background as string | undefined) || undefined,
      mask: (ctx.inputs.mask as string | undefined) || undefined,
      context: (ctx.inputs.visual_context as VisualContext | undefined) ?? undefined,
      mode: String(ctx.params.mode ?? "color_transfer") || undefined,
      strength: Number(ctx.params.strength ?? 0.6),
      shadowStrength: Number(ctx.params.shadow_strength ?? 0),
      highlightStrength: Number(ctx.params.highlight_strength ?? 0),
      protectSaturation: Boolean(ctx.params.protect_saturation ?? false),
      protectBrandColor: Boolean(ctx.params.protect_brand_color ?? true),
      outputDir: outputDir || undefined,
      outputName: String(ctx.params.output_name ?? "").trim() || undefined,
    });
    return {
      matched_image: result.matched_image,
      match_report: result.match_report,
      prompt_suffix: result.prompt_suffix,
    };
  },

  // Cleans the upstream subject's matte (erode/dilate, guided-filter edge
  // snapping, feather, colour decontamination) so it drops into a PSD
  // placeholder without white halos via the backend `refine_mask_edge`
  // command, exposing the refined image, refined mask, and an edge report.
  refineMaskEdge: async (ctx) => {
    const image = (ctx.inputs.image as string | undefined) ?? null;
    if (!image) throw new Error("Mask Edge Refine needs a connected image input");

    const outputDir = String(ctx.params.output_dir ?? "").trim() || (await getOutputDir());
    const result = await refineMaskEdge({
      image,
      mask: (ctx.inputs.mask as string | undefined) || undefined,
      background: (ctx.inputs.background as string | undefined) || undefined,
      placeholderMask: (ctx.inputs.placeholder_mask as string | undefined) || undefined,
      preset: String(ctx.params.preset ?? "natural") || undefined,
      erodePx: Number(ctx.params.erode_px ?? 1),
      dilatePx: Number(ctx.params.dilate_px ?? 0),
      featherPx: Number(ctx.params.feather_px ?? 4),
      guidedRadius: Number(ctx.params.guided_radius ?? 8),
      edgeDecontaminate: Boolean(ctx.params.edge_decontaminate ?? true),
      backgroundBlendStrength: Number(ctx.params.background_blend_strength ?? 0.4),
      outputDir: outputDir || undefined,
      outputName: String(ctx.params.output_name ?? "").trim() || undefined,
    });
    return {
      refined_image: result.refined_image,
      refined_mask: result.refined_mask,
      edge_report: result.edge_report,
    };
  },

  // Upscales (Lanczos) and sharpens (unsharp mask) the upstream subject to a
  // PSD placeholder's pixel target so it stays crisp at print DPI, via the
  // backend `enhance_image` command (CPU-only in Phase 1). Exposes the enhanced
  // image, the applied scale factor, and an enhance report.
  imageEnhance: async (ctx) => {
    const image = (ctx.inputs.image as string | undefined) ?? null;
    if (!image) throw new Error("Image Enhance needs a connected image input");

    const outputDir = String(ctx.params.output_dir ?? "").trim() || (await getOutputDir());
    const result = await enhanceImage({
      image,
      targetBounds: (ctx.inputs.target_bounds as Bounds | undefined) || undefined,
      mode: String(ctx.params.mode ?? "conservative") || undefined,
      targetWidth: Number(ctx.params.target_width ?? 0),
      targetHeight: Number(ctx.params.target_height ?? 0),
      targetDpi: Number(ctx.params.target_dpi ?? 300),
      maxPixels: Number(ctx.params.max_pixels ?? 48_000_000),
      scale: Number(ctx.params.scale ?? 2),
      denoiseStrength: Number(ctx.params.denoise_strength ?? 0.3),
      textureStrength: Number(ctx.params.texture_strength ?? 0.25),
      preserveTextLogo: Boolean(ctx.params.preserve_text_logo ?? true),
      outputDir: outputDir || undefined,
      outputName: String(ctx.params.output_name ?? "").trim() || undefined,
    });
    return {
      enhanced_image: result.enhanced_image,
      scale_factor: result.scale_factor,
      enhance_report: result.enhance_report,
    };
  },

  // Scans the upstream candidate image for local breakdowns (blur, alpha-rim
  // halos, colour mismatch, below-target resolution) and emits a QualityReport
  // via the backend `detect_quality_issues` command. Phase 1 is detect-only:
  // `fixed_image` is the unchanged input. Exposes the image passthrough, the
  // quality report, an optional issue overlay, and watchdog diagnostics.
  detailWatchdog: async (ctx) => {
    const image = (ctx.inputs.image as string | undefined) ?? null;
    if (!image) throw new Error("Detail Watchdog needs a connected image input");

    const outputDir = String(ctx.params.output_dir ?? "").trim() || (await getOutputDir());
    const result = await detectQualityIssues({
      image,
      visualContext: (ctx.inputs.visual_context as VisualContext | undefined) || undefined,
      targetBounds: (ctx.inputs.target_bounds as Bounds | undefined) || undefined,
      watchTargets: String(ctx.params.watch_targets ?? "").trim() || undefined,
      mode: String(ctx.params.mode ?? "balanced") || undefined,
      outputDir: outputDir || undefined,
      outputName: String(ctx.params.output_name ?? "").trim() || undefined,
    });
    return {
      fixed_image: result.fixed_image,
      quality_report: result.quality_report,
      issue_masks: result.issue_masks,
      watchdog_report: result.watchdog_report,
    };
  },

  // Writes the upstream image into the PSD template's placeholder (true
  // smart-object replacement when possible) and exports the .psd triplet via
  // the backend `compose_psd` command.
  psdExport: async (ctx) => {
    const image = (ctx.inputs.image as string | undefined) ?? null;
    const template = (ctx.inputs.template as string | undefined) ?? null;
    if (!image) throw new Error("PSD Export needs a connected image input");
    if (!template) throw new Error("PSD Export needs a connected PSD template input");

    // Fall back to the configured output directory when none is set on the node.
    const outputDir = String(ctx.params.output_dir ?? "").trim() || (await getOutputDir());
    if (!outputDir) throw new Error("PSD Export needs an output directory");

    const placeholderName = String(ctx.params.placeholder ?? "").trim();
    // Optional refined matte applied as the image's alpha, and any upstream
    // production metadata object merged into the exported _metadata.json.
    const mask = (ctx.inputs.mask as string | undefined) || undefined;
    const metadataInput = ctx.inputs.metadata;
    const metadata =
      metadataInput != null
        ? typeof metadataInput === "string"
          ? metadataInput
          : JSON.stringify(metadataInput)
        : undefined;
    const result = await composePsd({
      template,
      image,
      mask,
      outputDir,
      filename: String(ctx.params.filename ?? "final") || "final",
      placeholder: placeholderName ? JSON.stringify({ name: placeholderName }) : undefined,
      fitMode: (String(ctx.params.fit_mode ?? "contain") as "contain" | "cover" | "stretch"),
      smartObjectMode: (String(ctx.params.smart_object_mode ?? "disable") as "disable" | "replace_content"),
      metadata,
    });
    if (result.status !== "succeeded") {
      throw new Error(`PSD export failed: ${result.status}`);
    }
    return {
      psdPath: result.psd_path,
      previewPath: result.preview_path || null,
      metadataPath: result.metadata_path,
      placeholderKind: result.placeholder_kind,
      smartObjectMode: result.smart_object_mode,
    };
  },
};
