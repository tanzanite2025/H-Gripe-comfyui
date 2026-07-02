//! Run-event plumbing for Studio graph executions: the `studio:graph-run`
//! webview channel, the structured per-node error detail, and the node-scoped
//! log stream ([`StudioRunLogger`]). Everything the engine or a node executor
//! emits toward the run log goes through here.

use hgripe_api::{ApiResult, ApiTask};
use serde::Serialize;
use tauri::Emitter;

use super::graph::StudioGraphNode;

const STUDIO_GRAPH_RUN_EVENT: &str = "studio:graph-run";

/// Structured error details for a failed Studio node. The flat `message`
/// remains the always-present human-readable line (and is what the legacy
/// `error` string carries); the optional fields surface provider/broker
/// context (error code, retryability, provider request id) when the failure
/// came from an API call, so the run log and node card can show more than an
/// opaque string.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct StudioNodeErrorDetail {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

impl From<String> for StudioNodeErrorDetail {
    fn from(message: String) -> Self {
        Self {
            message,
            ..Self::default()
        }
    }
}

/// Build the structured detail for a non-success [`ApiResult`], carrying the
/// broker's error code/retryability plus the provider/operation/request ids
/// of the task that failed.
pub(super) fn studio_api_error_detail(task: &ApiTask, result: &ApiResult) -> StudioNodeErrorDetail {
    let (message, code, retryable) = match result.error.as_ref() {
        Some(error) => (
            error.message.clone(),
            Some(error.code.clone()),
            Some(error.retryable),
        ),
        None => ("provider call failed".to_string(), None, None),
    };
    StudioNodeErrorDetail {
        message,
        code,
        retryable,
        provider: Some(task.provider.clone()),
        operation: Some(task.operation.clone()),
        provider_request_id: result.provider_request_id.clone(),
        task_id: Some(task.id.clone()),
    }
}

/// Emits node-scoped `status: "log"` progress lines on the shared
/// `studio:graph-run` channel, so executors can stream context (which
/// provider/operation is being called, per-region repaint progress, output
/// summaries) into the webview's run log without changing a node's status.
pub(crate) struct StudioRunLogger<'a> {
    pub(super) app: &'a tauri::AppHandle,
    pub(super) run_id: &'a str,
}

impl StudioRunLogger<'_> {
    pub(super) fn node(&self, node: &StudioGraphNode, message: impl Into<String>) {
        emit_studio_run_event(
            self.app,
            StudioGraphRunEvent {
                run_id: self.run_id.to_string(),
                node_id: Some(node.id.clone()),
                kind: Some(node.kind.clone()),
                status: "log".to_string(),
                duration_ms: None,
                error: None,
                error_detail: None,
                message: Some(message.into()),
            },
        );
    }
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct StudioGraphRunEvent {
    run_id: String,
    node_id: Option<String>,
    kind: Option<String>,
    status: String,
    duration_ms: Option<u128>,
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_detail: Option<StudioNodeErrorDetail>,
    message: Option<String>,
}

pub(super) fn emit_studio_run_event(app: &tauri::AppHandle, payload: StudioGraphRunEvent) {
    let _ = app.emit(STUDIO_GRAPH_RUN_EVENT, payload);
}

pub(super) fn studio_node_event(
    run_id: &str,
    node: &StudioGraphNode,
    status: &str,
    duration_ms: Option<u128>,
    error: Option<String>,
    error_detail: Option<StudioNodeErrorDetail>,
) -> StudioGraphRunEvent {
    StudioGraphRunEvent {
        run_id: run_id.to_string(),
        node_id: Some(node.id.clone()),
        kind: Some(node.kind.clone()),
        status: status.to_string(),
        duration_ms,
        error,
        error_detail,
        message: None,
    }
}

pub(super) fn studio_graph_event(
    run_id: &str,
    status: &str,
    message: Option<String>,
) -> StudioGraphRunEvent {
    StudioGraphRunEvent {
        run_id: run_id.to_string(),
        node_id: None,
        kind: None,
        status: status.to_string(),
        duration_ms: None,
        error: None,
        error_detail: None,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hgripe_api::{ApiErrorInfo, ApiStatus};
    use serde_json::json;

    #[test]
    fn node_error_detail_from_string_is_message_only() {
        let detail = StudioNodeErrorDetail::from("boom".to_string());
        assert_eq!(detail.message, "boom");
        assert_eq!(detail.code, None);
        assert_eq!(detail.provider, None);
        // Optional fields must not clutter the serialized event payload.
        let value = serde_json::to_value(&detail).unwrap();
        assert_eq!(value, json!({ "message": "boom" }));
    }

    #[test]
    fn api_error_detail_carries_provider_context() {
        let mut task = ApiTask::new(
            "openai_compatible".to_string(),
            "image.generate".to_string(),
        );
        task.id = "studio-n1-1".to_string();
        let mut result = ApiResult::succeeded(task.id.clone(), None);
        result.status = ApiStatus::Failed;
        result.provider_request_id = Some("req-42".to_string());
        result.error = Some(ApiErrorInfo {
            code: "http_500".to_string(),
            message: "server exploded".to_string(),
            retryable: true,
        });
        let detail = studio_api_error_detail(&task, &result);
        assert_eq!(detail.message, "server exploded");
        assert_eq!(detail.code.as_deref(), Some("http_500"));
        assert_eq!(detail.retryable, Some(true));
        assert_eq!(detail.provider.as_deref(), Some("openai_compatible"));
        assert_eq!(detail.operation.as_deref(), Some("image.generate"));
        assert_eq!(detail.provider_request_id.as_deref(), Some("req-42"));
        assert_eq!(detail.task_id.as_deref(), Some("studio-n1-1"));
    }

    #[test]
    fn api_error_detail_without_error_info_still_identifies_the_task() {
        let mut task = ApiTask::new("replicate".to_string(), "run".to_string());
        task.id = "studio-n2-2".to_string();
        let mut result = ApiResult::succeeded(task.id.clone(), None);
        result.status = ApiStatus::Failed;
        let detail = studio_api_error_detail(&task, &result);
        assert_eq!(detail.message, "provider call failed");
        assert_eq!(detail.code, None);
        assert_eq!(detail.provider.as_deref(), Some("replicate"));
        assert_eq!(detail.task_id.as_deref(), Some("studio-n2-2"));
    }
}
