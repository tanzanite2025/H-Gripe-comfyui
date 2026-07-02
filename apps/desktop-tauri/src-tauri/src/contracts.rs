//! Shared production data contracts for the PSD-first AI production pipeline.
//!
//! These structs are the **single source of truth** for the JSON exchanged
//! between the Python bridge (which produces them), the Rust orchestration
//! layer (which (de)serializes them here), and the studio-ui front end (which
//! mirrors them 1:1 in `studio-ui/src/types/production.ts`). Field names are
//! `snake_case` so the same JSON object round-trips unchanged across all three
//! layers; keep the TypeScript interfaces in lock-step with any change here.
//!
//! - [`VisualContext`]: machine-usable production context extracted from a PSD
//!   template by the **PSD Context Analyze** node (background stats, lighting
//!   heuristics, placeholder geometry, ready-to-append prompt suffix).
//! - [`QualityReport`]: issue findings (face/hand/edge/colour/resolution) from
//!   the **Detail Watchdog** node.
//! - [`RepaintReport`]: per-region outcome of the **Detail Repaint** node
//!   (which issue regions were localized-repainted, skipped, or failed).
//! - [`ProductionMetadata`]: end-to-end workflow tracking written alongside an
//!   export.

use serde::{Deserialize, Serialize};

/// A rectangle in PSD canvas pixel coordinates.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Bounds {
    #[serde(default)]
    pub(crate) x: i64,
    #[serde(default)]
    pub(crate) y: i64,
    #[serde(default)]
    pub(crate) width: i64,
    #[serde(default)]
    pub(crate) height: i64,
}

/// Background appearance extracted from the template's background layer(s).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct BackgroundContext {
    /// Mean RGB colour, 0-255 per channel.
    #[serde(default)]
    pub(crate) mean_color: [u8; 3],
    /// Dominant palette as `#rrggbb` hex strings, most frequent first.
    #[serde(default)]
    pub(crate) dominant_palette: Vec<String>,
    /// Mean luminance, normalised 0.0-1.0.
    #[serde(default)]
    pub(crate) brightness: f64,
    /// Luminance spread (heuristic), normalised 0.0-1.0.
    #[serde(default)]
    pub(crate) contrast: f64,
    /// Optional path to a written histogram preview PNG.
    #[serde(default)]
    pub(crate) histogram_path: Option<String>,
    /// Optional path to the composited background preview PNG (a node output).
    #[serde(default)]
    pub(crate) image_path: Option<String>,
}

/// Lighting heuristics inferred from the background.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct LightingContext {
    /// Dominant light direction, e.g. `top-left` / `center`.
    #[serde(default)]
    pub(crate) direction: String,
    /// `hard` or `soft`, inferred from contrast.
    #[serde(default)]
    pub(crate) quality: String,
    /// Estimated colour temperature in Kelvin.
    #[serde(default)]
    pub(crate) color_temperature: u32,
    /// Human-readable summary of the lighting/background.
    #[serde(default)]
    pub(crate) description: String,
}

/// Where the generated subject will be placed inside the template.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct PlaceholderContext {
    /// Resolved placeholder layer name (empty when the whole canvas is used).
    #[serde(default)]
    pub(crate) layer_name: String,
    /// Placeholder rectangle in canvas pixels.
    #[serde(default)]
    pub(crate) bounds: Bounds,
    /// Optional path to a written placeholder mask PNG (a node output).
    #[serde(default)]
    pub(crate) mask_path: Option<String>,
    /// Optional inset "safe area" inside the bounds.
    #[serde(default)]
    pub(crate) safe_area: Option<Bounds>,
}

/// Structured visual context produced by the PSD Context Analyze node and
/// consumed by downstream production nodes (Light & Color Match, etc.).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct VisualContext {
    #[serde(default)]
    pub(crate) background: BackgroundContext,
    #[serde(default)]
    pub(crate) lighting: LightingContext,
    #[serde(default)]
    pub(crate) placeholder: PlaceholderContext,
    /// Lighting/colour description ready to append to a generation prompt.
    #[serde(default)]
    pub(crate) prompt_suffix: String,
}

