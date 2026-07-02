import { tauriInvoke } from "./core";
import type { QualityReport, RepaintReport } from "../types/production";

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
  /** Seam blend mode: `feather` (default) or `poisson` (gradient-domain). */
  blend?: string;
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
    const blend = req.blend === "poisson" ? "poisson" : "feather";
    const region_results = regions.map((region) => ({
      index: region.index,
      type: region.type,
      bbox: region.bbox,
      status: done.has(region.index) ? "repainted" : "no_repaint",
      blend: done.has(region.index) ? blend : null,
      feather_px:
        done.has(region.index) && blend === "feather"
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
        blend,
      },
    };
  }
  return (await invoke("composite_repaint", {
    image: req.image,
    manifest: JSON.stringify(req.manifest),
    repainted: JSON.stringify(req.repainted),
    featherPx: req.featherPx ?? null,
    blend: req.blend ?? null,
    outputDir: req.outputDir ?? null,
    outputName: req.outputName ?? null,
  })) as CompositeRepaintResult;
}

/** Result of the local inpaint step (mirrors Rust `LocalRepaintResult`). */
export interface LocalRepaintResult {
  /** Regenerated crops, ready to feed into {@link compositeRepaint}. */
  repainted: RepaintedCrop[];
  skipped: unknown[];
  /** Engine that actually ran (`provider` when no local backend was used). */
  engine: string;
  engine_requested: string;
  /** Why the local backend was not used (provider selected / missing deps/weight). */
  engine_fallback_reason?: string | null;
  /** Weight name when a local backend ran, else null. */
  backend_model?: string | null;
  /** Compute device the local backend bound (`cpu`/`cuda`); null on provider. */
  device?: string | null;
  /** Compute precision the local backend bound (`fp16`/`fp32`); null on provider. */
  precision?: string | null;
  /** Compute precision the node asked for (`auto`/`fp32`/`fp16`). */
  precision_requested?: string;
  /** Structural conditioning the node asked for (`off`/`canny`). */
  controlnet_requested?: string;
  requested_count: number;
  repainted_count: number;
}

export interface LocalRepaintRequest {
  /** Manifest returned by {@link prepareRepaintRegions}. */
  manifest: PrepareRepaintResult;
  /** Engine id: `provider` (default) or a local backend like `sd_inpaint`. */
  engine?: string;
  /** Repaint prompt applied to every region. */
  prompt?: string;
  /** Inline JSON mapping issue type -> prompt (overrides `prompt` per region). */
  promptMap?: string;
  negativePrompt?: string;
  strength?: number;
  guidanceScale?: number;
  steps?: number;
  /** Random seed (<0 / undefined = nondeterministic). */
  seed?: number;
  /** Compute precision for the local backend: `auto` (default) | `fp32` | `fp16`. */
  precision?: string;
  /** Structural conditioning for `sd_inpaint`: `off` (default) | `canny`. */
  controlnet?: string;
  outputDir?: string;
  outputName?: string;
}

/**
 * Run the opt-in **local** inpaint backend over a prepare manifest via the
 * backend (`local_repaint_regions`), an alternative to the remote `image.edit`
 * provider. `provider` (the default) or any backend whose deps/weights are
 * missing yields an empty `repainted` list and a recorded reason so the caller
 * falls back to the provider loop. The GPU inpaint pipeline only exists in the
 * desktop build, so outside Tauri this returns the provider-fallback shape.
 */
export async function localRepaintRegions(req: LocalRepaintRequest): Promise<LocalRepaintResult> {
  const invoke = tauriInvoke();
  const engine = req.engine ?? "provider";
  if (!invoke) {
    return {
      repainted: [],
      skipped: [],
      engine: "provider",
      engine_requested: engine,
      engine_fallback_reason:
        engine === "provider"
          ? "engine 'provider': remote image.edit owned by orchestrator"
          : "local inpaint unavailable in browser dev (mock)",
      backend_model: null,
      device: null,
      precision: null,
      precision_requested: req.precision ?? "auto",
      controlnet_requested: req.controlnet ?? "off",
      requested_count: req.manifest.regions?.length ?? 0,
      repainted_count: 0,
    };
  }
  return (await invoke("local_repaint_regions", {
    manifest: JSON.stringify(req.manifest),
    engine,
    prompt: req.prompt ?? null,
    promptMap: req.promptMap ?? null,
    negativePrompt: req.negativePrompt ?? null,
    strength: req.strength ?? null,
    guidanceScale: req.guidanceScale ?? null,
    steps: req.steps ?? null,
    seed: req.seed ?? null,
    precision: req.precision ?? null,
    controlnet: req.controlnet ?? null,
    outputDir: req.outputDir ?? null,
    outputName: req.outputName ?? null,
  })) as LocalRepaintResult;
}
