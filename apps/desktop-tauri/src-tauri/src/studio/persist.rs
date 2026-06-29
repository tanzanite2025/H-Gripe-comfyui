//! On-disk persistence for the Studio editor: the single-slot autosave, named
//! workflow files + native pickers, the project folder listing, the persisted
//! session pointers ("recents"), and the low-level project-scoped JSON store
//! primitives reused by [`super::history`].

use std::fs;
use std::path::{Path, PathBuf};

use hgripe_api::credentials_file_path;
use serde::{Deserialize, Serialize};

use super::graph::StudioWorkflowGraph;
use crate::modified_ms;

pub(super) fn studio_workspace_dir() -> PathBuf {
    let credentials = credentials_file_path(None);
    let base = credentials
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("user")
                .join("hgripe")
        });
    base.join("studio")
}

fn studio_autosave_path() -> PathBuf {
    studio_workspace_dir().join("autosave.workflow.json")
}

#[tauri::command]
pub(crate) fn read_studio_autosave() -> Result<Option<String>, String> {
    let path = studio_autosave_path();
    if !path.exists() {
        return Ok(None);
    }
    fs::read_to_string(&path)
        .map(Some)
        .map_err(|err| format!("failed to read Studio autosave {}: {err}", path.display()))
}

#[tauri::command]
pub(crate) fn write_studio_autosave(graph_json: String) -> Result<(), String> {
    let graph: StudioWorkflowGraph = serde_json::from_str(&graph_json)
        .map_err(|err| format!("invalid Studio graph JSON: {err}"))?;
    if graph.version != 1 {
        return Err(format!(
            "unsupported Studio graph version: {} (expected 1)",
            graph.version
        ));
    }

    let path = studio_autosave_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
    }
    fs::write(&path, graph_json)
        .map_err(|err| format!("failed to write Studio autosave {}: {err}", path.display()))
}

#[tauri::command]
pub(crate) fn clear_studio_autosave() -> Result<(), String> {
    let path = studio_autosave_path();
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!(
            "failed to remove Studio autosave {}: {err}",
            path.display()
        )),
    }
}

// --- Explicit workflow save/open + project folder ---------------------------
// Beyond the single-slot autosave, the editor can save/open named workflow
// files anywhere on disk and browse a chosen "project folder" of workflows.
// Recents (last project folder + recently opened files) persist next to the
// autosave so the editor reopens where the user left off.

fn studio_recents_path() -> PathBuf {
    studio_workspace_dir().join("recents.workflow.json")
}

/// A `.workflow.json` (or `.json`) file discovered in a project folder.
#[derive(Serialize)]
pub(crate) struct StudioWorkflowFile {
    /// File name including extension (e.g. `poster.workflow.json`).
    name: String,
    path: String,
    modified_ms: Option<u64>,
    size_bytes: u64,
}

/// Persisted editor session pointers: the active project folder and the
/// most-recently-opened workflow files (newest first).
#[derive(Serialize, Deserialize, Default)]
pub(crate) struct StudioRecents {
    #[serde(default)]
    project_dir: Option<String>,
    #[serde(default)]
    current_file: Option<String>,
    #[serde(default)]
    files: Vec<String>,
}

fn studio_is_workflow_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("json"))
        .unwrap_or(false)
}

/// Open a native save dialog scoped to workflow JSON and return the chosen
/// path, or `None` if cancelled.
#[tauri::command]
pub(crate) fn pick_workflow_save_path(
    app: tauri::AppHandle,
    default_name: Option<String>,
    dir: Option<String>,
) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let mut builder = app
        .dialog()
        .file()
        .set_title("Save Workflow")
        .add_filter("Workflow", &["json"])
        .set_file_name(default_name.unwrap_or_else(|| "workflow.json".to_string()));
    if let Some(dir) = dir.as_deref().filter(|d| !d.trim().is_empty()) {
        builder = builder.set_directory(dir);
    }
    builder.blocking_save_file().map(|path| path.to_string())
}

/// Open a native open dialog scoped to workflow JSON and return the chosen
/// path, or `None` if cancelled.
#[tauri::command]
pub(crate) fn pick_workflow_open_path(
    app: tauri::AppHandle,
    dir: Option<String>,
) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let mut builder = app
        .dialog()
        .file()
        .set_title("Open Workflow")
        .add_filter("Workflow", &["json"]);
    if let Some(dir) = dir.as_deref().filter(|d| !d.trim().is_empty()) {
        builder = builder.set_directory(dir);
    }
    builder.blocking_pick_file().map(|path| path.to_string())
}

