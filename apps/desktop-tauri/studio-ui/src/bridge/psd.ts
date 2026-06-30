// PSD Studio integration.
// Reuses the same backend commands the static PSD Studio tab uses, so the node
// editor shares provider profiles and the output directory rather than
// re-implementing them.

import { tauriInvoke } from "./core";
import type { Bounds, QualityReport, RepaintReport, VisualContext } from "../types/production";

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
  /** Optional matte (e.g. Mask Edge Refine's `refined_mask`) applied as the image's alpha. */
  mask?: string;
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
    mask: req.mask ?? null,
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

// --- Mask Edge Refine -------------------------------------------------------
// Wraps the Rust `refine_mask_edge` command, which shells out to the torch-free
// `edge_refine_cli.py` helper to clean a cut-out subject's matte (erode/dilate
// morphology, guided-filter edge snapping, feather, colour decontamination) so
// it drops into a PSD placeholder without white halos or fringing.

/** What `refine_mask_edge` did; snake_case to match the bridge JSON. */
export interface EdgeReport {
  preset: string;
  /** `explicit` when a mask was connected, else `alpha` (the image's own). */
  source_mask: string;
  erode_px: number;
  dilate_px: number;
  feather_px: number;
  guided_radius: number;
  edge_decontaminate: boolean;
  background_blend_strength: number;
  /** `true` when a background was connected and blended into the edge band. */
  background_applied: boolean;
  /** `true` when a trimap was connected and its unknown band was protected. */
  trimap_applied?: boolean;
  /** Pixels in the protected (unknown) band restored from the source matte. */
  protected_band_px?: number;
  edge_band_px: number;
  coverage_before: number;
  coverage_after: number;
  /** `[width, height]` of the written images. */
  output_size?: [number, number];
}

/** Result of the Mask Edge Refine node (`refine_mask_edge`). */
export interface RefineEdgeResult {
  refined_image: string;
  refined_mask: string;
  edge_report: EdgeReport;
}

export interface RefineMaskEdgeRequest {
  /** Path to the subject image whose matte is refined. */
  image: string;
  /** Explicit matte; defaults to the image's own alpha when omitted. */
  mask?: string;
  /** Target background for edge colour blending. */
  background?: string;
  /** PSD placeholder mask (advisory in Phase 1). */
  placeholderMask?: string;
  /**
   * Matting trimap (FG / unknown / BG levels) from the Subject Mask node. When
   * connected, the unknown band is protected from erode/feather so hair / fur /
   * glass continuous alpha survives the edge clean-up.
   */
  trimap?: string;
  /** `clean | natural | soft | custom`. */
  preset?: string;
  /** Bite N px in / grow N px out (custom preset only). */
  erodePx?: number;
  dilatePx?: number;
  /** Gaussian edge feather radius (custom preset only). */
  featherPx?: number;
  /** Guided-filter radius, 0 disables (custom preset only). */
  guidedRadius?: number;
  /** Pull opaque subject colour into the edge band (custom preset only). */
  edgeDecontaminate?: boolean;
  /** Blend the edge band toward the target background 0..1 (custom only). */
  backgroundBlendStrength?: number;
  /** Directory for the written PNGs. */
  outputDir?: string;
  /** Base name for the written PNGs. */
  outputName?: string;
}

/**
 * Refine a cut-out subject's mask edges for PSD compositing via the backend
 * (`refine_mask_edge`). The pixel work needs the Python/Pillow pipeline, which
 * only exists in the desktop build, so outside Tauri this returns a plausible
 * mock so the editor stays runnable in browser dev.
 */
