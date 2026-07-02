//! The `detailRepaint` node executor: localized issue-region repaint built on
//! top of a Detail Watchdog `QualityReport`. Crops each repaintable issue (via
//! `prepare_repaint_regions`), sends each crop + inpaint mask + repaint prompt
//! through the broker's `image.edit` operation (the same provider/credentials
//! path as `generate`), then pastes the results back with a feathered seam
//! (`composite_repaint`). Outputs the fixed image and a `RepaintReport`.
//!
//! When no `image.edit`-capable provider is configured (empty or `mock`), the
//! provider loop is skipped and the node passes the image through unchanged
//! (`repaint_report.status == "unchanged"`), mirroring the mock behaviour of
//! the other production nodes.

use std::collections::BTreeMap;

use hgripe_api::{ApiStatus, ApiTask, OutputType};
use serde_json::{json, Value};

use super::api_call::{
    execute_and_record_cancellable, studio_param_f64, studio_param_i64, studio_task_id,
};
use super::graph::{studio_non_empty, studio_output_map, studio_value_to_string, StudioGraphNode};
use super::run_cancel::StudioRunCancels;
use super::run_events::{studio_api_error_detail, StudioNodeErrorDetail, StudioRunLogger};
use crate::psd::{composite_repaint, prepare_repaint_regions};

