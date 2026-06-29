use crate::credentials::{load_credential_ref, CredentialEntry};
use crate::model::{ApiErrorInfo, ApiResult, ApiStatus, ApiTask, OutputFile};
use crate::outputs::write_task_output_bytes;
use crate::profiles::{load_provider_profile, ProviderProfile};
use crate::provider::{BrokerError, BrokerResult, Provider, ProviderExecutionContext};
use async_trait::async_trait;
use reqwest::header::CONTENT_TYPE;
use reqwest::{Client, StatusCode};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::env;
use std::time::Duration;

const DEFAULT_BASE_URL: &str = "https://api.replicate.com";

pub struct ReplicateProvider {
    client: Client,
}

struct JsonResponse {
    status: StatusCode,
    body: Value,
    provider_request_id: Option<String>,
}

impl Default for ReplicateProvider {
    fn default() -> Self {
        Self {
            client: Client::new(),
        }
    }
}

#[async_trait]
impl Provider for ReplicateProvider {
    fn name(&self) -> &'static str {
        "replicate"
    }

    fn supports(&self, operation: &str) -> bool {
        matches!(
            operation,
            "run" | "model.run" | "replicate.run" | "predictions.create"
        )
    }

    async fn execute(&self, task: &ApiTask) -> BrokerResult<ApiResult> {
        self.execute_with_context(task, &ProviderExecutionContext::default())
            .await
    }

    async fn execute_with_context(
        &self,
        task: &ApiTask,
        context: &ProviderExecutionContext,
    ) -> BrokerResult<ApiResult> {
        context.check_cancelled()?;
        let task = apply_provider_profile(task)?;
        self.run_prediction(&task, context).await
    }
}