export async function refineMaskEdge(req: RefineMaskEdgeRequest): Promise<RefineEdgeResult> {
  const invoke = tauriInvoke();
  if (!invoke) {
    const dir = (req.outputDir ?? "/mock/outputs").replace(/\/$/, "");
    const stem = req.outputName ?? "subject_refined";
    const preset = req.preset ?? "natural";
    const custom = preset === "custom";
    const background = (req.background ?? "").trim().length > 0;
    const blend = custom ? (req.backgroundBlendStrength ?? 0.4) : 0.4;
    return {
      refined_image: `${dir}/${stem}.png`,
      refined_mask: `${dir}/${stem}_mask.png`,
      edge_report: {
        preset,
        source_mask: (req.mask ?? "").trim().length > 0 ? "explicit" : "alpha",
        erode_px: custom ? (req.erodePx ?? 1) : 1,
        dilate_px: custom ? (req.dilatePx ?? 0) : 0,
        feather_px: custom ? (req.featherPx ?? 4) : 6,
        guided_radius: custom ? (req.guidedRadius ?? 8) : 8,
        edge_decontaminate: custom ? (req.edgeDecontaminate ?? true) : preset !== "soft",
        background_blend_strength: blend,
        background_applied: background && blend > 0,
        trimap_applied: (req.trimap ?? "").trim().length > 0,
        protected_band_px: (req.trimap ?? "").trim().length > 0 ? 2048 : 0,
        edge_band_px: 4096,
        coverage_before: 0.44,
        coverage_after: 0.4,
        output_size: [1024, 1400],
      },
    };
  }
  return (await invoke("refine_mask_edge", {
    image: req.image,
    mask: req.mask ?? null,
    background: req.background ?? null,
    placeholderMask: req.placeholderMask ?? null,
    trimap: req.trimap ?? null,
    preset: req.preset ?? null,
    erodePx: req.erodePx ?? null,
    dilatePx: req.dilatePx ?? null,
    featherPx: req.featherPx ?? null,
    guidedRadius: req.guidedRadius ?? null,
    edgeDecontaminate: req.edgeDecontaminate ?? null,
    backgroundBlendStrength: req.backgroundBlendStrength ?? null,
    outputDir: req.outputDir ?? null,
    outputName: req.outputName ?? null,
  })) as RefineEdgeResult;
}

// --- Image Enhance ----------------------------------------------------------
// Wraps the Rust `enhance_image` command, which shells out to the torch-free
// `image_enhance_cli.py` helper to upscale (Lanczos) and sharpen (unsharp mask)
// a low-resolution subject to a PSD placeholder's pixel target so it stays crisp
// at print DPI. CPU-only in Phase 1; GPU super-resolution is a future backend.

/** What `enhance_image` did; snake_case to match the bridge JSON. */
export interface EnhanceReport {
  /** `conservative | texture_rebuild | print_ready | custom`. */
  mode: string;
  scale_factor: number;
  /** `[width, height]` of the input image. */
  source_size?: [number, number];
  /** `[width, height]` of the written image. */
  output_size?: [number, number];
  /** `[width, height]` requested target, or null when a preset scale was used. */
  target_size?: [number, number] | null;
  target_dpi: number;
  max_pixels: number;
  /** `true` when the scale was reduced to honour `max_pixels`. */
  clamped: boolean;
  denoise_strength: number;
  texture_strength: number;
  preserve_text_logo: boolean;
  /** Upscale engine actually used (`cpu` or a backend id, e.g. `realesrgan`). */
  engine?: string;
  /** Engine the node asked for (differs from `engine` on fallback). */
  engine_requested?: string;
  /** Why the requested engine was not used (missing deps/weight, downscale, …). */
  engine_fallback_reason?: string | null;
  /** Weight file name when a model backend ran, else null. */
  backend_model?: string | null;
  processing_time_ms: number;
}

/** Result of the Image Enhance node (`enhance_image`). */
export interface EnhanceImageResult {
  enhanced_image: string;
  scale_factor: number;
  enhance_report: EnhanceReport;
}

export interface EnhanceImageRequest {
  /** Path to the low-resolution base image. */
  image: string;
  /** Connected PSD placeholder bounds {x,y,width,height}; sets the target size. */
  targetBounds?: Bounds;
  /** `conservative | texture_rebuild | print_ready | custom`. */
  mode?: string;
  /** Explicit target px (0 = auto from bounds / preset scale). */
  targetWidth?: number;
  targetHeight?: number;
  /** DPI written into the output PNG metadata. */
  targetDpi?: number;
  /** Cap on output pixels; the scale is reduced to fit (0 disables). */
  maxPixels?: number;
  /** Upscale factor used when no target size is given (custom only). */
  scale?: number;
  /** Gaussian-blur denoise blend 0..1 (custom only). */
  denoiseStrength?: number;
  /** Unsharp-mask detail strength 0..1 (custom only). */
  textureStrength?: number;
  /** Cap sharpening so logos / packaging text are not mangled. */
  preserveTextLogo?: boolean;
  /** Upscale engine: `cpu` (default) or `realesrgan` (opt-in, falls back to cpu). */
  engine?: string;
  /** Directory for the written PNG. */
  outputDir?: string;
  /** Base name for the written PNG. */
  outputName?: string;
}

