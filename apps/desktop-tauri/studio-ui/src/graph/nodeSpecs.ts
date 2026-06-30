// Node type catalogue. Each node kind declares its typed input/output ports
// and default params. The editor builds handles from this, the runtime reads
// it to wire inputs/outputs, and connection validation uses the port types.

import type { PortDataType, PortSpec } from "./model";

export type ParamControl =
  | "text"
  | "textarea"
  | "number"
  | "select"
  | "slider"
  | "checkbox"
  | "path";

export interface ParamSpec {
  key: string;
  label: string;
  control: ParamControl;
  options?: string[];
  defaultValue?: unknown;
  /** For `slider` / `number`. */
  min?: number;
  max?: number;
  step?: number;
  /** Optional hint shown under the control in the inspector. */
  hint?: string;
  /** Render this param directly on the node card (not just the inspector). */
  inline?: boolean;
  /**
   * Only show this param in the inspector when a sibling param's current value
   * is one of `in`. Lets a node hide irrelevant controls (e.g. show API fields
   * only when `mode === "api"`).
   */
  visibleWhen?: { param: string; in: string[] };
  /** For `path` controls: native file-picker extension filter. */
  pickerFilterName?: string;
  pickerExtensions?: string[];
}

/**
 * Where a node runs — the routing/grouping discriminator.
 * - `graph`  pure in-process node (no backend call).
 * - `local`  always a `python/bridge` CLI; must not touch the network.
 * - `compute` in-process native-Rust image/model work; must not touch the network.
 * - `api`    always a provider call (needs a profile + credentials_ref).
 * - `hybrid` user picks per-node via a `mode` param (e.g. `promptOptimize`).
 * See docs/card-executor-split-and-psd-chain-hardening.md.
 */
export type Executor = "graph" | "local" | "compute" | "api" | "hybrid";

export interface NodeSpec {
  kind: string;
  title: string;
  /** Short description shown in the inspector / node palette. */
  description: string;
  /** Palette grouping. */
  category: "input" | "generate" | "control" | "output" | "utility";
  /** Where the node runs; drives palette local/API grouping + broker routing. */
  executor: Executor;
  inputs: PortSpec[];
  outputs: PortSpec[];
  params: ParamSpec[];
}

function port(id: string, label: string, type: PortDataType): PortSpec {
  return { id, label, type };
}

