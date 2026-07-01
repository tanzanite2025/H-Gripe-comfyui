//! Local card processors that shell out to their Python bridge CLIs (colour
//! match, mask edge refine, image enhance, detail watchdog). Split out of
//! `psd.rs`; command names and result shapes are unchanged.

use serde::{Deserialize, Serialize};

use crate::contracts::QualityReport;

use super::{
    project_python, reject_unsafe_output_name, resolve_project_dir, run_bridge_oneshot,
    run_torch_cli,
};
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
    /// The match engine that actually ran (`cpu` heuristic or a backend id,
    /// e.g. `onnx_harmonize`).
    #[serde(default)]
    pub(crate) engine: String,
    /// The engine the node asked for (may differ from `engine` on fallback).
    #[serde(default)]
    pub(crate) engine_requested: String,
    /// Why the requested engine was not used (missing deps/weight, no
    /// background reference, …); else `null`.
    #[serde(default)]
    pub(crate) engine_fallback_reason: Option<String>,
    /// Weight file name when a learned backend ran, else `null`.
    #[serde(default)]
    pub(crate) backend_model: Option<String>,
    /// Compute device the learned backend bound (`cpu`/`cuda`); `null` on the
    /// CPU heuristic path, which runs no ML session.
    #[serde(default)]
    pub(crate) device: Option<String>,
    /// Compute device the node asked for (`auto`/`cpu`/`cuda`); an explicit
    /// `cuda` degrades to `cpu` when no accelerator provider is present.
    #[serde(default)]
    pub(crate) device_requested: String,
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
    engine: Option<String>,
    device: Option<String>,
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
        .arg("--engine")
        .arg(engine.as_deref().unwrap_or("cpu"))
        .arg("--device")
        .arg(device.as_deref().unwrap_or("auto"))
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
    /// The matte engine that actually ran (`cpu` heuristic or a backend id,
    /// e.g. `onnx_matting`).
    #[serde(default)]
    pub(crate) engine: String,
    /// The engine the node asked for (may differ from `engine` on fallback).
    #[serde(default)]
    pub(crate) engine_requested: String,
    /// Why the requested engine was not used (missing deps/weight, no trimap,
    /// unknown engine, runtime error); else null.
    #[serde(default)]
    pub(crate) engine_fallback_reason: Option<String>,
    /// The weight file the backend loaded (`null` on the CPU path).
    #[serde(default)]
    pub(crate) backend_model: Option<String>,
    /// Compute device the learned backend bound (`cpu`/`cuda`); `null` on the
    /// CPU heuristic path, which runs no ML session.
    #[serde(default)]
    pub(crate) device: Option<String>,
    /// Compute device the node asked for (`auto`/`cpu`/`cuda`); an explicit
    /// `cuda` degrades to `cpu` when no accelerator provider is present.
    #[serde(default)]
    pub(crate) device_requested: String,
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
    engine: Option<String>,
    device: Option<String>,
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
        .arg("--engine")
        .arg(engine.as_deref().unwrap_or("cpu"))
        .arg("--device")
        .arg(device.as_deref().unwrap_or("auto"))
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
    /// Compute device the model backend bound (`cpu`/`cuda`); `null` on the CPU
    /// resize path, which runs no ML session.
    #[serde(default)]
    pub(crate) device: Option<String>,
    /// Compute device the node asked for (`auto`/`cpu`/`cuda`); an explicit
    /// `cuda` degrades to `cpu` when no accelerator is present.
    #[serde(default)]
    pub(crate) device_requested: String,
    /// Compute precision the model backend bound (`fp16`/`fp32`); `null` on the
    /// CPU resize path, which runs no ML session.
    #[serde(default)]
    pub(crate) precision: Option<String>,
    /// Compute precision the node asked for (`auto`/`fp32`/`fp16`); an explicit
    /// `fp16` degrades to `fp32` on a CPU run.
    #[serde(default)]
    pub(crate) precision_requested: String,
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
    device: Option<String>,
    precision: Option<String>,
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

    let engine = engine.as_deref().unwrap_or("cpu");
    let mut argv: Vec<String> = vec![
        "--image".into(),
        image,
        "--mode".into(),
        mode.as_deref().unwrap_or("conservative").into(),
        "--target-width".into(),
        target_width.unwrap_or(0).to_string(),
        "--target-height".into(),
        target_height.unwrap_or(0).to_string(),
        "--target-bounds-json".into(),
        target_bounds.as_deref().unwrap_or("").into(),
        "--target-dpi".into(),
        target_dpi.unwrap_or(300).to_string(),
        "--max-pixels".into(),
        max_pixels.unwrap_or(48_000_000).to_string(),
        "--scale".into(),
        scale.unwrap_or(2.0).to_string(),
        "--denoise-strength".into(),
        denoise_strength.unwrap_or(0.3).to_string(),
        "--texture-strength".into(),
        texture_strength.unwrap_or(0.25).to_string(),
        "--engine".into(),
        engine.into(),
        "--device".into(),
        device.as_deref().unwrap_or("auto").into(),
        "--precision".into(),
        precision.as_deref().unwrap_or("auto").into(),
        "--output-dir".into(),
        output_dir.as_deref().unwrap_or("").into(),
        "--output-name".into(),
        output_name.as_deref().unwrap_or("").into(),
    ];
    if preserve_text_logo.unwrap_or(true) {
        argv.push("--preserve-text-logo".into());
    }

    // Only the torch engine (`realesrgan`) reloads a heavy model per call, so
    // only it is routed through the warm worker; the always-available CPU path
    // stays a one-shot and never spawns a worker.
    let stdout = if engine == "realesrgan" {
        run_torch_cli(&python, &dir, "image_enhance_cli.py", "image_enhance", &argv)
    } else {
        run_bridge_oneshot(&python, &dir, "image_enhance_cli.py", &argv)
    }
    .map_err(|err| format!("enhance_image failed: {err}"))?;
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
    /// Compute device the learned detector actually bound (`cpu`/`cuda`); `null`
    /// on the rule-only path, which runs no ML session.
    #[serde(default)]
    pub(crate) device: Option<String>,
    /// Compute device the node asked for (`auto`/`cpu`/`cuda`); an explicit
    /// `cuda` degrades to `cpu` when no accelerator provider is present.
    #[serde(default)]
    pub(crate) device_requested: String,
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
    device: Option<String>,
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
        .arg("--device")
        .arg(device.as_deref().unwrap_or("auto"))
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
