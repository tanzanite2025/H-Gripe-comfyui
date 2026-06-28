use crate::model::{ApiErrorInfo, ApiResult, ApiStatus, ApiTask, OutputFile};
use crate::outputs::write_task_output_bytes;
use crate::provider::{BrokerError, BrokerResult, Provider};
use async_trait::async_trait;
use reqwest::header::CONTENT_TYPE;
use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::time::sleep;

pub struct CustomHttpProvider {
    client: Client,
}

struct HttpResponse {
    status: StatusCode,
    headers: Value,
    body: Value,
    body_bytes: Vec<u8>,
    content_type: String,
    provider_request_id: Option<String>,
}

impl Default for CustomHttpProvider {
    fn default() -> Self {
        Self {
            client: Client::new(),
        }
    }
}

#[async_trait]
impl Provider for CustomHttpProvider {
    fn name(&self) -> &'static str {
        "custom_http"
    }

    fn supports(&self, operation: &str) -> bool {
        matches!(
            operation,
            "request" | "http.request" | "async_job" | "http.async_job"
        )
    }

    async fn execute(&self, task: &ApiTask) -> BrokerResult<ApiResult> {
        match task.operation.as_str() {
            "async_job" | "http.async_job" => self.execute_async_job(task).await,
            _ => self.execute_request(task).await,
        }
    }
}

impl CustomHttpProvider {
    async fn execute_request(&self, task: &ApiTask) -> BrokerResult<ApiResult> {
        let url = value_str(task, "url")
            .ok_or_else(|| BrokerError::Provider("custom_http requires params.url".to_string()))?;
        let response = self
            .send_task_request(task, "", url, inferred_default_method(task, ""), false)
            .await?;

        if response.status.is_server_error() {
            return Err(BrokerError::Provider(format!(
                "server error {} from custom_http",
                response.status.as_u16()
            )));
        }

        let output_files =
            if response.status.is_success() && value_bool(task, "save_response").unwrap_or(false) {
                vec![save_response_output(
                    task,
                    &response.body_bytes,
                    &response.content_type,
                    0,
                )?]
            } else {
                Vec::new()
            };
        let output_json = Some(response_output_json(&response, &output_files));

        if response.status.is_success() {
            let mut result = ApiResult::succeeded(task.id.clone(), output_json);
            result.provider_request_id = response.provider_request_id;
            result.output_files = output_files;
            Ok(result)
        } else {
            Ok(failed_http_result(
                task,
                response,
                output_files,
                output_json,
            ))
        }
    }