/// Open a native folder-picker and return the chosen directory, or `None`.
#[tauri::command]
pub(crate) fn pick_project_folder(app: tauri::AppHandle, dir: Option<String>) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let mut builder = app.dialog().file().set_title("Choose Project Folder");
    if let Some(dir) = dir.as_deref().filter(|d| !d.trim().is_empty()) {
        builder = builder.set_directory(dir);
    }
    builder.blocking_pick_folder().map(|path| path.to_string())
}

/// Read a workflow file from disk, validating it parses as a Studio graph.
#[tauri::command]
pub(crate) fn read_studio_workflow(path: String) -> Result<String, String> {
    let path = Path::new(path.trim());
    let text = fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    serde_json::from_str::<StudioWorkflowGraph>(&text)
        .map_err(|err| format!("not a valid Studio workflow ({}): {err}", path.display()))?;
    Ok(text)
}

/// Write a workflow file to disk, validating the payload first and creating
/// parent directories as needed.
#[tauri::command]
pub(crate) fn write_studio_workflow(path: String, graph_json: String) -> Result<(), String> {
    let graph: StudioWorkflowGraph = serde_json::from_str(&graph_json)
        .map_err(|err| format!("invalid Studio graph JSON: {err}"))?;
    if graph.version != 1 {
        return Err(format!(
            "unsupported Studio graph version: {} (expected 1)",
            graph.version
        ));
    }
    let path = Path::new(path.trim());
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
    }
    fs::write(path, graph_json).map_err(|err| format!("failed to write {}: {err}", path.display()))
}

/// List workflow JSON files in a project folder (non-recursive), newest first.
#[tauri::command]
pub(crate) fn list_studio_workflows(dir: String) -> Result<Vec<StudioWorkflowFile>, String> {
    let dir = dir.trim();
    if dir.is_empty() {
        return Err("project folder is empty".to_string());
    }
    let path = Path::new(dir);
    if !path.is_dir() {
        return Err(format!("not a directory: {dir}"));
    }

    let mut files = Vec::new();
    for entry in
        fs::read_dir(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let file_path = entry.path();
        if !file_path.is_file() || !studio_is_workflow_file(&file_path) {
            continue;
        }
        let name = match file_path.file_name().and_then(|s| s.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };
        let metadata = entry.metadata().ok();
        files.push(StudioWorkflowFile {
            name,
            path: file_path.to_string_lossy().to_string(),
            modified_ms: metadata.as_ref().and_then(modified_ms),
            size_bytes: metadata.as_ref().map(|m| m.len()).unwrap_or(0),
        });
    }

    files.sort_by(|a, b| {
        b.modified_ms
            .cmp(&a.modified_ms)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(files)
}

/// Read the persisted editor session pointers (project folder + recent files).
#[tauri::command]
pub(crate) fn read_studio_recents() -> Result<StudioRecents, String> {
    let path = studio_recents_path();
    if !path.exists() {
        return Ok(StudioRecents::default());
    }
    let text = fs::read_to_string(&path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|err| format!("invalid Studio recents {}: {err}", path.display()))
}

/// Persist the editor session pointers (project folder + recent files).
#[tauri::command]
pub(crate) fn write_studio_recents(recents: StudioRecents) -> Result<(), String> {
    let path = studio_recents_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(&recents)
        .map_err(|err| format!("failed to serialize Studio recents: {err}"))?;
    fs::write(&path, text).map_err(|err| format!("failed to write {}: {err}", path.display()))
}

/// Normalize a user-supplied workflow file name: reject empties and path
/// separators, and ensure a `.json` extension.
fn studio_normalize_workflow_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("name is empty".to_string());
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err("name must not contain path separators".to_string());
    }
    let has_json = Path::new(trimmed)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("json"))
        .unwrap_or(false);
    Ok(if has_json {
        trimmed.to_string()
    } else {
        format!("{trimmed}.json")
    })
}

/// Reject a user-supplied base file name that could escape the directory it is
/// later joined onto (path separators, or a `.`/`..` component). Used for
/// export targets where a downstream helper does `directory / name`, so an
/// untrusted workflow cannot redirect the write outside the chosen folder.
pub(crate) fn studio_reject_unsafe_basename(name: &str) -> Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("filename is empty".to_string());
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err("filename must not contain path separators".to_string());
    }
    if trimmed == "." || trimmed == ".." {
        return Err("filename is not a valid name".to_string());
    }
    Ok(())
}

