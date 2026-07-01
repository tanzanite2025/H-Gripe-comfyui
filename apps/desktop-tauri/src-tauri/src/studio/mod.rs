//! Studio graph editor backend, split into focused submodules:
//! - [`graph`]: the workflow graph schema + shared value-coercion helpers.
//! - [`exec`]: the topological execution engine, run events, and cancellation.
//! - [`node_registry`]: single source of truth mapping a node `kind` to its
//!   executor + resource lane (`studio_executor_for_kind` / `category_for_kind`).
//! - [`psd_analyze`]: the `psdContextAnalyze` node executor (PSD context bridge).
//! - [`color_match`]: the `matchLightColor` node executor (light/colour match).
//! - [`edge_refine`]: the `refineMaskEdge` node executor (mask edge refine).
//! - [`image_enhance`]: the `imageEnhance` node executor (routes the default
//!   `cpu` engine to the in-process fast path, other engines to Python).
//! - [`image_enhance_cpu`]: native-Rust replica of the CLI's `--engine cpu`
//!   pipeline, run in-process for common 8-bit inputs.
//! - [`detail_watchdog`]: the `detailWatchdog` node executor (CPU quality scan).
//! - [`psd_export`]: the `psdExport` node executor (PSD composition bridge).
//! - [`studio_image`]: decode-guard + colour-space loaders shared by native
//!   (`Compute`) Rust cards.
//! - [`pixel_ops`]: unified crop/resize buffer seam shared by native Rust cards.
//! - [`image_buffer`]: process-global decoded-buffer cache (keyed by
//!   `ResourceId`) so a compute card's output feeds the next one from memory
//!   instead of a PNG re-decode.
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
#[cfg(feature = "native-ffmpeg")]
mod ffmpeg_native;
mod frame_cache;
mod graph;
mod history;
pub(crate) mod image_buffer;
mod image_enhance;
mod image_enhance_cpu;
mod node_registry;
mod onnx_pool;
mod persist;
mod pixel_ops;
mod psd_analyze;
mod psd_export;
mod schedule;
mod studio_image;
mod subject_mask;
mod subject_matte;
mod subject_model;
mod subject_sam2;
mod subject_segment;
pub(crate) mod torch_worker;
pub(crate) mod video_engine;
pub(crate) mod video_worker;

// Glob re-exports so the original `crate::studio::*` command paths keep
// resolving from `main.rs`'s `generate_handler!`. A plain `use exec::cmd` only
// re-exports the function, not the hidden `__cmd__cmd` helper that the Tauri
// command macro generates beside it; the glob carries both.
pub(crate) use exec::*;
pub(crate) use history::*;
pub(crate) use schedule::StudioScheduler;
pub(crate) use persist::*;
pub(crate) use subject_model::set_resource_dir as set_subject_model_resource_dir;