/**
 * Upscale and sharpen a subject image for PSD placement via the backend
 * (`enhance_image`). The pixel work needs the Python/Pillow pipeline, which only
 * exists in the desktop build, so outside Tauri this returns a plausible mock so
 * the editor stays runnable in browser dev.
 */
export async function enhanceImage(req: EnhanceImageRequest): Promise<EnhanceImageResult> {
  const invoke = tauriInvoke();
  if (!invoke) {
    const dir = (req.outputDir ?? "/mock/outputs").replace(/\/$/, "");
    const stem = req.outputName ?? "subject_enhanced";
    const mode = req.mode ?? "conservative";
    const custom = mode === "custom";
    const presetScale: Record<string, number> = {
      conservative: 2.0,
      texture_rebuild: 2.0,
      print_ready: 2.0,
      custom: req.scale ?? 2.0,
    };
    const src: [number, number] = [512, 700];
    // Resolve the target the same way the CLI does: explicit px > bounds > scale.
    let targetW = req.targetWidth ?? 0;
    let targetH = req.targetHeight ?? 0;
    if (targetW <= 0 && targetH <= 0 && req.targetBounds) {
      targetW = req.targetBounds.width;
      targetH = req.targetBounds.height;
    }
    const hasTarget = targetW > 0 || targetH > 0;
    let scale = hasTarget
      ? Math.max(targetW > 0 ? targetW / src[0] : 0, targetH > 0 ? targetH / src[1] : 0)
      : presetScale[mode] ?? 2.0;
    const maxPixels = req.maxPixels ?? 48_000_000;
    let clamped = false;
    if (maxPixels > 0 && src[0] * scale * (src[1] * scale) > maxPixels) {
      scale *= Math.sqrt(maxPixels / (src[0] * scale * (src[1] * scale)));
      clamped = true;
    }
    const out: [number, number] = [Math.round(src[0] * scale), Math.round(src[1] * scale)];
    let texture = custom ? (req.textureStrength ?? 0.25) : { conservative: 0.25, texture_rebuild: 0.7, print_ready: 0.5 }[mode] ?? 0.25;
    const preserveTextLogo = req.preserveTextLogo ?? true;
    if (preserveTextLogo) texture = Math.min(texture, 0.4);
    const scaleFactor = Math.round((out[0] / src[0]) * 1e4) / 1e4;
    return {
      enhanced_image: `${dir}/${stem}.png`,
      scale_factor: scaleFactor,
      enhance_report: {
        mode,
        scale_factor: scaleFactor,
        source_size: src,
        output_size: out,
        target_size: hasTarget ? [targetW, targetH] : null,
        target_dpi: req.targetDpi ?? 300,
        max_pixels: maxPixels,
        clamped,
        denoise_strength: custom ? (req.denoiseStrength ?? 0.3) : { conservative: 0.3, texture_rebuild: 0.15, print_ready: 0.2 }[mode] ?? 0.3,
        texture_strength: texture,
        preserve_text_logo: preserveTextLogo,
        engine: "cpu",
        engine_requested: req.engine ?? "cpu",
        engine_fallback_reason:
          (req.engine ?? "cpu") === "cpu" ? null : "engine unavailable in browser dev mock",
        backend_model: null,
        processing_time_ms: 0,
      },
    };
  }
  return (await invoke("enhance_image", {
    image: req.image,
    targetBounds: req.targetBounds ? JSON.stringify(req.targetBounds) : null,
    mode: req.mode ?? null,
    targetWidth: req.targetWidth ?? null,
    targetHeight: req.targetHeight ?? null,
    targetDpi: req.targetDpi ?? null,
    maxPixels: req.maxPixels ?? null,
    scale: req.scale ?? null,
    denoiseStrength: req.denoiseStrength ?? null,
    textureStrength: req.textureStrength ?? null,
    preserveTextLogo: req.preserveTextLogo ?? null,
    engine: req.engine ?? null,
    outputDir: req.outputDir ?? null,
    outputName: req.outputName ?? null,
  })) as EnhanceImageResult;
}

