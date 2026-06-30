//! PSD tooling: listing exported PSD triplets and shelling out to the vendored
//! `compose_psd_cli.py` / `inspect_psd_cli.py` helpers. These reuse the
//! project's bundled Python interpreter (the portable `python_embeded` layout
//! when present, otherwise PATH `python`) so the proven psd-tools pipeline runs
//! without any separate runtime install.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::contracts::{QualityReport, RepaintReport, VisualContext};
use crate::modified_ms;
use crate::studio::studio_reject_unsafe_basename;

/// The Tauri resource directory captured at startup (see `set_resource_dir`).
/// When the installer bundles the `h-gripe.project.json` marker together with
/// the `python/bridge`, `custom_nodes` and `third_party` subtree under
/// `bundle.resources`, this directory *is* a self-contained project root, so the
/// PSD nodes keep working in a packaged build without the user pointing at a
/// separate source checkout.
static RESOURCE_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Record the bundled resource directory. Called once from the Tauri `setup`
/// hook; ignored if the resolver could not determine a resource path.
pub(crate) fn set_resource_dir(dir: Option<PathBuf>) {
    let _ = RESOURCE_DIR.set(dir);
}

fn resource_dir() -> Option<PathBuf> {
    RESOURCE_DIR.get().cloned().flatten()
}

/// A directory is an H-Gripe project root when it holds the explicit
/// `h-gripe.project.json` marker or the `python/bridge` runtime the PSD nodes
/// invoke. This intentionally no longer depends on ComfyUI's `main.py`, so the
/// ComfyUI main body can be removed without breaking the PSD nodes.
fn is_project_root(base: &Path) -> bool {
    base.join("h-gripe.project.json").is_file() || base.join("python").join("bridge").is_dir()
}

/// Accept `base` as the project root only if it looks like an H-Gripe project,
/// otherwise fail fast with an actionable message.
fn require_project_root(base: PathBuf) -> Result<PathBuf, String> {
    if is_project_root(&base) {
        Ok(base)
    } else {
        Err(format!(
            "not an H-Gripe project folder: {} (expected h-gripe.project.json or python/bridge; \
             set the project folder or HGRIPE_PROJECT_DIR)",
            base.display()
        ))
    }
}

/// Resolve the project directory that hosts the vendored `python/bridge`
/// helpers. Resolution order, first match wins:
///   1. the caller-provided path (the folder picked in the UI),
///   2. the `HGRIPE_PROJECT_DIR` environment variable (a packaging launcher can
///      point at the extracted project root without any UI),
///   3. the process working directory when it is a project root (the repo root
///      in dev),
///   4. the bundled Tauri resource directory (a packaged install).
///
/// Every branch requires an H-Gripe project root (`is_project_root`) so a
/// misconfigured folder fails fast.
pub(crate) fn resolve_project_dir(dir: &Option<String>) -> Result<PathBuf, String> {
    if let Some(d) = dir.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        return require_project_root(PathBuf::from(d));
    }
    if let Some(env_dir) = std::env::var_os("HGRIPE_PROJECT_DIR")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
    {
        return require_project_root(env_dir);
    }
    if let Ok(cwd) = std::env::current_dir() {
        if is_project_root(&cwd) {
            return Ok(cwd);
        }
    }
    if let Some(res) = resource_dir() {
        if is_project_root(&res) {
            return Ok(res);
        }
    }
    Err(
        "no H-Gripe project root found in the working directory or bundled resources \
         (expected h-gripe.project.json or python/bridge; set the project folder or \
         HGRIPE_PROJECT_DIR)"
            .to_string(),
    )
}

/// Pick a Python interpreter: prefer the portable `python_embeded` shipped in
/// the project root (the Windows embeddable layout), otherwise fall back to
/// PATH `python` / `python3`.
pub(crate) fn project_python(dir: &Path) -> PathBuf {
    for candidate in [
        dir.join("python_embeded").join("python.exe"),
        dir.join("python_embeded").join("python"),
    ] {
        if candidate.is_file() {
            return candidate;
        }
    }
    PathBuf::from(if cfg!(windows) { "python" } else { "python3" })
}

/// Validate a user-supplied `output_name` before handing it to a Python CLI
/// that joins it onto the output directory (`directory / f"{stem}.png"`). An
/// empty name is allowed (the CLI picks its own `<image>_<suffix>` default); a
/// non-empty name must be a plain basename so an untrusted workflow cannot use
/// `..` or a path separator to redirect the write outside the chosen folder.
fn reject_unsafe_output_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Ok(());
    }
    studio_reject_unsafe_basename(name)
}

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

/// Mean colour / colour temperature / contrast of the corrected region, before
/// or after matching. Mirrors the Python bridge's `_appearance`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ColorAppearance {
    #[serde(default)]
    pub(crate) mean_color: [u8; 3],
    #[serde(default)]
    pub(crate) color_temperature: u32,
    #[serde(default)]
    pub(crate) contrast: f64,
}

/// What `match_light_color` did: the mode/parameters, before/after appearance,
/// and (for the transfer modes) the Lab statistics it matched against. Fields
/// are `snake_case` to match the `color_match_cli.py` JSON.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct MatchReport {
    #[serde(default)]
    pub(crate) mode: String,
    #[serde(default)]
    pub(crate) strength: f64,
    #[serde(default)]
    pub(crate) shadow_strength: f64,
    #[serde(default)]
    pub(crate) highlight_strength: f64,
    #[serde(default)]
    pub(crate) protect_saturation: bool,
    #[serde(default)]
    pub(crate) protect_brand_color: bool,
    /// `false` for `prompt_only`, zero strength, or no background reference.
    #[serde(default)]
    pub(crate) applied: bool,
    #[serde(default)]
    pub(crate) before: ColorAppearance,
    #[serde(default)]
    pub(crate) after: ColorAppearance,
    /// Lab mean/std used by the transfer (absent for `histogram_match`).
    #[serde(default)]
    pub(crate) src_mean_lab: Option<Vec<f64>>,
    #[serde(default)]
    pub(crate) dst_mean_lab: Option<Vec<f64>>,
    #[serde(default)]
    pub(crate) src_std_lab: Option<Vec<f64>>,
    #[serde(default)]
    pub(crate) dst_std_lab: Option<Vec<f64>>,
    /// Set when the subject was passed through unchanged for a notable reason.
    #[serde(default)]
    pub(crate) note: Option<String>,
    /// `[width, height]` of the written image.
    #[serde(default)]
    pub(crate) output_size: Option<[i64; 2]>,
}