impl ReplicateProvider {
    async fn run_prediction(
        &self,
        task: &ApiTask,
        context: &ProviderExecutionContext,
    ) -> BrokerResult<ApiResult> {
        let credentials = resolve_credentials(task)?;
        let (submit_path, submit_body) = build_submit_request(task)?;
        let submit_url = endpoint_url(task, &submit_path, credentials.as_ref());

        let submit = tokio::select! {
            response = self.send_json(task, &submit_url, Some(submit_body), credentials.as_ref()) => response?,
            _ = context.cancellation().cancelled() => return Err(BrokerError::Cancelled),
        };

        if submit.status.is_server_error() {
            return Err(BrokerError::Provider(format!(
                "server error {} from replicate create prediction",
                submit.status.as_u16()
            )));
        }

        if !submit.status.is_success() {
            return Ok(failed_result(task, &submit, "create"));
        }

        let prediction_id = json_path_value(&submit.body, "id")
            .and_then(value_to_string)
            .ok_or_else(|| {
                BrokerError::Provider(
                    "replicate create prediction response did not contain an id".to_string(),
                )
            })?;
        let poll_url = resolve_poll_url(task, &submit.body, &prediction_id, credentials.as_ref());

        let max_polls = value_u64(task, "max_polls").unwrap_or(60).max(1);
        let poll_interval_ms = value_u64(task, "poll_interval_ms").unwrap_or(2000);
        let download_outputs = value_bool(task, "download_outputs").unwrap_or(true);

        let mut last_body = submit.body.clone();
        let mut last_status_value: Option<String> = None;
        let mut last_request_id = submit.provider_request_id.clone();

        if context.cancellation().is_cancelled() {
            return Ok(self
                .cancelled_prediction_result(
                    task,
                    &submit,
                    &prediction_id,
                    &last_body,
                    0,
                    max_polls,
                    poll_interval_ms,
                    last_status_value.as_deref(),
                    last_request_id,
                    credentials.as_ref(),
                )
                .await);
        }

        for poll_count in 1..=max_polls {
            if poll_count > 1 && poll_interval_ms > 0 {
                if let Err(BrokerError::Cancelled) =
                    context.sleep(Duration::from_millis(poll_interval_ms)).await
                {
                    return Ok(self
                        .cancelled_prediction_result(
                            task,
                            &submit,
                            &prediction_id,
                            &last_body,
                            poll_count.saturating_sub(1),
                            max_polls,
                            poll_interval_ms,
                            last_status_value.as_deref(),
                            last_request_id,
                            credentials.as_ref(),
                        )
                        .await);
                }
            }

            let poll = tokio::select! {
                response = self.send_json(task, &poll_url, None, credentials.as_ref()) => response?,
                _ = context.cancellation().cancelled() => {
                    return Ok(self
                        .cancelled_prediction_result(
                            task,
                            &submit,
                            &prediction_id,
                            &last_body,
                            poll_count.saturating_sub(1),
                            max_polls,
                            poll_interval_ms,
                            last_status_value.as_deref(),
                            last_request_id,
                            credentials.as_ref(),
                        )
                        .await);
                }
            };

            if poll.status.is_server_error() {
                return Err(BrokerError::Provider(format!(
                    "server error {} from replicate poll",
                    poll.status.as_u16()
                )));
            }

            if !poll.status.is_success() {
                return Ok(failed_result(task, &poll, "poll"));
            }

            last_body = poll.body.clone();
            if poll.provider_request_id.is_some() {
                last_request_id = poll.provider_request_id.clone();
            }

            let status_value = json_path_value(&poll.body, "status").and_then(value_to_string);
            last_status_value = status_value.clone();
            let normalized = status_value
                .as_deref()
                .map(normalized_status_value)
                .unwrap_or_default();

            if is_success_status(&normalized) {
                let output_value = poll.body.get("output").cloned().unwrap_or(Value::Null);
                let output_files = if download_outputs {
                    self.download_outputs(task, &output_value, credentials.as_ref())
                        .await?
                } else {
                    Vec::new()
                };

                let output_json = prediction_output_json(
                    &prediction_id,
                    &normalized,
                    &poll.body,
                    &output_value,
                    &output_files,
                    poll_count,
                    max_polls,
                    poll_interval_ms,
                );
                let mut result = ApiResult::succeeded(task.id.clone(), Some(output_json));
                result.provider_request_id = last_request_id.or(Some(prediction_id));
                result.output_files = output_files;
                return Ok(result);
            }

            if is_failure_status(&normalized) {
                return Ok(prediction_failed_result(
                    task,
                    &prediction_id,
                    &normalized,
                    &poll.body,
                    last_request_id,
                ));
            }
        }

        let output_json = prediction_output_json(
            &prediction_id,
            &last_status_value
                .as_deref()
                .map(normalized_status_value)
                .unwrap_or_default(),
            &last_body,
            &last_body.get("output").cloned().unwrap_or(Value::Null),
            &[],
            max_polls,
            max_polls,
            poll_interval_ms,
        );
        Ok(ApiResult {
            id: task.id.clone(),
            status: ApiStatus::Failed,
            output_files: Vec::new(),
            output_json: Some(output_json),
            metadata: BTreeMap::new(),
            cost: None,
            duration_ms: 0,
            provider_request_id: last_request_id.or(Some(prediction_id.clone())),
            cache_hit: false,
            error: Some(ApiErrorInfo {
                code: "poll_timeout".to_string(),
                message: format!(
                    "replicate prediction {prediction_id} did not finish after {max_polls} polls"
                ),
                retryable: true,
            }),
        })
    }