// --- Detail Watchdog --------------------------------------------------------
// Wraps the Rust `detect_quality_issues` command, which shells out to the
// torch-free `detail_watchdog_cli.py` helper to scan a candidate image for
// local breakdowns (global/region blur, alpha-rim halos, colour mismatch,
// below-target resolution) and emit a {@link QualityReport}. Phase 1 is
// detect-only (no automatic repaint) and CPU-only (Pillow + numpy, no ML), so
// semantic targets that need a GPU/VLM (hands/text/logo) are reported skipped.

/** Diagnostics for a Detail Watchdog run; snake_case to match the bridge JSON. */
export interface WatchdogReport {
  /** `strict | balanced | lenient`. */
  mode: string;
  /** Targets that actually ran. */
  watch_targets: string[];
  /** Requested targets skipped in Phase 1 (need the GPU/VLM backend). */
  skipped_targets: string[];
  /** `[width, height]` of the analysed image. */
  image_size?: [number, number];
  /** `[width, height]` of the connected placeholder target, when available. */
  target_size?: [number, number] | null;
  /** Laplacian-variance sharpness of the whole image (higher = sharper). */
  global_sharpness: number;
  /** Detection engine that actually ran: `rules` (CPU baseline) or an ML id. */
  engine?: string;
  /** Engine the node asked for (may differ from `engine` on fallback). */
  engine_requested?: string;
  /** Why the rule-only path was used when an ML engine could not run; else null. */
  engine_fallback_reason?: string | null;
  /** Learned detector passes that ran on top of the rule layer. */
  detectors?: string[];
  /** File name of the weight the ML detector loaded, when one ran. */
  backend_model?: string | null;
}

/** Result of the Detail Watchdog node (`detect_quality_issues`). */
export interface DetectQualityResult {
  /** Phase 1 passthrough of the input image (never repainted). */
  fixed_image: string;
  quality_report: QualityReport;
  /** Issue-overlay PNG path, or null when nothing was flagged / overlay off. */
  issue_masks: string | null;
  watchdog_report: WatchdogReport;
}

export interface DetectQualityRequest {
  /** Path to the candidate image to inspect. */
  image: string;
  /** Connected VisualContext (background colour + placeholder bounds). */
  visualContext?: VisualContext;
  /** Connected PSD placeholder bounds {x,y,width,height}. */
  targetBounds?: Bounds;
  /** Comma list of `face,hands,text,logo,product_edges` (empty = all). */
  watchTargets?: string;
  /** `strict | balanced | lenient` detection sensitivity. */
  mode?: string;
  /** Detection engine: `rules` (default CPU layer) or an opt-in ML detector id. */
  engine?: string;
  /** Directory for the written overlay PNG. */
  outputDir?: string;
  /** Base name for the written overlay PNG. */
  outputName?: string;
}

const _WATCHDOG_ALL_TARGETS = ["face", "hands", "text", "logo", "product_edges"];
const _WATCHDOG_UNSUPPORTED = ["hands", "text", "logo"];
const _WATCHDOG_COLOR_DELTA: Record<string, number> = {
  strict: 28,
  balanced: 40,
  lenient: 55,
};

function parseWatchTargets(raw?: string): string[] {
  const requested = (raw ?? "")
    .split(",")
    .map((t) => t.trim().toLowerCase())
    .filter(Boolean);
  const pool = requested.length > 0 ? requested : _WATCHDOG_ALL_TARGETS;
  return _WATCHDOG_ALL_TARGETS.filter((t) => pool.includes(t));
}

/**
 * Scan a candidate image for quality breakdowns via the backend
 * (`detect_quality_issues`). The pixel analysis needs the Python/Pillow
 * pipeline, which only exists in the desktop build, so outside Tauri this
 * returns a plausible mock so the editor stays runnable in browser dev.
 */
