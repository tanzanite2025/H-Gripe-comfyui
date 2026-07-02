//! The `promptOptimize` node executor (the Hybrid lane): `local` mode runs a
//! model-free normalise/dedupe/booster-tag pass in-process, `api` mode sends
//! the prompt through a `text.generate`-capable provider, and any other mode
//! passes the text through unchanged.

use std::collections::{BTreeMap, HashSet};

use hgripe_api::{ApiStatus, ApiTask, OutputType};
use serde_json::{json, Value};

use super::api_call::{
    execute_and_record_cancellable, studio_param_f64, studio_param_i64, studio_task_id,
};
use super::graph::{studio_output_map, studio_value_to_string, StudioGraphNode};
use super::run_cancel::StudioRunCancels;
use super::run_events::{studio_api_error_detail, StudioNodeErrorDetail, StudioRunLogger};

/// Booster tags appended (deduped) per local preset. Mirrors the TS
/// `PRESET_TAGS` in `runtime/promptOptimize.ts`; keep both in sync.
fn studio_preset_tags(preset: &str) -> &'static [&'static str] {
    match preset {
        "photographic" => &[
            "photorealistic",
            "high detail",
            "sharp focus",
            "natural lighting",
            "8k",
        ],
        "anime" => &[
            "anime style",
            "vibrant colors",
            "clean lineart",
            "highly detailed",
        ],
        "cinematic" => &[
            "cinematic lighting",
            "dramatic composition",
            "depth of field",
            "film grain",
        ],
        "detailed" => &[
            "highly detailed",
            "intricate",
            "ultra quality",
            "masterpiece",
        ],
        _ => &[],
    }
}

/// Model-free prompt optimisation for the `local` mode. Mirrors the TS
/// `optimizePromptLocally`: collapse whitespace, split on commas, drop empties,
/// case-insensitively dedupe (keeping first occurrence), then append the
/// preset's booster tags (also deduped). An empty input yields an empty string.
fn studio_optimize_prompt_locally(text: &str, preset: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut segments: Vec<String> = collapsed
        .split(',')
        .map(|segment| segment.trim().to_string())
        .filter(|segment| !segment.is_empty())
        .collect();
    if !segments.is_empty() {
        for tag in studio_preset_tags(preset) {
            segments.push((*tag).to_string());
        }
    }

    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for segment in segments {
        if seen.insert(segment.to_lowercase()) {
            out.push(segment);
        }
    }
    out.join(", ")
}

// Providers whose broker `supports("text.generate")` returns true. Mirrors the
// capability declared in crates/hgripe-api (openai_compatible + mock) and the
// TS `promptOptimizeProviderSupported`; keep all three in sync.
fn studio_prompt_optimize_provider_supported(provider: &str) -> bool {
    matches!(provider.trim(), "openai_compatible" | "mock")
}