/// Find an unused `"{stem} copy[.N].json"` path next to a source workflow.
fn studio_unique_copy_path(parent: &Path, stem: &str) -> Result<PathBuf, String> {
    let first = parent.join(format!("{stem} copy.json"));
    if !first.exists() {
        return Ok(first);
    }
    for n in 2..1000 {
        let candidate = parent.join(format!("{stem} copy {n}.json"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err("too many copies of this workflow".to_string())
}

/// Rename a workflow file within its folder; returns the new path.
#[tauri::command]
pub(crate) fn rename_studio_workflow(path: String, new_name: String) -> Result<String, String> {
    let from = Path::new(path.trim());
    if !studio_is_workflow_file(from) {
        return Err(format!("not a workflow file: {}", from.display()));
    }
    let parent = from
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| "file has no parent directory".to_string())?;
    let file_name = studio_normalize_workflow_name(&new_name)?;
    let to = parent.join(&file_name);
    if to == from {
        return Ok(from.to_string_lossy().to_string());
    }
    if to.exists() {
        return Err(format!("{file_name} already exists"));
    }
    fs::rename(from, &to).map_err(|err| format!("failed to rename {}: {err}", from.display()))?;
    Ok(to.to_string_lossy().to_string())
}

/// Delete a workflow file from disk.
#[tauri::command]
pub(crate) fn delete_studio_workflow(path: String) -> Result<(), String> {
    let target = Path::new(path.trim());
    if !studio_is_workflow_file(target) {
        return Err(format!("not a workflow file: {}", target.display()));
    }
    fs::remove_file(target).map_err(|err| format!("failed to delete {}: {err}", target.display()))
}

/// Copy a workflow file to a fresh `"… copy.json"` sibling; returns its path.
#[tauri::command]
pub(crate) fn duplicate_studio_workflow(path: String) -> Result<String, String> {
    let from = Path::new(path.trim());
    if !studio_is_workflow_file(from) {
        return Err(format!("not a workflow file: {}", from.display()));
    }
    let parent = from
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| "file has no parent directory".to_string())?;
    let stem = from
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("workflow");
    let to = studio_unique_copy_path(parent, stem)?;
    fs::copy(from, &to).map_err(|err| format!("failed to copy {}: {err}", from.display()))?;
    Ok(to.to_string_lossy().to_string())
}

// --- Project-scoped JSON stores ---------------------------------------------
// Some renderer state (named graph snapshots, run history) can be persisted
// into the active project folder as a single JSON file so it travels with the
// project and survives a cache wipe / machine change, instead of living only in
// browser localStorage. The renderer owns the JSON shape (an array); the
// backend just reads/writes one file per store, mirroring the autosave slot.
// The command wrappers built on these live in [`super::history`].

/// Resolve `<dir>/<filename>`, validating that `dir` is a real directory.
fn studio_store_path(dir: &str, filename: &str) -> Result<PathBuf, String> {
    let dir = dir.trim();
    if dir.is_empty() {
        return Err("project folder is empty".to_string());
    }
    let path = Path::new(dir);
    if !path.is_dir() {
        return Err(format!("not a directory: {dir}"));
    }
    Ok(path.join(filename))
}

/// Read a project-scoped store file as raw JSON text, or `"[]"` if absent.
pub(super) fn read_studio_store(dir: &str, filename: &str) -> Result<String, String> {
    let path = studio_store_path(dir, filename)?;
    if !path.exists() {
        return Ok("[]".to_string());
    }
    fs::read_to_string(&path).map_err(|err| format!("failed to read {}: {err}", path.display()))
}

/// Write `json` to a project-scoped store file, validating it as JSON first.
pub(super) fn write_studio_store(dir: &str, filename: &str, json: &str) -> Result<(), String> {
    serde_json::from_str::<serde_json::Value>(json)
        .map_err(|err| format!("invalid JSON for {filename}: {err}"))?;
    let path = studio_store_path(dir, filename)?;
    fs::write(&path, json).map_err(|err| format!("failed to write {}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reject_unsafe_basename_accepts_plain_names() {
        assert!(studio_reject_unsafe_basename("final").is_ok());
        assert!(studio_reject_unsafe_basename("  result  ").is_ok());
        assert!(studio_reject_unsafe_basename("my.output").is_ok());
    }

    #[test]
    fn reject_unsafe_basename_rejects_traversal_and_separators() {
        assert!(studio_reject_unsafe_basename("").is_err());
        assert!(studio_reject_unsafe_basename("   ").is_err());
        assert!(studio_reject_unsafe_basename(".").is_err());
        assert!(studio_reject_unsafe_basename("..").is_err());
        assert!(studio_reject_unsafe_basename("../evil").is_err());
        assert!(studio_reject_unsafe_basename("..\\evil").is_err());
        assert!(studio_reject_unsafe_basename("sub/dir").is_err());
        assert!(studio_reject_unsafe_basename("/etc/passwd").is_err());
    }
}
