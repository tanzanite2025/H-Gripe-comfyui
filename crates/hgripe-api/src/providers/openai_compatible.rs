use crate::credentials::{load_credential_ref, CredentialEntry};
use crate::model::{ApiErrorInfo, ApiResult, ApiStatus, ApiTask};
use crate::provider::{BrokerError, BrokerResult, Provider};
use async_trait::async_trait;
use reqwest::header::CONTENT_TYPE;
use reqwest::{Client, StatusCode};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::env;
use std::time::Duration;

pub struct OpenAiCompatibleProvider {
    client: Client,
}

struct JsonResponse {
    status: StatusCode,
    body: Value,
    provider_request_id: Option<String>,
}

impl Default for OpenAiCompatibleProvider {
    fn default() -> Self {
        Self {
            client: Client::new(),
        }
    }
}

#[async_trait]
impl Provider for OpenAiCompatibleProvider {
    fn name(&self) -> &'static str {
        "openai_compatible"
    }

    fn supports(&self, operation: &str) -> bool {
        matches!(
            operation,
            "chat.completions"
                | "chat.generate"
                | "text.generate"
                | "vision.analyze"
                | "image.generate"
        )
    }

    async fn execute(&self, task: &ApiTask) -> BrokerResult<ApiResult> {
        match task.operation.as_str() {
            "image.generate" => self.execute_image(task).await,
            _ => self.execute_chat(task).await,
        }
    }
}

impl OpenAiCompatibleProvider {
    async fn execute_chat(&self, task: &ApiTask) -> BrokerResult<ApiResult> {
        if value_bool(task, "stream").unwrap_or(false) {
            return Err(BrokerError::Provider(
                "stream=true is not supported by the CLI broker yet".to_string(),
            ));
        }

        let mut body = Map::new();
        body.insert("model".to_string(), json!(required_str(task, "model")?));

        if let Some(messages) = value(task, "messages") {
            if !messages.is_array() {
                return Err(BrokerError::Provider(
                    "messages must be a JSON array".to_string(),
                ));
            }
            body.insert("messages".to_string(), messages.clone());
        } else {
            body.insert("messages".to_string(), json!(prompt_messages(task)?));
        }

        copy_optional_fields(
            task,
            &mut body,
            &[
                "temperature",
                "top_p",
                "max_tokens",
                "presence_penalty",
                "frequency_penalty",
                "stop",
                "response_format",
                "tools",
                "tool_choice",
                "seed",
                "user",
                "modalities",
                "reasoning_effort",
            ],
        );
        merge_extra_body(task, &mut body)?;

        let path = value_str(task, "path").unwrap_or("/chat/completions");
        let response = self.send_json(task, path, Value::Object(body)).await?;

        if response.status.is_success() {
            let text = extract_chat_text(&response.body);
            let mut result = ApiResult::succeeded(
                task.id.clone(),
                Some(json!({
                    "text": text,
                    "raw": response.body,
                })),
            );
            result.provider_request_id = response.provider_request_id;
            Ok(result)
        } else {
            Ok(failed_result(task, response))
        }
    }

    async fn execute_image(&self, task: &ApiTask) -> BrokerResult<ApiResult> {
        let mut body = Map::new();
        body.insert("prompt".to_string(), json!(required_prompt(task)?));

        if let Some(model) = value_str(task, "model") {
            body.insert("model".to_string(), json!(model));
        }

        copy_optional_fields(
            task,
            &mut body,
            &[
                "n",
                "size",
                "quality",
                "style",
                "response_format",
                "user",
                "background",
                "moderation",
                "output_format",
            ],
        );
        merge_extra_body(task, &mut body)?;

        let path = value_str(task, "path").unwrap_or("/images/generations");
        let response = self.send_json(task, path, Value::Object(body)).await?;

        if response.status.is_success() {
            let images = response
                .body
                .get("data")
                .cloned()
                .unwrap_or_else(|| Value::Array(Vec::new()));
            let mut result = ApiResult::succeeded(
                task.id.clone(),
                Some(json!({
                    "images": images,
                    "raw": response.body,
                })),
            );
            result.provider_request_id = response.provider_request_id;
            Ok(result)
        } else {
            Ok(failed_result(task, response))
        }
    }

