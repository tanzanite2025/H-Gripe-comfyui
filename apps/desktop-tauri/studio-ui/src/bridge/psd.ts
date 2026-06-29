// PSD Studio integration.
// Reuses the same backend commands the static PSD Studio tab uses, so the node
// editor shares provider profiles and the output directory rather than
// re-implementing them.

import { tauriInvoke } from "./core";
import type { VisualContext } from "../types/production";

// Fields are snake_case to match the Rust `ProviderProfileSummary`.
export interface ProviderProfile {
  profile_ref: string;
  provider?: string | null;
  model?: string | null;
  credentials_ref?: string | null;
  params_count?: number;
}

/** List H-Gripe provider profiles (`get_profiles`). */
export async function listProfiles(): Promise<ProviderProfile[]> {
  const invoke = tauriInvoke();
  if (!invoke) {
    return [
      { profile_ref: "mock-openai", provider: "openai", model: "gpt-image-1", credentials_ref: "openai-key" },
      { profile_ref: "mock-local", provider: "local", model: "sdxl", credentials_ref: null },
    ];
  }
  return (await invoke("get_profiles")) as ProviderProfile[];
}

/** Resolve the configured output directory (`get_runtime_info().output_dir`). */
export async function getOutputDir(): Promise<string> {
  const invoke = tauriInvoke();
  if (!invoke) return "/mock/outputs";
  const info = (await invoke("get_runtime_info")) as { output_dir?: { path?: string } };
  return info.output_dir?.path ?? "";
}

// Fields are snake_case to match the Rust `PsdOutputFile`.
export interface PsdOutput {
  name: string;
  psd_path: string;
  preview_path?: string | null;
  metadata_path?: string | null;
  smart_object?: boolean;
}

/** List `.psd` outputs in a directory (`list_psd_outputs`). */
export async function listPsdOutputs(dir: string): Promise<PsdOutput[]> {
  const invoke = tauriInvoke();
  if (!invoke) {
    return [
      { name: "fox-poster", psd_path: "/mock/outputs/fox-poster.psd", preview_path: "/mock/outputs/fox-poster_preview.png", smart_object: true },
      { name: "banner", psd_path: "/mock/outputs/banner.psd", preview_path: null, smart_object: false },
    ];
  }
  return (await invoke("list_psd_outputs", { dir })) as PsdOutput[];
}

// --- PSD compose / export ---------------------------------------------------
// Wraps the Rust `compose_psd` command, which shells out to the torch-free
// `compose_psd_cli.py` helper to write the generated image into a PSD
// template's placeholder (true smart-object content replacement when possible)
// and export `<filename>.psd` + `_preview.png` + `_metadata.json`.

export interface ComposePsdRequest {
  /** Path to the `.psd` template. */
  template: string;
  /** Path to the generated image to place into the placeholder. */
  image: string;
  /** Directory the exported files are written to. */
  outputDir: string;
  /** Base name for the exported triplet (default `final`). */
  filename?: string;
  /** JSON: `{"name": "<layer>"}` or `{left,top,width,height}`. */
  placeholder?: string;
  fitMode?: "contain" | "cover" | "stretch";
  zOrder?: "above_background" | "placeholder" | "top";
  smartObjectMode?: "disable" | "replace_content";
  hidePlaceholder?: "enable" | "disable";
  /** JSON object merged into the exported metadata. */
  metadata?: string;
  savePreview?: boolean;
}

// Fields are snake_case to match the Rust `ComposePsdResult` serialization.
export interface ComposePsdResult {
  status: string;
  psd_path: string;
  /** Empty string when preview generation was disabled. */
  preview_path: string;
  metadata_path: string;
  placeholder_kind: string | null;
  smart_object_mode: string;
}

/**
 * Compose + export a PSD via the backend (`compose_psd`). Outside Tauri there is
 * no Python/psd-tools pipeline, so this returns a mocked succeeded result so the
 * editor stays runnable in browser dev.
 */
export async function composePsd(req: ComposePsdRequest): Promise<ComposePsdResult> {
  const invoke = tauriInvoke();
  if (!invoke) {
    const base = `${req.outputDir}/${req.filename ?? "final"}`;
    return {
      status: "succeeded",
      psd_path: `${base}.psd`,
      preview_path: req.savePreview === false ? "" : `${base}_preview.png`,
      metadata_path: `${base}_metadata.json`,
      placeholder_kind: req.smartObjectMode === "replace_content" ? "smartobject" : "pixel",
      smart_object_mode: req.smartObjectMode ?? "disable",
    };
  }
  return (await invoke("compose_psd", {
    template: req.template,
    image: req.image,
    outputDir: req.outputDir,
    filename: req.filename ?? null,
    placeholder: req.placeholder ?? null,
    fitMode: req.fitMode ?? null,
    zOrder: req.zOrder ?? null,
    smartObjectMode: req.smartObjectMode ?? null,
    hidePlaceholder: req.hidePlaceholder ?? null,
    metadata: req.metadata ?? null,
    savePreview: req.savePreview ?? null,
  })) as ComposePsdResult;
}

