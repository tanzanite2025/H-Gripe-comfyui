//! Tauri command handlers, grouped by domain. Each submodule owns a slice of
//! the desktop bridge surface; `main.rs` wires them into the invoke handler and
//! keeps only the crate-root shared helpers (`broker`, `runtime_paths`,
//! `modified_ms`) that the studio modules also reach through `crate::`.

pub(crate) mod config;
pub(crate) mod history;
pub(crate) mod media;
pub(crate) mod runtime;
pub(crate) mod shell;
pub(crate) mod tasks;