pub(super) async fn execute_studio_prompt_optimize(
    node: &StudioGraphNode,
    inputs: &BTreeMap<String, Value>,
    cancels: &tauri::State<'_, StudioRunCancels>,
    run_id: &str,
    logger: &StudioRunLogger<'_>,
) -> Result<BTreeMap<String, Value>, StudioNodeErrorDetail> {
    let raw = if inputs.contains_key("text") {
        studio_value_to_string(inputs.get("text"))
    } else {
        studio_value_to_string(node.params.get("text"))
    };
    let mode = studio_value_to_string(node.params.get("mode"));

    match mode.as_str() {
        "local" => {
            let preset = studio_value_to_string(node.params.get("preset"));
            let optimized = studio_optimize_prompt_locally(&raw, preset.trim());
            Ok(studio_output_map([("text", json!(optimized))]))
        }
        "api" => {
            if raw.trim().is_empty() {
                return Ok(studio_output_map([("text", json!(raw))]));
            }
            let mut provider = studio_value_to_string(node.params.get("provider"))
                .trim()
                .to_string();
            if provider.is_empty() {
                provider = "openai_compatible".to_string();
            }
            if !studio_prompt_optimize_provider_supported(&provider) {
                return Err(format!(
                    "Provider \"{provider}\" can't optimize prompts (no text.generate support). \
                     Pick an OpenAI-compatible chat profile, or switch mode to \"local\"/\"off\"."
                )
                .into());
            }
            let mut task = ApiTask::new(provider, "text.generate".to_string());
            task.id = studio_task_id(&node.id);
            task.output_type = OutputType::Text;
            // Cache identical optimisations (same text+instruction+model+sampling)
            // so re-runs don't re-bill the LLM; the broker derives the key.
            task.cache_policy.enabled = true;
            task.retry_policy.max_attempts = 1;
            task.retry_policy.backoff_ms = 200;
            task.retry_policy.timeout_ms = Some(60_000);
            task.inputs.insert("prompt".to_string(), json!(raw));

            let model = studio_value_to_string(node.params.get("model"));
            if !model.trim().is_empty() {
                task.params.insert("model".to_string(), json!(model));
            }
            let instruction = studio_value_to_string(node.params.get("instruction"));
            if !instruction.trim().is_empty() {
                task.params
                    .insert("system_prompt".to_string(), json!(instruction));
            }
            // Optional sampling controls (forwarded to the chat call when set).
            if let Some(temperature) = studio_param_f64(node, "temperature") {
                task.params
                    .insert("temperature".to_string(), json!(temperature));
            }
            if let Some(max_tokens) = studio_param_i64(node, "max_tokens") {
                task.params
                    .insert("max_tokens".to_string(), json!(max_tokens));
            }
            if let Some(seed) = studio_param_i64(node, "seed") {
                task.params.insert("seed".to_string(), json!(seed));
            }
            let credentials_ref = studio_value_to_string(node.params.get("credentials_ref"));
            if !credentials_ref.is_empty() {
                task.credentials_ref = Some(credentials_ref);
            }

            logger.node(
                node,
                format!(
                    "calling {} {} (task {})",
                    task.provider, task.operation, task.id
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
            if !matches!(result.status, ApiStatus::Succeeded | ApiStatus::Cached) {
                return Err(studio_api_error_detail(&task_for_detail, &result));
            }
            if result.cache_hit {
                logger.node(node, "optimized prompt served from cache");
            }
            let optimized = result
                .output_json
                .as_ref()
                .and_then(|value| value.get("text"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| raw.clone());
            let result_json = serde_json::to_value(result).map_err(|err| {
                StudioNodeErrorDetail::from(format!("failed to encode ApiResult: {err}"))
            })?;
            Ok(studio_output_map([
                ("text", json!(optimized)),
                ("result", result_json),
            ]))
        }
        _ => Ok(studio_output_map([("text", json!(raw))])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optimize_prompt_locally_normalises_and_dedupes() {
        assert_eq!(
            studio_optimize_prompt_locally("  a fox ,   running  \n , river ", "cleanup"),
            "a fox, running, river"
        );
        assert_eq!(
            studio_optimize_prompt_locally("Fox, fox, FOX, river", "cleanup"),
            "Fox, river"
        );
    }

    #[test]
    fn optimize_prompt_locally_appends_preset_tags_deduped() {
        assert_eq!(
            studio_optimize_prompt_locally("a cat", "photographic"),
            "a cat, photorealistic, high detail, sharp focus, natural lighting, 8k"
        );
        assert_eq!(
            studio_optimize_prompt_locally("a cat, masterpiece", "detailed"),
            "a cat, masterpiece, highly detailed, intricate, ultra quality"
        );
    }

    #[test]
    fn optimize_prompt_locally_empty_stays_empty() {
        assert_eq!(studio_optimize_prompt_locally("   ", "photographic"), "");
        assert_eq!(studio_optimize_prompt_locally("", "cleanup"), "");
    }

    #[test]
    fn prompt_optimize_provider_support_matches_broker() {
        assert!(studio_prompt_optimize_provider_supported(
            "openai_compatible"
        ));
        assert!(studio_prompt_optimize_provider_supported("  mock  "));
        assert!(!studio_prompt_optimize_provider_supported("custom_http"));
        assert!(!studio_prompt_optimize_provider_supported("replicate"));
        assert!(!studio_prompt_optimize_provider_supported(""));
    }
}
