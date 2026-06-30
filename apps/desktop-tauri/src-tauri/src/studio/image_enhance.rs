//! The `imageEnhance` node executor: bridges a graph node to the CPU image
//! enhancement pipeline (`crate::psd::enhance_image`), upscaling and sharpening
//! a low-resolution subject to a PSD placeholder's pixel target and exposing the
//! enhanced image, the applied scale factor, and an enhance report as flat
//! output ports.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::graph::{
    studio_output_map, studio_truthy, studio_value_to_number, studio_value_to_string,
    StudioGraphNode,
};
use crate::psd::enhance_image;
use crate::runtime_paths;

fn optional(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Read a numeric param, falling back to `default` when the key is absent.
fn number_param(node: &StudioGraphNode, key: &str, default: f64) -> f64 {
    match node.params.get(key) {
        Some(value) => studio_value_to_number(Some(value)),
        None => default,
    }
}

/// Read a boolean param, falling back to `default` when the key is absent.
fn bool_param(node: &StudioGraphNode, key: &str, default: bool) -> bool {
    node.params.get(key).map(studio_truthy).unwrap_or(default)
}

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

    let output_dir = {
        let configured = studio_value_to_string(node.params.get("output_dir"));
        if configured.trim().is_empty() {
            runtime_paths()?.output_dir.to_string_lossy().to_string()
        } else {
            configured
        }
    };

    let result = enhance_image(
        None,
        image,
        target_bounds,
        optional(studio_value_to_string(node.params.get("mode"))),
        Some(number_param(node, "target_width", 0.0) as i64),
        Some(number_param(node, "target_height", 0.0) as i64),
        Some(number_param(node, "target_dpi", 300.0) as i64),
        Some(number_param(node, "max_pixels", 48_000_000.0) as i64),
        Some(number_param(node, "scale", 2.0)),
        Some(number_param(node, "denoise_strength", 0.3)),
        Some(number_param(node, "texture_strength", 0.25)),
        Some(bool_param(node, "preserve_text_logo", true)),
        optional(studio_value_to_string(node.params.get("engine"))),
        // `device` selects the compute device for the learned upscaler (default
        // `auto`); ignored by the CPU resize path.
        optional(studio_value_to_string(node.params.get("device"))),
        Some(output_dir),
        optional(studio_value_to_string(node.params.get("output_name"))),
    )?;

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
