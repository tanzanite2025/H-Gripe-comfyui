//! The `psdExport` node executor: bridges a graph node to the desktop PSD
//! composition pipeline (`crate::psd::compose_psd`), turning connected image /
//! template inputs into a composed `.psd` plus preview/metadata outputs.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::graph::{studio_output_map, studio_value_to_string, StudioGraphNode};
use crate::psd::compose_psd;
use crate::runtime_paths;

pub(super) fn execute_studio_psd_export(
    node: &StudioGraphNode,
    inputs: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, String> {
    let image = studio_value_to_string(inputs.get("image"));
    if image.is_empty() {
        return Err("PSD Export needs a connected image input".to_string());
    }
    let template = studio_value_to_string(inputs.get("template"));
    if template.is_empty() {
        return Err("PSD Export needs a connected PSD template input".to_string());
    }

    let output_dir = {
        let configured = studio_value_to_string(node.params.get("output_dir"));
        if configured.trim().is_empty() {
            runtime_paths()?.output_dir.to_string_lossy().to_string()
        } else {
            configured
        }
    };
    let filename = {
        let configured = studio_value_to_string(node.params.get("filename"));
        if configured.trim().is_empty() {
            "final".to_string()
        } else {
            configured
        }
    };
    let placeholder_name = studio_value_to_string(node.params.get("placeholder"));
    let placeholder = if placeholder_name.trim().is_empty() {
        None
    } else {
        Some(json!({ "name": placeholder_name }).to_string())
    };

    // Optional explicit matte (e.g. Mask Edge Refine's `refined_mask`) applied
    // as the image's alpha before compositing.
    let mask = Some(
        studio_value_to_string(inputs.get("mask"))
            .trim()
            .to_string(),
    )
    .filter(|value| !value.is_empty());

    // Optional upstream production metadata (any JSON object) merged into the
    // exported `_metadata.json`.
    let metadata = match inputs.get("metadata") {
        Some(value) if !value.is_null() => Some(
            serde_json::to_string(value)
                .map_err(|err| format!("failed to encode metadata input: {err}"))?,
        ),
        _ => None,
    };

    let result = compose_psd(
        None,
        template,
        image,
        mask,
        output_dir,
        Some(filename),
        placeholder,
        Some(
            studio_value_to_string(node.params.get("fit_mode"))
                .trim()
                .to_string(),
        )
        .filter(|value| !value.is_empty()),
        None,
        Some(
            studio_value_to_string(node.params.get("smart_object_mode"))
                .trim()
                .to_string(),
        )
        .filter(|value| !value.is_empty()),
        None,
        metadata,
        None,
    )?;

    if result.status != "succeeded" {
        return Err(format!("PSD export failed: {}", result.status));
    }

    let result_json = serde_json::to_value(&result)
        .map_err(|err| format!("failed to encode ComposePsdResult: {err}"))?;
    Ok(studio_output_map([
        ("psdPath", json!(result.psd_path)),
        ("previewPath", json!(result.preview_path)),
        ("metadataPath", json!(result.metadata_path)),
        ("placeholderKind", json!(result.placeholder_kind)),
        ("smartObjectMode", json!(result.smart_object_mode)),
        ("result", result_json),
    ]))
}