    async fn execute_async_job(&self, task: &ApiTask) -> BrokerResult<ApiResult> {
        let submit_url = value_str(task, "url").ok_or_else(|| {
            BrokerError::Provider("custom_http async_job requires params.url".to_string())
        })?;
        let submit_response = self
            .send_task_request(
                task,
                "",
                submit_url,
                inferred_default_method(task, ""),
                false,
            )
            .await?;

        if submit_response.status.is_server_error() {
            return Err(BrokerError::Provider(format!(
                "server error {} from custom_http async submit",
                submit_response.status.as_u16()
            )));
        }

        if !submit_response.status.is_success() {
            let output_json = Some(response_output_json(&submit_response, &[]));
            return Ok(failed_http_result(
                task,
                submit_response,
                Vec::new(),
                output_json,
            ));
        }

        let job_id = resolve_job_id(task, &submit_response.body)?;
        let poll_url = resolve_poll_url(task, &submit_response.body, &job_id)?;
        let max_polls = value_u64(task, "max_polls").unwrap_or(60).max(1);
        let poll_interval_ms = value_u64(task, "poll_interval_ms").unwrap_or(2000);
        let status_path = value_str(task, "status_path").unwrap_or("status");
        let result_path = value_str(task, "result_path");
        let success_values = normalized_string_list(
            task,
            "success_values",
            &[
                "succeeded",
                "success",
                "completed",
                "complete",
                "done",
                "ready",
                "finished",
            ],
        );
        let failure_values = normalized_string_list(
            task,
            "failure_values",
            &[
                "failed",
                "failure",
                "error",
                "errored",
                "cancelled",
                "canceled",
                "timeout",
            ],
        );

        let mut last_response: Option<HttpResponse> = None;
        let mut last_status_value: Option<String> = None;

        for poll_count in 1..=max_polls {
            if poll_count > 1 && poll_interval_ms > 0 {
                sleep(Duration::from_millis(poll_interval_ms)).await;
            }

            let response = self
                .send_task_request(task, "poll", &poll_url, "GET", true)
                .await?;

            if response.status.is_server_error() {
                return Err(BrokerError::Provider(format!(
                    "server error {} from custom_http async poll",
                    response.status.as_u16()
                )));
            }

            let status_value =
                json_path_value(&response.body, status_path).and_then(value_to_string);
            let normalized_status = status_value
                .as_deref()
                .map(normalized_status_value)
                .unwrap_or_default();
            last_status_value = status_value.clone();

            if !response.status.is_success() {
                let output_json = Some(async_job_output_json(
                    &submit_response,
                    &response,
                    &[],
                    &job_id,
                    poll_count,
                    max_polls,
                    poll_interval_ms,
                    status_path,
                    status_value.as_deref(),
                    result_path,
                    false,
                    false,
                ));
                return Ok(failed_http_result(task, response, Vec::new(), output_json));
            }

            if success_values.contains(&normalized_status) {
                let mut output_files = Vec::new();
                let mut body_saved = false;
                let mut download_saved = false;

                if value_bool(task, "save_response").unwrap_or(false) {
                    output_files.push(save_response_output(
                        task,
                        &response.body_bytes,
                        &response.content_type,
                        output_files.len(),
                    )?);
                    body_saved = true;
                }

                if value_bool(task, "download_result").unwrap_or(false) {
                    let download_url_path =
                        value_str(task, "download_url_path").unwrap_or("result.url");
                    let download_url =
                        json_path_value(&response.body, download_url_path)
                            .and_then(value_to_string)
                            .ok_or_else(|| {
                                BrokerError::Provider(format!(
                                    "download_result=true but download_url_path '{download_url_path}' was not found"
                                ))
                            })?;
                    output_files.push(
                        self.download_result_output(task, &download_url, output_files.len())
                            .await?,
                    );
                    download_saved = true;
                }

                let output_json = Some(async_job_output_json(
                    &submit_response,
                    &response,
                    &output_files,
                    &job_id,
                    poll_count,
                    max_polls,
                    poll_interval_ms,
                    status_path,
                    status_value.as_deref(),
                    result_path,
                    body_saved,
                    download_saved,
                ));
                let mut result = ApiResult::succeeded(task.id.clone(), output_json);
                result.provider_request_id = response
                    .provider_request_id
                    .clone()
                    .or_else(|| submit_response.provider_request_id.clone());
                result.output_files = output_files;
                return Ok(result);
            }

            if failure_values.contains(&normalized_status) {
                let output_json = Some(async_job_output_json(
                    &submit_response,
                    &response,
                    &[],
                    &job_id,
                    poll_count,
                    max_polls,
                    poll_interval_ms,
                    status_path,
                    status_value.as_deref(),
                    result_path,
                    false,
                    false,
                ));
                return Ok(ApiResult {
                    id: task.id.clone(),
                    status: ApiStatus::Failed,
                    output_files: Vec::new(),
                    output_json,
                    metadata: BTreeMap::new(),
                    cost: None,
                    duration_ms: 0,
                    provider_request_id: response
                        .provider_request_id
                        .or(submit_response.provider_request_id),
                    cache_hit: false,
                    error: Some(ApiErrorInfo {
                        code: normalized_status,
                        message: format!(
                            "custom_http async job {job_id} failed with status {}",
                            status_value.unwrap_or_else(|| "unknown".to_string())
                        ),
                        retryable: false,
                    }),
                });
            }

            last_response = Some(response);
        }

        let final_response = last_response.as_ref().unwrap_or(&submit_response);
        let output_json = Some(async_job_output_json(
            &submit_response,
            final_response,
            &[],
            &job_id,
            max_polls,
            max_polls,
            poll_interval_ms,
            status_path,
            last_status_value.as_deref(),
            result_path,
            false,
            false,
        ));
        Ok(ApiResult {
            id: task.id.clone(),
            status: ApiStatus::Failed,
            output_files: Vec::new(),
            output_json,
            metadata: BTreeMap::new(),
            cost: None,
            duration_ms: 0,
            provider_request_id: final_response
                .provider_request_id
                .clone()
                .or_else(|| submit_response.provider_request_id.clone()),
            cache_hit: false,
            error: Some(ApiErrorInfo {
                code: "poll_timeout".to_string(),
                message: format!(
                    "custom_http async job {job_id} did not finish after {max_polls} polls"
                ),
                retryable: true,
            }),
        })
    }

