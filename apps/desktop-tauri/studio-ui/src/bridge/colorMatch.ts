import { tauriInvoke } from "./core";
import type { VisualContext } from "../types/production";

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
  /** Match engine that actually ran (`cpu` heuristic, or a backend id). */
  engine?: string;
  /** Engine the node asked for (differs from `engine` on fallback). */
  engine_requested?: string;
  /** Why the requested engine was not used (missing deps/weight, no background, …). */
  engine_fallback_reason?: string | null;
  /** Weight file name when a learned backend ran, else `null`. */
  backend_model?: string | null;
  /** Compute device the learned backend bound (`cpu`/`cuda`); null on cpu. */
  device?: string | null;
  /** Compute device the node asked for (`auto`/`cpu`/`cuda`). */
  device_requested?: string;
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
  /** Match engine: `cpu` (default heuristic) or `onnx_harmonize` (opt-in, falls back to cpu). */
  engine?: string;
  /** Compute device for the learned matcher: `auto` (default) | `cpu` | `cuda`. */
  device?: string;
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
        engine: "cpu",
        engine_requested: req.engine ?? "cpu",
        engine_fallback_reason:
          (req.engine ?? "cpu") === "cpu" ? null : "engine unavailable in browser dev mock",
        backend_model: null,
        device: null,
        device_requested: req.device ?? "auto",
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
    engine: req.engine ?? null,
    device: req.device ?? null,
    outputDir: req.outputDir ?? null,
    outputName: req.outputName ?? null,
  })) as ColorMatchResult;
}
