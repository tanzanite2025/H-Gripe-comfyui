//! Shared plumbing for API-lane node executors: the cancellable
//! broker-call-plus-history wrapper, Studio task-id minting, and optional
//! numeric param readers used when building an [`ApiTask`].

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use hgripe_api::{
    record_task_failure, record_task_result, ApiErrorInfo, ApiResult, ApiStatus, ApiTask,
    BrokerError, ProviderExecutionContext,
};
use serde_json::Value;

use super::graph::StudioGraphNode;
use super::run_cancel::{studio_run_token, StudioRunCancels};
use crate::broker;

/// Execute `task` through the broker with the run's cancellation token,
/// recording the outcome (success, cancellation, or failure) in task history.
pub(super) async fn execute_and_record_cancellable(
    task: ApiTask,
    cancels: &tauri::State<'_, StudioRunCancels>,
    run_id: &str,
) -> Result<ApiResult, String> {
    let history_task = task.clone();
    let context = ProviderExecutionContext::new(studio_run_token(cancels, run_id));

    match broker().execute_with_context(task, context).await {
        Ok(result) => {
            let _ = record_task_result(&history_task, &result);
            Ok(result)
        }
        Err(BrokerError::Cancelled) => {
            let result = ApiResult {
                id: history_task.id.clone(),
                status: ApiStatus::Cancelled,
                output_files: Vec::new(),
                output_json: None,
                metadata: BTreeMap::new(),
                cost: None,
                duration_ms: 0,
                provider_request_id: None,
                cache_hit: false,
                error: Some(ApiErrorInfo {
                    code: "cancelled".to_string(),
                    message: "Studio run cancelled".to_string(),
                    retryable: false,
                }),
            };
            let _ = record_task_result(&history_task, &result);
            Ok(result)
        }
        Err(err) => {
            let message = err.to_string();
            let _ = record_task_failure(&history_task, message.clone());
            Err(message)
        }
    }
}

pub(super) fn studio_task_id(node_id: &str) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    format!("studio-{node_id}-{millis}")
}

/// Read an optional numeric param (accepts a JSON number or a numeric string;
/// blank/non-numeric yields `None` so the field is omitted from the task).
pub(super) fn studio_param_f64(node: &StudioGraphNode, key: &str) -> Option<f64> {
    match node.params.get(key) {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                trimmed.parse::<f64>().ok()
            }
        }
        _ => None,
    }
}

/// Like [`studio_param_f64`] but truncates to an integer (for `max_tokens`/`seed`).
pub(super) fn studio_param_i64(node: &StudioGraphNode, key: &str) -> Option<i64> {
    studio_param_f64(node, key).map(|value| value as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numeric_params_parse_from_number_or_string() {
        let mut params = BTreeMap::new();
        params.insert("temperature".to_string(), json!(0.7));
        params.insert("max_tokens".to_string(), json!("128"));
        params.insert("seed".to_string(), json!(42));
        params.insert("blank".to_string(), json!("  "));
        let node = StudioGraphNode {
            id: "n1".to_string(),
            kind: "promptOptimize".to_string(),
            params,
        };
        assert_eq!(studio_param_f64(&node, "temperature"), Some(0.7));
        assert_eq!(studio_param_i64(&node, "max_tokens"), Some(128));
        assert_eq!(studio_param_i64(&node, "seed"), Some(42));
        assert_eq!(studio_param_f64(&node, "blank"), None);
        assert_eq!(studio_param_f64(&node, "missing"), None);
    }
}
