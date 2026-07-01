//! PSD template operations: list exported PSD triplets and shell out to the
//! vendored `compose_psd_cli.py` / `inspect_psd_cli.py` / analyze helpers via
//! the project's bundled Python. Split out of `psd.rs`; the command names and
//! result shapes are unchanged.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::contracts::VisualContext;
use crate::modified_ms;
use crate::studio::studio_reject_unsafe_basename;

use super::{project_python, resolve_project_dir};
#[derive(Serialize)]
pub(crate) struct PsdOutputFile {
    /// Base name shared by the triplet (e.g. `final` for `final.psd`).
    name: String,
    psd_path: String,
    preview_path: Option<String>,
    metadata_path: Option<String>,
    /// PSD file modification time in milliseconds since the Unix epoch.
    modified_ms: Option<u64>,
    size_bytes: u64,
    /// True when the export's metadata records a true smart-object content
    /// replacement (`smart_object_mode == "replace_content"`).
    smart_object: bool,
}

/// Cheap check for whether a `_metadata.json` records a smart-object content
/// replacement, without pulling in a JSON parser.
fn metadata_has_smart_object(metadata_path: &Option<String>) -> bool {
    let Some(path) = metadata_path else {
        return false;
    };
    match fs::read_to_string(path) {
        Ok(text) => text.contains("\"smart_object_mode\"") && text.contains("\"replace_content\""),
        Err(_) => false,
    }
}