/// Result of the **Light & Color Match** node: the written matched image, a
/// prompt suffix (for prompt-side alignment), and the [`MatchReport`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ColorMatchResult {
    #[serde(default)]
    pub(crate) matched_image: String,
    #[serde(default)]
    pub(crate) prompt_suffix: String,
    #[serde(default)]
    pub(crate) match_report: MatchReport,
}

/// Match a generated subject image's light & colour toward a PSD background so
/// the composite stops looking pasted-on. This is the **Light & Color Match**
/// node's backend (the second PSD production node): it consumes the upstream
/// image, the background preview, and optionally the [`VisualContext`] from PSD
/// Context Analyze.
///
/// Like the other PSD commands it shells out to `python/bridge/color_match_cli.py`
/// using the project's bundled Python (Pillow + numpy, no OpenCV in Phase 1).
/// `mode` is `prompt_only | color_transfer | histogram_match | hybrid`; the
/// correction is weighted toward shadows/highlights and (when
/// `protect_brand_color`) spares high-chroma pixels. `context` is the
/// serialized `VisualContext` JSON used for the prompt suffix.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) fn match_light_color(
    dir: Option<String>,
    image: String,
    background: Option<String>,
    mask: Option<String>,
    context: Option<String>,
    mode: Option<String>,
    strength: Option<f64>,
    shadow_strength: Option<f64>,
    highlight_strength: Option<f64>,
    protect_saturation: Option<bool>,
    protect_brand_color: Option<bool>,
    output_dir: Option<String>,
    output_name: Option<String>,
) -> Result<ColorMatchResult, String> {
    let dir = resolve_project_dir(&dir)?;
    let python = project_python(&dir);
    let script = dir.join("python").join("bridge").join("color_match_cli.py");
    if !script.is_file() {
        return Err(format!(
            "color_match_cli.py not found at {}",
            script.display()
        ));
    }
    reject_unsafe_output_name(output_name.as_deref().unwrap_or(""))?;

    let mut cmd = std::process::Command::new(&python);
    cmd.arg(&script)
        .arg("--image")
        .arg(&image)
        .arg("--background")
        .arg(background.as_deref().unwrap_or(""))
        .arg("--mask")
        .arg(mask.as_deref().unwrap_or(""))
        .arg("--context")
        .arg(context.as_deref().unwrap_or(""))
        .arg("--mode")
        .arg(mode.as_deref().unwrap_or("color_transfer"))
        .arg("--strength")
        .arg(strength.unwrap_or(0.6).to_string())
        .arg("--shadow-strength")
        .arg(shadow_strength.unwrap_or(0.0).to_string())
        .arg("--highlight-strength")
        .arg(highlight_strength.unwrap_or(0.0).to_string())
        .arg("--output-dir")
        .arg(output_dir.as_deref().unwrap_or(""))
        .arg("--output-name")
        .arg(output_name.as_deref().unwrap_or(""))
        .current_dir(&dir);
    if protect_saturation.unwrap_or(false) {
        cmd.arg("--protect-saturation");
    }
    if protect_brand_color.unwrap_or(false) {
        cmd.arg("--protect-brand-color");
    }
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
        return Err(format!("match_light_color failed: {}", stderr.trim()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str::<ColorMatchResult>(stdout.trim()).map_err(|err| {
        format!(
            "could not parse match_light_color output: {err} (raw: {})",
            stdout.trim()
        )
    })
}

/// What `refine_mask_edge` did: the resolved preset/morphology parameters, the
/// edge-band size and the mask coverage before/after. Fields are `snake_case`
/// to match the `edge_refine_cli.py` JSON.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct EdgeReport {
    #[serde(default)]
    pub(crate) preset: String,
    /// `explicit` when a mask was connected, else `alpha` (the image's own).
    #[serde(default)]
    pub(crate) source_mask: String,
    #[serde(default)]
    pub(crate) erode_px: i64,
    #[serde(default)]
    pub(crate) dilate_px: i64,
    #[serde(default)]
    pub(crate) feather_px: f64,
    #[serde(default)]
    pub(crate) guided_radius: i64,
    #[serde(default)]
    pub(crate) edge_decontaminate: bool,
    #[serde(default)]
    pub(crate) background_blend_strength: f64,
    /// `true` when a background was connected and blended into the edge band.
    #[serde(default)]
    pub(crate) background_applied: bool,
    #[serde(default)]
    pub(crate) edge_band_px: i64,
    #[serde(default)]
    pub(crate) coverage_before: f64,
    #[serde(default)]
    pub(crate) coverage_after: f64,
    /// `[width, height]` of the written images.
    #[serde(default)]
    pub(crate) output_size: Option<[i64; 2]>,
}

/// Result of the **Mask Edge Refine** node: the written refined RGBA image, the
/// refined matte (as a grayscale PNG), and the [`EdgeReport`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct RefineEdgeResult {
    #[serde(default)]
    pub(crate) refined_image: String,
    #[serde(default)]
    pub(crate) refined_mask: String,
    #[serde(default)]
    pub(crate) edge_report: EdgeReport,
}