// --- PSD inspection ---------------------------------------------------------
// Wraps the Rust `inspect_psd` command, which shells out to the torch-free
// `inspect_psd_cli.py` helper to read a PSD template's layers via psd-tools.
// Used to validate a real PSD on disk before a run: that the template path
// points at a file, and that a configured placeholder layer name truly exists.

// Fields are snake_case to match the Rust `PsdLayerInfo` serialization.
export interface PsdLayer {
  name: string;
  /** "group" | "smartobject" | "pixel". */
  kind: string;
}

// Fields are snake_case to match the Rust `InspectPsdResult` serialization.
export interface InspectPsdResult {
  status: string;
  /** `false` when the template path does not point at a file on disk. */
  exists: boolean;
  width: number;
  height: number;
  layers: PsdLayer[];
  /** Subset of the requested `names` that were not found in the PSD. */
  missing: string[];
}

/**
 * Inspect a PSD template's layers via the backend (`inspect_psd`). Reading a
 * `.psd` from disk requires the Python/psd-tools pipeline, which only exists in
 * the desktop build, so outside Tauri this resolves to `null` and callers fall
 * back to the syntactic path check.
 */
export async function inspectPsd(
  template: string,
  names?: string[],
): Promise<InspectPsdResult | null> {
  const invoke = tauriInvoke();
  if (!invoke) return null;
  return (await invoke("inspect_psd", {
    template,
    names: names ?? null,
  })) as InspectPsdResult;
}

// --- PSD context analyze ----------------------------------------------------
// Wraps the Rust `analyze_psd_context` command, which shells out to the
// torch-free `analyze_psd_cli.py` helper to distil a PSD template into a
// `VisualContext` (background/lighting heuristics, placeholder geometry, a
// written mask + background preview, and a ready-to-append prompt suffix).

export interface AnalyzePsdRequest {
  /** Path to the `.psd` template. */
  template: string;
  /** Background layer name to sample (empty/omitted: composite the whole PSD). */
  backgroundLayer?: string;
  /** Placeholder layer name (empty/omitted: use the whole canvas). */
  targetPlaceholder?: string;
  /** Reference layer names (advisory in Phase 1). */
  referenceLayers?: string[];
  /** Directory for the written mask + background preview PNGs. */
  outputDir?: string;
}

/**
 * Analyze a PSD template into a {@link VisualContext} via the backend
 * (`analyze_psd_context`). Reading a `.psd` and writing the mask/background
 * previews requires the Python/psd-tools pipeline, which only exists in the
 * desktop build, so outside Tauri this returns a plausible mock so the editor
 * stays runnable in browser dev.
 */
export async function analyzePsdContext(req: AnalyzePsdRequest): Promise<VisualContext> {
  const invoke = tauriInvoke();
  if (!invoke) {
    const base = `${(req.outputDir ?? "/mock/outputs").replace(/\/$/, "")}/template`;
    return {
      background: {
        mean_color: [128, 116, 102],
        dominant_palette: ["#c8b6a0", "#202025", "#f4eee7"],
        brightness: 0.48,
        contrast: 0.72,
        histogram_path: null,
        image_path: `${base}_background.png`,
      },
      lighting: {
        direction: "top-left",
        quality: "hard",
        color_temperature: 5500,
        description: "warm background with hard key light from top-left, color temperature 5500k",
      },
      placeholder: {
        layer_name: req.targetPlaceholder ?? "",
        bounds: { x: 320, y: 180, width: 1024, height: 1400 },
        mask_path: `${base}_placeholder_mask.png`,
        safe_area: { x: 371, y: 250, width: 922, height: 1260 },
      },
      prompt_suffix:
        "matched with the PSD background lighting: hard key light from top-left, " +
        "warm neutral background, color temperature 5500k, realistic contact shadow, " +
        "consistent highlight direction, no floating object",
    };
  }
  return (await invoke("analyze_psd_context", {
    template: req.template,
    backgroundLayer: req.backgroundLayer ?? null,
    targetPlaceholder: req.targetPlaceholder ?? null,
    referenceLayers: req.referenceLayers ?? null,
    outputDir: req.outputDir ?? null,
  })) as VisualContext;
}

