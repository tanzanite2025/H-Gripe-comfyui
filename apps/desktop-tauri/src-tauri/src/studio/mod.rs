//! Studio graph editor backend, split into focused submodules:
//! - [`graph`]: the workflow graph schema + shared value-coercion helpers.
//! - [`exec`]: the topological execution engine, run events, and cancellation.
//! - [`psd_analyze`]: the `psdContextAnalyze` node executor (PSD context bridge).
//! - [`color_match`]: the `matchLightColor` node executor (light/colour match).
//! - [`psd_export`]: the `psdExport` node executor (PSD composition bridge).
//! - [`persist`]: on-disk autosave, workflow files, recents, and pickers.
//! - [`history`]: project-scoped snapshot / run-history JSON stores.
//!
//! The Tauri commands keep their original `crate::studio::*` paths via the
//! re-exports below, so `main.rs`'s `invoke_handler` registration is unchanged.

mod color_match;
mod exec;
mod graph;
mod history;
mod persist;
mod psd_analyze;
mod psd_export;

// Glob re-exports so the original `crate::studio::*` command paths keep
// resolving from `main.rs`'s `generate_handler!`. A plain `use exec::cmd` only
// re-exports the function, not the hidden `__cmd__cmd` helper that the Tauri
// command macro generates beside it; the glob carries both.
pub(crate) use exec::*;
pub(crate) use history::*;
pub(crate) use persist::*;