    async fn send_json(
        &self,
        task: &ApiTask,
        path: &str,
        request_body: Value,
    ) -> BrokerResult<JsonResponse> {
        let credentials = resolve_credentials(task)?;
        let url = endpoint_url(task, path, credentials.as_ref());
        let mut request = self.client.post(url).json(&request_body);

        if let Some(timeout_ms) = task.retry_policy.timeout_ms {
            request = request.timeout(Duration::from_millis(timeout_ms));
        }

        if let Some(api_key) = resolve_api_key(task, credentials.as_ref())? {
            request = request.bearer_auth(api_key);
        }

        if let Some(headers) = credentials
            .as_ref()
            .and_then(|entry| entry.headers.as_ref())
        {
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

        let response = request.send().await.map_err(|err| {
            BrokerError::Provider(format!("OpenAI-compatible request failed: {err}"))
        })?;
        let status = response.status();
        let headers = response.headers().clone();
        let provider_request_id = headers
            .get("x-request-id")
            .or_else(|| headers.get("request-id"))
            .or_else(|| headers.get("openai-request-id"))
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let content_type = headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        let text = response.text().await.map_err(|err| {
            BrokerError::Provider(format!("failed to read OpenAI-compatible body: {err}"))
        })?;

        if status.is_server_error() {
            return Err(BrokerError::Provider(format!(
                "server error {} from OpenAI-compatible provider",
                status.as_u16()
            )));
        }

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

fn credentials_file(task: &ApiTask) -> Option<&str> {
    value_str(task, "credentials_file")
}

fn required_str<'a>(task: &'a ApiTask, key: &str) -> BrokerResult<&'a str> {
    value_str(task, key)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| BrokerError::Provider(format!("openai_compatible requires {key}")))
}

fn required_prompt(task: &ApiTask) -> BrokerResult<&str> {
    value_str(task, "prompt")
        .or_else(|| value_str(task, "text"))
        .or_else(|| value_str(task, "input"))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            BrokerError::Provider("openai_compatible requires prompt/text/input".to_string())
        })
}

fn prompt_messages(task: &ApiTask) -> BrokerResult<Vec<Value>> {
    let prompt = required_prompt(task)?;
    let mut messages = Vec::new();

    if let Some(system_prompt) = value_str(task, "system_prompt") {
        if !system_prompt.trim().is_empty() {
            messages.push(json!({
                "role": "system",
                "content": system_prompt,
            }));
        }
    }

    messages.push(json!({
        "role": "user",
        "content": prompt,
    }));

    Ok(messages)
}

fn copy_optional_fields(task: &ApiTask, body: &mut Map<String, Value>, keys: &[&str]) {
    for key in keys {
        if let Some(value) = value(task, key) {
            body.insert((*key).to_string(), value.clone());
        }
    }
}

fn merge_extra_body(task: &ApiTask, body: &mut Map<String, Value>) -> BrokerResult<()> {
    if let Some(extra) = value(task, "extra_body") {
        let extra = extra
            .as_object()
            .ok_or_else(|| BrokerError::Provider("extra_body must be a JSON object".to_string()))?;
        for (key, value) in extra {
            body.insert(key.clone(), value.clone());
        }
    }
    Ok(())
}

fn endpoint_url(task: &ApiTask, path: &str, credentials: Option<&CredentialEntry>) -> String {
    let base_url = value_str(task, "base_url")
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            credentials
                .and_then(|entry| entry.base_url.as_deref())
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string)
        })
        .or_else(|| env::var("HGRIPE_OPENAI_COMPATIBLE_BASE_URL").ok())
        .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    format!("{}{}", base_url.trim_end_matches('/'), path)
}

fn resolve_credentials(task: &ApiTask) -> BrokerResult<Option<CredentialEntry>> {
    let Some(credential_ref) = task.credentials_ref.as_deref() else {
        return Ok(None);
    };
    let credential_ref = credential_ref.trim();
    if credential_ref.is_empty() {
        return Ok(None);
    }

    let credentials = load_credential_ref(credential_ref, credentials_file(task))?;
    credentials
        .ok_or_else(|| {
            BrokerError::Provider(format!("credentials_ref '{credential_ref}' was not found"))
        })
        .map(Some)
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

    Ok(env::var("HGRIPE_OPENAI_COMPATIBLE_API_KEY")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            env::var("OPENAI_API_KEY")
                .ok()
                .filter(|value| !value.is_empty())
        }))
}

fn extract_chat_text(body: &Value) -> String {
    body.get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| {
            choice
                .get("message")
                .and_then(|message| message.get("content"))
                .or_else(|| choice.get("text"))
        })
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn failed_result(task: &ApiTask, response: JsonResponse) -> ApiResult {
    let error = response.body.get("error");
    let code = error
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| response.status.as_u16().to_string());
    let message = error
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            format!(
                "OpenAI-compatible request failed with status {}",
                response.status.as_u16()
            )
        });

    ApiResult {
        id: task.id.clone(),
        status: ApiStatus::Failed,
        output_files: Vec::new(),
        output_json: Some(json!({
            "status_code": response.status.as_u16(),
            "raw": response.body,
        })),
        metadata: BTreeMap::new(),
        cost: None,
        duration_ms: 0,
        provider_request_id: response.provider_request_id,
        cache_hit: false,
        error: Some(ApiErrorInfo {
            code,
            message,
            retryable: false,
        }),
    }
}