// --- Light & Color Match ----------------------------------------------------
// Wraps the Rust `match_light_color` command, which shells out to the
// torch-free `color_match_cli.py` helper to nudge a generated subject's light &
// colour toward a PSD background (Reinhard Lab transfer / histogram match) while
// sparing brand colours, returning the matched image and a report.

/** Mean colour / colour temperature / contrast of the corrected region. */
export interface ColorAppearance {
  mean_color: [number, number, number];
  color_temperature: number;
  contrast: number;
}

/** What `match_light_color` did; snake_case to match the bridge JSON. */
export interface MatchReport {
  mode: string;
  strength: number;
  shadow_strength: number;
  highlight_strength: number;
  protect_saturation: boolean;
  protect_brand_color: boolean;
  /** `false` for `prompt_only`, zero strength, or no background reference. */
  applied: boolean;
  before: ColorAppearance;
  after: ColorAppearance;
  /** Lab mean/std used by the transfer (absent for `histogram_match`). */
  src_mean_lab?: number[];
  dst_mean_lab?: number[];
  src_std_lab?: number[];
  dst_std_lab?: number[];
  note?: string;
  /** `[width, height]` of the written image. */
  output_size?: [number, number];
}

/** Result of the Light & Color Match node (`match_light_color`). */
export interface ColorMatchResult {
  matched_image: string;
  prompt_suffix: string;
  match_report: MatchReport;
}

export interface MatchLightColorRequest {
  /** Path to the subject image to correct. */
  image: string;
  /** Background reference image (the PSD background preview). */
  background?: string;
  /** Optional mask narrowing the corrected region. */
  mask?: string;
  /** Upstream VisualContext, used for the prompt suffix. */
  context?: VisualContext;
  /** `prompt_only | color_transfer | histogram_match | hybrid`. */
  mode?: string;
  /** Match strength 0..1. */
  strength?: number;
  /** Extra correction weight in shadows / highlights, 0..1. */
  shadowStrength?: number;
  highlightStrength?: number;
  /** Match luminance only, keeping the subject's own chroma. */
  protectSaturation?: boolean;
  /** Damp the shift on high-chroma (brand) pixels. */
  protectBrandColor?: boolean;
  /** Directory for the written matched PNG. */
  outputDir?: string;
  /** Base name for the matched PNG. */
  outputName?: string;
}

/**
 * Match a subject image's light & colour to a PSD background via the backend
 * (`match_light_color`). The pixel work needs the Python/Pillow pipeline, which
 * only exists in the desktop build, so outside Tauri this returns a plausible
 * mock so the editor stays runnable in browser dev.
 */
export async function matchLightColor(req: MatchLightColorRequest): Promise<ColorMatchResult> {
  const invoke = tauriInvoke();
  if (!invoke) {
    const dir = (req.outputDir ?? "/mock/outputs").replace(/\/$/, "");
    const mode = req.mode ?? "color_transfer";
    const before: ColorAppearance = { mean_color: [110, 118, 150], color_temperature: 7200, contrast: 0.31 };
    const applied = mode !== "prompt_only" && (req.strength ?? 0.6) > 0;
    return {
      matched_image: `${dir}/${req.outputName ?? "subject_matched"}.png`,
      prompt_suffix:
        req.context?.prompt_suffix ??
        "matched with the PSD background lighting: soft key light from center, " +
          "neutral background, color temperature 5500k, realistic contact shadow, " +
          "consistent highlight direction, no floating object",
      match_report: {
        mode,
        strength: req.strength ?? 0.6,
        shadow_strength: req.shadowStrength ?? 0,
        highlight_strength: req.highlightStrength ?? 0,
        protect_saturation: req.protectSaturation ?? false,
        protect_brand_color: req.protectBrandColor ?? true,
        applied,
        before,
        after: applied ? { mean_color: [150, 138, 120], color_temperature: 5200, contrast: 0.27 } : before,
        output_size: [1024, 1400],
      },
    };
  }
  return (await invoke("match_light_color", {
    image: req.image,
    background: req.background ?? null,
    mask: req.mask ?? null,
    context: req.context ? JSON.stringify(req.context) : null,
    mode: req.mode ?? null,
    strength: req.strength ?? null,
    shadowStrength: req.shadowStrength ?? null,
    highlightStrength: req.highlightStrength ?? null,
    protectSaturation: req.protectSaturation ?? null,
    protectBrandColor: req.protectBrandColor ?? null,
    outputDir: req.outputDir ?? null,
    outputName: req.outputName ?? null,
  })) as ColorMatchResult;
}
