use crate::model::{ApiErrorInfo, ApiResult, ApiStatus, ApiTask};
use crate::provider::{BrokerError, BrokerResult, Provider};
use async_trait::async_trait;
use reqwest::header::CONTENT_TYPE;
use reqwest::{Client, Method};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::time::Duration;

pub struct CustomHttpProvider {
    client: Client,
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
        matches!(operation, "request" | "http.request")
    }

    async fn execute(&self, task: &ApiTask) -> BrokerResult<ApiResult> {
        let url = value_str(task, "url")
            .ok_or_else(|| BrokerError::Provider("custom_http requires params.url".to_string()))?;
        let method = value_str(task, "method").unwrap_or_else(|| {
            if value(task, "json").is_some() || value_str(task, "body").is_some() {
                "POST"
            } else {
                "GET"
            }
        });
        let method = Method::from_bytes(method.as_bytes())
            .map_err(|err| BrokerError::Provider(format!("invalid HTTP method: {err}")))?;

        let mut request = self.client.request(method, url);

        if let Some(timeout_ms) = task.retry_policy.timeout_ms {
            request = request.timeout(Duration::from_millis(timeout_ms));
        }

        if let Some(headers) = value(task, "headers").and_then(Value::as_object) {
            for (name, header_value) in headers {
                if let Some(header_value) = header_value.as_str() {
                    request = request.header(name, header_value);
                }
            }
        }

        if let Some(query) = value(task, "query").and_then(Value::as_object) {
            let pairs = string_pairs(query);
            request = request.query(&pairs);
        }

        if let Some(json_body) = value(task, "json") {
            request = request.json(json_body);
        } else if let Some(body) = value_str(task, "body") {
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
        let text = response
            .text()
            .await
            .map_err(|err| BrokerError::Provider(format!("failed to read HTTP body: {err}")))?;

        if status.is_server_error() {
            return Err(BrokerError::Provider(format!(
                "server error {} from custom_http",
                status.as_u16()
            )));
        }

        let body_json = if content_type.contains("application/json") {
            serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!(text))
        } else {
            json!(text)
        };

        let output_json = Some(json!({
            "status_code": status.as_u16(),
            "headers": header_json,
            "body": body_json,
        }));

        if status.is_success() {
            let mut result = ApiResult::succeeded(task.id.clone(), output_json);
            result.provider_request_id = provider_request_id;
            Ok(result)
        } else {
            Ok(ApiResult {
                id: task.id.clone(),
                status: ApiStatus::Failed,
                output_files: Vec::new(),
                output_json,
                metadata: BTreeMap::new(),
                cost: None,
                duration_ms: 0,
                provider_request_id,
                cache_hit: false,
                error: Some(ApiErrorInfo {
                    code: status.as_u16().to_string(),
                    message: format!("HTTP request failed with status {}", status.as_u16()),
                    retryable: false,
                }),
            })
        }
    }
}

fn value<'a>(task: &'a ApiTask, key: &str) -> Option<&'a Value> {
    task.params.get(key).or_else(|| task.inputs.get(key))
}

fn value_str<'a>(task: &'a ApiTask, key: &str) -> Option<&'a str> {
    value(task, key).and_then(Value::as_str)
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