    async fn send_json(
        &self,
        task: &ApiTask,
        url: &str,
        body: Option<Value>,
        credentials: Option<&CredentialEntry>,
    ) -> BrokerResult<JsonResponse> {
        let mut request = match &body {
            Some(body) => self.client.post(url).json(body),
            None => self.client.get(url),
        };

        if let Some(timeout_ms) = task.retry_policy.timeout_ms {
            request = request.timeout(Duration::from_millis(timeout_ms));
        }

        if let Some(api_key) = resolve_api_key(task, credentials)? {
            request = request.bearer_auth(api_key);
        }

        if let Some(headers) = credentials.and_then(|entry| entry.headers.as_ref()) {
            for (name, header_value) in headers {
                request = request.header(name, header_value);
            }
        }

        if let Some(headers) = value(task, "headers").and_then(Value::as_object) {
            for (name, header_value) in headers {
                if let Some(header_value) = header_value.as_str() {
                    request = request.header(name, header_value);
                }
            }
        }

        let response = request
            .send()
            .await
            .map_err(|err| BrokerError::Provider(format!("replicate request failed: {err}")))?;
        let status = response.status();
        let headers = response.headers().clone();
        let provider_request_id = headers
            .get("x-request-id")
            .or_else(|| headers.get("request-id"))
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let content_type = headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        let text = response.text().await.map_err(|err| {
            BrokerError::Provider(format!("failed to read replicate body: {err}"))
        })?;
        let body = if content_type.contains("application/json") {
            serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!(text))
        } else {
            serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!(text))
        };

        Ok(JsonResponse {
            status,
            body,
            provider_request_id,
        })
    }

    async fn download_outputs(
        &self,
        task: &ApiTask,
        output: &Value,
        credentials: Option<&CredentialEntry>,
    ) -> BrokerResult<Vec<OutputFile>> {
        let urls = collect_output_urls(output);
        let mut output_files = Vec::new();
        for (index, url) in urls.iter().enumerate() {
            output_files.push(self.download_output(task, url, index, credentials).await?);
        }
        Ok(output_files)
    }

    async fn download_output(
        &self,
        task: &ApiTask,
        url: &str,
        index: usize,
        credentials: Option<&CredentialEntry>,
    ) -> BrokerResult<OutputFile> {
        let mut request = self.client.get(url);

        if let Some(timeout_ms) = task.retry_policy.timeout_ms {
            request = request.timeout(Duration::from_millis(timeout_ms));
        }

        if value_bool(task, "download_with_auth").unwrap_or(false) {
            if let Some(api_key) = resolve_api_key(task, credentials)? {
                request = request.bearer_auth(api_key);
            }
        }

        let response = request
            .send()
            .await
            .map_err(|err| BrokerError::Provider(format!("replicate download failed: {err}")))?;
        let status = response.status();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        let bytes = response
            .bytes()
            .await
            .map_err(|err| {
                BrokerError::Provider(format!("failed to read replicate download: {err}"))
            })?
            .to_vec();

        if !status.is_success() {
            return Err(BrokerError::Provider(format!(
                "replicate download failed with status {}",
                status.as_u16()
            )));
        }

        let mime_type = normalized_content_type(&content_type);
        let extension = value_str(task, "output_extension")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| extension_from_url(url))
            .unwrap_or_else(|| extension_for_content_type(mime_type.as_deref()));

        write_task_output_bytes(
            value_str(task, "output_dir"),
            task,
            index,
            &bytes,
            mime_type.as_deref(),
            &extension,
        )
    }

    #[allow(clippy::too_many_arguments)]
    async fn cancelled_prediction_result(
        &self,
        task: &ApiTask,
        submit: &JsonResponse,
        prediction_id: &str,
        last_body: &Value,
        poll_count: u64,
        max_polls: u64,
        poll_interval_ms: u64,
        status_value: Option<&str>,
        provider_request_id: Option<String>,
        credentials: Option<&CredentialEntry>,
    ) -> ApiResult {
        let cancel_json = self
            .send_cancel_prediction(task, submit, prediction_id, credentials)
            .await;
        let cancel_request_id = cancel_json
            .get("provider_request_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let normalized_status = status_value
            .map(normalized_status_value)
            .unwrap_or_else(|| "cancelled".to_string());
        let mut output_json = prediction_output_json(
            prediction_id,
            &normalized_status,
            last_body,
            &last_body.get("output").cloned().unwrap_or(Value::Null),
            &[],
            poll_count,
            max_polls,
            poll_interval_ms,
        );
        if let Some(output) = output_json.as_object_mut() {
            output.insert("cancel".to_string(), cancel_json);
        }

        ApiResult {
            id: task.id.clone(),
            status: ApiStatus::Cancelled,
            output_files: Vec::new(),
            output_json: Some(output_json),
            metadata: BTreeMap::new(),
            cost: None,
            duration_ms: 0,
            provider_request_id: cancel_request_id
                .or(provider_request_id)
                .or_else(|| submit.provider_request_id.clone())
                .or_else(|| Some(prediction_id.to_string())),
            cache_hit: false,
            error: Some(ApiErrorInfo {
                code: "cancelled".to_string(),
                message: format!("replicate prediction {prediction_id} was cancelled"),
                retryable: false,
            }),
        }
    }

    async fn send_cancel_prediction(
        &self,
        task: &ApiTask,
        submit: &JsonResponse,
        prediction_id: &str,
        credentials: Option<&CredentialEntry>,
    ) -> Value {
        let cancel_url = resolve_cancel_url(task, &submit.body, prediction_id, credentials);
        match self
            .send_json(task, &cancel_url, Some(json!({})), credentials)
            .await
        {
            Ok(response) => json!({
                "sent": true,
                "url": cancel_url,
                "status_code": response.status.as_u16(),
                "body": response.body,
                "provider_request_id": response.provider_request_id,
            }),
            Err(err) => json!({
                "sent": false,
                "url": cancel_url,
                "error": err.to_string(),
            }),
        }
    }
}