pub(super) async fn execute_studio_detail_repaint(
    node: &StudioGraphNode,
    inputs: &BTreeMap<String, Value>,
    cancels: &tauri::State<'_, StudioRunCancels>,
    run_id: &str,
    logger: &StudioRunLogger<'_>,
) -> Result<BTreeMap<String, Value>, StudioNodeErrorDetail> {
    let image = studio_value_to_string(inputs.get("image"));
    if image.trim().is_empty() {
        return Err("Detail Repaint needs a connected image input"
            .to_string()
            .into());
    }

    // The QualityReport from Detail Watchdog, forwarded to the CLI as JSON.
    let quality_report = match inputs.get("quality_report") {
        Some(value) if !value.is_null() => Some(
            serde_json::to_string(value)
                .map_err(|err| format!("failed to encode quality_report input: {err}"))?,
        ),
        _ => None,
    };

    let output_dir = {
        let configured = studio_value_to_string(node.params.get("output_dir"));
        if configured.trim().is_empty() {
            crate::runtime_paths()?
                .output_dir
                .to_string_lossy()
                .to_string()
        } else {
            configured
        }
    };
    let output_name = {
        let configured = studio_value_to_string(node.params.get("output_name"));
        let trimmed = configured.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    };

    let repaint_actions = {
        let configured = studio_value_to_string(node.params.get("repaint_actions"));
        let trimmed = configured.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    };
    let min_confidence = studio_param_f64(node, "min_confidence");
    let padding = studio_param_i64(node, "region_padding");
    let max_regions = studio_param_i64(node, "max_regions");
    let feather_px = studio_param_f64(node, "feather_px");

    let prepared = prepare_repaint_regions(
        None,
        image.clone(),
        quality_report,
        repaint_actions,
        min_confidence,
        padding,
        max_regions,
        Some(false),
        Some(output_dir.clone()),
        None,
    )?;
    let manifest = serde_json::to_string(&prepared)
        .map_err(|err| format!("failed to encode repaint manifest: {err}"))?;

    let provider = studio_value_to_string(node.params.get("provider"))
        .trim()
        .to_string();
    let operation = {
        let configured = studio_value_to_string(node.params.get("operation"));
        let trimmed = configured.trim();
        if trimmed.is_empty() {
            "image.edit".to_string()
        } else {
            trimmed.to_string()
        }
    };
    let credentials_ref = studio_value_to_string(node.params.get("credentials_ref"))
        .trim()
        .to_string();
    let prompt_base = studio_value_to_string(node.params.get("repaint_prompt_base"))
        .trim()
        .to_string();

    // Only call a real provider; mock/empty means no `image.edit` capability,
    // so we leave every region unrepainted and pass the image through.
    let provider_can_edit = !provider.is_empty() && provider != "mock";
    let mut repainted: Vec<Value> = Vec::new();
    if provider_can_edit {
        let region_count = prepared.regions.len();
        logger.node(
            node,
            format!("repainting {region_count} region(s) via {provider} {operation}"),
        );
        for (region_index, region) in prepared.regions.iter().enumerate() {
            let mut task = ApiTask::new(provider.clone(), operation.clone());
            task.id = studio_task_id(&node.id);
            task.output_type = OutputType::Image;
            task.cache_policy.enabled = false;
            task.retry_policy.max_attempts = 1;
            task.retry_policy.backoff_ms = 200;
            task.retry_policy.timeout_ms = Some(120_000);

            task.inputs
                .insert("image_path".to_string(), json!(region.crop_path));
            task.inputs
                .insert("mask_path".to_string(), json!(region.mask_path));
            let issue = region.issue_type.clone().unwrap_or_default();
            let prompt = if prompt_base.is_empty() {
                let label = if issue.is_empty() { "flagged" } else { &issue };
                format!(
                    "Repaint and restore this {label} region with clean, realistic detail; \
                     keep the style, lighting and colours consistent with the surroundings."
                )
            } else if issue.is_empty() {
                prompt_base.clone()
            } else {
                format!("{prompt_base} (issue: {issue})")
            };
            task.inputs.insert("prompt".to_string(), json!(prompt));
            task.params.insert("save_outputs".to_string(), json!(true));

            for (key, value) in &node.params {
                if matches!(
                    key.as_str(),
                    "provider"
                        | "operation"
                        | "credentials_ref"
                        | "repaint_prompt_base"
                        | "repaint_actions"
                        | "min_confidence"
                        | "region_padding"
                        | "max_regions"
                        | "feather_px"
                        | "output_dir"
                        | "output_name"
                ) {
                    continue;
                }
                if studio_non_empty(value) {
                    task.params.insert(key.clone(), value.clone());
                }
            }
            if !credentials_ref.is_empty() {
                task.credentials_ref = Some(credentials_ref.clone());
            }

            logger.node(
                node,
                format!(
                    "region {}/{}: calling {} {} (task {})",
                    region_index + 1,
                    region_count,
                    task.provider,
                    task.operation,
                    task.id
                ),
            );
            let task_for_detail = task.clone();
            let result = execute_and_record_cancellable(task, cancels, run_id)
                .await
                .map_err(|message| StudioNodeErrorDetail {
                    provider: Some(task_for_detail.provider.clone()),
                    operation: Some(task_for_detail.operation.clone()),
                    task_id: Some(task_for_detail.id.clone()),
                    ..StudioNodeErrorDetail::from(message)
                })?;
            if matches!(result.status, ApiStatus::Succeeded | ApiStatus::Cached) {
                if let Some(file) = result.output_files.first() {
                    repainted.push(json!({ "index": region.index, "path": file.path.clone() }));
                }
            } else {
                // A per-region provider failure leaves that region unrepainted
                // rather than aborting the whole node.
                let detail = studio_api_error_detail(&task_for_detail, &result);
                logger.node(
                    node,
                    format!(
                        "region {}/{} left unrepainted: {}{}",
                        region_index + 1,
                        region_count,
                        detail.message,
                        detail
                            .code
                            .as_deref()
                            .map(|code| format!(" [{code}]"))
                            .unwrap_or_default()
                    ),
                );
            }
        }
        logger.node(
            node,
            format!("{}/{} region(s) repainted", repainted.len(), region_count),
        );
    }

    let repainted_json = serde_json::to_string(&repainted)
        .map_err(|err| format!("failed to encode repainted list: {err}"))?;
    let composed = composite_repaint(
        None,
        image,
        manifest,
        repainted_json,
        feather_px,
        Some(output_dir),
        output_name,
    )?;

    let report = serde_json::to_value(&composed.repaint_report)
        .map_err(|err| format!("failed to encode RepaintReport: {err}"))?;
    Ok(studio_output_map([
        ("fixed_image", json!(composed.fixed_image)),
        ("repaint_report", report),
    ]))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn prepare_repaint_result_parses_hardening_fields() {
        // The prepare manifest must carry the v1 decode-hardening fields so the
        // composite step (and the report) can surface them.
        let value = json!({
            "regions": [],
            "skipped": [],
            "image_size": [128, 96],
            "selected_count": 0,
            "mask_edit_is_transparent": true,
            "source_mode": "CMYK",
            "exif_transposed": true,
            "max_decode_pixels": 96_000_000
        });
        let result: crate::psd::PrepareRepaintResult = serde_json::from_value(value).unwrap();
        assert_eq!(result.source_mode, "CMYK");
        assert!(result.exif_transposed);
        assert_eq!(result.max_decode_pixels, 96_000_000);
    }

    #[test]
    fn repaint_report_parses_hardening_fields() {
        let value = json!({
            "status": "repainted",
            "regions": [],
            "repainted_count": 1,
            "requested_count": 1,
            "image_size": [64, 64],
            "source_mode": "RGBA",
            "exif_transposed": false,
            "max_decode_pixels": 96_000_000
        });
        let report: crate::contracts::RepaintReport = serde_json::from_value(value).unwrap();
        assert_eq!(report.source_mode, "RGBA");
        assert!(!report.exif_transposed);
        assert_eq!(report.max_decode_pixels, 96_000_000);
    }

    #[test]
    fn repaint_report_defaults_for_legacy_json() {
        // Older records lack the v1 fields; they must still deserialize with
        // safe defaults so historical runs remain readable.
        let report: crate::contracts::RepaintReport = serde_json::from_value(json!({
            "status": "unchanged",
            "repainted_count": 0,
            "requested_count": 0
        }))
        .unwrap();
        assert_eq!(report.source_mode, "");
        assert!(!report.exif_transposed);
        assert_eq!(report.max_decode_pixels, 0);
    }
}