    async fn send_task_request(
        &self,
        task: &ApiTask,
        prefix: &str,
        url: &str,
        default_method: &str,
        fallback_headers: bool,
    ) -> BrokerResult<HttpResponse> {
        let method_key = prefixed_key(prefix, "method");
        let method = value_str(task, &method_key).unwrap_or(default_method);
        let method = Method::from_bytes(method.as_bytes())
            .map_err(|err| BrokerError::Provider(format!("invalid HTTP method: {err}")))?;

        let mut request = self.client.request(method, url);

        if let Some(timeout_ms) = task.retry_policy.timeout_ms {
            request = request.timeout(Duration::from_millis(timeout_ms));
        }

        let headers_key = prefixed_key(prefix, "headers");
        if let Some(headers) = value(task, &headers_key)
            .and_then(Value::as_object)
            .or_else(|| {
                if fallback_headers {
                    value(task, "headers").and_then(Value::as_object)
                } else {
                    None
                }
            })
        {
            for (name, header_value) in headers {
                if let Some(header_value) = header_value.as_str() {
                    request = request.header(name, header_value);
                }
            }
        }

        let query_key = prefixed_key(prefix, "query");
        if let Some(query) = value(task, &query_key).and_then(Value::as_object) {
            let pairs = string_pairs(query);
            request = request.query(&pairs);
        }

        let json_key = prefixed_key(prefix, "json");
        let body_key = prefixed_key(prefix, "body");
        if let Some(json_body) = value(task, &json_key) {
            request = request.json(json_body);
        } else if let Some(body) = value_str(task, &body_key) {
            request = request.body(body.to_string());
        }

        let response = request
            .send()
            .await
            .map_err(|err| BrokerError::Provider(format!("HTTP request failed: {err}")))?;
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
        let header_json = headers_to_json(&headers);
        let body_bytes = response
            .bytes()
            .await
            .map_err(|err| BrokerError::Provider(format!("failed to read HTTP body: {err}")))?;
        let body_bytes = body_bytes.to_vec();
        let body = response_body_json(&body_bytes, &content_type);

        Ok(HttpResponse {
            status,
            headers: header_json,
            body,
            body_bytes,
            content_type,
            provider_request_id,
        })
    }

    async fn download_result_output(
        &self,
        task: &ApiTask,
        url: &str,
        index: usize,
    ) -> BrokerResult<OutputFile> {
        let mut request = self.client.get(url);

        if let Some(timeout_ms) = task.retry_policy.timeout_ms {
            request = request.timeout(Duration::from_millis(timeout_ms));
        }

        if let Some(headers) = value(task, "download_headers")
            .and_then(Value::as_object)
            .or_else(|| value(task, "headers").and_then(Value::as_object))
        {
            for (name, header_value) in headers {
                if let Some(header_value) = header_value.as_str() {
                    request = request.header(name, header_value);
                }
            }
        }

        let response = request
            .send()
            .await
            .map_err(|err| BrokerError::Provider(format!("download_result failed: {err}")))?;
        let status = response.status();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body_bytes = response.bytes().await.map_err(|err| {
            BrokerError::Provider(format!("failed to read downloaded result: {err}"))
        })?;

        if !status.is_success() {
            return Err(BrokerError::Provider(format!(
                "download_result failed with status {}",
                status.as_u16()
            )));
        }

        save_bytes_output(
            task,
            &body_bytes,
            &content_type,
            index,
            value_str(task, "download_output_extension")
                .or_else(|| value_str(task, "output_extension")),
        )
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

fn inferred_default_method(task: &ApiTask, prefix: &str) -> &'static str {
    let json_key = prefixed_key(prefix, "json");
    let body_key = prefixed_key(prefix, "body");
    if value(task, &json_key).is_some() || value_str(task, &body_key).is_some() {
        "POST"
    } else {
        "GET"
    }
}

fn prefixed_key(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        key.to_string()
    } else {
        format!("{prefix}_{key}")
    }
}