export async function detectQualityIssues(
  req: DetectQualityRequest,
): Promise<DetectQualityResult> {
  const invoke = tauriInvoke();
  if (!invoke) {
    const mode = req.mode ?? "balanced";
    const targets = parseWatchTargets(req.watchTargets);
    const skipped = targets.filter((t) => _WATCHDOG_UNSUPPORTED.includes(t));
    const src: [number, number] = [512, 700];
    const issues: QualityReport["issues"] = [];

    // Below-target resolution: the connected placeholder is larger than source.
    let target: [number, number] | null = null;
    if (req.targetBounds && (req.targetBounds.width > 0 || req.targetBounds.height > 0)) {
      target = [req.targetBounds.width, req.targetBounds.height];
    } else if (req.visualContext?.placeholder?.bounds) {
      const b = req.visualContext.placeholder.bounds;
      if (b.width > 0 || b.height > 0) target = [b.width, b.height];
    }
    if (target && (src[0] < target[0] * 0.9 || src[1] < target[1] * 0.9)) {
      issues.push({
        type: "low_resolution",
        confidence: 0.8,
        bbox: [0, 0, src[0], src[1]],
        suggested_action: "image_enhance",
      });
    }

    // Colour mismatch: mock subject mean vs connected background mean.
    const bg = req.visualContext?.background?.mean_color;
    if (bg && bg.length === 3) {
      const subject = [180, 170, 160];
      const delta = Math.sqrt(
        bg.reduce((acc, v, i) => acc + (v - subject[i]) ** 2, 0),
      );
      if (delta > (_WATCHDOG_COLOR_DELTA[mode] ?? 40)) {
        issues.push({
          type: "color_mismatch",
          confidence: Math.min(0.99, Math.round((delta / 100) * 100) / 100),
          bbox: [0, 0, src[0], src[1]],
          suggested_action: "color_match",
        });
      }
    }

    const status = issues.length === 0 ? "passed" : issues.length >= 2 ? "failed" : "warning";
    const dir = (req.outputDir ?? "/mock/outputs").replace(/\/$/, "");
    const stem = req.outputName ?? "candidate_issues";
    return {
      fixed_image: req.image,
      quality_report: { status, issues },
      issue_masks: issues.length > 0 ? `${dir}/${stem}.png` : null,
      watchdog_report: {
        mode,
        watch_targets: targets,
        skipped_targets: skipped,
        image_size: src,
        target_size: target,
        global_sharpness: 142.0,
        engine: "rules",
        engine_requested: req.engine ?? "rules",
        engine_fallback_reason:
          (req.engine ?? "rules") === "rules"
            ? null
            : "ML detector unavailable in browser dev (mock)",
        detectors: [],
        backend_model: null,
      },
    };
  }
  return (await invoke("detect_quality_issues", {
    image: req.image,
    visualContext: req.visualContext ? JSON.stringify(req.visualContext) : null,
    targetBounds: req.targetBounds ? JSON.stringify(req.targetBounds) : null,
    watchTargets: req.watchTargets ?? null,
    mode: req.mode ?? null,
    engine: req.engine ?? null,
    outputDir: req.outputDir ?? null,
    outputName: req.outputName ?? null,
  })) as DetectQualityResult;
}

// --- Engine capability probe ------------------------------------------------
// The `doctor`-style cross-card probe behind the opt-in ML `engine` seams. The
// inspector uses it to grey out engines whose optional deps / weights are
// missing on this box (the CPU/`rules` baseline always stays available), and
// the Dashboard surfaces it as a capability report.

/** Availability of one `engine` option (mirrors Rust `EngineAvailability`). */
export interface EngineAvailability {
  available: boolean;
  reason: string;
}

/** Per-card engine probe (mirrors Rust `CardEngineProbe`). */
export interface CardEngineProbe {
  /** Node kind whose `engine` param these cover, e.g. `imageEnhance`. */
  node_kind: string;
  /** Bridge CLI that produced the probe. */
  cli: string;
  /** Engine id -> availability (e.g. `cpu`/`realesrgan`, `rules`/`onnx_defect`). */
  engines: Record<string, EngineAvailability>;
  /** Why the probe could not run, when `engines` is empty. */
  error?: string | null;
}

/** Cross-card engine capability report (mirrors Rust `EngineProbeReport`). */
export interface EngineProbeReport {
  cards: CardEngineProbe[];
  /** Shared weight cache (`HGRIPE_MODEL_CACHE` or the bundled dir). */
  model_cache_dir?: string | null;
}

/**
 * Probe the opt-in ML `engine` seams across local cards (`probe_engines`).
 *
 * Outside the desktop shell (browser preview) there is no Python bridge, so we
 * return an empty report; the inspector then leaves every engine enabled rather
 * than greying options out from a probe that never ran.
 */
export async function probeEngines(): Promise<EngineProbeReport> {
  const invoke = tauriInvoke();
  if (!invoke) {
    return { cards: [], model_cache_dir: null };
  }
  return (await invoke("probe_engines", { dir: null })) as EngineProbeReport;
}

