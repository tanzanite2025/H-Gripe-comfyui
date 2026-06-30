// Production data contracts for the PSD-first AI production pipeline.
//
// These interfaces are the TypeScript mirror of the **single source of truth**
// Rust structs in `apps/desktop-tauri/src-tauri/src/contracts.rs`. The same JSON
// object round-trips unchanged across the Python bridge (producer), the Rust
// orchestration layer, and this front end, so field names stay snake_case and
// must be kept in lock-step with the Rust definitions.

/** A rectangle in PSD canvas pixel coordinates. */
export interface Bounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Background appearance extracted from the template's background layer(s). */
export interface BackgroundContext {
  /** Mean RGB colour, 0-255 per channel. */
  mean_color: [number, number, number];
  /** Dominant palette as `#rrggbb` hex strings, most frequent first. */
  dominant_palette: string[];
  /** Mean luminance, normalised 0.0-1.0. */
  brightness: number;
  /** Luminance spread (heuristic), normalised 0.0-1.0. */
  contrast: number;
  /** Optional path to a written histogram preview PNG. */
  histogram_path: string | null;
  /** Optional path to the composited background preview PNG (a node output). */
  image_path: string | null;
}

/** Lighting heuristics inferred from the background. */
export interface LightingContext {
  /** Dominant light direction, e.g. `top-left` / `center`. */
  direction: string;
  /** `hard` or `soft`, inferred from contrast. */
  quality: string;
  /** Estimated colour temperature in Kelvin. */
  color_temperature: number;
  /** Human-readable summary of the lighting/background. */
  description: string;
}

/** Where the generated subject will be placed inside the template. */
export interface PlaceholderContext {
  /** Resolved placeholder layer name (empty when the whole canvas is used). */
  layer_name: string;
  /** Placeholder rectangle in canvas pixels. */
  bounds: Bounds;
  /** Optional path to a written placeholder mask PNG (a node output). */
  mask_path: string | null;
  /** Optional inset "safe area" inside the bounds. */
  safe_area: Bounds | null;
}

/**
 * Structured visual context produced by the PSD Context Analyze node and
 * consumed by downstream production nodes (Light & Color Match, etc.).
 */
export interface VisualContext {
  background: BackgroundContext;
  lighting: LightingContext;
  placeholder: PlaceholderContext;
  /** Lighting/colour description ready to append to a generation prompt. */
  prompt_suffix: string;
}

/** A single detected quality issue (Detail Watchdog). */
export interface QualityIssue {
  /** e.g. `face_blur | hand_error | edge_halo | color_mismatch | low_resolution`. */
  type: string;
  confidence: number;
  /** `[x1, y1, x2, y2]` in canvas pixels. */
  bbox: [number, number, number, number];
  suggested_action: string;
}

/** Aggregate quality findings for a candidate image. */
export interface QualityReport {
  /** `passed | warning | failed`. */
  status: string;
  issues: QualityIssue[];
}

/** Per-region outcome of a Detail Repaint run (mirrors Rust `RepaintRegionResult`). */
export interface RepaintRegionResult {
  /** Index of the issue in the source QualityReport. */
  index: number;
  /** Issue type carried over from the QualityReport (e.g. `face_blur`). */
  type?: string | null;
  /** `[x1, y1, x2, y2]` issue box in canvas pixels. */
  bbox?: [number, number, number, number] | null;
  /** `repainted | no_repaint | bad_geometry | skipped`. */
  status: string;
  /** Seam feather radius actually used when the region was repainted. */
  feather_px?: number | null;
}

/** Outcome of the Detail Repaint node (mirrors Rust `RepaintReport`). */
export interface RepaintReport {
  /** `unchanged | partial | repainted`. */
  status: string;
  regions: RepaintRegionResult[];
  /** How many regions were actually repainted. */
  repainted_count: number;
  /** How many regions the composite step was asked to handle. */
  requested_count: number;
  /** `[width, height]` of the fixed image. */
  image_size: [number, number];
}

/** Exported artifact paths recorded for a finished workflow. */
export interface ExportedArtifacts {
  psd: string;
  preview: string;
  metadata: string;
}

/**
 * A single bezier/lasso path edit (Subject Mask). Phase 1 stores these but does
 * NOT rasterise them — the field is versioned so a workflow saved now stays
 * loadable once Phase 3 adds rasterisation. Mirrors the Rust `EditPaths` schema
 * in `docs/cards/subject-mask-matte.md`.
 */
export interface EditPathPoint {
  x: number;
  y: number;
  /** Bezier in-control handle, when the path is a pen curve. */
  in?: [number, number];
  /** Bezier out-control handle, when the path is a pen curve. */
  out?: [number, number];
}

