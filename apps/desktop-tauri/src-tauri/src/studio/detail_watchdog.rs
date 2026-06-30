//! The `detailWatchdog` node executor: bridges a graph node to the CPU quality
//! watchdog (`crate::psd::detect_quality_issues`), scanning a candidate image
//! for local breakdowns (blur, halos, colour mismatch, missing resolution) and
//! exposing the (Phase 1 unchanged) image, the quality report, and the issue
//! overlay as flat output ports.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::graph::{studio_output_map, studio_value_to_string, StudioGraphNode};
use crate::psd::detect_quality_issues;
use crate::runtime_paths;

fn optional(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Encode an optional connected JSON input ({...}) as a string for the CLI.
fn encode_input(inputs: &BTreeMap<String, Value>, key: &str) -> Result<Option<String>, String> {
    match inputs.get(key) {
        Some(value) if !value.is_null() => {
            Ok(Some(serde_json::to_string(value).map_err(|err| {
                format!("failed to encode {key} input: {err}")
            })?))
        }
        _ => Ok(None),
    }
}

pub(super) fn execute_studio_detail_watchdog(
    node: &StudioGraphNode,
    inputs: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, String> {
    let image = studio_value_to_string(inputs.get("image"));
    if image.trim().is_empty() {
        return Err("Detail Watchdog needs a connected image input".to_string());
    }

    // Optional connected VisualContext (background colour + placeholder bounds)
    // and a standalone placeholder-bounds object; both forwarded as JSON.
    let visual_context = encode_input(inputs, "visual_context")?;
    let target_bounds = encode_input(inputs, "target_bounds")?;

    let output_dir = {
        let configured = studio_value_to_string(node.params.get("output_dir"));
        if configured.trim().is_empty() {
            runtime_paths()?.output_dir.to_string_lossy().to_string()
        } else {
            configured
        }
    };

    let result = detect_quality_issues(
        None,
        image,
        visual_context,
        target_bounds,
        optional(studio_value_to_string(node.params.get("watch_targets"))),
        optional(studio_value_to_string(node.params.get("mode"))),
        Some(output_dir),
        optional(studio_value_to_string(node.params.get("output_name"))),
    )?;

    let report = serde_json::to_value(&result.quality_report)
        .map_err(|err| format!("failed to encode QualityReport: {err}"))?;
    let watchdog = serde_json::to_value(&result.watchdog_report)
        .map_err(|err| format!("failed to encode WatchdogReport: {err}"))?;

    Ok(studio_output_map([
        ("fixed_image", json!(result.fixed_image)),
        ("quality_report", report),
        ("issue_masks", json!(result.issue_masks)),
        ("watchdog_report", watchdog),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::psd::WatchdogReport;

    fn node() -> StudioGraphNode {
        StudioGraphNode {
            id: "n1".to_string(),
            kind: "detailWatchdog".to_string(),
            params: BTreeMap::new(),
        }
    }

    #[test]
    fn rejects_missing_image_input() {
        // No connected `image` input: must fail fast before shelling out to the
        // python bridge, with a clear message.
        let err = execute_studio_detail_watchdog(&node(), &BTreeMap::new()).unwrap_err();
        assert!(err.contains("connected image input"), "{err}");
    }

    #[test]
    fn blank_image_input_is_rejected() {
        let mut inputs = BTreeMap::new();
        inputs.insert("image".to_string(), json!("   "));
        let err = execute_studio_detail_watchdog(&node(), &inputs).unwrap_err();
        assert!(err.contains("connected image input"), "{err}");
    }

    #[test]
    fn watchdog_report_parses_hardening_fields() {
        // The new v1 hardening fields must deserialize from the python bridge
        // JSON (and `mask_consumed` reflects the advisory Phase 1 mask).
        let value = json!({
            "mode": "balanced",
            "watch_targets": ["face", "product_edges"],
            "skipped_targets": ["hands"],
            "image_size": [128, 96],
            "target_size": null,
            "global_sharpness": 142.5,
            "source_mode": "CMYK",
            "exif_transposed": true,
            "max_decode_pixels": 96_000_000,
            "mask_consumed": false
        });
        let report: WatchdogReport = serde_json::from_value(value).unwrap();
        assert_eq!(report.source_mode, "CMYK");
        assert!(report.exif_transposed);
        assert_eq!(report.max_decode_pixels, 96_000_000);
        assert!(!report.mask_consumed);
    }

    #[test]
    fn watchdog_report_defaults_for_legacy_json() {
        // Older records lack the v1 fields; they must still deserialize with
        // safe defaults so historical runs remain readable.
        let report: WatchdogReport = serde_json::from_value(json!({
            "mode": "balanced",
            "global_sharpness": 80.0
        }))
        .unwrap();
        assert_eq!(report.source_mode, "");
        assert!(!report.exif_transposed);
        assert_eq!(report.max_decode_pixels, 0);
        assert!(!report.mask_consumed);
    }
}