// --- Detail Repaint ---------------------------------------------------------
// The two pixel halves of the Detail Repaint node, wrapping the Rust
// `prepare_repaint_regions` / `composite_repaint` commands (which shell out to
// the torch-free `detail_repaint_cli.py`). `prepare` crops each repaintable
// QualityReport issue + writes an inpaint mask; the orchestrator then sends each
// crop through the broker's `image.edit`; `composite` pastes the repainted crops
// back with a feathered seam. The provider call lives in the executor, not here.

/** A cropped issue region + its inpaint mask (mirrors Rust `PreparedRepaintRegion`). */
export interface PreparedRepaintRegion {
  index: number;
  type?: string | null;
  confidence: number;
  suggested_action?: string | null;
  /** `[x1, y1, x2, y2]` issue box (clamped) in canvas pixels. */
  bbox: [number, number, number, number];
  /** `[x1, y1, x2, y2]` padded crop window in canvas pixels. */
  crop_box: [number, number, number, number];
  /** `[x1, y1, x2, y2]` issue core in the crop's own coordinates. */
  inner_box: [number, number, number, number];
  /** `[width, height]` of the crop / mask PNG. */
  size: [number, number];
  crop_path: string;
  mask_path: string;
}

/** Manifest from the prepare step (mirrors Rust `PrepareRepaintResult`). */
export interface PrepareRepaintResult {
  regions: PreparedRepaintRegion[];
  skipped: unknown[];
  image_size: [number, number];
  selected_count: number;
  /** When true the mask's transparent pixels mark the edit area (OpenAI style). */
  mask_edit_is_transparent: boolean;
}

export interface PrepareRepaintRequest {
  /** Path to the candidate image to repaint. */
  image: string;
  /** QualityReport from Detail Watchdog; its issues drive region selection. */
  qualityReport?: QualityReport;
  /** Comma list of `suggested_action` values to repaint (default detail_redraw). */
  repaintActions?: string;
  /** Only repaint issues at/above this confidence (0..1). */
  minConfidence?: number;
  /** Context padding (px) added around each issue bbox. */
  padding?: number;
  /** Cap how many regions are repainted (highest confidence first). */
  maxRegions?: number;
  /** Mark the edit area opaque/white instead of transparent. */
  invertMask?: boolean;
  outputDir?: string;
  outputName?: string;
}

const _REPAINT_DEFAULT_ACTIONS = ["detail_redraw"];

function _clampBox(
  box: [number, number, number, number],
  width: number,
  height: number,
): [number, number, number, number] {
  let [x1, y1, x2, y2] = box;
  x1 = Math.max(0, Math.min(Math.round(x1), width - 1));
  y1 = Math.max(0, Math.min(Math.round(y1), height - 1));
  x2 = Math.max(x1 + 1, Math.min(Math.round(x2), width));
  y2 = Math.max(y1 + 1, Math.min(Math.round(y2), height));
  return [x1, y1, x2, y2];
}

/**
 * Crop repaintable issue regions + write inpaint masks via the backend
 * (`prepare_repaint_regions`). The pixel work needs the Python/Pillow pipeline,
 * which only exists in the desktop build, so outside Tauri this returns a
 * plausible mock (mirroring the CLI's selection + geometry) so the editor stays
 * runnable in browser dev.
 */
