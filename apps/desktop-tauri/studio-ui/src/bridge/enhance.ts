import { tauriInvoke } from "./core";
import type { Bounds } from "../types/production";

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
  /** Compute device the model backend bound (`cpu`/`cuda`); null on cpu. */
  device?: string | null;
  /** Compute device the node asked for (`auto`/`cpu`/`cuda`). */
  device_requested?: string;
  /** Compute precision the model backend bound (`fp16`/`fp32`); null on cpu. */
  precision?: string | null;
  /** Compute precision the node asked for (`auto`/`fp32`/`fp16`). */
  precision_requested?: string;
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
  /** Compute device for the learned upscaler: `auto` (default) | `cpu` | `cuda`. */
  device?: string;
  /** Compute precision for the learned upscaler: `auto` (default) | `fp32` | `fp16`. */
  precision?: string;
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
        device: null,
        device_requested: req.device ?? "auto",
        precision: null,
        precision_requested: req.precision ?? "auto",
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
    device: req.device ?? null,
    precision: req.precision ?? null,
    outputDir: req.outputDir ?? null,
    outputName: req.outputName ?? null,
  })) as EnhanceImageResult;
}