fn build_submit_request(task: &ApiTask) -> BrokerResult<(String, Value)> {
    let input = value(task, "input").cloned().unwrap_or_else(|| json!({}));
    if !input.is_object() {
        return Err(BrokerError::Provider(
            "replicate params.input must be a JSON object".to_string(),
        ));
    }

    let mut body = Map::new();
    body.insert("input".to_string(), input);
    merge_extra_body(task, &mut body)?;

    let version = value_str(task, "version")
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(version) = version {
        body.insert("version".to_string(), json!(version));
        return Ok(("/v1/predictions".to_string(), Value::Object(body)));
    }

    let model = value_str(task, "model")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            BrokerError::Provider(
                "replicate requires params.version or params.model (owner/name)".to_string(),
            )
        })?;

    if let Some((model, version)) = model.split_once(':') {
        body.insert("version".to_string(), json!(version));
        let _ = model;
        return Ok(("/v1/predictions".to_string(), Value::Object(body)));
    }

    if !model.contains('/') {
        return Err(BrokerError::Provider(format!(
            "replicate model '{model}' must be in 'owner/name' form, or set params.version"
        )));
    }

    Ok((
        format!("/v1/models/{model}/predictions"),
        Value::Object(body),
    ))
}

fn resolve_poll_url(
    task: &ApiTask,
    submit_body: &Value,
    prediction_id: &str,
    credentials: Option<&CredentialEntry>,
) -> String {
    if let Some(url) = json_path_value(submit_body, "urls.get")
        .and_then(value_to_string)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return url;
    }

    endpoint_url(
        task,
        &format!("/v1/predictions/{prediction_id}"),
        credentials,
    )
}

fn resolve_cancel_url(
    task: &ApiTask,
    submit_body: &Value,
    prediction_id: &str,
    credentials: Option<&CredentialEntry>,
) -> String {
    if let Some(url) = json_path_value(submit_body, "urls.cancel")
        .and_then(value_to_string)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return url;
    }

    endpoint_url(
        task,
        &format!("/v1/predictions/{prediction_id}/cancel"),
        credentials,
    )
}

fn collect_output_urls(output: &Value) -> Vec<String> {
    let mut urls = Vec::new();
    push_output_urls(output, &mut urls);
    urls
}

