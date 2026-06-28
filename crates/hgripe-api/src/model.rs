use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutputType {
    Text,
    Image,
    Video,
    Audio,
    Json,
    Files,
    Any,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApiStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Cached,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachePolicy {
    pub enabled: bool,
    pub ttl_seconds: Option<u64>,
    pub key: Option<String>,
}

impl Default for CachePolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            ttl_seconds: None,
            key: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub backoff_ms: u64,
    pub timeout_ms: Option<u64>,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            backoff_ms: 500,
            timeout_ms: Some(120_000),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApiTask {
    pub id: String,
    pub provider: String,
    pub operation: String,
    pub inputs: BTreeMap<String, Value>,
    pub params: BTreeMap<String, Value>,
    pub credentials_ref: Option<String>,
    pub output_type: OutputType,
    pub cache_policy: CachePolicy,
    pub retry_policy: RetryPolicy,
}

impl ApiTask {
    pub fn new(provider: impl Into<String>, operation: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            provider: provider.into(),
            operation: operation.into(),
            inputs: BTreeMap::new(),
            params: BTreeMap::new(),
            credentials_ref: None,
            output_type: OutputType::Any,
            cache_policy: CachePolicy::default(),
            retry_policy: RetryPolicy::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutputFile {
    pub path: String,
    pub mime_type: Option<String>,
    pub size_bytes: Option<u64>,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApiCost {
    pub amount: f64,
    pub currency: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiErrorInfo {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApiResult {
    pub id: String,
    pub status: ApiStatus,
    pub output_files: Vec<OutputFile>,
    pub output_json: Option<Value>,
    pub metadata: BTreeMap<String, Value>,
    pub cost: Option<ApiCost>,
    pub duration_ms: u128,
    pub provider_request_id: Option<String>,
    pub cache_hit: bool,
    pub error: Option<ApiErrorInfo>,
}

impl ApiResult {
    pub fn succeeded(task_id: impl Into<String>, output_json: Option<Value>) -> Self {
        Self {
            id: task_id.into(),
            status: ApiStatus::Succeeded,
            output_files: Vec::new(),
            output_json,
            metadata: BTreeMap::new(),
            cost: None,
            duration_ms: 0,
            provider_request_id: None,
            cache_hit: false,
            error: None,
        }
    }

    pub fn failed(task_id: impl Into<String>, error: ApiErrorInfo) -> Self {
        Self {
            id: task_id.into(),
            status: ApiStatus::Failed,
            output_files: Vec::new(),
            output_json: None,
            metadata: BTreeMap::new(),
            cost: None,
            duration_ms: 0,
            provider_request_id: None,
            cache_hit: false,
            error: Some(error),
        }
    }
}