export async function prepareRepaintRegions(
  req: PrepareRepaintRequest,
): Promise<PrepareRepaintResult> {
  const invoke = tauriInvoke();
  if (!invoke) {
    const dir = (req.outputDir ?? "/mock/outputs").replace(/\/$/, "");
    const stem = req.outputName ?? "candidate_repaint";
    const actions = new Set(
      (req.repaintActions ?? "")
        .split(",")
        .map((a) => a.trim())
        .filter(Boolean),
    );
    if (actions.size === 0) for (const a of _REPAINT_DEFAULT_ACTIONS) actions.add(a);
    const minConfidence = Math.max(0, Math.min(1, req.minConfidence ?? 0));
    const padding = Math.max(0, req.padding ?? 24);
    const maxRegions = Math.max(1, req.maxRegions ?? 8);
    const invert = req.invertMask ?? false;
    const size: [number, number] = [512, 700];

    const issues = req.qualityReport?.issues ?? [];
    const skipped: unknown[] = [];
    const selected: { index: number; issue: QualityReport["issues"][number] }[] = [];
    issues.forEach((issue, index) => {
      if (!Array.isArray(issue.bbox) || issue.bbox.length !== 4) {
        skipped.push({ index, type: issue.type, reason: "no_bbox" });
      } else if (!actions.has(issue.suggested_action)) {
        skipped.push({ index, type: issue.type, reason: "action_not_repaintable" });
      } else if ((issue.confidence ?? 0) < minConfidence) {
        skipped.push({ index, type: issue.type, reason: "below_min_confidence" });
      } else {
        selected.push({ index, issue });
      }
    });
    selected.sort((a, b) => (b.issue.confidence ?? 0) - (a.issue.confidence ?? 0));
    for (const over of selected.splice(maxRegions)) {
      skipped.push({ index: over.index, type: over.issue.type, reason: "over_max_regions" });
    }

    const regions: PreparedRepaintRegion[] = selected.map(({ index, issue }) => {
      const bbox = _clampBox(issue.bbox, size[0], size[1]);
      const cropBox = _clampBox(
        [bbox[0] - padding, bbox[1] - padding, bbox[2] + padding, bbox[3] + padding],
        size[0],
        size[1],
      );
      const inner: [number, number, number, number] = [
        bbox[0] - cropBox[0],
        bbox[1] - cropBox[1],
        bbox[2] - cropBox[0],
        bbox[3] - cropBox[1],
      ];
      return {
        index,
        type: issue.type,
        confidence: Math.round((issue.confidence ?? 0) * 1e4) / 1e4,
        suggested_action: issue.suggested_action,
        bbox,
        crop_box: cropBox,
        inner_box: inner,
        size: [cropBox[2] - cropBox[0], cropBox[3] - cropBox[1]],
        crop_path: `${dir}/${stem}_region${index}.png`,
        mask_path: `${dir}/${stem}_region${index}_mask.png`,
      };
    });

    return {
      regions,
      skipped,
      image_size: size,
      selected_count: regions.length,
      mask_edit_is_transparent: !invert,
    };
  }
  return (await invoke("prepare_repaint_regions", {
    image: req.image,
    qualityReport: req.qualityReport ? JSON.stringify(req.qualityReport) : null,
    repaintActions: req.repaintActions ?? null,
    minConfidence: req.minConfidence ?? null,
    padding: req.padding ?? null,
    maxRegions: req.maxRegions ?? null,
    invertMask: req.invertMask ?? null,
    outputDir: req.outputDir ?? null,
    outputName: req.outputName ?? null,
  })) as PrepareRepaintResult;
}

/** A repainted crop returned by the provider, keyed back to its region index. */
export interface RepaintedCrop {
  index: number;
  path: string;
}

/** Result of the composite step (mirrors Rust `CompositeRepaintResult`). */
export interface CompositeRepaintResult {
  fixed_image: string;
  repaint_report: RepaintReport;
}

export interface CompositeRepaintRequest {
  /** Path to the original candidate image. */
  image: string;
  /** Manifest returned by {@link prepareRepaintRegions}. */
  manifest: PrepareRepaintResult;
  /** Repainted crops returned by the provider (blank list = nothing repainted). */
  repainted: RepaintedCrop[];
  /** Seam feather radius (0 / undefined = auto from the issue size). */
  featherPx?: number;
  outputDir?: string;
  outputName?: string;
}

/**
 * Paste repainted crops back into the candidate with a feathered seam via the
 * backend (`composite_repaint`). The pixel work needs the Python/Pillow
 * pipeline, which only exists in the desktop build, so outside Tauri this
 * returns a plausible mock so the editor stays runnable in browser dev.
 */
