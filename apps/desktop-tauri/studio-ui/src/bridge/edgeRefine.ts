import { tauriInvoke } from "./core";

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
  /** Matte engine that actually ran (`cpu` heuristic or a backend id). */
  engine?: string;
  /** Engine the node asked for (may differ from `engine` on fallback). */
  engine_requested?: string;
  /** Why the requested engine was not used (deps/weight, no trimap, …); else null. */
  engine_fallback_reason?: string | null;
  /** Weight file the backend loaded (`null` on the CPU path). */
  backend_model?: string | null;
  /** Compute device the learned backend bound (`cpu`/`cuda`); null on cpu. */
  device?: string | null;
  /** Compute device the node asked for (`auto`/`cpu`/`cuda`). */
  device_requested?: string;
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
  /**
   * Matte engine: `cpu` (default heuristic) or an opt-in learned matter id
   * (e.g. `onnx_matting`). A learned matter needs a connected trimap and falls
   * back to `cpu` when its deps / weights are missing.
   */
  engine?: string;
  /** Compute device for the learned matter: `auto` (default) | `cpu` | `cuda`. */
  device?: string;
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
        engine: "cpu",
        engine_requested: (req.engine ?? "cpu").trim() || "cpu",
        engine_fallback_reason:
          (req.engine ?? "cpu").trim() && (req.engine ?? "cpu").trim() !== "cpu"
            ? "engine unavailable in browser dev mock"
            : null,
        backend_model: null,
        device: null,
        device_requested: req.device ?? "auto",
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
    engine: req.engine ?? null,
    device: req.device ?? null,
    outputDir: req.outputDir ?? null,
    outputName: req.outputName ?? null,
  })) as RefineEdgeResult;
}
