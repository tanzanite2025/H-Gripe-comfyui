//! Detail Repaint pipeline: prepare repaintable regions, composite provider
//! results back, and the local (in-process torch) repaint path. Split out of
//! `psd.rs`; command names and result shapes are unchanged.

use serde::{Deserialize, Serialize};

use crate::contracts::RepaintReport;

use super::{
    apply_model_env, detail_repaint_script, no_window, project_python, reject_unsafe_output_name,
    resolve_project_dir, run_bridge_oneshot, run_torch_cli,
};
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
    apply_model_env(&mut cmd);
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
    blend: Option<String>,
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
        .arg("--blend")
        .arg(blend.as_deref().unwrap_or("feather"))
        .arg("--output-dir")
        .arg(output_dir.as_deref().unwrap_or(""))
        .arg("--output-name")
        .arg(output_name.as_deref().unwrap_or(""))
        .current_dir(&dir);
    apply_model_env(&mut cmd);
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
    /// Compute device the local backend bound (`cpu`/`cuda`); `null` on the
    /// remote `provider` path, which runs no local session.
    #[serde(default)]
    pub(crate) device: Option<String>,
    /// Compute precision the local backend bound (`fp16`/`fp32`); `null` on the
    /// remote `provider` path, which runs no local session.
    #[serde(default)]
    pub(crate) precision: Option<String>,
    /// Compute precision the node asked for (`auto`/`fp32`/`fp16`); an explicit
    /// `fp16` degrades to `fp32` on a CPU run.
    #[serde(default)]
    pub(crate) precision_requested: String,
    /// Structural conditioning the node asked for (`off`/`canny`); a backend
    /// that cannot honour it degrades to the provider with a recorded reason.
    #[serde(default)]
    pub(crate) controlnet_requested: String,
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
    precision: Option<String>,
    controlnet: Option<String>,
    output_dir: Option<String>,
    output_name: Option<String>,
) -> Result<LocalRepaintResult, String> {
    let dir = resolve_project_dir(&dir)?;
    let python = project_python(&dir);
    // Existence check (keeps the precise "not found" message); the launch path
    // below re-resolves the script itself.
    detail_repaint_script(&dir)?;
    reject_unsafe_output_name(output_name.as_deref().unwrap_or(""))?;

    let engine = engine.as_deref().unwrap_or("provider");
    let argv: Vec<String> = vec![
        "repaint".into(),
        "--manifest".into(),
        manifest,
        "--engine".into(),
        engine.into(),
        "--prompt".into(),
        prompt.as_deref().unwrap_or("").into(),
        "--prompt-map".into(),
        prompt_map.as_deref().unwrap_or("").into(),
        "--negative-prompt".into(),
        negative_prompt.as_deref().unwrap_or("").into(),
        "--strength".into(),
        strength.unwrap_or(0.75).to_string(),
        "--guidance-scale".into(),
        guidance_scale.unwrap_or(7.5).to_string(),
        "--steps".into(),
        steps.unwrap_or(30).to_string(),
        "--seed".into(),
        seed.unwrap_or(-1).to_string(),
        "--precision".into(),
        precision.as_deref().unwrap_or("auto").into(),
        "--controlnet".into(),
        controlnet.as_deref().unwrap_or("off").into(),
        "--output-dir".into(),
        output_dir.as_deref().unwrap_or("").into(),
        "--output-name".into(),
        output_name.as_deref().unwrap_or("").into(),
    ];

    // Only the torch backends load a heavy pipeline per call, so only they are
    // routed through the warm worker; the default `provider` (remote
    // `image.edit`) and any other engine stay a one-shot.
    let stdout = if matches!(engine, "sd_inpaint" | "sdxl_inpaint" | "flux_fill") {
        run_torch_cli(
            &python,
            &dir,
            "detail_repaint_cli.py",
            "detail_repaint",
            &argv,
        )
    } else {
        run_bridge_oneshot(&python, &dir, "detail_repaint_cli.py", &argv)
    }
    .map_err(|err| format!("local_repaint_regions failed: {err}"))?;
    serde_json::from_str::<LocalRepaintResult>(stdout.trim()).map_err(|err| {
        format!(
            "could not parse local_repaint_regions output: {err} (raw: {})",
            stdout.trim()
        )
    })
}