fn response_body_json(body_bytes: &[u8], content_type: &str) -> Value {
    if content_type.contains("application/json") {
        return serde_json::from_slice::<Value>(body_bytes).unwrap_or_else(|_| {
            String::from_utf8(body_bytes.to_vec())
                .map(Value::String)
                .unwrap_or(Value::Null)
        });
    }

    if is_text_content_type(content_type) {
        return String::from_utf8(body_bytes.to_vec())
            .map(Value::String)
            .unwrap_or(Value::Null);
    }

    Value::Null
}

fn is_text_content_type(content_type: &str) -> bool {
    let content_type = content_type.to_ascii_lowercase();
    content_type.starts_with("text/")
        || content_type.contains("application/xml")
        || content_type.contains("application/xhtml")
        || content_type.contains("application/javascript")
        || content_type.contains("application/x-www-form-urlencoded")
}

fn failed_http_result(
    task: &ApiTask,
    response: HttpResponse,
    output_files: Vec<OutputFile>,
    output_json: Option<Value>,
) -> ApiResult {
    ApiResult {
        id: task.id.clone(),
        status: ApiStatus::Failed,
        output_files,
        output_json,
        metadata: BTreeMap::new(),
        cost: None,
        duration_ms: 0,
        provider_request_id: response.provider_request_id,
        cache_hit: false,
        error: Some(ApiErrorInfo {
            code: response.status.as_u16().to_string(),
            message: format!(
                "HTTP request failed with status {}",
                response.status.as_u16()
            ),
            retryable: false,
        }),
    }
}

fn response_output_json(response: &HttpResponse, output_files: &[OutputFile]) -> Value {
    json!({
        "status_code": response.status.as_u16(),
        "headers": response.headers.clone(),
        "body": response.body.clone(),
        "body_size_bytes": response.body_bytes.len(),
        "body_saved": !output_files.is_empty(),
    })
}

#[allow(clippy::too_many_arguments)]
fn async_job_output_json(
    submit_response: &HttpResponse,
    final_response: &HttpResponse,
    output_files: &[OutputFile],
    job_id: &str,
    poll_count: u64,
    max_polls: u64,
    poll_interval_ms: u64,
    status_path: &str,
    status_value: Option<&str>,
    result_path: Option<&str>,
    body_saved: bool,
    download_saved: bool,
) -> Value {
    let result = result_path
        .and_then(|path| json_path_value(&final_response.body, path))
        .cloned()
        .unwrap_or_else(|| final_response.body.clone());

    json!({
        "status_code": final_response.status.as_u16(),
        "headers": final_response.headers.clone(),
        "body": final_response.body.clone(),
        "body_size_bytes": final_response.body_bytes.len(),
        "body_saved": body_saved,
        "download_saved": download_saved,
        "job_id": job_id,
        "polling": {
            "poll_count": poll_count,
            "max_polls": max_polls,
            "poll_interval_ms": poll_interval_ms,
            "status_path": status_path,
            "status_value": status_value,
        },
        "submit": {
            "status_code": submit_response.status.as_u16(),
            "headers": submit_response.headers.clone(),
            "body": submit_response.body.clone(),
            "body_size_bytes": submit_response.body_bytes.len(),
        },
        "result": result,
        "output_files": output_files,
    })
}

fn save_response_output(
    task: &ApiTask,
    body_bytes: &[u8],
    content_type: &str,
    index: usize,
) -> BrokerResult<OutputFile> {
    save_bytes_output(
        task,
        body_bytes,
        content_type,
        index,
        value_str(task, "output_extension"),
    )
}