fn push_output_urls(output: &Value, urls: &mut Vec<String>) {
    match output {
        Value::String(value) => {
            if is_downloadable_url(value) {
                urls.push(value.clone());
            }
        }
        Value::Array(items) => {
            for item in items {
                push_output_urls(item, urls);
            }
        }
        _ => {}
    }
}

fn is_downloadable_url(value: &str) -> bool {
    let value = value.trim();
    value.starts_with("http://") || value.starts_with("https://")
}

fn extension_from_url(url: &str) -> Option<String> {
    let without_query = url.split(['?', '#']).next().unwrap_or(url);
    let last_segment = without_query.rsplit('/').next().unwrap_or("");
    let (_, extension) = last_segment.rsplit_once('.')?;
    let extension: String = extension
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect();
    if extension.is_empty() {
        None
    } else {
        Some(extension.to_ascii_lowercase())
    }
}

#[allow(clippy::too_many_arguments)]
fn prediction_output_json(
    prediction_id: &str,
    status: &str,
    prediction: &Value,
    output: &Value,
    output_files: &[OutputFile],
    poll_count: u64,
    max_polls: u64,
    poll_interval_ms: u64,
) -> Value {
    json!({
        "id": prediction_id,
        "status": status,
        "output": output.clone(),
        "prediction": prediction.clone(),
        "output_files": output_files,
        "polling": {
            "poll_count": poll_count,
            "max_polls": max_polls,
            "poll_interval_ms": poll_interval_ms,
        },
    })
}

fn failed_result(task: &ApiTask, response: &JsonResponse, stage: &str) -> ApiResult {
    let detail = response
        .body
        .get("detail")
        .and_then(Value::as_str)
        .or_else(|| response.body.get("title").and_then(Value::as_str))
        .map(str::to_string)
        .unwrap_or_else(|| {
            format!(
                "replicate {stage} request failed with status {}",
                response.status.as_u16()
            )
        });

    ApiResult {
        id: task.id.clone(),
        status: ApiStatus::Failed,
        output_files: Vec::new(),
        output_json: Some(json!({
            "status_code": response.status.as_u16(),
            "stage": stage,
            "body": response.body.clone(),
        })),
        metadata: BTreeMap::new(),
        cost: None,
        duration_ms: 0,
        provider_request_id: response.provider_request_id.clone(),
        cache_hit: false,
        error: Some(ApiErrorInfo {
            code: response.status.as_u16().to_string(),
            message: detail,
            retryable: false,
        }),
    }
}

fn prediction_failed_result(
    task: &ApiTask,
    prediction_id: &str,
    status: &str,
    prediction: &Value,
    provider_request_id: Option<String>,
) -> ApiResult {
    let message = prediction
        .get("error")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            format!("replicate prediction {prediction_id} ended with status {status}")
        });

    ApiResult {
        id: task.id.clone(),
        status: ApiStatus::Failed,
        output_files: Vec::new(),
        output_json: Some(prediction_output_json(
            prediction_id,
            status,
            prediction,
            &prediction.get("output").cloned().unwrap_or(Value::Null),
            &[],
            0,
            0,
            0,
        )),
        metadata: BTreeMap::new(),
        cost: None,
        duration_ms: 0,
        provider_request_id: provider_request_id.or_else(|| Some(prediction_id.to_string())),
        cache_hit: false,
        error: Some(ApiErrorInfo {
            code: status.to_string(),
            message,
            retryable: false,
        }),
    }
}

fn value<'a>(task: &'a ApiTask, key: &str) -> Option<&'a Value> {
    task.params.get(key).or_else(|| task.inputs.get(key))
}

fn value_str<'a>(task: &'a ApiTask, key: &str) -> Option<&'a str> {
    value(task, key).and_then(Value::as_str)
}

fn value_bool(task: &ApiTask, key: &str) -> Option<bool> {
    value(task, key).and_then(Value::as_bool)
}

fn value_u64(task: &ApiTask, key: &str) -> Option<u64> {
    match value(task, key)? {
        Value::Number(number) => number.as_u64(),
        Value::String(value) => value.trim().parse().ok(),
        _ => None,
    }
}