/// Clean up a cut-out subject's matte so it drops into a PSD placeholder without
/// white halos, fringing or jagged semi-transparent edges. This is the **Mask
/// Edge Refine** node's backend (the third PSD production node): it consumes the
/// subject image, an optional explicit matte (defaults to the image's alpha),
/// and an optional target background for edge colour blending.
///
/// Like the other PSD commands it shells out to `python/bridge/edge_refine_cli.py`
/// using the project's bundled Python (Pillow + numpy, no OpenCV in Phase 1).
/// `preset` is `clean | natural | soft | custom`; the numeric morphology
/// parameters apply only when `preset` is `custom`.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) fn refine_mask_edge(
    dir: Option<String>,
    image: String,
    mask: Option<String>,
    background: Option<String>,
    placeholder_mask: Option<String>,
    trimap: Option<String>,
    preset: Option<String>,
    erode_px: Option<i64>,
    dilate_px: Option<i64>,
    feather_px: Option<f64>,
    guided_radius: Option<i64>,
    edge_decontaminate: Option<bool>,
    background_blend_strength: Option<f64>,
    output_dir: Option<String>,
    output_name: Option<String>,
) -> Result<RefineEdgeResult, String> {
    let dir = resolve_project_dir(&dir)?;
    let python = project_python(&dir);
    let script = dir.join("python").join("bridge").join("edge_refine_cli.py");
    if !script.is_file() {
        return Err(format!(
            "edge_refine_cli.py not found at {}",
            script.display()
        ));
    }
    reject_unsafe_output_name(output_name.as_deref().unwrap_or(""))?;

    let mut cmd = std::process::Command::new(&python);
    cmd.arg(&script)
        .arg("--image")
        .arg(&image)
        .arg("--mask")
        .arg(mask.as_deref().unwrap_or(""))
        .arg("--background")
        .arg(background.as_deref().unwrap_or(""))
        .arg("--placeholder-mask")
        .arg(placeholder_mask.as_deref().unwrap_or(""))
        .arg("--trimap")
        .arg(trimap.as_deref().unwrap_or(""))
        .arg("--preset")
        .arg(preset.as_deref().unwrap_or("natural"))
        .arg("--erode-px")
        .arg(erode_px.unwrap_or(1).to_string())
        .arg("--dilate-px")
        .arg(dilate_px.unwrap_or(0).to_string())
        .arg("--feather-px")
        .arg(feather_px.unwrap_or(4.0).to_string())
        .arg("--guided-radius")
        .arg(guided_radius.unwrap_or(8).to_string())
        .arg("--background-blend-strength")
        .arg(background_blend_strength.unwrap_or(0.4).to_string())
        .arg("--output-dir")
        .arg(output_dir.as_deref().unwrap_or(""))
        .arg("--output-name")
        .arg(output_name.as_deref().unwrap_or(""))
        .current_dir(&dir);
    if edge_decontaminate.unwrap_or(false) {
        cmd.arg("--edge-decontaminate");
    }
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
        return Err(format!("refine_mask_edge failed: {}", stderr.trim()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str::<RefineEdgeResult>(stdout.trim()).map_err(|err| {
        format!(
            "could not parse refine_mask_edge output: {err} (raw: {})",
            stdout.trim()
        )
    })
}

/// What `enhance_image` did: the resolved mode, source/output/target sizes, the
/// applied scale factor and the per-step strengths. Fields are `snake_case` to
/// match the `image_enhance_cli.py` JSON.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct EnhanceReport {
    #[serde(default)]
    pub(crate) mode: String,
    #[serde(default)]
    pub(crate) scale_factor: f64,
    /// `[width, height]` of the input image.
    #[serde(default)]
    pub(crate) source_size: Option<[i64; 2]>,
    /// `[width, height]` of the written image.
    #[serde(default)]
    pub(crate) output_size: Option<[i64; 2]>,
    /// `[width, height]` requested target, or `null` when a preset scale was used.
    #[serde(default)]
    pub(crate) target_size: Option<[i64; 2]>,
    #[serde(default)]
    pub(crate) target_dpi: u32,
    #[serde(default)]
    pub(crate) max_pixels: i64,
    /// `true` when the scale was reduced to honour `max_pixels`.
    #[serde(default)]
    pub(crate) clamped: bool,
    #[serde(default)]
    pub(crate) denoise_strength: f64,
    #[serde(default)]
    pub(crate) texture_strength: f64,
    #[serde(default)]
    pub(crate) preserve_text_logo: bool,
    /// The upscale engine actually used (`cpu` or a backend id, e.g. `realesrgan`).
    #[serde(default)]
    pub(crate) engine: String,
    /// The engine the node asked for (may differ from `engine` on fallback).
    #[serde(default)]
    pub(crate) engine_requested: String,
    /// Why the requested engine was not used (missing deps/weight, downscale, …).
    #[serde(default)]
    pub(crate) engine_fallback_reason: Option<String>,
    /// Weight file name when a model backend ran, else `null`.
    #[serde(default)]
    pub(crate) backend_model: Option<String>,
    #[serde(default)]
    pub(crate) processing_time_ms: i64,
}

/// Result of the **Image Enhance** node: the written enhanced image, the actual
/// scale factor applied, and the [`EnhanceReport`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct EnhanceImageResult {
    #[serde(default)]
    pub(crate) enhanced_image: String,
    #[serde(default)]
    pub(crate) scale_factor: f64,
    #[serde(default)]
    pub(crate) enhance_report: EnhanceReport,
}