export async function compositeRepaint(
  req: CompositeRepaintRequest,
): Promise<CompositeRepaintResult> {
  const invoke = tauriInvoke();
  if (!invoke) {
    const dir = (req.outputDir ?? "/mock/outputs").replace(/\/$/, "");
    const stem = req.outputName ?? "candidate_repainted";
    const done = new Set(
      req.repainted.filter((r) => (r.path ?? "").trim().length > 0).map((r) => r.index),
    );
    const regions = req.manifest.regions ?? [];
    const region_results = regions.map((region) => ({
      index: region.index,
      type: region.type,
      bbox: region.bbox,
      status: done.has(region.index) ? "repainted" : "no_repaint",
      feather_px: done.has(region.index)
        ? Math.max(2, Math.min(24, Math.round(Math.min(...region.size) * 0.06)))
        : null,
    }));
    const repaintedCount = region_results.filter((r) => r.status === "repainted").length;
    const status =
      repaintedCount === 0 ? "unchanged" : repaintedCount === regions.length ? "repainted" : "partial";
    return {
      // With nothing repainted the candidate is unchanged, so echo the input.
      fixed_image: repaintedCount === 0 ? req.image : `${dir}/${stem}.png`,
      repaint_report: {
        status,
        regions: region_results,
        repainted_count: repaintedCount,
        requested_count: regions.length,
        image_size: req.manifest.image_size ?? [0, 0],
      },
    };
  }
  return (await invoke("composite_repaint", {
    image: req.image,
    manifest: JSON.stringify(req.manifest),
    repainted: JSON.stringify(req.repainted),
    featherPx: req.featherPx ?? null,
    outputDir: req.outputDir ?? null,
    outputName: req.outputName ?? null,
  })) as CompositeRepaintResult;
}

// --- Detail Repaint: local inpaint engine seam ------------------------------
// The opt-in Phase-2 alternative to the remote `image.edit` provider loop: a
// local diffusion backend (`inpaint_backends/`) repaints each prepared crop
// offline, returning crops in the same `{index, path}` shape `compositeRepaint`
// reads. `provider` (the default) and any unavailable engine degrade to an
// empty repaint set with `engine_fallback_reason` recorded — the executor then
// runs the provider path / passes the image through.

/** Per-region prompt override for the local inpaint backend. */
export interface InpaintPromptOverride {
  index: number;
  prompt: string;
}

/** Result of the local inpaint step (mirrors Rust `LocalInpaintResult`). */
export interface LocalInpaintResult {
  repainted: RepaintedCrop[];
  /** Engine that actually ran (`provider` when it fell back). */
  engine: string;
  /** Engine the node asked for. */
  engine_requested: string;
  /** Why it degraded to the provider path, when it did. */
  engine_fallback_reason?: string | null;
  /** Short identifier of the resolved weight, for telemetry. */
  backend_model?: string | null;
  requested_count: number;
  repainted_count: number;
}

export interface LocalInpaintRequest {
  /** Manifest returned by {@link prepareRepaintRegions}. */
  manifest: PrepareRepaintResult;
  /** `provider` (default) or a local backend id such as `sd_inpaint`. */
  engine?: string;
  /** Per-region prompt overrides (keyed by region index). */
  prompts?: InpaintPromptOverride[];
  /** Base prompt for regions without an override (issue type appended). */
  repaintPromptBase?: string;
  steps?: number;
  guidance?: number;
  strength?: number;
  /** Seed for reproducible inpaint (-1 / undefined = unseeded). */
  seed?: number;
  outputDir?: string;
  outputName?: string;
}

/**
 * Run the opt-in local inpaint backend over a prepared manifest
 * (`local_inpaint_regions`). The diffusion work needs the Python/torch
 * pipeline, which only exists in the desktop build *and* only when the engine's
 * optional deps / weights are present; outside Tauri (browser dev) there is no
 * local backend, so this returns an empty repaint set flagged as a provider
 * fallback — the executor then takes the provider path / passes through.
 */
export async function localInpaintRegions(
  req: LocalInpaintRequest,
): Promise<LocalInpaintResult> {
  const engine = (req.engine ?? "provider").trim() || "provider";
  const invoke = tauriInvoke();
  if (!invoke) {
    return {
      repainted: [],
      engine: "provider",
      engine_requested: engine,
      engine_fallback_reason:
        engine === "provider"
          ? "provider engine (no local backend)"
          : "local inpaint unavailable in browser dev (mock)",
      backend_model: null,
      requested_count: req.manifest.regions?.length ?? 0,
      repainted_count: 0,
    };
  }
  return (await invoke("local_inpaint_regions", {
    manifest: JSON.stringify(req.manifest),
    engine,
    prompts: req.prompts ? JSON.stringify(req.prompts) : null,
    repaintPromptBase: req.repaintPromptBase ?? null,
    steps: req.steps ?? null,
    guidance: req.guidance ?? null,
    strength: req.strength ?? null,
    seed: req.seed ?? null,
    outputDir: req.outputDir ?? null,
    outputName: req.outputName ?? null,
  })) as LocalInpaintResult;
}