fn merge_extra_body(task: &ApiTask, body: &mut Map<String, Value>) -> BrokerResult<()> {
    let Some(extra_body) = value(task, "extra_body") else {
        return Ok(());
    };
    let extra_body = extra_body.as_object().ok_or_else(|| {
        BrokerError::Provider("replicate params.extra_body must be a JSON object".to_string())
    })?;
    for (key, item) in extra_body {
        body.insert(key.clone(), item.clone());
    }
    Ok(())
}

fn endpoint_url(task: &ApiTask, path: &str, credentials: Option<&CredentialEntry>) -> String {
    let base_url = value_str(task, "base_url")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            credentials
                .and_then(|entry| entry.base_url.as_deref())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .or_else(|| {
            env::var("HGRIPE_REPLICATE_BASE_URL")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    format!("{}{}", base_url.trim_end_matches('/'), path)
}

fn credentials_file(task: &ApiTask) -> Option<&str> {
    value_str(task, "credentials_file")
}

fn profiles_file(task: &ApiTask) -> Option<&str> {
    value_str(task, "profiles_file")
}

fn profile_ref(task: &ApiTask) -> Option<&str> {
    value_str(task, "profile_ref").or_else(|| value_str(task, "provider_profile_ref"))
}

fn resolve_credentials(task: &ApiTask) -> BrokerResult<Option<CredentialEntry>> {
    let Some(credential_ref) = task.credentials_ref.as_deref() else {
        return Ok(None);
    };
    let credential_ref = credential_ref.trim();
    if credential_ref.is_empty() {
        return Ok(None);
    }

    let credential =
        load_credential_ref(credential_ref, credentials_file(task))?.ok_or_else(|| {
            BrokerError::Provider(format!("credentials_ref '{credential_ref}' was not found"))
        })?;
    if let Some(provider) = credential.provider.as_deref() {
        let provider = provider.trim();
        if !provider.is_empty() && provider != "replicate" {
            return Err(BrokerError::Provider(format!(
                "credentials_ref '{credential_ref}' is for provider '{provider}', not replicate"
            )));
        }
    }

    Ok(Some(credential))
}

fn resolve_api_key(
    task: &ApiTask,
    credentials: Option<&CredentialEntry>,
) -> BrokerResult<Option<String>> {
    if value_bool(task, "no_auth").unwrap_or(false) {
        return Ok(None);
    }

    if let Some(api_key) = value_str(task, "api_key") {
        let api_key = api_key.trim();
        if !api_key.is_empty() {
            return Ok(Some(api_key.to_string()));
        }
    }

    if let Some(api_key_env) = value_str(task, "api_key_env") {
        let api_key_env = api_key_env.trim();
        if api_key_env.is_empty() {
            return Ok(None);
        }
        return Ok(env::var(api_key_env).ok().filter(|value| !value.is_empty()));
    }

    if let Some(credentials) = credentials {
        if let Some(api_key) = credentials.api_key.as_deref() {
            let api_key = api_key.trim();
            if !api_key.is_empty() {
                return Ok(Some(api_key.to_string()));
            }
        }

        if let Some(api_key_env) = credentials.api_key_env.as_deref() {
            let api_key_env = api_key_env.trim();
            if api_key_env.is_empty() {
                return Ok(None);
            }
            return Ok(env::var(api_key_env).ok().filter(|value| !value.is_empty()));
        }
    }

    Ok(env::var("HGRIPE_REPLICATE_API_KEY")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            env::var("REPLICATE_API_TOKEN")
                .ok()
                .filter(|value| !value.is_empty())
        }))
}

