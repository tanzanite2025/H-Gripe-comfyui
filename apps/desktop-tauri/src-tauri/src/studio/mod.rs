//! Studio graph editor backend, split into focused submodules:
//! - [`graph`]: the workflow graph schema + shared value-coercion helpers.
//! - [`exec`]: the topological execution engine, run events, and cancellation.
//! - [`psd_analyze`]: the `psdContextAnalyze` node executor (PSD context bridge).
//! - [`color_match`]: the `matchLightColor` node executor (light/colour match).
//! - [`edge_refine`]: the `refineMaskEdge` node executor (mask edge refine).
//! - [`image_enhance`]: the `imageEnhance` node executor (CPU upscale/sharpen).
//! - [`detail_watchdog`]: the `detailWatchdog` node executor (CPU quality scan).
//! - [`psd_export`]: the `psdExport` node executor (PSD composition bridge).
//! - [`studio_image`]: decode-guard + colour-space loaders shared by native
//!   (`Compute`) Rust cards.
//! - [`subject_mask`]: the `subjectMask` node executor (native-Rust matte).
//! - [`subject_matte`]: continuous alpha matting (ViTMatte / trimap, Compute lane).
//! - [`subject_sam2`]: SAM 2 interactive point-prompt segmenter (Compute lane).
//! - [`persist`]: on-disk autosave, workflow files, recents, and pickers.
//! - [`history`]: project-scoped snapshot / run-history JSON stores.
//!
//! The Tauri commands keep their original `crate::studio::*` paths via the
//! re-exports below, so `main.rs`'s `invoke_handler` registration is unchanged.

mod color_match;
mod crop;
mod detail_watchdog;
mod edge_refine;
mod exec;
mod graph;
mod history;
mod image_enhance;
mod persist;
mod psd_analyze;
mod psd_export;
mod studio_image;
mod subject_mask;
mod subject_matte;
mod subject_model;
mod subject_sam2;
mod subject_segment;

// Glob re-exports so the original `crate::studio::*` command paths keep
// resolving from `main.rs`'s `generate_handler!`. A plain `use exec::cmd` only
// re-exports the function, not the hidden `__cmd__cmd` helper that the Tauri
// command macro generates beside it; the glob carries both.
pub(crate) use exec::*;
pub(crate) use history::*;
pub(crate) use persist::*;
pub(crate) use subject_model::set_resource_dir as set_subject_model_resource_dir;
