import { tauriInvoke } from "./core";
import type { Bounds, QualityReport, VisualContext } from "../types/production";

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
  /** Compute device the learned detector bound (`cpu`/`cuda`); null on rules. */
  device?: string | null;
  /** Compute device the node asked for (`auto`/`cpu`/`cuda`). */
  device_requested?: string;
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
  /** Compute device for the learned detector: `auto` (default) | `cpu` | `cuda`. */
  device?: string;
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
        // The rule layer runs no ML session, so no device is bound; echo the
        // request so the inspector still reflects the chosen device.
        device: null,
        device_requested: req.device ?? "auto",
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
    device: req.device ?? null,
    outputDir: req.outputDir ?? null,
    outputName: req.outputName ?? null,
  })) as DetectQualityResult;
}
