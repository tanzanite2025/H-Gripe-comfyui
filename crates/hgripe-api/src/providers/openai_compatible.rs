use crate::credentials::{load_credential_ref, CredentialEntry};
use crate::model::{ApiErrorInfo, ApiResult, ApiStatus, ApiTask, OutputFile};
use crate::outputs::write_task_output_bytes;
use crate::profiles::{load_provider_profile, ProviderProfile};
use crate::provider::{BrokerError, BrokerResult, Provider};
use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use reqwest::header::CONTENT_TYPE;
use reqwest::multipart::{Form, Part};
use reqwest::{Client, StatusCode};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::Path;
use std::time::Duration;

pub struct OpenAiCompatibleProvider {
    client: Client,
}

struct JsonResponse {
    status: StatusCode,
    body: Value,
    provider_request_id: Option<String>,
}

struct MultipartImage {
    bytes: Vec<u8>,
    filename: String,
    mime_type: String,
}

impl MultipartImage {
    fn into_part(self) -> BrokerResult<Part> {
        Part::bytes(self.bytes)
            .file_name(self.filename)
            .mime_str(&self.mime_type)
            .map_err(|err| {
                BrokerError::Provider(format!("invalid multipart image MIME type: {err}"))
            })
    }
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
                | "image.edit"
                | "image.generate"
        )
    }

    async fn execute(&self, task: &ApiTask) -> BrokerResult<ApiResult> {
        let task = apply_provider_profile(task)?;
        match task.operation.as_str() {
            "image.edit" => self.execute_image_edit(&task).await,
            "image.generate" => self.execute_image(&task).await,
            _ => self.execute_chat(&task).await,
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
            let mut images = response
                .body
                .get("data")
                .cloned()
                .unwrap_or_else(|| Value::Array(Vec::new()));
            let output_files = if value_bool(task, "save_outputs").unwrap_or(false) {
                self.save_image_outputs(task, &mut images).await?
            } else {
                Vec::new()
            };
            let mut result = ApiResult::succeeded(
                task.id.clone(),
                Some(json!({
                    "images": images,
                    "raw": response.body,
                })),
            );
            result.provider_request_id = response.provider_request_id;
            result.output_files = output_files;
            Ok(result)
        } else {
            Ok(failed_result(task, response))
        }
    }

    async fn execute_image_edit(&self, task: &ApiTask) -> BrokerResult<ApiResult> {
        let image = multipart_image_from_task(task, "image")?.ok_or_else(|| {
            BrokerError::Provider(
                "image.edit requires image_data_url, image_b64, image_base64, or image_path"
                    .to_string(),
            )
        })?;

        let mut form = Form::new()
            .part("image", image.into_part()?)
            .text("prompt", required_prompt(task)?.to_string());

        if let Some(mask) = multipart_image_from_task(task, "mask")? {
            form = form.part("mask", mask.into_part()?);
        }

        if let Some(model) = value_str(task, "model") {
            form = form.text("model", model.to_string());
        }

        form = copy_optional_form_fields(
            task,
            form,
            &[
                "n",
                "size",
                "quality",
                "response_format",
                "user",
                "background",
                "output_format",
                "input_fidelity",
            ],
        );
        form = merge_extra_body_form(task, form)?;

        let path = value_str(task, "path").unwrap_or("/images/edits");
        let response = self.send_multipart(task, path, form).await?;

        if response.status.is_success() {
            let mut images = response
                .body
                .get("data")
                .cloned()
                .unwrap_or_else(|| Value::Array(Vec::new()));
            let output_files = if value_bool(task, "save_outputs").unwrap_or(false) {
                self.save_image_outputs(task, &mut images).await?
            } else {
                Vec::new()
            };
            let mut result = ApiResult::succeeded(
                task.id.clone(),
                Some(json!({
                    "images": images,
                    "raw": response.body,
                })),
            );
            result.provider_request_id = response.provider_request_id;
            result.output_files = output_files;
            Ok(result)
        } else {
            Ok(failed_result(task, response))
        }
    }

    async fn save_image_outputs(
        &self,
        task: &ApiTask,
        images: &mut Value,
    ) -> BrokerResult<Vec<OutputFile>> {
        let Some(items) = images.as_array_mut() else {
            return Ok(Vec::new());
        };

        let mut files = Vec::new();
        for (index, item) in items.iter_mut().enumerate() {
            let Some(item_object) = item.as_object_mut() else {
                continue;
            };

            let Some(image_bytes) = self.image_bytes_from_item(task, item_object).await? else {
                continue;
            };

            let (mime_type, extension) =
                detect_image_type(&image_bytes).unwrap_or_else(|| fallback_image_type(task));
            let output_file = write_task_output_bytes(
                value_str(task, "output_dir"),
                task,
                index,
                &image_bytes,
                Some(mime_type),
                extension,
            )?;

            item_object.insert("local_path".to_string(), json!(output_file.path.clone()));
            if let Some(mime_type) = output_file.mime_type.as_deref() {
                item_object.insert("local_mime_type".to_string(), json!(mime_type));
            }
            if let Some(size_bytes) = output_file.size_bytes {
                item_object.insert("local_size_bytes".to_string(), json!(size_bytes));
            }
            if let Some(sha256) = output_file.sha256.as_deref() {
                item_object.insert("local_sha256".to_string(), json!(sha256));
            }

            files.push(output_file);
        }

        Ok(files)
    }

    async fn image_bytes_from_item(
        &self,
        task: &ApiTask,
        item: &Map<String, Value>,
    ) -> BrokerResult<Option<Vec<u8>>> {
        if let Some(b64_json) = item.get("b64_json").and_then(Value::as_str) {
            let b64_json = b64_json.trim();
            if !b64_json.is_empty() {
                let bytes = BASE64_STANDARD.decode(b64_json).map_err(|err| {
                    BrokerError::Provider(format!("failed to decode b64_json image: {err}"))
                })?;
                return Ok(Some(bytes));
            }
        }

        if !value_bool(task, "download_url_outputs").unwrap_or(false) {
            return Ok(None);
        }

        let Some(url) = item.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        let url = url.trim();
        if url.is_empty() {
            return Ok(None);
        }

        let mut request = self.client.get(url);
        if let Some(timeout_ms) = task.retry_policy.timeout_ms {
            request = request.timeout(Duration::from_millis(timeout_ms));
        }
        let response = request.send().await.map_err(|err| {
            BrokerError::Provider(format!("failed to download image output: {err}"))
        })?;
        if !response.status().is_success() {
            return Err(BrokerError::Provider(format!(
                "failed to download image output: HTTP {}",
                response.status().as_u16()
            )));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|err| BrokerError::Provider(format!("failed to read image output: {err}")))?;
        Ok(Some(bytes.to_vec()))
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

    async fn send_multipart(
        &self,
        task: &ApiTask,
        path: &str,
        form: Form,
    ) -> BrokerResult<JsonResponse> {
        let credentials = resolve_credentials(task)?;
        let url = endpoint_url(task, path, credentials.as_ref());
        let mut request = self.client.post(url).multipart(form);

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

fn multipart_image_from_task(task: &ApiTask, prefix: &str) -> BrokerResult<Option<MultipartImage>> {
    let data_url_key = format!("{prefix}_data_url");
    if let Some(data_url) = value_str(task, &data_url_key)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let (mime_type, bytes) = decode_data_url(data_url)?;
        return Ok(Some(MultipartImage {
            bytes,
            filename: multipart_filename(task, prefix, &mime_type),
            mime_type,
        }));
    }

    for key in [format!("{prefix}_b64"), format!("{prefix}_base64")] {
        if let Some(encoded) = value_str(task, &key)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let bytes = BASE64_STANDARD.decode(encoded).map_err(|err| {
                BrokerError::Provider(format!("failed to decode {key} image: {err}"))
            })?;
            let mime_type = value_str(task, &format!("{prefix}_mime_type"))
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .or_else(|| detect_image_type(&bytes).map(|(mime_type, _)| mime_type.to_string()))
                .unwrap_or_else(|| "image/png".to_string());
            return Ok(Some(MultipartImage {
                bytes,
                filename: multipart_filename(task, prefix, &mime_type),
                mime_type,
            }));
        }
    }

    let path_key = format!("{prefix}_path");
    if let Some(path) = value_str(task, &path_key)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let path = Path::new(path);
        let bytes = fs::read(path).map_err(|err| {
            BrokerError::Provider(format!(
                "failed to read {path_key} {}: {err}",
                path.display()
            ))
        })?;
        let mime_type = detect_image_type(&bytes)
            .map(|(mime_type, _)| mime_type.to_string())
            .or_else(|| mime_type_from_path(path))
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let filename = value_str(task, &format!("{prefix}_filename"))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| filename_for_mime_type(prefix, &mime_type));
        return Ok(Some(MultipartImage {
            bytes,
            filename,
            mime_type,
        }));
    }

    Ok(None)
}