/// Upscale and sharpen a low-resolution subject so it fills a PSD placeholder at
/// the target DPI without going soft. This is the **Image Enhance / Super
/// Resolution** node's backend (the fourth PSD production node): it consumes the
/// base image plus an optional target size (explicit pixels or a connected
/// placeholder-bounds object) and returns the enhanced image and a report.
///
/// Like the other PSD commands it shells out to `python/bridge/image_enhance_cli.py`
/// using the project's bundled Python (Pillow + numpy; CPU-only in Phase 1, no
/// GPU super-resolution). `mode` is `conservative | texture_rebuild | print_ready
/// | custom`; the numeric strengths and `scale` apply only when `mode` is `custom`.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) fn enhance_image(
    dir: Option<String>,
    image: String,
    target_bounds: Option<String>,
    mode: Option<String>,
    target_width: Option<i64>,
    target_height: Option<i64>,
    target_dpi: Option<i64>,
    max_pixels: Option<i64>,
    scale: Option<f64>,
    denoise_strength: Option<f64>,
    texture_strength: Option<f64>,
    preserve_text_logo: Option<bool>,
    engine: Option<String>,
    output_dir: Option<String>,
    output_name: Option<String>,
) -> Result<EnhanceImageResult, String> {
    let dir = resolve_project_dir(&dir)?;
    let python = project_python(&dir);
    let script = dir
        .join("python")
        .join("bridge")
        .join("image_enhance_cli.py");
    if !script.is_file() {
        return Err(format!(
            "image_enhance_cli.py not found at {}",
            script.display()
        ));
    }
    reject_unsafe_output_name(output_name.as_deref().unwrap_or(""))?;

    let mut cmd = std::process::Command::new(&python);
    cmd.arg(&script)
        .arg("--image")
        .arg(&image)
        .arg("--mode")
        .arg(mode.as_deref().unwrap_or("conservative"))
        .arg("--target-width")
        .arg(target_width.unwrap_or(0).to_string())
        .arg("--target-height")
        .arg(target_height.unwrap_or(0).to_string())
        .arg("--target-bounds-json")
        .arg(target_bounds.as_deref().unwrap_or(""))
        .arg("--target-dpi")
        .arg(target_dpi.unwrap_or(300).to_string())
        .arg("--max-pixels")
        .arg(max_pixels.unwrap_or(48_000_000).to_string())
        .arg("--scale")
        .arg(scale.unwrap_or(2.0).to_string())
        .arg("--denoise-strength")
        .arg(denoise_strength.unwrap_or(0.3).to_string())
        .arg("--texture-strength")
        .arg(texture_strength.unwrap_or(0.25).to_string())
        .arg("--engine")
        .arg(engine.as_deref().unwrap_or("cpu"))
        .arg("--output-dir")
        .arg(output_dir.as_deref().unwrap_or(""))
        .arg("--output-name")
        .arg(output_name.as_deref().unwrap_or(""))
        .current_dir(&dir);
    if preserve_text_logo.unwrap_or(true) {
        cmd.arg("--preserve-text-logo");
    }
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
        return Err(format!("enhance_image failed: {}", stderr.trim()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str::<EnhanceImageResult>(stdout.trim()).map_err(|err| {
        format!(
            "could not parse enhance_image output: {err} (raw: {})",
            stdout.trim()
        )
    })
}

/// Diagnostic summary of a Detail Watchdog run: the resolved mode, which watch
/// targets ran, which were skipped (CPU Phase 1 cannot do hands/text/logo), and
/// the measured global sharpness. Fields are `snake_case` to match the
/// `detail_watchdog_cli.py` JSON.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct WatchdogReport {
    #[serde(default)]
    pub(crate) mode: String,
    #[serde(default)]
    pub(crate) watch_targets: Vec<String>,
    #[serde(default)]
    pub(crate) skipped_targets: Vec<String>,
    /// `[width, height]` of the analysed image.
    #[serde(default)]
    pub(crate) image_size: Option<[i64; 2]>,
    /// `[width, height]` of the connected placeholder target, when available.
    #[serde(default)]
    pub(crate) target_size: Option<[i64; 2]>,
    /// Laplacian-variance sharpness of the whole image (higher = sharper).
    #[serde(default)]
    pub(crate) global_sharpness: f64,
    /// Pillow mode of the decoded source before normalising to 8-bit RGB
    /// (e.g. `RGB`, `RGBA`, `CMYK`, `I;16`, `P`).
    #[serde(default)]
    pub(crate) source_mode: String,
    /// Whether an EXIF orientation tag was applied to upright the input.
    #[serde(default)]
    pub(crate) exif_transposed: bool,
    /// Decode-pixel ceiling enforced before decoding (0 disables the guard).
    #[serde(default)]
    pub(crate) max_decode_pixels: i64,
    /// Whether the optional `--mask` was consumed. Phase 1 detection runs on the
    /// image's own alpha rim, so the supplied matte is advisory only (`false`).
    #[serde(default)]
    pub(crate) mask_consumed: bool,
    /// Detection engine that actually ran: `rules` (always-on CPU baseline) or a
    /// learned detector id (e.g. `onnx_defect`) when its deps/weight were present.
    #[serde(default)]
    pub(crate) engine: String,
    /// Engine the node asked for (may differ from `engine` on fallback).
    #[serde(default)]
    pub(crate) engine_requested: String,
    /// Why the rule-only path was used when an ML engine was requested but could
    /// not run (missing dep/weight, unknown engine, runtime error); else null.
    #[serde(default)]
    pub(crate) engine_fallback_reason: Option<String>,
    /// Learned detector passes that ran on top of the rule layer.
    #[serde(default)]
    pub(crate) detectors: Vec<String>,
    /// File name of the weight the ML detector loaded, when one ran.
    #[serde(default)]
    pub(crate) backend_model: Option<String>,
}

/// Result of the **Detail Watchdog** node: the (unchanged, Phase 1) candidate
/// image, the shared [`QualityReport`], an optional issue-overlay PNG, and the
/// [`WatchdogReport`] diagnostics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct DetectQualityResult {
    #[serde(default)]
    pub(crate) fixed_image: String,
    #[serde(default)]
    pub(crate) quality_report: QualityReport,
    #[serde(default)]
    pub(crate) issue_masks: Option<String>,
    #[serde(default)]
    pub(crate) watchdog_report: WatchdogReport,
}