/// A single detected quality issue (Detail Watchdog).
//
// Defined now as the shared contract; wired up in a later phase (Detail
// Watchdog), hence `allow(dead_code)` until a node constructs it.
#[allow(dead_code)]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct QualityIssue {
    /// e.g. `face_blur | hand_error | edge_halo | color_mismatch | low_resolution`.
    #[serde(rename = "type", default)]
    pub(crate) issue_type: String,
    #[serde(default)]
    pub(crate) confidence: f64,
    /// `[x1, y1, x2, y2]` in canvas pixels.
    #[serde(default)]
    pub(crate) bbox: [i64; 4],
    #[serde(default)]
    pub(crate) suggested_action: String,
}

/// Aggregate quality findings for a candidate image.
#[allow(dead_code)]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct QualityReport {
    /// `passed | warning | failed`.
    #[serde(default)]
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) issues: Vec<QualityIssue>,
}

/// Outcome of repainting a single issue region (Detail Repaint).
#[allow(dead_code)]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct RepaintRegionResult {
    /// Index of the source issue in the [`QualityReport`].
    #[serde(default)]
    pub(crate) index: u32,
    /// The issue type that was repainted, e.g. `face_blur`.
    #[serde(rename = "type", default)]
    pub(crate) issue_type: Option<String>,
    /// `[x1, y1, x2, y2]` of the original issue, in canvas pixels.
    #[serde(default)]
    pub(crate) bbox: Option<[i64; 4]>,
    /// `repainted | no_repaint | skipped | bad_geometry`.
    #[serde(default)]
    pub(crate) status: String,
    /// Seam feather radius actually applied (only for `repainted`).
    #[serde(default)]
    pub(crate) feather_px: Option<f64>,
    /// Seam blend actually applied (`feather` | `poisson`); a `poisson`
    /// request degrades to `feather` on a too-small region.
    #[serde(default)]
    pub(crate) blend: Option<String>,
}

/// Per-region outcome of the **Detail Repaint** node: which issue regions were
/// localized-repainted via the provider, pasted back, and edge-fused.
#[allow(dead_code)]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct RepaintReport {
    /// `repainted | partial | unchanged`.
    #[serde(default)]
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) regions: Vec<RepaintRegionResult>,
    /// How many regions were actually repainted + pasted back.
    #[serde(default)]
    pub(crate) repainted_count: u32,
    /// How many regions the composite step was asked to handle.
    #[serde(default)]
    pub(crate) requested_count: u32,
    /// `[width, height]` of the fixed image.
    #[serde(default)]
    pub(crate) image_size: [i64; 2],
    /// Seam blend mode the composite ran (`feather` | `poisson`).
    #[serde(default)]
    pub(crate) blend: String,
    /// Pillow mode of the decoded candidate before normalising to 8-bit RGBA.
    #[serde(default)]
    pub(crate) source_mode: String,
    /// Whether an EXIF orientation tag was applied to upright the candidate.
    #[serde(default)]
    pub(crate) exif_transposed: bool,
    /// Decode-pixel ceiling enforced before decoding (0 disables the guard).
    #[serde(default)]
    pub(crate) max_decode_pixels: i64,
}

/// Exported artifact paths recorded for a finished workflow.
#[allow(dead_code)]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ExportedArtifacts {
    #[serde(default)]
    pub(crate) psd: String,
    #[serde(default)]
    pub(crate) preview: String,
    #[serde(default)]
    pub(crate) metadata: String,
}

/// End-to-end production workflow tracking, written alongside an export.
#[allow(dead_code)]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ProductionMetadata {
    #[serde(default)]
    pub(crate) workflow_id: String,
    #[serde(default)]
    pub(crate) source_psd: String,
    #[serde(default)]
    pub(crate) provider_profile: String,
    #[serde(default)]
    pub(crate) prompt: String,
    #[serde(default)]
    pub(crate) prompt_suffix: String,
    #[serde(default)]
    pub(crate) generated_files: Vec<String>,
    #[serde(default)]
    pub(crate) enhance_steps: Vec<String>,
    #[serde(default)]
    pub(crate) quality_report: Option<QualityReport>,
    #[serde(default)]
    pub(crate) exported: ExportedArtifacts,
}