fn decode_data_url(data_url: &str) -> BrokerResult<(String, Vec<u8>)> {
    let (metadata, encoded) = data_url
        .split_once(',')
        .ok_or_else(|| BrokerError::Provider("data URL is missing comma separator".to_string()))?;
    let metadata = metadata.trim();
    if !metadata.starts_with("data:") || !metadata.contains(";base64") {
        return Err(BrokerError::Provider(
            "image data URL must use base64 encoding".to_string(),
        ));
    }

    let mime_type = metadata
        .trim_start_matches("data:")
        .split(';')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or("application/octet-stream")
        .to_string();
    let bytes = BASE64_STANDARD
        .decode(encoded.trim())
        .map_err(|err| BrokerError::Provider(format!("failed to decode image data URL: {err}")))?;
    Ok((mime_type, bytes))
}

fn multipart_filename(task: &ApiTask, prefix: &str, mime_type: &str) -> String {
    value_str(task, &format!("{prefix}_filename"))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| filename_for_mime_type(prefix, mime_type))
}

fn filename_for_mime_type(prefix: &str, mime_type: &str) -> String {
    let extension = match mime_type {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/png" => "png",
        _ => "bin",
    };
    format!("{prefix}.{extension}")
}

fn mime_type_from_path(path: &Path) -> Option<String> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => Some("image/png".to_string()),
        Some("jpg" | "jpeg") => Some("image/jpeg".to_string()),
        Some("webp") => Some("image/webp".to_string()),
        Some("gif") => Some("image/gif".to_string()),
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
    insert_optional_string(&mut merged.params, "model", profile.model.as_deref());
    insert_optional_string(&mut merged.params, "path", profile.path.as_deref());
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

    if key == "headers" || key == "extra_body" {
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

fn detect_image_type(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(("image/png", "png"));
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some(("image/jpeg", "jpg"));
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return Some(("image/webp", "webp"));
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some(("image/gif", "gif"));
    }
    None
}

fn fallback_image_type(task: &ApiTask) -> (&'static str, &'static str) {
    match value_str(task, "output_format")
        .or_else(|| value_str(task, "response_format"))
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "jpeg" | "jpg" => ("image/jpeg", "jpg"),
        "webp" => ("image/webp", "webp"),
        "gif" => ("image/gif", "gif"),
        _ => ("image/png", "png"),
    }
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

fn copy_optional_form_fields(task: &ApiTask, mut form: Form, keys: &[&str]) -> Form {
    for key in keys {
        if let Some(value) = value(task, key) {
            form = form.text((*key).to_string(), multipart_value_to_string(value));
        }
    }
    form
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

fn merge_extra_body_form(task: &ApiTask, mut form: Form) -> BrokerResult<Form> {
    if let Some(extra) = value(task, "extra_body") {
        let extra = extra
            .as_object()
            .ok_or_else(|| BrokerError::Provider("extra_body must be a JSON object".to_string()))?;
        for (key, value) in extra {
            form = form.text(key.clone(), multipart_value_to_string(value));
        }
    }
    Ok(form)
}

fn multipart_value_to_string(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
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