fn save_bytes_output(
    task: &ApiTask,
    body_bytes: &[u8],
    content_type: &str,
    index: usize,
    extension_override: Option<&str>,
) -> BrokerResult<OutputFile> {
    let mime_type = normalized_content_type(content_type);
    let extension = extension_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| extension_for_content_type(mime_type.as_deref()));

    write_task_output_bytes(
        value_str(task, "output_dir"),
        task,
        index,
        body_bytes,
        mime_type.as_deref(),
        &extension,
    )
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
        "application/xml" | "text/xml" => "xml",
        "audio/mpeg" => "mp3",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/webm" => "webm",
        "image/gif" => "gif",
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "text/csv" => "csv",
        "text/html" => "html",
        "text/plain" => "txt",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        _ => "bin",
    }
    .to_string()
}

fn resolve_job_id(task: &ApiTask, submit_body: &Value) -> BrokerResult<String> {
    if let Some(path) = value_str(task, "job_id_path")
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return json_path_value(submit_body, path)
            .and_then(value_to_string)
            .ok_or_else(|| BrokerError::Provider(format!("job_id_path '{path}' was not found")));
    }

    for path in [
        "id",
        "job_id",
        "task_id",
        "data.id",
        "data.job_id",
        "data.task_id",
    ] {
        if let Some(value) = json_path_value(submit_body, path).and_then(value_to_string) {
            return Ok(value);
        }
    }

    Err(BrokerError::Provider(
        "custom_http async_job could not find job id; set job_id_path".to_string(),
    ))
}

fn resolve_poll_url(task: &ApiTask, submit_body: &Value, job_id: &str) -> BrokerResult<String> {
    if let Some(poll_url) = value_str(task, "poll_url")
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(apply_job_id_template(poll_url, job_id));
    }

    if let Some(path) = value_str(task, "poll_url_path")
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return json_path_value(submit_body, path)
            .and_then(value_to_string)
            .map(|value| apply_job_id_template(&value, job_id))
            .ok_or_else(|| BrokerError::Provider(format!("poll_url_path '{path}' was not found")));
    }

    for path in [
        "poll_url",
        "status_url",
        "urls.poll",
        "urls.status",
        "data.poll_url",
        "data.status_url",
    ] {
        if let Some(value) = json_path_value(submit_body, path).and_then(value_to_string) {
            return Ok(apply_job_id_template(&value, job_id));
        }
    }

    Err(BrokerError::Provider(
        "custom_http async_job requires poll_url or poll_url_path".to_string(),
    ))
}

fn apply_job_id_template(template: &str, job_id: &str) -> String {
    template.replace("{job_id}", job_id).replace("{id}", job_id)
}

fn normalized_string_list(task: &ApiTask, key: &str, defaults: &[&str]) -> Vec<String> {
    let values = value(task, key)
        .map(string_list_from_value)
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| defaults.iter().map(|value| (*value).to_string()).collect());

    values
        .into_iter()
        .map(|value| normalized_status_value(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn string_list_from_value(value: &Value) -> Vec<String> {
    match value {
        Value::Array(values) => values.iter().filter_map(value_to_string).collect(),
        Value::String(value) => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect(),
        _ => value_to_string(value).into_iter().collect(),
    }
}

fn normalized_status_value(value: &str) -> String {
    value.trim().to_ascii_lowercase()
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

    if path.starts_with('/') {
        return value.pointer(path);
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

fn string_pairs(map: &Map<String, Value>) -> Vec<(String, String)> {
    map.iter()
        .map(|(key, value)| {
            let rendered = value
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| value.to_string());
            (key.clone(), rendered)
        })
        .collect()
}

fn headers_to_json(headers: &reqwest::header::HeaderMap) -> Value {
    let mut output = Map::new();
    for (name, value) in headers {
        if let Ok(value) = value.to_str() {
            output.insert(name.as_str().to_string(), json!(value));
        }
    }
    Value::Object(output)
}