/// Scan a directory (non-recursively) for PSD exports produced by the PSD
/// nodes and group each `<base>.psd` with its `<base>_preview.png` and
/// `<base>_metadata.json` siblings when present.
#[tauri::command]
pub(crate) fn list_psd_outputs(dir: String) -> Result<Vec<PsdOutputFile>, String> {
    let dir = dir.trim();
    if dir.is_empty() {
        return Err("output directory is empty".to_string());
    }
    let path = Path::new(dir);
    if !path.is_dir() {
        return Err(format!("not a directory: {dir}"));
    }

    let mut outputs = Vec::new();
    for entry in
        fs::read_dir(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let psd_path = entry.path();
        let is_psd = psd_path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("psd"))
            .unwrap_or(false);
        if !is_psd {
            continue;
        }
        let base = match psd_path.file_stem().and_then(|s| s.to_str()) {
            Some(stem) => stem.to_string(),
            None => continue,
        };

        let sibling = |suffix: &str| {
            let candidate = path.join(format!("{base}{suffix}"));
            candidate
                .is_file()
                .then(|| candidate.to_string_lossy().to_string())
        };
        let preview_path = sibling("_preview.png");
        let metadata_path = sibling("_metadata.json");
        let smart_object = metadata_has_smart_object(&metadata_path);

        let metadata = entry.metadata().ok();
        outputs.push(PsdOutputFile {
            name: base,
            psd_path: psd_path.to_string_lossy().to_string(),
            preview_path,
            metadata_path,
            modified_ms: metadata.as_ref().and_then(modified_ms),
            size_bytes: metadata.as_ref().map(|m| m.len()).unwrap_or(0),
            smart_object,
        });
    }

    // Newest first, falling back to name for stable ordering.
    outputs.sort_by(|a, b| {
        b.modified_ms
            .cmp(&a.modified_ms)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(outputs)
}

/// Result of a `compose_psd` run, mirroring the JSON printed by the
/// `compose_psd_cli.py` helper.
#[derive(Serialize, Deserialize)]
pub(crate) struct ComposePsdResult {
    pub(crate) status: String,
    pub(crate) psd_path: String,
    /// Empty string when preview generation was disabled.
    pub(crate) preview_path: String,
    pub(crate) metadata_path: String,
    pub(crate) placeholder_kind: Option<String>,
    pub(crate) smart_object_mode: String,
}

/// Compose a generated image into a PSD template's placeholder (true
/// smart-object content replacement when applicable) and export
/// `<filename>.psd` + `<filename>_preview.png` + `<filename>_metadata.json`.
///
/// This shells out to `python/bridge/compose_psd_cli.py` using the project's
/// bundled Python (`python_embeded` when present), reusing the proven, vendored
/// psd-tools pipeline. `dir` is the project root (defaults to the process
/// working dir); the rest map 1:1 onto the CLI flags.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) fn compose_psd(
    dir: Option<String>,
    template: String,
    image: String,
    mask: Option<String>,
    output_dir: String,
    filename: Option<String>,
    placeholder: Option<String>,
    fit_mode: Option<String>,
    z_order: Option<String>,
    smart_object_mode: Option<String>,
    hide_placeholder: Option<String>,
    metadata: Option<String>,
    save_preview: Option<bool>,
) -> Result<ComposePsdResult, String> {
    let dir = resolve_project_dir(&dir)?;
    let python = project_python(&dir);
    let script = dir.join("python").join("bridge").join("compose_psd_cli.py");
    if !script.is_file() {
        return Err(format!(
            "compose_psd_cli.py not found at {}",
            script.display()
        ));
    }

    // The helper joins `filename` onto `output_dir` (`directory / f"{base}.psd"`),
    // so a name with path separators or `..` could write outside the chosen
    // folder. Validate it here before handing the value to the CLI.
    let filename = filename.as_deref().unwrap_or("final");
    studio_reject_unsafe_basename(filename)?;

    let mut cmd = std::process::Command::new(&python);
    cmd.arg(&script)
        .arg("--template")
        .arg(&template)
        .arg("--image")
        .arg(&image)
        .arg("--mask")
        .arg(mask.as_deref().unwrap_or(""))
        .arg("--output-dir")
        .arg(&output_dir)
        .arg("--filename")
        .arg(filename)
        .arg("--placeholder")
        .arg(placeholder.as_deref().unwrap_or("{}"))
        .arg("--fit-mode")
        .arg(fit_mode.as_deref().unwrap_or("contain"))
        .arg("--z-order")
        .arg(z_order.as_deref().unwrap_or("above_background"))
        .arg("--smart-object-mode")
        .arg(smart_object_mode.as_deref().unwrap_or("disable"))
        .arg("--hide-placeholder")
        .arg(hide_placeholder.as_deref().unwrap_or("enable"))
        .arg("--metadata")
        .arg(metadata.as_deref().unwrap_or("{}"))
        .arg("--save-preview")
        .arg(if save_preview.unwrap_or(true) {
            "enable"
        } else {
            "disable"
        })
        .current_dir(&dir);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: don't pop a console window for the child.
        cmd.creation_flags(0x0800_0000);
    }

    let output = cmd
        .output()
        .map_err(|err| format!("failed to launch {}: {err}", python.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("compose_psd failed: {}", stderr.trim()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str::<ComposePsdResult>(stdout.trim()).map_err(|err| {
        format!(
            "could not parse compose_psd output: {err} (raw: {})",
            stdout.trim()
        )
    })
}

/// A single PSD layer, mirroring the rows printed by `inspect_psd_cli.py`.
#[derive(Serialize, Deserialize)]
pub(crate) struct PsdLayerInfo {
    name: String,
    /// "group" | "smartobject" | "pixel".
    kind: String,
}

/// Result of an `inspect_psd` run, mirroring the JSON printed by the
/// `inspect_psd_cli.py` helper.
#[derive(Serialize, Deserialize)]
pub(crate) struct InspectPsdResult {
    status: String,
    /// `false` when the template path does not point at a file on disk.
    exists: bool,
    width: u32,
    height: u32,
    /// Flat list of every layer (groups and their children), newest-first as
    /// PSD stores them.
    layers: Vec<PsdLayerInfo>,
    /// Subset of the requested `names` that were not found in the PSD.
    missing: Vec<String>,
}

/// Inspect a PSD template: report whether it exists on disk, its canvas size,
/// and the names/kinds of its layers, plus which of the requested placeholder
/// `names` are missing. This lets the editor validate a real PSD before a run
/// (file present, placeholder layer name actually exists) instead of only
/// surfacing the problem mid-compose.
///
/// Like `compose_psd`, this shells out to `python/bridge/inspect_psd_cli.py`
/// using the project's bundled Python, reusing the vendored psd-tools pipeline.
#[tauri::command]
pub(crate) fn inspect_psd(
    dir: Option<String>,
    template: String,
    names: Option<Vec<String>>,
) -> Result<InspectPsdResult, String> {
    let dir = resolve_project_dir(&dir)?;
    let python = project_python(&dir);
    let script = dir.join("python").join("bridge").join("inspect_psd_cli.py");
    if !script.is_file() {
        return Err(format!(
            "inspect_psd_cli.py not found at {}",
            script.display()
        ));
    }
    let names_json =
        serde_json::to_string(&names.unwrap_or_default()).map_err(|err| err.to_string())?;

    let mut cmd = std::process::Command::new(&python);
    cmd.arg(&script)
        .arg("--template")
        .arg(&template)
        .arg("--names")
        .arg(&names_json)
        .current_dir(&dir);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: don't pop a console window for the child.
        cmd.creation_flags(0x0800_0000);
    }

    let output = cmd
        .output()
        .map_err(|err| format!("failed to launch {}: {err}", python.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("inspect_psd failed: {}", stderr.trim()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str::<InspectPsdResult>(stdout.trim()).map_err(|err| {
        format!(
            "could not parse inspect_psd output: {err} (raw: {})",
            stdout.trim()
        )
    })
}

/// Analyze a PSD template into a machine-usable [`VisualContext`]: background
/// colour/lighting heuristics, the target placeholder's geometry, and a
/// ready-to-append prompt suffix. This is the **PSD Context Analyze** node's
/// backend (the first PSD production node): downstream nodes (Light & Color
/// Match, etc.) consume the returned context so the user never hand-describes
/// the template's lighting/colour.
///
/// Like `compose_psd` / `inspect_psd`, it shells out to
/// `python/bridge/analyze_psd_cli.py` using the project's bundled Python,
/// reusing the vendored psd-tools + Pillow pipeline. `background_layer` /
/// `target_placeholder` may be empty (auto: whole-canvas placeholder, full
/// composite background); `output_dir` is where the placeholder mask and
/// background preview PNGs are written (defaults to the CLI's choice when
/// omitted). `reference_layers` is currently advisory (Phase 1 is heuristic).
#[tauri::command]
pub(crate) fn analyze_psd_context(
    dir: Option<String>,
    template: String,
    background_layer: Option<String>,
    target_placeholder: Option<String>,
    reference_layers: Option<Vec<String>>,
    output_dir: Option<String>,
) -> Result<VisualContext, String> {
    let dir = resolve_project_dir(&dir)?;
    let python = project_python(&dir);
    let script = dir.join("python").join("bridge").join("analyze_psd_cli.py");
    if !script.is_file() {
        return Err(format!(
            "analyze_psd_cli.py not found at {}",
            script.display()
        ));
    }
    let references_json = serde_json::to_string(&reference_layers.unwrap_or_default())
        .map_err(|err| err.to_string())?;

    let mut cmd = std::process::Command::new(&python);
    cmd.arg(&script)
        .arg("--template")
        .arg(&template)
        .arg("--background-layer")
        .arg(background_layer.as_deref().unwrap_or(""))
        .arg("--target-placeholder")
        .arg(target_placeholder.as_deref().unwrap_or(""))
        .arg("--reference-layers")
        .arg(&references_json)
        .arg("--output-dir")
        .arg(output_dir.as_deref().unwrap_or(""))
        .current_dir(&dir);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: don't pop a console window for the child.
        cmd.creation_flags(0x0800_0000);
    }

    let output = cmd
        .output()
        .map_err(|err| format!("failed to launch {}: {err}", python.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("analyze_psd_context failed: {}", stderr.trim()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str::<VisualContext>(stdout.trim()).map_err(|err| {
        format!(
            "could not parse analyze_psd_context output: {err} (raw: {})",
            stdout.trim()
        )
    })
}
