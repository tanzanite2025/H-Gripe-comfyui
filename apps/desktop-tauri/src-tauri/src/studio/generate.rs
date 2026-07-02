//! The `generate` node executor: a single provider image call through the
//! broker (`image.generate` by default), wired for cancellation and task
//! history via [`execute_and_record_cancellable`].

use std::collections::BTreeMap;

use hgripe_api::{ApiStatus, ApiTask, OutputType};
use serde_json::{json, Value};

use super::api_call::{execute_and_record_cancellable, studio_task_id};
use super::graph::{studio_non_empty, studio_output_map, studio_value_to_string, StudioGraphNode};
use super::run_cancel::StudioRunCancels;
use super::run_events::{studio_api_error_detail, StudioNodeErrorDetail, StudioRunLogger};

pub(super) async fn execute_studio_generate(
    node: &StudioGraphNode,
    inputs: &BTreeMap<String, Value>,
    cancels: &tauri::State<'_, StudioRunCancels>,
    run_id: &str,
    logger: &StudioRunLogger<'_>,
) -> Result<BTreeMap<String, Value>, StudioNodeErrorDetail> {
    let mut task = ApiTask::new(
        studio_value_to_string(node.params.get("provider"))
            .trim()
            .to_string(),
        studio_value_to_string(node.params.get("operation"))
            .trim()
            .to_string(),
    );
    if task.provider.is_empty() {
        task.provider = "mock".to_string();
    }
    if task.operation.is_empty() {
        task.operation = "image.generate".to_string();
    }
    task.id = studio_task_id(&node.id);
    task.output_type = OutputType::Image;
    task.cache_policy.enabled = false;
    task.retry_policy.max_attempts = 1;
    task.retry_policy.backoff_ms = 200;
    task.retry_policy.timeout_ms = Some(60_000);

    let prompt = studio_value_to_string(inputs.get("prompt"));
    if !prompt.is_empty() {
        task.inputs.insert("prompt".to_string(), json!(prompt));
    }
    let reference = studio_value_to_string(inputs.get("reference"));
    if !reference.is_empty() {
        task.inputs
            .insert("image_path".to_string(), json!(reference));
    }

    for (key, value) in &node.params {
        if matches!(key.as_str(), "provider" | "operation" | "credentials_ref") {
            continue;
        }
        if studio_non_empty(value) {
            task.params.insert(key.clone(), value.clone());
        }
    }
    if let Some(seed) = inputs.get("seed") {
        task.params.insert("seed".to_string(), seed.clone());
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
    logger.node(
        node,
        format!(
            "{} output file(s) in {} ms{}",
            result.output_files.len(),
            result.duration_ms,
            if result.cache_hit { " (cache hit)" } else { "" }
        ),
    );
    let image = result
        .output_files
        .first()
        .map(|file| json!(file.path.clone()))
        .unwrap_or(Value::Null);
    let result_json = serde_json::to_value(result)
        .map_err(|err| StudioNodeErrorDetail::from(format!("failed to encode ApiResult: {err}")))?;
    Ok(studio_output_map([
        ("image", image),
        ("result", result_json),
    ]))
}
