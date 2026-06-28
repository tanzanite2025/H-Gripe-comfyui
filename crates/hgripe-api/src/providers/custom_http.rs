use crate::credentials::{load_credential_ref, CredentialEntry};
use crate::model::{ApiErrorInfo, ApiResult, ApiStatus, ApiTask, OutputFile};
use crate::outputs::write_task_output_bytes;
use crate::profiles::{load_provider_profile, ProviderProfile};
use crate::provider::{BrokerError, BrokerResult, Provider};
use async_trait::async_trait;
use reqwest::header::CONTENT_TYPE;
use reqwest::multipart::{Form, Part};
use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::Path;
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
        let task = apply_provider_profile(task)?;
        match task.operation.as_str() {
            "async_job" | "http.async_job" => self.execute_async_job(&task).await,
            _ => self.execute_request(&task).await,
        }
    }
}

impl CustomHttpProvider {
    async fn execute_request(&self, task: &ApiTask) -> BrokerResult<ApiResult> {
        let url = value_str(task, "url")
            .ok_or_else(|| BrokerError::Provider("custom_http requires params.url".to_string()))?;
        let credentials = resolve_credentials(task)?;
        let response = self
            .send_task_request(
                task,
                "",
                url,
                inferred_default_method(task, ""),
                false,
                credentials.as_ref(),
            )
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
        let credentials = resolve_credentials(task)?;
        let submit_response = self
            .send_task_request(
                task,
                "",
                submit_url,
                inferred_default_method(task, ""),
                false,
                credentials.as_ref(),
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
                .send_task_request(task, "poll", &poll_url, "GET", true, credentials.as_ref())
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
                        self.download_result_output(
                            task,
                            &download_url,
                            output_files.len(),
                            credentials.as_ref(),
                        )
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
        credentials: Option<&CredentialEntry>,
    ) -> BrokerResult<HttpResponse> {
        let method_key = prefixed_key(prefix, "method");
        let method = value_str(task, &method_key).unwrap_or(default_method);
        let method = Method::from_bytes(method.as_bytes())
            .map_err(|err| BrokerError::Provider(format!("invalid HTTP method: {err}")))?;
        let url = request_url(task, url, credentials)?;

        let mut request = self.client.request(method, url);

        if let Some(timeout_ms) = task.retry_policy.timeout_ms {
            request = request.timeout(Duration::from_millis(timeout_ms));
        }

        if let Some(api_key) = resolve_api_key(task, credentials)?
            .filter(|_| !has_auth_header(task, prefix, fallback_headers, credentials))
        {
            request = request.bearer_auth(api_key);
        }

        if let Some(headers) = credentials.and_then(|entry| entry.headers.as_ref()) {
            for (name, header_value) in headers {
                request = request.header(name, header_value);
            }
        }

        if let Some(headers) = task_headers(task, prefix, fallback_headers) {
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
        if let Some(form) = multipart_form_from_task(task, prefix)? {
            request = request.multipart(form);
        } else if let Some(json_body) = value(task, &json_key) {
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
        credentials: Option<&CredentialEntry>,
    ) -> BrokerResult<OutputFile> {
        let url = request_url(task, url, credentials)?;
        let mut request = self.client.get(url);

        if let Some(timeout_ms) = task.retry_policy.timeout_ms {
            request = request.timeout(Duration::from_millis(timeout_ms));
        }

        if let Some(api_key) = resolve_api_key(task, credentials)?
            .filter(|_| !has_download_auth_header(task, credentials))
        {
            request = request.bearer_auth(api_key);
        }

        if let Some(headers) = credentials.and_then(|entry| entry.headers.as_ref()) {
            for (name, header_value) in headers {
                request = request.header(name, header_value);
            }
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

fn credentials_file(task: &ApiTask) -> Option<&str> {
    value_str(task, "credentials_file")
}

fn profiles_file(task: &ApiTask) -> Option<&str> {
    value_str(task, "profiles_file")
}

fn profile_ref(task: &ApiTask) -> Option<&str> {
    value_str(task, "profile_ref").or_else(|| value_str(task, "provider_profile_ref"))
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
    insert_optional_string(&mut merged.params, "url", profile.path.as_deref());
    insert_optional_string(
        &mut merged.params,
        "api_key_env",
        profile.api_key_env.as_deref(),
    );
    if let Some(no_auth) = profile.no_auth {
        merged.params.insert("no_auth".to_string(), json!(no_auth));
    }
    if let Some(headers) = &profile.headers {
        merge_task_param(&mut merged.params, "headers".to_string(), json!(headers));
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

    if key == "headers" || key.ends_with("_headers") || key == "query" || key.ends_with("_query") {
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
        if !provider.is_empty() && provider != "custom_http" {
            return Err(BrokerError::Provider(format!(
                "credentials_ref '{credential_ref}' is for provider '{provider}', not custom_http"
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

    Ok(env::var("HGRIPE_CUSTOM_HTTP_API_KEY")
        .ok()
        .filter(|value| !value.is_empty()))
}

fn request_url(
    task: &ApiTask,
    url: &str,
    credentials: Option<&CredentialEntry>,
) -> BrokerResult<String> {
    let url = url.trim();
    if url.starts_with("http://") || url.starts_with("https://") {
        return Ok(url.to_string());
    }

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
            env::var("HGRIPE_CUSTOM_HTTP_BASE_URL")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .ok_or_else(|| {
            BrokerError::Provider(
                "custom_http URL must be absolute or base_url must be configured".to_string(),
            )
        })?;

    let path = if url.starts_with('/') {
        url.to_string()
    } else {
        format!("/{url}")
    };
    Ok(format!("{}{}", base_url.trim_end_matches('/'), path))
}

fn task_headers<'a>(
    task: &'a ApiTask,
    prefix: &str,
    fallback_headers: bool,
) -> Option<&'a Map<String, Value>> {
    let headers_key = prefixed_key(prefix, "headers");
    value(task, &headers_key)
        .and_then(Value::as_object)
        .or_else(|| {
            if fallback_headers {
                value(task, "headers").and_then(Value::as_object)
            } else {
                None
            }
        })
}

fn has_auth_header(
    task: &ApiTask,
    prefix: &str,
    fallback_headers: bool,
    credentials: Option<&CredentialEntry>,
) -> bool {
    credentials
        .and_then(|entry| entry.headers.as_ref())
        .is_some_and(|headers| headers.keys().any(|name| is_authorization_header(name)))
        || task_headers(task, prefix, fallback_headers)
            .is_some_and(|headers| headers.keys().any(|name| is_authorization_header(name)))
}

fn has_download_auth_header(task: &ApiTask, credentials: Option<&CredentialEntry>) -> bool {
    credentials
        .and_then(|entry| entry.headers.as_ref())
        .is_some_and(|headers| headers.keys().any(|name| is_authorization_header(name)))
        || value(task, "download_headers")
            .and_then(Value::as_object)
            .or_else(|| value(task, "headers").and_then(Value::as_object))
            .is_some_and(|headers| headers.keys().any(|name| is_authorization_header(name)))
}

fn is_authorization_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("authorization") || name.eq_ignore_ascii_case("proxy-authorization")
}

fn multipart_enabled(task: &ApiTask, prefix: &str) -> bool {
    let fields_key = prefixed_key(prefix, "multipart_fields");
    let files_key = prefixed_key(prefix, "multipart_files");
    let file_path_key = prefixed_key(prefix, "multipart_file_path");
    value(task, &fields_key).is_some()
        || value(task, &files_key).is_some()
        || value_str(task, &file_path_key)
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
}

fn inferred_default_method(task: &ApiTask, prefix: &str) -> &'static str {
    let json_key = prefixed_key(prefix, "json");
    let body_key = prefixed_key(prefix, "body");
    if multipart_enabled(task, prefix)
        || value(task, &json_key).is_some()
        || value_str(task, &body_key).is_some()
    {
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

fn multipart_form_from_task(task: &ApiTask, prefix: &str) -> BrokerResult<Option<Form>> {
    if !multipart_enabled(task, prefix) {
        return Ok(None);
    }

    let mut form = Form::new();

    let fields_key = prefixed_key(prefix, "multipart_fields");
    if let Some(fields) = value(task, &fields_key).and_then(Value::as_object) {
        for (key, field_value) in fields {
            form = form.text(key.clone(), multipart_value_to_string(field_value));
        }
    }

    let files_key = prefixed_key(prefix, "multipart_files");
    if let Some(files) = value(task, &files_key) {
        form = add_multipart_files(form, files)?;
    }

    let file_path_key = prefixed_key(prefix, "multipart_file_path");
    if let Some(path) = value_str(task, &file_path_key)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let field_key = prefixed_key(prefix, "multipart_file_field");
        let filename_key = prefixed_key(prefix, "multipart_file_name");
        let mime_type_key = prefixed_key(prefix, "multipart_file_mime_type");
        let field = value_str(task, &field_key)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("file");
        let file = MultipartFileSpec {
            field: field.to_string(),
            path: path.to_string(),
            filename: value_str(task, &filename_key)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            mime_type: value_str(task, &mime_type_key)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
        };
        form = form.part(file.field.clone(), multipart_file_part(&file)?);
    }

    Ok(Some(form))
}

struct MultipartFileSpec {
    field: String,
    path: String,
    filename: Option<String>,
    mime_type: Option<String>,
}

fn add_multipart_files(mut form: Form, files: &Value) -> BrokerResult<Form> {
    match files {
        Value::Array(items) => {
            for item in items {
                let file = multipart_file_spec_from_value(item)?;
                form = form.part(file.field.clone(), multipart_file_part(&file)?);
            }
        }
        Value::Object(map) => {
            for (field, value) in map {
                let file = match value {
                    Value::String(path) => MultipartFileSpec {
                        field: field.clone(),
                        path: path.clone(),
                        filename: None,
                        mime_type: None,
                    },
                    Value::Object(_) => {
                        let mut file = multipart_file_spec_from_value(value)?;
                        if file.field.trim().is_empty() || file.field == "file" {
                            file.field = field.clone();
                        }
                        file
                    }
                    _ => {
                        return Err(BrokerError::Provider(
                            "multipart_files object values must be paths or file objects"
                                .to_string(),
                        ));
                    }
                };
                form = form.part(file.field.clone(), multipart_file_part(&file)?);
            }
        }
        _ => {
            return Err(BrokerError::Provider(
                "multipart_files must be an array or object".to_string(),
            ));
        }
    }

    Ok(form)
}

fn multipart_file_spec_from_value(value: &Value) -> BrokerResult<MultipartFileSpec> {
    let object = value.as_object().ok_or_else(|| {
        BrokerError::Provider("multipart file entry must be an object".to_string())
    })?;
    let field = object
        .get("field")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("file")
        .to_string();
    let path = object
        .get("path")
        .or_else(|| object.get("file_path"))
        .or_else(|| object.get("source_path"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            BrokerError::Provider("multipart file entry requires path/file_path".to_string())
        })?
        .to_string();
    let filename = object
        .get("filename")
        .or_else(|| object.get("file_name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let mime_type = object
        .get("mime_type")
        .or_else(|| object.get("content_type"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    Ok(MultipartFileSpec {
        field,
        path,
        filename,
        mime_type,
    })
}

fn multipart_file_part(file: &MultipartFileSpec) -> BrokerResult<Part> {
    let path = Path::new(&file.path);
    let bytes = fs::read(path).map_err(|err| {
        BrokerError::Provider(format!(
            "failed to read multipart file {}: {err}",
            path.display()
        ))
    })?;
    let filename = file
        .filename
        .clone()
        .or_else(|| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "upload.bin".to_string());
    let mime_type = file
        .mime_type
        .clone()
        .or_else(|| mime_type_from_path(path))
        .unwrap_or_else(|| "application/octet-stream".to_string());

    Part::bytes(bytes)
        .file_name(filename)
        .mime_str(&mime_type)
        .map_err(|err| BrokerError::Provider(format!("invalid multipart MIME type: {err}")))
}

fn multipart_value_to_string(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

fn mime_type_from_path(path: &Path) -> Option<String> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("aac") => Some("audio/aac".to_string()),
        Some("csv") => Some("text/csv".to_string()),
        Some("flac") => Some("audio/flac".to_string()),
        Some("gif") => Some("image/gif".to_string()),
        Some("htm" | "html") => Some("text/html".to_string()),
        Some("jpg" | "jpeg") => Some("image/jpeg".to_string()),
        Some("json") => Some("application/json".to_string()),
        Some("m4a") => Some("audio/mp4".to_string()),
        Some("mov") => Some("video/quicktime".to_string()),
        Some("mp3") => Some("audio/mpeg".to_string()),
        Some("mp4") => Some("video/mp4".to_string()),
        Some("ogg" | "oga") => Some("audio/ogg".to_string()),
        Some("opus") => Some("audio/opus".to_string()),
        Some("pdf") => Some("application/pdf".to_string()),
        Some("png") => Some("image/png".to_string()),
        Some("txt") => Some("text/plain".to_string()),
        Some("wav") => Some("audio/wav".to_string()),
        Some("webm") => Some("video/webm".to_string()),
        Some("webp") => Some("image/webp".to_string()),
        Some("xml") => Some("application/xml".to_string()),
        _ => None,
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
