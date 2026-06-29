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