/// Scan a candidate image for local quality breakdowns (blur, halos, colour
/// mismatch, missing resolution) and emit a [`QualityReport`]. This is the
/// **Detail Watchdog** node's backend (the fifth PSD production node).
///
/// Phase 1 is **detect + report only** (no automatic repaint) and shells out to
/// `python/bridge/detail_watchdog_cli.py` using the project's bundled Python
/// (Pillow + numpy; no OpenCV, no ML). `mode` is `strict | balanced | lenient`;
/// `watch_targets` is a comma list of `face,hands,text,logo,product_edges`
/// (hands/text/logo need the later GPU/VLM backend and are reported as skipped).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) fn detect_quality_issues(
    dir: Option<String>,
    image: String,
    visual_context: Option<String>,
    target_bounds: Option<String>,
    watch_targets: Option<String>,
    mode: Option<String>,
    engine: Option<String>,
    output_dir: Option<String>,
    output_name: Option<String>,
) -> Result<DetectQualityResult, String> {
    let dir = resolve_project_dir(&dir)?;
    let python = project_python(&dir);
    let script = dir
        .join("python")
        .join("bridge")
        .join("detail_watchdog_cli.py");
    if !script.is_file() {
        return Err(format!(
            "detail_watchdog_cli.py not found at {}",
            script.display()
        ));
    }
    reject_unsafe_output_name(output_name.as_deref().unwrap_or(""))?;

    let mut cmd = std::process::Command::new(&python);
    cmd.arg(&script)
        .arg("--image")
        .arg(&image)
        .arg("--mode")
        .arg(mode.as_deref().unwrap_or("balanced"))
        .arg("--watch-targets")
        .arg(watch_targets.as_deref().unwrap_or(""))
        .arg("--engine")
        .arg(engine.as_deref().unwrap_or("rules"))
        .arg("--visual-context")
        .arg(visual_context.as_deref().unwrap_or(""))
        .arg("--target-bounds")
        .arg(target_bounds.as_deref().unwrap_or(""))
        .arg("--output-dir")
        .arg(output_dir.as_deref().unwrap_or(""))
        .arg("--output-name")
        .arg(output_name.as_deref().unwrap_or(""))
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
        return Err(format!("detect_quality_issues failed: {}", stderr.trim()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str::<DetectQualityResult>(stdout.trim()).map_err(|err| {
        format!(
            "could not parse detect_quality_issues output: {err} (raw: {})",
            stdout.trim()
        )
    })
}

/// Availability of one `engine` option for a card, as reported by a CLI
/// `--probe-engines` call. `available=false` carries a human `reason` the UI
/// shows when greying the option out.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct EngineAvailability {
    #[serde(default)]
    pub(crate) available: bool,
    #[serde(default)]
    pub(crate) reason: String,
}

/// Engine capability probe for one card (node kind): which `engine` values its
/// CLI can actually run right now. `error` is set (engines empty) when the probe
/// itself could not run, so the UI degrades to "all enabled" rather than hiding
/// the always-available CPU path.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct CardEngineProbe {
    /// The node kind whose `engine` param these cover (e.g. `imageEnhance`).
    pub(crate) node_kind: String,
    /// The bridge CLI that produced the probe.
    pub(crate) cli: String,
    /// Engine id -> availability (e.g. `cpu`/`realesrgan`, `rules`/`onnx_defect`).
    pub(crate) engines: BTreeMap<String, EngineAvailability>,
    /// Why the probe could not run, when `engines` is empty.
    #[serde(default)]
    pub(crate) error: Option<String>,
}

/// Cross-card engine capability report (the `doctor`-style probe). Aggregates
/// every local card that exposes an opt-in ML `engine` seam so the UI can grey
/// out engines whose deps/weights are missing on this box.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct EngineProbeReport {
    pub(crate) cards: Vec<CardEngineProbe>,
    /// The shared weight cache (`HGRIPE_MODEL_CACHE` or the bundled dir).
    #[serde(default)]
    pub(crate) model_cache_dir: Option<String>,
}

/// Shape of a single CLI's `--probe-engines` JSON. `engines` carries extra
/// per-engine fields (e.g. `native_scale`) that the UI does not need, so we
/// only pull `available` + `reason`.
#[derive(Debug, Clone, Deserialize)]
struct CliEngineProbe {
    #[serde(default)]
    engines: BTreeMap<String, EngineAvailability>,
    #[serde(default)]
    model_cache_dir: Option<String>,
}

/// Run one card CLI's `--probe-engines` and parse its JSON.
fn run_engine_probe(python: &Path, dir: &Path, cli_name: &str) -> Result<CliEngineProbe, String> {
    let script = dir.join("python").join("bridge").join(cli_name);
    if !script.is_file() {
        return Err(format!("{cli_name} not found at {}", script.display()));
    }
    let mut cmd = std::process::Command::new(python);
    cmd.arg(&script).arg("--probe-engines").current_dir(dir);
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
        return Err(format!("{cli_name} --probe-engines failed: {}", stderr.trim()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str::<CliEngineProbe>(stdout.trim())
        .map_err(|err| format!("could not parse {cli_name} probe: {err} (raw: {})", stdout.trim()))
}

/// Probe the opt-in ML `engine` seams across the local cards (the `doctor`
/// cross-card capability report). The CPU/rule baseline is always available; a
/// learned engine reports `available=false` with a reason when its optional
/// dependency or weight is missing, which the inspector uses to grey out the
/// option (and fall back to the baseline). A card whose probe fails to run
/// returns an `error` and no engines, so the UI leaves its select untouched.
#[tauri::command]
pub(crate) fn probe_engines(dir: Option<String>) -> Result<EngineProbeReport, String> {
    let dir = resolve_project_dir(&dir)?;
    let python = project_python(&dir);

    // (node kind, CLI) for every card that exposes an `engine` param.
    const CARDS: [(&str, &str); 3] = [
        ("imageEnhance", "image_enhance_cli.py"),
        ("detailWatchdog", "detail_watchdog_cli.py"),
        ("detailRepaint", "detail_repaint_cli.py"),
    ];

    let mut cards = Vec::with_capacity(CARDS.len());
    let mut model_cache_dir = None;
    for (node_kind, cli_name) in CARDS {
        let probe = run_engine_probe(&python, &dir, cli_name);
        let card = match probe {
            Ok(parsed) => {
                if model_cache_dir.is_none() {
                    model_cache_dir = parsed.model_cache_dir.clone();
                }
                CardEngineProbe {
                    node_kind: node_kind.to_string(),
                    cli: cli_name.to_string(),
                    engines: parsed.engines,
                    error: None,
                }
            }
            Err(err) => CardEngineProbe {
                node_kind: node_kind.to_string(),
                cli: cli_name.to_string(),
                engines: BTreeMap::new(),
                error: Some(err),
            },
        };
        cards.push(card);
    }

    Ok(EngineProbeReport {
        cards,
        model_cache_dir,
    })
}

/// One issue region prepared for repaint: the padded crop + same-size inpaint
/// mask the orchestrator sends to the provider, plus the geometry the composite
/// step needs to paste the result back. Fields are `snake_case` to match the
/// `detail_repaint_cli.py` manifest; extra fields are tolerated.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct PreparedRepaintRegion {
    #[serde(default)]
    pub(crate) index: u32,
    #[serde(rename = "type", default)]
    pub(crate) issue_type: Option<String>,
    #[serde(default)]
    pub(crate) confidence: f64,
    #[serde(default)]
    pub(crate) suggested_action: Option<String>,
    #[serde(default)]
    pub(crate) bbox: [i64; 4],
    #[serde(default)]
    pub(crate) crop_box: [i64; 4],
    #[serde(default)]
    pub(crate) inner_box: [i64; 4],
    #[serde(default)]
    pub(crate) size: [i64; 2],
    /// Path to the padded crop PNG (the provider `image.edit` image input).
    #[serde(default)]
    pub(crate) crop_path: String,
    /// Path to the same-size inpaint mask PNG (the provider mask input).
    #[serde(default)]
    pub(crate) mask_path: String,
}

/// Result of the **Detail Repaint** prepare step: the regions selected from the
/// quality report (each with a crop + mask to send to the provider) and the
/// issues that were skipped (with reasons).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct PrepareRepaintResult {
    #[serde(default)]
    pub(crate) regions: Vec<PreparedRepaintRegion>,
    #[serde(default)]
    pub(crate) skipped: Vec<serde_json::Value>,
    #[serde(default)]
    pub(crate) image_size: [i64; 2],
    #[serde(default)]
    pub(crate) selected_count: u32,
    /// `true` when the inpaint mask marks the edit area transparent (OpenAI
    /// convention); `false` when inverted (opaque/white = edit).
    #[serde(default)]
    pub(crate) mask_edit_is_transparent: bool,
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

/// Result of the **Detail Repaint** composite step: the fixed image (issue
/// cores repainted and edge-fused back in) and the per-region [`RepaintReport`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct CompositeRepaintResult {
    #[serde(default)]
    pub(crate) fixed_image: String,
    #[serde(default)]
    pub(crate) repaint_report: RepaintReport,
}