export const NODE_SPECS: Record<string, NodeSpec> = {
  prompt: {
    kind: "prompt",
    executor: "graph",
    title: "Prompt",
    description: "A text prompt fed into generation nodes.",
    category: "input",
    inputs: [],
    outputs: [port("text", "text", "text")],
    params: [
      {
        key: "text",
        label: "Prompt",
        control: "textarea",
        defaultValue: "",
        inline: true,
      },
    ],
  },
  promptOptimize: {
    kind: "promptOptimize",
    executor: "hybrid",
    title: "Prompt Optimize",
    description:
      "Initial text node. Enter a prompt, then optionally optimize it — `local` applies model-free cleanup/booster presets, `api` rewrites it through an LLM provider profile (local server or cloud). Outputs the (optimized) prompt text.",
    category: "input",
    inputs: [port("text", "text", "text")],
    outputs: [port("text", "text", "text")],
    params: [
      {
        key: "text",
        label: "Prompt",
        control: "textarea",
        defaultValue: "",
        hint: "the initial prompt (a connected `text` input overrides this)",
        inline: true,
      },
      {
        key: "mode",
        label: "Optimize",
        control: "select",
        options: ["off", "local", "api"],
        defaultValue: "off",
        hint: "off = pass through · local = rule-based · api = LLM via profile",
        inline: true,
      },
      {
        key: "preset",
        label: "Local preset",
        control: "select",
        options: ["cleanup", "photographic", "anime", "cinematic", "detailed"],
        defaultValue: "cleanup",
        hint: "used by `local` mode: dedupe + append booster tags",
        visibleWhen: { param: "mode", in: ["local"] },
      },
      {
        key: "provider",
        label: "Provider",
        control: "text",
        defaultValue: "openai_compatible",
        hint: "used by `api` mode (set automatically when you pick a profile)",
        visibleWhen: { param: "mode", in: ["api"] },
      },
      {
        key: "model",
        label: "Model",
        control: "text",
        defaultValue: "",
        hint: "used by `api` mode",
        visibleWhen: { param: "mode", in: ["api"] },
      },
      {
        key: "instruction",
        label: "Instruction",
        control: "textarea",
        defaultValue:
          "Rewrite the text below into a single high-quality English image-generation prompt. Keep the original intent, add useful visual detail, and output only the prompt.",
        hint: "used by `api` mode (sent as the system prompt)",
        visibleWhen: { param: "mode", in: ["api"] },
      },
      {
        key: "credentials_ref",
        label: "Credentials",
        control: "text",
        defaultValue: "",
        hint: "used by `api` mode (set automatically when you pick a profile)",
        visibleWhen: { param: "mode", in: ["api"] },
      },
      {
        key: "temperature",
        label: "Temperature",
        control: "number",
        defaultValue: "",
        min: 0,
        max: 2,
        step: 0.1,
        hint: "used by `api` mode (optional): sampling randomness, leave blank for provider default",
        visibleWhen: { param: "mode", in: ["api"] },
      },
      {
        key: "max_tokens",
        label: "Max tokens",
        control: "number",
        defaultValue: "",
        min: 1,
        step: 1,
        hint: "used by `api` mode (optional): cap the optimized prompt length",
        visibleWhen: { param: "mode", in: ["api"] },
      },
      {
        key: "seed",
        label: "Seed",
        control: "number",
        defaultValue: "",
        step: 1,
        hint: "used by `api` mode (optional): fix for reproducible output",
        visibleWhen: { param: "mode", in: ["api"] },
      },
    ],
  },
  batch: {
    kind: "batch",
    executor: "graph",
    title: "Batch",
    description:
      "Sweeps a list of text items (one per line). A normal Run emits the first item; use \"Run ×N\" to fan out one run per item.",
    category: "input",
    inputs: [],
    outputs: [port("item", "item", "text")],
    params: [
      {
        key: "items",
        label: "Items (one per line)",
        control: "textarea",
        defaultValue: "",
        hint: "one prompt / value per line",
        inline: true,
      },
    ],
  },
  imageSource: {
    kind: "imageSource",
    executor: "graph",
    title: "Image Source",
    description: "An image file on disk, used as a reference / input image.",
    category: "input",
    inputs: [],
    outputs: [port("image", "image", "image")],
    params: [
      {
        key: "path",
        label: "Image path",
        control: "path",
        defaultValue: "",
        hint: "absolute path to an image file",
        inline: true,
        pickerFilterName: "Images",
        pickerExtensions: ["png", "jpg", "jpeg", "webp", "gif", "bmp", "tiff"],
      },
    ],
  },
  psdTemplate: {
    kind: "psdTemplate",
    executor: "graph",
    title: "PSD Template",
    description: "A .psd template path carried through to export.",
    category: "input",
    inputs: [],
    outputs: [port("template", "template", "any")],
    params: [
      {
        key: "path",
        label: "Template path",
        control: "path",
        defaultValue: "",
        hint: "absolute path to a .psd template",
        inline: true,
        pickerFilterName: "PSD",
        pickerExtensions: ["psd"],
      },
    ],
  },
  number: {
    kind: "number",
    executor: "graph",
    title: "Number",
    description: "A numeric value (seed, count, …) fed into other nodes.",
    category: "input",
    inputs: [],
    outputs: [port("value", "value", "number")],
    params: [
      { key: "value", label: "Value", control: "number", defaultValue: 0, inline: true },
    ],
  },
  generate: {
    kind: "generate",
    executor: "api",
    title: "Generate",
    description:
      "Run an image generation operation through the H-Gripe broker.",
    category: "generate",
    inputs: [
      port("prompt", "prompt", "text"),
      port("reference", "reference", "image"),
      port("seed", "seed", "number"),
    ],
    outputs: [port("image", "image", "image")],
    params: [
      { key: "provider", label: "Provider", control: "text", defaultValue: "mock" },
      {
        key: "operation",
        label: "Operation",
        control: "select",
        options: ["image.generate", "image.edit", "echo"],
        defaultValue: "image.generate",
        inline: true,
      },
      { key: "model", label: "Model", control: "text", defaultValue: "" },
      { key: "size", label: "Size", control: "text", defaultValue: "1024x1024" },
      {
        key: "steps",
        label: "Steps",
        control: "slider",
        defaultValue: 20,
        min: 1,
        max: 50,
        step: 1,
        inline: true,
      },
      {
        key: "seed",
        label: "Seed",
        control: "number",
        defaultValue: 0,
        hint: "overridden by a connected seed input",
      },
      {
        key: "credentials_ref",
        label: "Credentials",
        control: "text",
        defaultValue: "",
        hint: "set automatically when you pick a profile",
      },
    ],
  },
  compare: {
    kind: "compare",
    executor: "graph",
    title: "Compare",
    description:
      "Compares two values and emits 1 (true) or 0 (false). Numeric when both sides parse as numbers, else string comparison. Wire `result` into an If's `cond`.",
    category: "control",
    inputs: [port("a", "a", "any"), port("b", "b", "any")],
    outputs: [port("result", "result", "number")],
    params: [
      {
        key: "op",
        label: "Operator",
        control: "select",
        options: ["==", "!=", ">", ">=", "<", "<="],
        defaultValue: "==",
        inline: true,
      },
    ],
  },
  logic: {
    kind: "logic",
    executor: "graph",
    title: "Logic",
    description:
      "Boolean logic on the truthiness of its inputs, emitting 1 (true) or 0 (false). `not` uses only `a`. Wire `result` into an If's `cond`.",
    category: "control",
    inputs: [port("a", "a", "any"), port("b", "b", "any")],
    outputs: [port("result", "result", "number")],
    params: [
      {
        key: "op",
        label: "Operator",
        control: "select",
        options: ["and", "or", "xor", "not"],
        defaultValue: "and",
        inline: true,
      },
    ],
  },
  if: {
    kind: "if",
    executor: "graph",
    title: "If",
    description:
      "Conditional gate: forwards `value` to the `true` or `false` output based on a condition. The branch that is not taken is pruned (its downstream nodes are skipped).",
    category: "control",
    inputs: [port("value", "value", "any"), port("cond", "cond", "any")],
    outputs: [port("true", "true", "any"), port("false", "false", "any")],
    params: [
      {
        key: "cond",
        label: "Condition (when no input wired)",
        control: "select",
        options: ["true", "false"],
        defaultValue: "true",
        hint: "If a `cond` input is connected, its truthiness wins.",
        inline: true,
      },
    ],
  },
  switch: {
    kind: "switch",
    executor: "graph",
    title: "Switch",
    description:
      "Multi-way router: forwards `value` to the output matching `index` (0/1/2), else to `default`. Unselected branches are pruned (skipped).",
    category: "control",
    inputs: [port("value", "value", "any"), port("index", "index", "number")],
    outputs: [
      port("0", "0", "any"),
      port("1", "1", "any"),
      port("2", "2", "any"),
      port("default", "default", "any"),
    ],
    params: [
      {
        key: "index",
        label: "Index (when no input wired)",
        control: "number",
        defaultValue: 0,
        min: 0,
        step: 1,
        inline: true,
      },
    ],
  },
  reroute: {
    kind: "reroute",
    executor: "graph",
    title: "Reroute",
    description:
      "Pass-through relay: forwards its input unchanged. Use it to tidy long edges and route wires around the canvas.",
    category: "utility",
    inputs: [port("in", "in", "any")],
    outputs: [port("out", "out", "any")],
    params: [],
  },
  preview: {
    kind: "preview",
    executor: "graph",
    title: "Preview",
    description:
      "Display a thumbnail of an image. The original path is preserved for export.",
    category: "output",
    inputs: [port("image", "image", "image")],
    outputs: [],
    params: [],
  },
  save: {
    kind: "save",
    executor: "graph",
    title: "Export",
    description:
      "Sink node: collects the resulting image path (and optional PSD template) for export.",
    category: "output",
    inputs: [
      port("image", "image", "image"),
      port("template", "template", "any"),
    ],
    outputs: [],
    params: [
      { key: "filename", label: "File name", control: "text", defaultValue: "output.png", inline: true },
    ],
  },
  psdContextAnalyze: {
    kind: "psdContextAnalyze",
    executor: "local",
    title: "PSD Context Analyze",
    description:
      "Read a PSD template into a structured visual context: background colour & lighting heuristics, placeholder geometry + safe area, a placeholder mask & background preview, and a prompt suffix for downstream generation.",
    category: "input",
    inputs: [port("template", "template", "any")],
    outputs: [
      port("visual_context", "visual context", "any"),
      port("prompt_suffix", "prompt suffix", "text"),
      port("background_image", "background", "image"),
      port("placeholder_mask", "placeholder mask", "image"),
      port("placeholder_bounds", "placeholder bounds", "any"),
    ],
    params: [
      {
        key: "psd_path",
        label: "PSD path",
        control: "path",
        defaultValue: "",
        hint: "used when no PSD Template node is connected",
        pickerFilterName: "PSD",
        pickerExtensions: ["psd"],
      },
      {
        key: "background_layer",
        label: "Background layer",
        control: "text",
        defaultValue: "",
        hint: "layer to sample (empty = composite the whole PSD)",
        inline: true,
      },
      {
        key: "target_placeholder",
        label: "Placeholder layer",
        control: "text",
        defaultValue: "",
        hint: "placeholder to measure (empty = whole canvas)",
        inline: true,
      },
      {
        key: "reference_layers",
        label: "Reference layers",
        control: "textarea",
        defaultValue: "",
        hint: "one layer name per line (advisory in Phase 1)",
      },
      {
        key: "output_dir",
        label: "Output dir",
        control: "path",
        defaultValue: "",
        hint: "leave empty to use the configured output directory",
      },
    ],
  },
  matchLightColor: {
    kind: "matchLightColor",
    executor: "local",
    title: "Light & Color Match",
    description:
      "Nudge a generated subject's light & colour toward the PSD background so the composite stops looking pasted-on: Reinhard Lab transfer / histogram match weighted toward shadows & highlights, sparing brand colours. Emits the matched image, a match report, and a prompt suffix.",
    category: "control",
    inputs: [
      port("image", "image", "image"),
      port("visual_context", "visual context", "any"),
      port("background", "background", "image"),
      port("mask", "mask", "image"),
    ],
    outputs: [
      port("matched_image", "matched image", "image"),
      port("match_report", "match report", "any"),
      port("prompt_suffix", "prompt suffix", "text"),
    ],
    params: [
      {
        key: "mode",
        label: "Mode",
        control: "select",
        options: ["prompt_only", "color_transfer", "histogram_match", "hybrid"],
        defaultValue: "color_transfer",
        inline: true,
      },
      {
        key: "strength",
        label: "Strength",
        control: "slider",
        min: 0,
        max: 1,
        step: 0.05,
        defaultValue: 0.6,
        inline: true,
        visibleWhen: { param: "mode", in: ["color_transfer", "histogram_match", "hybrid"] },
      },
      {
        key: "shadow_strength",
        label: "Shadow strength",
        control: "slider",
        min: 0,
        max: 1,
        step: 0.05,
        defaultValue: 0,
        hint: "extra correction weight in shadows",
        visibleWhen: { param: "mode", in: ["color_transfer", "histogram_match", "hybrid"] },
      },
      {
        key: "highlight_strength",
        label: "Highlight strength",
        control: "slider",
        min: 0,
        max: 1,
        step: 0.05,
        defaultValue: 0,
        hint: "extra correction weight in highlights",
        visibleWhen: { param: "mode", in: ["color_transfer", "histogram_match", "hybrid"] },
      },
      {
        key: "protect_brand_color",
        label: "Protect brand colour",
        control: "checkbox",
        defaultValue: true,
        hint: "damp the shift on high-chroma (brand) pixels so logos/packaging keep their colour",
        visibleWhen: { param: "mode", in: ["color_transfer", "histogram_match", "hybrid"] },
      },
      {
        key: "protect_saturation",
        label: "Protect saturation",
        control: "checkbox",
        defaultValue: false,
        hint: "match luminance only, keeping the subject's own chroma",
        visibleWhen: { param: "mode", in: ["color_transfer", "histogram_match", "hybrid"] },
      },
      {
        key: "output_dir",
        label: "Output dir",
        control: "path",
        defaultValue: "",
        hint: "leave empty to use the configured output directory",
      },
      {
        key: "output_name",
        label: "Output name",
        control: "text",
        defaultValue: "",
        hint: "base name for the matched PNG (empty = <image>_matched)",
        inline: true,
      },
    ],
  },
  subjectMask: {
    kind: "subjectMask",
    executor: "compute",
    title: "Subject Mask / Matte",
    description:
      "Select the subject and produce a mask / cutout / alpha triplet. Phase 1 runs in-process in native Rust (no python bridge): magic-wand flood select + brush/eraser strokes (carried in edit_paths), morphology (grow/shrink, fill holes) and a final feather. Emits the mask, alpha image, cutout, and an enriched matte report. Auto-subject model modes (SAM/RMBG/BiRefNet) are Phase 2.",
    category: "control",
    inputs: [
      port("image", "image", "image"),
      port("reference", "reference", "image"),
      port("visual_context", "visual context", "any"),
      port("placeholder_mask", "placeholder mask", "image"),
      port("previous_mask", "previous mask", "image"),
      port("edit_paths", "edit paths", "any"),
    ],
    outputs: [
      port("mask", "mask", "image"),
      port("alpha_image", "alpha image", "image"),
      port("cutout_image", "cutout image", "image"),
      port("trimap", "trimap", "image"),
      port("matte_report", "matte report", "any"),
      port("edit_paths", "edit paths", "any"),
    ],
    params: [
      {
        key: "mode",
        label: "Mode",
        control: "select",
        options: [
          "hybrid",
          "manual_brush",
          "manual_pen",
          "auto_subject",
          "auto_product",
          "auto_person",
          "auto_transparent_object",
        ],
        defaultValue: "hybrid",
        inline: true,
        hint: "Phase 1 runs the manual_* / hybrid modes; the auto_* model modes are Phase 2",
      },
      {
        key: "wand_tolerance",
        label: "Wand tolerance",
        control: "slider",
        min: 0,
        max: 255,
        step: 1,
        defaultValue: 24,
        hint: "colour distance for the magic-wand flood select",
      },
      {
        key: "grow_px",
        label: "Grow / shrink px",
        control: "slider",
        min: -16,
        max: 16,
        step: 1,
        defaultValue: 0,
        hint: "positive dilates the matte, negative erodes it",
      },
      {
        key: "fill_holes",
        label: "Fill holes",
        control: "checkbox",
        defaultValue: false,
        hint: "close enclosed interior gaps before feather",
      },
      {
        key: "feather_px",
        label: "Feather px",
        control: "slider",
        min: 0,
        max: 16,
        step: 1,
        defaultValue: 0,
        hint: "soften the matte edge (applied last)",
      },
      {
        key: "alpha_matting",
        label: "Alpha matting",
        control: "checkbox",
        defaultValue: false,
        hint: "resolve the binary edge into continuous alpha (hair / glass) via a trimap — ViTMatte when its weight is present, else a deterministic feather fallback",
      },
      {
        key: "matting_band_px",
        label: "Matting band px",
        control: "slider",
        min: 0,
        max: 32,
        step: 1,
        defaultValue: 12,
        hint: "width of the trimap unknown band the matter resolves (only when alpha matting is on)",
      },
      {
        key: "output_dir",
        label: "Output dir",
        control: "path",
        defaultValue: "",
        hint: "leave empty to use the configured output directory",
      },
      {
        key: "output_name",
        label: "Output name",
        control: "text",
        defaultValue: "",
        hint: "base name for the triplet PNGs (empty = <image>_mask)",
        inline: true,
      },
    ],
  },
  refineMaskEdge: {
    kind: "refineMaskEdge",
    executor: "local",
    title: "Mask Edge Refine",
    description:
      "Clean a cut-out subject's matte so it drops into a PSD placeholder without white halos or fringing: erode/dilate morphology, guided-filter edge snapping, feather, and edge colour decontamination. Connect the Subject Mask 'trimap' output to protect its unknown band (hair / fur / glass continuous alpha) from the erode/feather clean-up so fine detail survives. Emits the refined image, refined mask, and an edge report. Presets hide the detail; pick 'custom' to expose every parameter.",
    category: "control",
    inputs: [
      port("image", "image", "image"),
      port("mask", "mask", "image"),
      port("background", "background", "image"),
      port("placeholder_mask", "placeholder mask", "image"),
      port("trimap", "trimap", "image"),
    ],
    outputs: [
      port("refined_image", "refined image", "image"),
      port("refined_mask", "refined mask", "image"),
      port("edge_report", "edge report", "any"),
    ],
    params: [
      {
        key: "preset",
        label: "Preset",
        control: "select",
        options: ["clean", "natural", "soft", "custom"],
        defaultValue: "natural",
        inline: true,
        hint: "clean = tight 1px bite, natural = soft 6px feather, soft = no bite, custom = expose all",
      },
      {
        key: "erode_px",
        label: "Erode px",
        control: "slider",
        min: 0,
        max: 4,
        step: 1,
        defaultValue: 1,
        hint: "bite the matte inward to cut white fringe",
        visibleWhen: { param: "preset", in: ["custom"] },
      },
      {
        key: "dilate_px",
        label: "Dilate px",
        control: "slider",
        min: 0,
        max: 4,
        step: 1,
        defaultValue: 0,
        hint: "grow the matte outward",
        visibleWhen: { param: "preset", in: ["custom"] },
      },
      {
        key: "feather_px",
        label: "Feather px",
        control: "slider",
        min: 0,
        max: 16,
        step: 1,
        defaultValue: 4,
        hint: "soften the edge transition",
        visibleWhen: { param: "preset", in: ["custom"] },
      },
      {
        key: "guided_radius",
        label: "Guided radius",
        control: "slider",
        min: 0,
        max: 16,
        step: 1,
        defaultValue: 8,
        hint: "snap the matte to luminance edges (0 disables)",
        visibleWhen: { param: "preset", in: ["custom"] },
      },
      {
        key: "edge_decontaminate",
        label: "Edge decontaminate",
        control: "checkbox",
        defaultValue: true,
        hint: "bleed opaque subject colour into the edge band to kill residual fringe",
        visibleWhen: { param: "preset", in: ["custom"] },
      },
      {
        key: "background_blend_strength",
        label: "Background blend",
        control: "slider",
        min: 0,
        max: 1,
        step: 0.05,
        defaultValue: 0.4,
        hint: "blend the edge band toward the connected background colour",
        visibleWhen: { param: "preset", in: ["custom"] },
      },
      {
        key: "output_dir",
        label: "Output dir",
        control: "path",
        defaultValue: "",
        hint: "leave empty to use the configured output directory",
      },
      {
        key: "output_name",
        label: "Output name",
        control: "text",
        defaultValue: "",
        hint: "base name for the refined PNGs (empty = <image>_refined)",
        inline: true,
      },
    ],
  },
  imageEnhance: {
    kind: "imageEnhance",
    executor: "local",
    title: "Image Enhance",
    description:
      "Upscale (Lanczos) and sharpen (unsharp mask) a low-resolution subject so it fills a PSD placeholder crisply at print DPI. Connect placeholder bounds to auto-size, or set explicit target pixels. CPU-only in Phase 1 (no GPU super-resolution). Emits the enhanced image, the applied scale factor, and an enhance report. Presets hide the detail; pick 'custom' to expose denoise/texture/scale.",
    category: "control",
    inputs: [
      port("image", "image", "image"),
      port("target_bounds", "target bounds", "any"),
    ],
    outputs: [
      port("enhanced_image", "enhanced image", "image"),
      port("scale_factor", "scale factor", "number"),
      port("enhance_report", "enhance report", "any"),
    ],
    params: [
      {
        key: "mode",
        label: "Mode",
        control: "select",
        options: ["conservative", "texture_rebuild", "print_ready", "custom"],
        defaultValue: "conservative",
        inline: true,
        hint: "conservative = gentle, texture_rebuild = strong detail, print_ready = balanced, custom = expose sliders",
      },
      {
        key: "engine",
        label: "Engine",
        control: "select",
        options: ["cpu", "realesrgan"],
        defaultValue: "cpu",
        inline: true,
        hint: "cpu = built-in Lanczos+sharpen (always available); realesrgan = opt-in GPU/CPU model, falls back to cpu when its weight/deps are missing",
      },
      {
        key: "target_width",
        label: "Target width",
        control: "slider",
        min: 0,
        max: 8192,
        step: 1,
        defaultValue: 0,
        hint: "explicit target px (0 = auto from connected bounds or preset scale)",
      },
      {
        key: "target_height",
        label: "Target height",
        control: "slider",
        min: 0,
        max: 8192,
        step: 1,
        defaultValue: 0,
        hint: "explicit target px (0 = auto from connected bounds or preset scale)",
      },
      {
        key: "target_dpi",
        label: "Target DPI",
        control: "slider",
        min: 72,
        max: 600,
        step: 1,
        defaultValue: 300,
        hint: "DPI written into the output PNG metadata",
      },
      {
        key: "scale",
        label: "Scale",
        control: "slider",
        min: 1,
        max: 8,
        step: 0.25,
        defaultValue: 2,
        hint: "upscale factor when no target size is given",
        visibleWhen: { param: "mode", in: ["custom"] },
      },
      {
        key: "denoise_strength",
        label: "Denoise",
        control: "slider",
        min: 0,
        max: 1,
        step: 0.05,
        defaultValue: 0.3,
        hint: "Gaussian-blur denoise blend before upscaling",
        visibleWhen: { param: "mode", in: ["custom"] },
      },
      {
        key: "texture_strength",
        label: "Texture",
        control: "slider",
        min: 0,
        max: 1,
        step: 0.05,
        defaultValue: 0.25,
        hint: "unsharp-mask detail strength after upscaling",
        visibleWhen: { param: "mode", in: ["custom"] },
      },
      {
        key: "max_pixels",
        label: "Max pixels",
        control: "slider",
        min: 1000000,
        max: 96000000,
        step: 1000000,
        defaultValue: 48000000,
        hint: "cap output pixels; scale is reduced to fit",
        visibleWhen: { param: "mode", in: ["custom"] },
      },
      {
        key: "preserve_text_logo",
        label: "Preserve text/logo",
        control: "checkbox",
        defaultValue: true,
        hint: "cap sharpening so logos / packaging text are not mangled",
      },
      {
        key: "output_dir",
        label: "Output dir",
        control: "path",
        defaultValue: "",
        hint: "leave empty to use the configured output directory",
      },
      {
        key: "output_name",
        label: "Output name",
        control: "text",
        defaultValue: "",
        hint: "base name for the enhanced PNG (empty = <image>_enhanced)",
        inline: true,
      },
    ],
  },
  detailWatchdog: {
    kind: "detailWatchdog",
    executor: "local",
    title: "Detail Watchdog",
    description:
      "Scan a candidate image for local breakdowns (global/region blur, alpha-rim halos, colour mismatch vs the connected background, below-target resolution) and emit a QualityReport so the workflow can decide whether to re-run or hand-fix. Detect-only in Phase 1 (no automatic repaint): 'fixed_image' is the unchanged input. CPU-only (no ML) — semantic targets needing a GPU/VLM (hands/text/logo) are reported skipped. Connect a VisualContext and/or placeholder bounds for the resolution and colour checks.",
    category: "control",
    inputs: [
      port("image", "image", "image"),
      port("visual_context", "visual context", "any"),
      port("target_bounds", "target bounds", "any"),
    ],
    outputs: [
      port("fixed_image", "fixed image", "image"),
      port("quality_report", "quality report", "any"),
      port("issue_masks", "issue masks", "image"),
      port("watchdog_report", "watchdog report", "any"),
    ],
    params: [
      {
        key: "mode",
        label: "Mode",
        control: "select",
        options: ["strict", "balanced", "lenient"],
        defaultValue: "balanced",
        inline: true,
        hint: "detection sensitivity: strict = flags more, lenient = flags less",
      },
      {
        key: "watch_targets",
        label: "Watch targets",
        control: "text",
        defaultValue: "",
        hint: "comma list of face,hands,text,logo,product_edges (empty = all); hands/text/logo are skipped unless an ML engine covers them",
      },
      {
        key: "engine",
        label: "Engine",
        control: "select",
        options: ["rules", "onnx_defect"],
        defaultValue: "rules",
        inline: true,
        hint: "rules = built-in CPU rule layer (always available); onnx_defect = opt-in ML detector for hands/text/logo, falls back to rules when its weight/deps are missing",
      },
      {
        key: "output_dir",
        label: "Output dir",
        control: "path",
        defaultValue: "",
        hint: "leave empty to use the configured output directory",
      },
      {
        key: "output_name",
        label: "Output name",
        control: "text",
        defaultValue: "",
        hint: "base name for the issue-overlay PNG (empty = <image>_issues)",
        inline: true,
      },
    ],
  },
  detailRepaint: {
    kind: "detailRepaint",
    executor: "api",
    title: "Detail Repaint",
    description:
      "Localized repaint of the issue regions a Detail Watchdog flagged. Crops each repaintable issue (suggested_action in 'Repaint actions') with padding, writes an inpaint mask, sends each crop through the broker's image.edit operation (same provider/credentials path as Generate), then pastes the results back with a feathered seam. Outputs the fixed image and a RepaintReport. With no edit-capable provider configured (empty / 'mock') every region is left unrepainted and the image passes through unchanged.",
    category: "control",
    inputs: [
      port("image", "image", "image"),
      port("quality_report", "quality report", "any"),
    ],
    outputs: [
      port("fixed_image", "fixed image", "image"),
      port("repaint_report", "repaint report", "any"),
    ],
    params: [
      {
        key: "provider",
        label: "Provider",
        control: "text",
        defaultValue: "mock",
        hint: "an image.edit-capable provider (set automatically when you pick a profile); empty/mock passes through",
      },
      {
        key: "operation",
        label: "Operation",
        control: "select",
        options: ["image.edit"],
        defaultValue: "image.edit",
        inline: true,
      },
      {
        key: "credentials_ref",
        label: "Credentials",
        control: "text",
        defaultValue: "",
        hint: "set automatically when you pick a profile",
      },
      {
        key: "repaint_prompt_base",
        label: "Repaint prompt",
        control: "text",
        defaultValue: "",
        hint: "base prompt for each region (empty = a generic restore prompt; the issue type is appended)",
      },
      {
        key: "repaint_actions",
        label: "Repaint actions",
        control: "text",
        defaultValue: "detail_redraw",
        hint: "comma list of suggested_action values to repaint",
        inline: true,
      },
      {
        key: "min_confidence",
        label: "Min confidence",
        control: "number",
        defaultValue: 0,
        hint: "only repaint issues at/above this confidence (0..1)",
        inline: true,
      },
      {
        key: "region_padding",
        label: "Region padding",
        control: "number",
        defaultValue: 24,
        hint: "context padding (px) added around each issue box",
        inline: true,
      },
      {
        key: "max_regions",
        label: "Max regions",
        control: "number",
        defaultValue: 8,
        hint: "cap how many regions are repainted (highest confidence first)",
        inline: true,
      },
      {
        key: "feather_px",
        label: "Feather px",
        control: "number",
        defaultValue: 0,
        hint: "seam feather radius (0 = auto from the issue size)",
        inline: true,
      },
      {
        key: "output_dir",
        label: "Output dir",
        control: "path",
        defaultValue: "",
        hint: "leave empty to use the configured output directory",
      },
      {
        key: "output_name",
        label: "Output name",
        control: "text",
        defaultValue: "",
        hint: "base name for the fixed image (empty = <image>_repainted)",
        inline: true,
      },
    ],
  },
  psdExport: {
    kind: "psdExport",
    executor: "local",
    title: "PSD Export",
    description:
      "Write the generated image into a PSD template's placeholder (true smart-object replacement when possible) and export final.psd + preview.png + metadata.json. Accepts an optional refined mask (applied as the image's alpha) and a production metadata object merged into the exported metadata.",
    category: "output",
    inputs: [
      port("image", "image", "image"),
      port("template", "template", "any"),
      port("mask", "mask", "image"),
      port("metadata", "metadata", "any"),
    ],
    outputs: [],
    params: [
      { key: "filename", label: "File name", control: "text", defaultValue: "final", inline: true },
      {
        key: "output_dir",
        label: "Output dir",
        control: "path",
        defaultValue: "",
        hint: "leave empty to use the configured output directory",
      },
      {
        key: "placeholder",
        label: "Placeholder layer",
        control: "text",
        defaultValue: "",
        hint: "template layer name to replace (empty = whole canvas)",
        inline: true,
      },
      {
        key: "fit_mode",
        label: "Fit",
        control: "select",
        options: ["contain", "cover", "stretch"],
        defaultValue: "contain",
        inline: true,
      },
      {
        key: "smart_object_mode",
        label: "Smart object",
        control: "select",
        options: ["disable", "replace_content"],
        defaultValue: "replace_content",
        hint: "replace_content rewrites the smart object (stays editable in Photoshop)",
        inline: true,
      },
    ],
  },
};

export function nodeSpec(kind: string): NodeSpec {
  const spec = NODE_SPECS[kind];
  if (!spec) throw new Error(`unknown node kind: ${kind}`);
  return spec;
}

export function defaultParams(kind: string): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const p of nodeSpec(kind).params) out[p.key] = p.defaultValue ?? "";
  return out;
}

/** Node kinds grouped by palette category, in display order. */
export function paletteGroups(): { category: NodeSpec["category"]; specs: NodeSpec[] }[] {
  const order: NodeSpec["category"][] = ["input", "generate", "control", "utility", "output"];
  return order.map((category) => ({
    category,
    specs: Object.values(NODE_SPECS).filter((s) => s.category === category),
  }));
}
