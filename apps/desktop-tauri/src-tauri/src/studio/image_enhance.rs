//! The `imageEnhance` node executor. The default `cpu` engine runs the
//! in-process native-Rust pipeline ([`super::image_enhance_cpu`]); a learned
//! engine (`realesrgan`, …) — or an input the fast path cannot reproduce
//! faithfully (a CMYK JPEG or float source) — is served by the colour-managed
//! Python bridge (`crate::psd::enhance_image`). Both paths upscale/sharpen a
//! low-resolution subject to a PSD placeholder's pixel target and expose the
//! enhanced image, the applied scale factor, and an enhance report as flat
//! output ports with identical shape.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::graph::{
    bool_param, number_param, optional, resolve_output_dir, studio_output_map,
    studio_value_to_string, StudioGraphNode,
};
use super::image_enhance_cpu::{self, CpuEnhanceParams};
use crate::psd::{enhance_image, EnhanceImageResult};

pub(super) fn execute_studio_image_enhance(
    node: &StudioGraphNode,
    inputs: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, String> {
    let image = studio_value_to_string(inputs.get("image"));
    if image.trim().is_empty() {
        return Err("Image Enhance needs a connected image input".to_string());
    }

    // Optional connected placeholder bounds ({x, y, width, height}) used to
    // auto-derive the target size; forwarded to the CLI as a JSON string.
    let target_bounds = match inputs.get("target_bounds") {
        Some(value) if !value.is_null() => Some(
            serde_json::to_string(value)
                .map_err(|err| format!("failed to encode target_bounds input: {err}"))?,
        ),
        _ => None,
    };

    let output_dir = resolve_output_dir(node)?;
    let mode = optional(studio_value_to_string(node.params.get("mode")));
    let target_width = number_param(node, "target_width", 0.0) as i64;
    let target_height = number_param(node, "target_height", 0.0) as i64;
    let target_dpi = number_param(node, "target_dpi", 300.0) as i64;
    let max_pixels = number_param(node, "max_pixels", 48_000_000.0) as i64;
    let scale = number_param(node, "scale", 2.0);
    let denoise_strength = number_param(node, "denoise_strength", 0.3);
    let texture_strength = number_param(node, "texture_strength", 0.25);
    let preserve_text_logo = bool_param(node, "preserve_text_logo", true);
    let engine = optional(studio_value_to_string(node.params.get("engine")));
    // `device` selects the compute device for the learned upscaler (default
    // `auto`); ignored by the CPU resize path.
    let device = optional(studio_value_to_string(node.params.get("device")));
    // `precision` selects fp16/fp32 for the learned upscaler (default `auto`);
    // ignored by the CPU resize path.
    let precision = optional(studio_value_to_string(node.params.get("precision")));
    let output_name = optional(studio_value_to_string(node.params.get("output_name")));

    // The default `cpu` engine runs in-process; a learned engine — or an input
    // the fast path cannot reproduce faithfully — falls through to Python.
    let engine_is_cpu = engine
        .as_deref()
        .map(|e| e.trim().eq_ignore_ascii_case("cpu"))
        .unwrap_or(true);
    if engine_is_cpu {
        let cpu_params = CpuEnhanceParams {
            image_path: image.clone(),
            output_dir: output_dir.clone(),
            output_name: output_name.clone(),
            mode: mode.clone(),
            target_bounds: target_bounds.clone(),
            target_width,
            target_height,
            target_dpi,
            max_pixels,
            scale,
            denoise_strength,
            texture_strength,
            preserve_text_logo,
            device_requested: device.clone().unwrap_or_else(|| "auto".to_string()),
            precision_requested: precision.clone().unwrap_or_else(|| "auto".to_string()),
        };
        if let Some(result) = image_enhance_cpu::try_enhance(&cpu_params)? {
            return to_output_map(result);
        }
    }

    let result = enhance_image(
        None,
        image,
        target_bounds,
        mode,
        Some(target_width),
        Some(target_height),
        Some(target_dpi),
        Some(max_pixels),
        Some(scale),
        Some(denoise_strength),
        Some(texture_strength),
        Some(preserve_text_logo),
        engine,
        device,
        precision,
        Some(output_dir),
        output_name,
    )?;

    to_output_map(result)
}

/// Encode an [`EnhanceImageResult`] into the node's flat output ports. Shared
/// by the in-process and Python paths so both emit an identical output shape.
fn to_output_map(result: EnhanceImageResult) -> Result<BTreeMap<String, Value>, String> {
    let report = serde_json::to_value(&result.enhance_report)
        .map_err(|err| format!("failed to encode EnhanceReport: {err}"))?;

    Ok(studio_output_map([
        ("enhanced_image", json!(result.enhanced_image)),
        ("scale_factor", json!(result.scale_factor)),
        ("enhance_report", report),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node() -> StudioGraphNode {
        StudioGraphNode {
            id: "n1".to_string(),
            kind: "imageEnhance".to_string(),
            params: BTreeMap::new(),
        }
    }

    #[test]
    fn rejects_missing_image_input() {
        // No connected `image` input: must fail fast before shelling out to the
        // python bridge, with a clear message.
        let err = execute_studio_image_enhance(&node(), &BTreeMap::new()).unwrap_err();
        assert!(err.contains("connected image input"), "{err}");
    }

    #[test]
    fn blank_image_input_is_rejected() {
        let mut inputs = BTreeMap::new();
        inputs.insert("image".to_string(), json!("   "));
        let err = execute_studio_image_enhance(&node(), &inputs).unwrap_err();
        assert!(err.contains("connected image input"), "{err}");
    }
}
