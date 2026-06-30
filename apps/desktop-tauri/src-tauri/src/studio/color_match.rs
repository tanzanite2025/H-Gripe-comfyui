//! The `matchLightColor` node executor: bridges a graph node to the light &
//! colour matching pipeline (`crate::psd::match_light_color`), nudging a
//! connected subject image toward a PSD background and exposing the matched
//! image, the match report, and a prompt suffix as flat output ports.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::graph::{
    studio_output_map, studio_truthy, studio_value_to_number, studio_value_to_string,
    StudioGraphNode,
};
use crate::psd::match_light_color;
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

pub(super) fn execute_studio_match_light_color(
    node: &StudioGraphNode,
    inputs: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, String> {
    let image = studio_value_to_string(inputs.get("image"));
    if image.trim().is_empty() {
        return Err("Light & Color Match needs a connected image input".to_string());
    }

    // The upstream `visual_context` arrives as a JSON object; forward it as a
    // serialized string for the prompt suffix (None when nothing is wired).
    let context = match inputs.get("visual_context") {
        Some(value) if !value.is_null() => Some(
            serde_json::to_string(value)
                .map_err(|err| format!("failed to encode visual_context: {err}"))?,
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

    let result = match_light_color(
        None,
        image,
        optional(studio_value_to_string(inputs.get("background"))),
        optional(studio_value_to_string(inputs.get("mask"))),
        context,
        optional(studio_value_to_string(node.params.get("mode"))),
        Some(number_param(node, "strength", 0.6)),
        Some(number_param(node, "shadow_strength", 0.0)),
        Some(number_param(node, "highlight_strength", 0.0)),
        Some(bool_param(node, "protect_saturation", false)),
        Some(bool_param(node, "protect_brand_color", true)),
        // `engine` selects the opt-in learned matcher (default `cpu`); the bridge
        // falls back to the always-on CPU heuristic when it is unavailable.
        optional(studio_value_to_string(node.params.get("engine"))),
        Some(output_dir),
        optional(studio_value_to_string(node.params.get("output_name"))),
    )?;

    let report = serde_json::to_value(&result.match_report)
        .map_err(|err| format!("failed to encode MatchReport: {err}"))?;

    Ok(studio_output_map([
        ("matched_image", json!(result.matched_image)),
        ("match_report", report),
        ("prompt_suffix", json!(result.prompt_suffix)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node() -> StudioGraphNode {
        StudioGraphNode {
            id: "n1".to_string(),
            kind: "matchLightColor".to_string(),
            params: BTreeMap::new(),
        }
    }

    #[test]
    fn rejects_missing_image_input() {
        // No connected `image` input: must fail fast before shelling out to the
        // python bridge, with a clear message.
        let err = execute_studio_match_light_color(&node(), &BTreeMap::new()).unwrap_err();
        assert!(err.contains("connected image"), "{err}");
    }

    #[test]
    fn blank_image_input_is_rejected() {
        let mut inputs = BTreeMap::new();
        inputs.insert("image".to_string(), json!("   "));
        let err = execute_studio_match_light_color(&node(), &inputs).unwrap_err();
        assert!(err.contains("connected image"), "{err}");
    }

    #[test]
    fn number_and_bool_params_fall_back_to_defaults() {
        // Mirrors the defaults the executor passes to the python bridge so a
        // change to either side is caught here.
        let node = node();
        assert_eq!(number_param(&node, "strength", 0.6), 0.6);
        assert!(!bool_param(&node, "protect_saturation", false));
        assert!(bool_param(&node, "protect_brand_color", true));
    }
}