export interface EditPath {
  id: string;
  /** `add` | `subtract` | `intersect`. */
  mode: string;
  /** `pen` | `lasso`. */
  tool: string;
  closed: boolean;
  points: EditPathPoint[];
}

/** A freehand brush/eraser stroke (applied by the Rust backend on run). */
export interface BrushStroke {
  id: string;
  /** `add` (brush) | `subtract` (eraser). */
  mode: string;
  /** Stroke radius in image pixels. */
  radius: number;
  /** Polyline of `[x, y]` points the stroke passes through. */
  points: [number, number][];
}

/**
 * A recorded morphology / selection operation queued for the backend to apply
 * (in order) when the node runs. Phase 1 records the *intent* here rather than
 * re-implementing the exact Rust morphology in the webview, so the preview and
 * the executed result cannot drift.
 */
export interface MaskOperation {
  /** `wand` | `invert` | `fill_holes` | `smooth` | `grow` | `shrink` | `feather` | `rect` | `ellipse`. */
  type: string;
  /** Operation-specific scalar (tolerance / px / radius), when relevant. */
  amount?: number;
  /** `[x, y]` seed for `wand`, or `[x1, y1, x2, y2]` for marquee ops. */
  region?: number[];
}

/**
 * Re-editable record of all manual edits for the Subject Mask card. Stored on
 * the node as the `edit_paths` param and round-tripped through the workflow
 * file. Mirrors the Rust `EditPaths` struct.
 */
export interface EditPaths {
  version: 1;
  paths: EditPath[];
  brush_strokes: BrushStroke[];
  /**
   * Trimap "unknown band" strokes for alpha matting (same shape as
   * `brush_strokes`). When present, the backend paints these regions as the
   * trimap *unknown* level on top of the auto `matting_band_px` ring, so the
   * matter (ViTMatte / builtin guided filter) resolves soft alpha exactly where
   * the user marked hair / fur / glass. Non-empty ⇒ matting runs even if the
   * node's `alpha_matting` toggle is off. Read as `edit_paths.matte_strokes`.
   */
  matte_strokes: BrushStroke[];
  /** Ordered morphology / selection operations applied by the backend. */
  operations: MaskOperation[];
  /**
   * SAM 2 point prompts in image-pixel space. When an auto mode runs with at
   * least one *positive* point, the backend routes to the interactive SAM 2
   * segmenter ("segment what the user clicked / not"); empty ⇒ the prompt-free
   * salient / builtin pipeline. Each point carries a `label`: `1` includes
   * (foreground), `0` excludes (background). Read by the Rust backend as
   * `edit_paths.points`; a legacy `[x, y]` pair is read as a positive point.
   */
  points: PointPrompt[];
}

/**
 * A SAM 2 point prompt: an image-space location plus whether it includes or
 * excludes that region. Mirrors SAM 2's `point_labels` (1 = positive /
 * foreground, 0 = negative / background).
 */
export interface PointPrompt {
  x: number;
  y: number;
  /** `1` = positive (include), `0` = negative (exclude). */
  label: 0 | 1;
}

export function emptyEditPaths(): EditPaths {
  return { version: 1, paths: [], brush_strokes: [], matte_strokes: [], operations: [], points: [] };
}

/** A subject detected by a Phase 2 model (empty in Phase 1). */
export interface DetectedSubject {
  label: string;
  confidence: number;
  /** `[x1, y1, x2, y2]` in image pixels. */
  bbox: [number, number, number, number];
}

/** Provenance + operations record for a matte run (mirrors Rust `matte_report`). */
export interface MatteReport {
  mode: string;
  /** `rust-native` in Phase 1, the model id (e.g. `birefnet`) in Phase 2. */
  provider: string;
  source_mode: string;
  exif_transposed: boolean;
  max_decode_pixels: number;
  image_size: [number, number];
  mask_coverage: number;
  detected_subjects: DetectedSubject[];
  operations: { type: string; [k: string]: unknown }[];
  /** Completeness flag for the mask / alpha / cutout triplet. */
  triplet: { mask: boolean; alpha_image: boolean; cutout_image: boolean };
  processing_time_ms: number;
}

/** Result of a Subject Mask run (mirrors Rust `SubjectMaskResult`). */
export interface SubjectMaskResult {
  mask_path: string;
  alpha_image_path: string;
  cutout_image_path: string;
  edit_paths_path: string;
  matte_report: MatteReport;
}

/** End-to-end production workflow tracking, written alongside an export. */
export interface ProductionMetadata {
  workflow_id: string;
  source_psd: string;
  provider_profile: string;
  prompt: string;
  prompt_suffix: string;
  generated_files: string[];
  enhance_steps: string[];
  quality_report: QualityReport | null;
  exported: ExportedArtifacts;
}
