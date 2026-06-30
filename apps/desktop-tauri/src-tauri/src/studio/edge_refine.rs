//! The `refineMaskEdge` node executor: bridges a graph node to the mask edge
//! refinement pipeline (`crate::psd::refine_mask_edge`), cleaning up a cut-out
//! subject's matte and exposing the refined image, the refined mask, and an
//! edge report as flat output ports.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::graph::{
    studio_output_map, studio_truthy, studio_value_to_number, studio_value_to_string,
    StudioGraphNode,
};
use crate::psd::refine_mask_edge;
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

pub(super) fn execute_studio_refine_mask_edge(
    node: &StudioGraphNode,
    inputs: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, String> {
    let image = studio_value_to_string(inputs.get("image"));
    if image.trim().is_empty() {
        return Err("Mask Edge Refine needs a connected image input".to_string());
    }

    let output_dir = {
        let configured = studio_value_to_string(node.params.get("output_dir"));
        if configured.trim().is_empty() {
            runtime_paths()?.output_dir.to_string_lossy().to_string()
        } else {
            configured
        }
    };

    let result = refine_mask_edge(
        None,
        image,
        optional(studio_value_to_string(inputs.get("mask"))),
        optional(studio_value_to_string(inputs.get("background"))),
        optional(studio_value_to_string(inputs.get("placeholder_mask"))),
        optional(studio_value_to_string(inputs.get("trimap"))),
        optional(studio_value_to_string(node.params.get("preset"))),
        Some(number_param(node, "erode_px", 1.0) as i64),
        Some(number_param(node, "dilate_px", 0.0) as i64),
        Some(number_param(node, "feather_px", 4.0)),
        Some(number_param(node, "guided_radius", 8.0) as i64),
        Some(bool_param(node, "edge_decontaminate", true)),
        Some(number_param(node, "background_blend_strength", 0.4)),
        Some(output_dir),
        optional(studio_value_to_string(node.params.get("output_name"))),
    )?;

    let report = serde_json::to_value(&result.edge_report)
        .map_err(|err| format!("failed to encode EdgeReport: {err}"))?;

    Ok(studio_output_map([
        ("refined_image", json!(result.refined_image)),
        ("refined_mask", json!(result.refined_mask)),
        ("edge_report", report),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node() -> StudioGraphNode {
        StudioGraphNode {
            id: "n1".to_string(),
            kind: "refineMaskEdge".to_string(),
            params: BTreeMap::new(),
        }
    }

    #[test]
    fn rejects_missing_image_input() {
        // No connected `image` input: must fail fast before shelling out to the
        // python bridge, with a clear message.
        let err = execute_studio_refine_mask_edge(&node(), &BTreeMap::new()).unwrap_err();
        assert!(err.contains("connected image input"), "{err}");
    }

    #[test]
    fn blank_image_input_is_rejected() {
        let mut inputs = BTreeMap::new();
        inputs.insert("image".to_string(), json!("   "));
        let err = execute_studio_refine_mask_edge(&node(), &inputs).unwrap_err();
        assert!(err.contains("connected image input"), "{err}");
    }

    #[test]
    fn param_defaults_match_python_bridge() {
        // The defaults wired here must mirror edge_refine_cli.py's argparse
        // defaults so an unconfigured node behaves identically to the CLI.
        let n = node();
        assert_eq!(number_param(&n, "erode_px", 1.0), 1.0);
        assert_eq!(number_param(&n, "dilate_px", 0.0), 0.0);
        assert_eq!(number_param(&n, "feather_px", 4.0), 4.0);
        assert_eq!(number_param(&n, "guided_radius", 8.0), 8.0);
        assert!(bool_param(&n, "edge_decontaminate", true));
        assert_eq!(number_param(&n, "background_blend_strength", 0.4), 0.4);
    }
}