#[cfg(windows)]
fn no_window(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    // CREATE_NO_WINDOW: don't pop a console window for the child.
    cmd.creation_flags(0x0800_0000);
}

#[cfg(not(windows))]
fn no_window(_cmd: &mut std::process::Command) {}

/// Resolve the project's `python/bridge/detail_repaint_cli.py`, erroring if the
/// helper is missing from the checkout / bundle.
fn detail_repaint_script(dir: &Path) -> Result<PathBuf, String> {
    let script = dir
        .join("python")
        .join("bridge")
        .join("detail_repaint_cli.py");
    if !script.is_file() {
        return Err(format!(
            "detail_repaint_cli.py not found at {}",
            script.display()
        ));
    }
    Ok(script)
}

/// Crop each repaintable issue region out of a candidate image and write a
/// same-size inpaint mask for it. This is the first half of the **Detail
/// Repaint** node (the Phase-2 follow-up to Detail Watchdog): the orchestrator
/// then sends each returned crop + mask + repaint prompt to a provider's
/// `image.edit` operation before calling [`composite_repaint`] to paste the
/// results back.
///
/// Shells out to `python/bridge/detail_repaint_cli.py prepare` using the
/// project's bundled Python (Pillow + numpy; no OpenCV, no ML). Only issues
/// whose `suggested_action` is in `repaint_actions` (default `detail_redraw`)
/// and at/above `min_confidence` are selected, highest-confidence first, capped
/// at `max_regions`.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_repaint_regions(
    dir: Option<String>,
    image: String,
    quality_report: Option<String>,
    repaint_actions: Option<String>,
    min_confidence: Option<f64>,
    padding: Option<i64>,
    max_regions: Option<i64>,
    invert_mask: Option<bool>,
    output_dir: Option<String>,
    output_name: Option<String>,
) -> Result<PrepareRepaintResult, String> {
    let dir = resolve_project_dir(&dir)?;
    let python = project_python(&dir);
    let script = detail_repaint_script(&dir)?;
    reject_unsafe_output_name(output_name.as_deref().unwrap_or(""))?;

    let mut cmd = std::process::Command::new(&python);
    cmd.arg(&script)
        .arg("prepare")
        .arg("--image")
        .arg(&image)
        .arg("--quality-report")
        .arg(quality_report.as_deref().unwrap_or(""))
        .arg("--repaint-actions")
        .arg(repaint_actions.as_deref().unwrap_or(""))
        .arg("--min-confidence")
        .arg(min_confidence.unwrap_or(0.0).to_string())
        .arg("--padding")
        .arg(padding.unwrap_or(24).to_string())
        .arg("--max-regions")
        .arg(max_regions.unwrap_or(8).to_string())
        .arg("--output-dir")
        .arg(output_dir.as_deref().unwrap_or(""))
        .arg("--output-name")
        .arg(output_name.as_deref().unwrap_or(""))
        .current_dir(&dir);
    if invert_mask.unwrap_or(false) {
        cmd.arg("--invert-mask");
    }
    no_window(&mut cmd);

    let output = cmd
        .output()
        .map_err(|err| format!("failed to launch {}: {err}", python.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("prepare_repaint_regions failed: {}", stderr.trim()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str::<PrepareRepaintResult>(stdout.trim()).map_err(|err| {
        format!(
            "could not parse prepare_repaint_regions output: {err} (raw: {})",
            stdout.trim()
        )
    })
}

/// Paste the provider-repainted crops back into the candidate image, fusing
/// each patch seam with a feathered alpha (the "secondary edge fusion"), and
/// write the final fixed image. This is the second half of the **Detail
/// Repaint** node.
///
/// `manifest` is the JSON returned by [`prepare_repaint_regions`]; `repainted`
/// is a JSON list of `{index, path}` mapping each region to the crop the
/// provider returned (regions with no entry stay unrepainted). Shells out to
/// `python/bridge/detail_repaint_cli.py composite`.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) fn composite_repaint(
    dir: Option<String>,
    image: String,
    manifest: String,
    repainted: String,
    feather_px: Option<f64>,
    output_dir: Option<String>,
    output_name: Option<String>,
) -> Result<CompositeRepaintResult, String> {
    let dir = resolve_project_dir(&dir)?;
    let python = project_python(&dir);
    let script = detail_repaint_script(&dir)?;
    reject_unsafe_output_name(output_name.as_deref().unwrap_or(""))?;

    let mut cmd = std::process::Command::new(&python);
    cmd.arg(&script)
        .arg("composite")
        .arg("--image")
        .arg(&image)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--repainted")
        .arg(&repainted)
        .arg("--feather-px")
        .arg(feather_px.unwrap_or(0.0).to_string())
        .arg("--output-dir")
        .arg(output_dir.as_deref().unwrap_or(""))
        .arg("--output-name")
        .arg(output_name.as_deref().unwrap_or(""))
        .current_dir(&dir);
    no_window(&mut cmd);

    let output = cmd
        .output()
        .map_err(|err| format!("failed to launch {}: {err}", python.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("composite_repaint failed: {}", stderr.trim()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str::<CompositeRepaintResult>(stdout.trim()).map_err(|err| {
        format!(
            "could not parse composite_repaint output: {err} (raw: {})",
            stdout.trim()
        )
    })
}

/// One repainted crop produced by the local inpaint backend: the region
/// `index` and the path to the regenerated crop PNG, ready to feed straight
/// into [`composite_repaint`]'s `repainted` list.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct LocalRepaintedCrop {
    #[serde(default)]
    pub(crate) index: u32,
    #[serde(default)]
    pub(crate) path: String,
}

/// Result of the **Detail Repaint** local `repaint` step: the regenerated crops
/// plus the engine telemetry the UI uses to explain a fallback to the remote
/// provider. An empty `repainted` list (with a `engine_fallback_reason`) means
/// the orchestrator should run its remote `image.edit` loop instead.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct LocalRepaintResult {
    #[serde(default)]
    pub(crate) repainted: Vec<LocalRepaintedCrop>,
    #[serde(default)]
    pub(crate) skipped: Vec<serde_json::Value>,
    /// Engine that actually ran (`provider` when no local backend was used).
    #[serde(default)]
    pub(crate) engine: String,
    /// Engine the node asked for (differs from `engine` on fallback).
    #[serde(default)]
    pub(crate) engine_requested: String,
    /// Why the local backend was not used (provider selected, missing deps/weight).
    #[serde(default)]
    pub(crate) engine_fallback_reason: Option<String>,
    /// Weight name when a local backend ran, else null.
    #[serde(default)]
    pub(crate) backend_model: Option<String>,
    #[serde(default)]
    pub(crate) requested_count: u32,
    #[serde(default)]
    pub(crate) repainted_count: u32,
}

/// Run the opt-in **local** inpaint backend over a prepare manifest, an
/// alternative to the remote `image.edit` provider for the **Detail Repaint**
/// node (Phase 2, `docs/phase2-algorithm-roadmap.md` §3). `provider` (the
/// default) or any backend whose optional deps/weights are missing yields an
/// empty `repainted` list and a recorded reason, so the orchestrator falls back
/// to the remote provider — this never hard-fails on a box without the model.
///
/// `manifest` is the JSON returned by [`prepare_repaint_regions`]; the returned
/// `repainted` list feeds straight into [`composite_repaint`]. Shells out to
/// `python/bridge/detail_repaint_cli.py repaint`.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) fn local_repaint_regions(
    dir: Option<String>,
    manifest: String,
    engine: Option<String>,
    prompt: Option<String>,
    prompt_map: Option<String>,
    negative_prompt: Option<String>,
    strength: Option<f64>,
    guidance_scale: Option<f64>,
    steps: Option<i64>,
    seed: Option<i64>,
    output_dir: Option<String>,
    output_name: Option<String>,
) -> Result<LocalRepaintResult, String> {
    let dir = resolve_project_dir(&dir)?;
    let python = project_python(&dir);
    let script = detail_repaint_script(&dir)?;
    reject_unsafe_output_name(output_name.as_deref().unwrap_or(""))?;

    let mut cmd = std::process::Command::new(&python);
    cmd.arg(&script)
        .arg("repaint")
        .arg("--manifest")
        .arg(&manifest)
        .arg("--engine")
        .arg(engine.as_deref().unwrap_or("provider"))
        .arg("--prompt")
        .arg(prompt.as_deref().unwrap_or(""))
        .arg("--prompt-map")
        .arg(prompt_map.as_deref().unwrap_or(""))
        .arg("--negative-prompt")
        .arg(negative_prompt.as_deref().unwrap_or(""))
        .arg("--strength")
        .arg(strength.unwrap_or(0.75).to_string())
        .arg("--guidance-scale")
        .arg(guidance_scale.unwrap_or(7.5).to_string())
        .arg("--steps")
        .arg(steps.unwrap_or(30).to_string())
        .arg("--seed")
        .arg(seed.unwrap_or(-1).to_string())
        .arg("--output-dir")
        .arg(output_dir.as_deref().unwrap_or(""))
        .arg("--output-name")
        .arg(output_name.as_deref().unwrap_or(""))
        .current_dir(&dir);
    no_window(&mut cmd);

    let output = cmd
        .output()
        .map_err(|err| format!("failed to launch {}: {err}", python.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("local_repaint_regions failed: {}", stderr.trim()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str::<LocalRepaintResult>(stdout.trim()).map_err(|err| {
        format!(
            "could not parse local_repaint_regions output: {err} (raw: {})",
            stdout.trim()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{
        is_project_root, reject_unsafe_output_name, require_project_root, resolve_project_dir,
        CliEngineProbe, EngineProbeReport,
    };
    use std::fs;
    use std::path::PathBuf;

    fn unique_tmp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("hgripe_psd_{tag}_{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn require_project_root_accepts_folder_with_marker() {
        let dir = unique_tmp_dir("root_marker");
        fs::write(dir.join("h-gripe.project.json"), b"{}\n").unwrap();
        assert_eq!(require_project_root(dir.clone()).unwrap(), dir);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn require_project_root_accepts_folder_with_python_bridge() {
        let dir = unique_tmp_dir("root_bridge");
        fs::create_dir_all(dir.join("python").join("bridge")).unwrap();
        assert!(is_project_root(&dir));
        assert_eq!(require_project_root(dir.clone()).unwrap(), dir);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn require_project_root_rejects_unmarked_folder() {
        let dir = unique_tmp_dir("root_missing");
        assert!(!is_project_root(&dir));
        let err = require_project_root(dir.clone()).unwrap_err();
        assert!(err.contains("not an H-Gripe project folder"));
        assert!(err.contains("HGRIPE_PROJECT_DIR"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_project_dir_uses_explicit_folder_when_valid() {
        let dir = unique_tmp_dir("explicit");
        fs::write(dir.join("h-gripe.project.json"), b"{}\n").unwrap();
        let resolved = resolve_project_dir(&Some(dir.to_string_lossy().to_string())).unwrap();
        assert_eq!(resolved, dir);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_project_dir_errors_on_unmarked_explicit_folder() {
        let dir = unique_tmp_dir("explicit_bad");
        let err = resolve_project_dir(&Some(dir.to_string_lossy().to_string())).unwrap_err();
        assert!(err.contains("not an H-Gripe project folder"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn output_name_allows_empty_so_cli_picks_default() {
        assert!(reject_unsafe_output_name("").is_ok());
        assert!(reject_unsafe_output_name("   ").is_ok());
    }

    #[test]
    fn output_name_allows_plain_basenames() {
        assert!(reject_unsafe_output_name("matched").is_ok());
        assert!(reject_unsafe_output_name("  result  ").is_ok());
        assert!(reject_unsafe_output_name("my.output").is_ok());
    }

    #[test]
    fn cli_engine_probe_parses_card_probe_json() {
        // Mirror a `detail_watchdog_cli.py --probe-engines` payload: the CPU/rule
        // baseline is available, the ML engine is not, and extra per-engine
        // fields (e.g. native_scale) are tolerated.
        let raw = r#"{
            "engines": {
                "rules": {"available": true, "reason": "built-in CPU rule layer"},
                "onnx_defect": {"available": false, "reason": "missing optional dependency: onnxruntime", "native_scale": null}
            },
            "model_cache_dir": "/cache/models"
        }"#;
        let probe: CliEngineProbe = serde_json::from_str(raw).unwrap();
        assert_eq!(probe.model_cache_dir.as_deref(), Some("/cache/models"));
        assert!(probe.engines["rules"].available);
        assert!(!probe.engines["onnx_defect"].available);
        assert!(probe.engines["onnx_defect"].reason.contains("onnxruntime"));
    }

    #[test]
    fn engine_probe_report_round_trips() {
        // The cross-card report serialises to the shape the UI bridge expects.
        let raw = r#"{
            "cards": [
                {"node_kind": "imageEnhance", "cli": "image_enhance_cli.py",
                 "engines": {"cpu": {"available": true, "reason": "built-in CPU path"}}},
                {"node_kind": "detailWatchdog", "cli": "detail_watchdog_cli.py",
                 "engines": {}, "error": "probe failed"}
            ],
            "model_cache_dir": null
        }"#;
        let report: EngineProbeReport = serde_json::from_str(raw).unwrap();
        assert_eq!(report.cards.len(), 2);
        assert_eq!(report.cards[0].node_kind, "imageEnhance");
        assert!(report.cards[0].engines["cpu"].available);
        assert!(report.cards[0].error.is_none());
        assert_eq!(report.cards[1].error.as_deref(), Some("probe failed"));
        assert!(report.cards[1].engines.is_empty());
    }

    #[test]
    fn local_repaint_result_parses_backend_and_fallback() {
        // A successful local run: a backend ran and returned one repainted crop.
        let raw = r#"{
            "repainted": [{"index": 0, "path": "/out/hero_region0_repainted.png"}],
            "skipped": [],
            "engine": "sd_inpaint",
            "engine_requested": "sd_inpaint",
            "engine_fallback_reason": null,
            "backend_model": "sd-inpaint",
            "requested_count": 1,
            "repainted_count": 1
        }"#;
        let res: super::LocalRepaintResult = serde_json::from_str(raw).unwrap();
        assert_eq!(res.engine, "sd_inpaint");
        assert_eq!(res.repainted.len(), 1);
        assert_eq!(res.repainted[0].index, 0);
        assert!(res.engine_fallback_reason.is_none());
        assert_eq!(res.backend_model.as_deref(), Some("sd-inpaint"));

        // The provider-fallback shape: no local repaint, a recorded reason.
        let fallback = r#"{
            "repainted": [],
            "engine": "provider",
            "engine_requested": "sd_inpaint",
            "engine_fallback_reason": "missing optional dependency: torch",
            "requested_count": 2,
            "repainted_count": 0
        }"#;
        let res: super::LocalRepaintResult = serde_json::from_str(fallback).unwrap();
        assert_eq!(res.engine, "provider");
        assert!(res.repainted.is_empty());
        assert_eq!(
            res.engine_fallback_reason.as_deref(),
            Some("missing optional dependency: torch")
        );
        assert!(res.backend_model.is_none());
    }

    #[test]
    fn output_name_rejects_traversal_and_separators() {
        assert!(reject_unsafe_output_name(".").is_err());
        assert!(reject_unsafe_output_name("..").is_err());
        assert!(reject_unsafe_output_name("../evil").is_err());
        assert!(reject_unsafe_output_name("..\\evil").is_err());
        assert!(reject_unsafe_output_name("sub/dir").is_err());
        assert!(reject_unsafe_output_name("/etc/passwd").is_err());
    }
}