fn apply_provider_profile(task: &ApiTask) -> BrokerResult<ApiTask> {
    let Some(profile_ref) = profile_ref(task)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(task.clone());
    };

    let profile = load_provider_profile(profile_ref, profiles_file(task))?.ok_or_else(|| {
        BrokerError::Provider(format!("provider profile '{profile_ref}' was not found"))
    })?;

    if let Some(provider) = profile.provider.as_deref() {
        let provider = provider.trim();
        if !provider.is_empty() && provider != task.provider {
            return Err(BrokerError::Provider(format!(
                "provider profile '{profile_ref}' is for provider '{provider}', not '{}'",
                task.provider
            )));
        }
    }

    Ok(merge_provider_profile(task, &profile))
}

fn merge_provider_profile(task: &ApiTask, profile: &ProviderProfile) -> ApiTask {
    let mut merged = task.clone();
    let task_params = task.params.clone();
    merged.params = BTreeMap::new();

    if let Some(params) = &profile.params {
        for (key, value) in params {
            insert_effective_param(&mut merged.params, key, value.clone());
        }
    }

    insert_optional_string(&mut merged.params, "base_url", profile.base_url.as_deref());
    insert_optional_string(&mut merged.params, "model", profile.model.as_deref());
    insert_optional_string(
        &mut merged.params,
        "api_key_env",
        profile.api_key_env.as_deref(),
    );
    if let Some(no_auth) = profile.no_auth {
        merged.params.insert("no_auth".to_string(), json!(no_auth));
    }
    if let Some(headers) = &profile.headers {
        merged.params.insert("headers".to_string(), json!(headers));
    }
    if let Some(extra_body) = &profile.extra_body {
        merged
            .params
            .insert("extra_body".to_string(), json!(extra_body));
    }

    for (key, value) in task_params {
        merge_task_param(&mut merged.params, key, value);
    }

    if task
        .credentials_ref
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
    {
        merged.credentials_ref = profile
            .credentials_ref
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
    }

    merged
}

fn insert_optional_string(params: &mut BTreeMap<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        params.insert(key.to_string(), json!(value));
    }
}

fn insert_effective_param(params: &mut BTreeMap<String, Value>, key: &str, value: Value) {
    if !value_is_blank_string(&value) {
        params.insert(key.to_string(), value);
    }
}

fn merge_task_param(params: &mut BTreeMap<String, Value>, key: String, value: Value) {
    if value_is_blank_string(&value) && params.contains_key(&key) {
        return;
    }

    if key == "headers" || key == "extra_body" || key == "input" {
        if let (Some(existing), Some(incoming)) = (
            params.get_mut(&key).and_then(Value::as_object_mut),
            value.as_object(),
        ) {
            for (item_key, item_value) in incoming {
                if !value_is_blank_string(item_value) {
                    existing.insert(item_key.clone(), item_value.clone());
                }
            }
            return;
        }
    }

    params.insert(key, value);
}

fn value_is_blank_string(value: &Value) -> bool {
    value.as_str().map(str::trim).is_some_and(str::is_empty)
}

fn normalized_content_type(content_type: &str) -> Option<String> {
    content_type
        .split(';')
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn extension_for_content_type(content_type: Option<&str>) -> String {
    match content_type.unwrap_or("").to_ascii_lowercase().as_str() {
        "application/json" => "json",
        "application/pdf" => "pdf",
        "audio/mpeg" => "mp3",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/webm" => "webm",
        "image/gif" => "gif",
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "text/plain" => "txt",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        _ => "bin",
    }
    .to_string()
}

fn normalized_status_value(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn is_success_status(status: &str) -> bool {
    status == "succeeded"
}

fn is_failure_status(status: &str) -> bool {
    matches!(status, "failed" | "canceled" | "cancelled")
}

fn value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn json_path_value<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let path = path.trim();
    if path.is_empty() {
        return Some(value);
    }

    let mut current = value;
    for segment in path.split('.') {
        if segment.is_empty() {
            return None;
        }
        current = match current {
            Value::Object(map) => map.get(segment)?,
            Value::Array(items) => {
                let index = segment.parse::<usize>().ok()?;
                items.get(index)?
            }
            _ => return None,
        };
    }
    Some(current)
}
