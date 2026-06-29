//! The `psdContextAnalyze` node executor: bridges a graph node to the PSD
//! context analysis pipeline (`crate::psd::analyze_psd_context`), turning a
//! connected (or param) PSD template into a structured `VisualContext` plus the
//! flat output ports downstream nodes wire to (prompt suffix, background
//! preview, placeholder mask, placeholder bounds).

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::graph::{studio_output_map, studio_value_to_string, StudioGraphNode};
use crate::psd::analyze_psd_context;
use crate::runtime_paths;

/// Split a multi-line param value into trimmed, non-empty lines.
fn lines(value: &str) -> Vec<String> {
    value
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

fn optional(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub(super) fn execute_studio_psd_context_analyze(
    node: &StudioGraphNode,
    inputs: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, String> {
    // A connected `template` input wins; otherwise fall back to the `psd_path`
    // param so the node also works as a standalone source.
    let template = {
        let wired = studio_value_to_string(inputs.get("template"));
        if wired.trim().is_empty() {
            studio_value_to_string(node.params.get("psd_path"))
        } else {
            wired
        }
    };
    if template.trim().is_empty() {
        return Err(
            "PSD Context Analyze needs a PSD template (connect a PSD Template node or set psd_path)"
                .to_string(),
        );
    }

    let output_dir = {
        let configured = studio_value_to_string(node.params.get("output_dir"));
        if configured.trim().is_empty() {
            runtime_paths()?.output_dir.to_string_lossy().to_string()
        } else {
            configured
        }
    };

    let references = lines(&studio_value_to_string(node.params.get("reference_layers")));

    let context = analyze_psd_context(
        None,
        template,
        optional(studio_value_to_string(node.params.get("background_layer"))),
        optional(studio_value_to_string(
            node.params.get("target_placeholder"),
        )),
        if references.is_empty() {
            None
        } else {
            Some(references)
        },
        Some(output_dir),
    )?;

    let visual_context = serde_json::to_value(&context)
        .map_err(|err| format!("failed to encode VisualContext: {err}"))?;
    let placeholder_bounds = serde_json::to_value(&context.placeholder.bounds)
        .map_err(|err| format!("failed to encode placeholder bounds: {err}"))?;

    Ok(studio_output_map([
        ("visual_context", visual_context),
        ("prompt_suffix", json!(context.prompt_suffix)),
        ("background_image", json!(context.background.image_path)),
        ("placeholder_mask", json!(context.placeholder.mask_path)),
        ("placeholder_bounds", placeholder_bounds),
    ]))
}
