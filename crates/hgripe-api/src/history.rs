use crate::model::{ApiErrorInfo, ApiResult, ApiStatus, ApiTask, OutputFile, OutputType};
use crate::outputs::output_dir_from_env;
use crate::provider::{BrokerError, BrokerResult};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const HISTORY_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryRecord {
    pub schema_version: u32,
    pub timestamp_ms: u128,
    pub task_id: String,
    pub provider: String,
    pub operation: String,
    pub output_type: OutputType,
    pub credentials_ref: Option<String>,
    pub status: ApiStatus,
    pub duration_ms: u128,
    pub cache_hit: bool,
    pub provider_request_id: Option<String>,
    pub output_dir: String,
    pub output_file_count: usize,
    pub output_files: Vec<OutputFile>,
    pub output_json_summary: Option<Value>,
    pub error: Option<ApiErrorInfo>,
}

pub fn record_task_result(task: &ApiTask, result: &ApiResult) -> BrokerResult<Option<PathBuf>> {
    if history_disabled() {
        return Ok(None);
    }

    let paths = RuntimePaths::from_env()?;
    let record = build_history_record(task, result, &paths.output_dir);

    append_history_record(&paths.history_file, &record)?;
    upsert_sqlite_history_record(&paths.history_db, &record)?;
    Ok(Some(paths.history_file))
}

pub fn build_history_record(
    task: &ApiTask,
    result: &ApiResult,
    output_dir: &Path,
) -> HistoryRecord {
    HistoryRecord {
        schema_version: HISTORY_SCHEMA_VERSION,
        timestamp_ms: now_ms(),
        task_id: task.id.clone(),
        provider: task.provider.clone(),
        operation: task.operation.clone(),
        output_type: task.output_type.clone(),
        credentials_ref: task.credentials_ref.clone(),
        status: result.status.clone(),
        duration_ms: result.duration_ms,
        cache_hit: result.cache_hit,
        provider_request_id: result.provider_request_id.clone(),
        output_dir: output_dir.to_string_lossy().to_string(),
        output_file_count: result.output_files.len(),
        output_files: result.output_files.clone(),
        output_json_summary: result.output_json.as_ref().map(summarize_json),
        error: result.error.clone(),
    }
}

pub fn record_task_failure(
    task: &ApiTask,
    message: impl Into<String>,
) -> BrokerResult<Option<PathBuf>> {
    if history_disabled() {
        return Ok(None);
    }

    let paths = RuntimePaths::from_env()?;
    let record = HistoryRecord {
        schema_version: HISTORY_SCHEMA_VERSION,
        timestamp_ms: now_ms(),
        task_id: task.id.clone(),
        provider: task.provider.clone(),
        operation: task.operation.clone(),
        output_type: task.output_type.clone(),
        credentials_ref: task.credentials_ref.clone(),
        status: ApiStatus::Failed,
        duration_ms: 0,
        cache_hit: false,
        provider_request_id: None,
        output_dir: paths.output_dir.to_string_lossy().to_string(),
        output_file_count: 0,
        output_files: Vec::new(),
        output_json_summary: None,
        error: Some(ApiErrorInfo {
            code: "broker_error".to_string(),
            message: message.into(),
            retryable: false,
        }),
    };

    append_history_record(&paths.history_file, &record)?;
    upsert_sqlite_history_record(&paths.history_db, &record)?;
    Ok(Some(paths.history_file))
}

pub fn append_history_record(path: &Path, record: &HistoryRecord) -> BrokerResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            BrokerError::Provider(format!(
                "failed to create history directory {}: {err}",
                parent.display()
            ))
        })?;
    }

    let encoded = serde_json::to_string(record)
        .map_err(|err| BrokerError::Provider(format!("failed to encode history record: {err}")))?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|err| {
            BrokerError::Provider(format!(
                "failed to open history file {}: {err}",
                path.display()
            ))
        })?;
    writeln!(file, "{encoded}").map_err(|err| {
        BrokerError::Provider(format!(
            "failed to write history file {}: {err}",
            path.display()
        ))
    })
}

pub fn upsert_sqlite_history_record(path: &Path, record: &HistoryRecord) -> BrokerResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            BrokerError::Provider(format!(
                "failed to create history database directory {}: {err}",
                parent.display()
            ))
        })?;
    }

    let connection = Connection::open(path).map_err(|err| {
        BrokerError::Provider(format!(
            "failed to open history database {}: {err}",
            path.display()
        ))
    })?;
    init_sqlite_history(&connection)?;

    let output_files_json = serde_json::to_string(&record.output_files).map_err(|err| {
        BrokerError::Provider(format!("failed to encode history output files: {err}"))
    })?;
    let output_json_summary_json = record
        .output_json_summary
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|err| {
            BrokerError::Provider(format!("failed to encode history output summary: {err}"))
        })?;
    let error_json = record
        .error
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|err| BrokerError::Provider(format!("failed to encode history error: {err}")))?;
    let record_json = serde_json::to_string(record)
        .map_err(|err| BrokerError::Provider(format!("failed to encode history record: {err}")))?;

    connection
        .execute(
            r#"
            INSERT OR REPLACE INTO task_history (
                task_id,
                schema_version,
                timestamp_ms,
                provider,
                operation,
                output_type,
                credentials_ref,
                status,
                duration_ms,
                cache_hit,
                provider_request_id,
                output_dir,
                output_file_count,
                output_files_json,
                output_json_summary_json,
                error_json,
                record_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
            "#,
            params![
                record.task_id.as_str(),
                HISTORY_SCHEMA_VERSION as i64,
                u128_to_i64(record.timestamp_ms),
                record.provider.as_str(),
                record.operation.as_str(),
                json_scalar_string(&record.output_type)?,
                record.credentials_ref.as_deref(),
                json_scalar_string(&record.status)?,
                u128_to_i64(record.duration_ms),
                if record.cache_hit { 1_i64 } else { 0_i64 },
                record.provider_request_id.as_deref(),
                record.output_dir.as_str(),
                usize_to_i64(record.output_file_count),
                output_files_json,
                output_json_summary_json,
                error_json,
                record_json,
            ],
        )
        .map_err(|err| {
            BrokerError::Provider(format!(
                "failed to write history database {}: {err}",
                path.display()
            ))
        })?;

    Ok(())
}

pub fn list_recent_history_records(path: &Path, limit: usize) -> BrokerResult<Vec<HistoryRecord>> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let connection = Connection::open(path).map_err(|err| {
        BrokerError::Provider(format!(
            "failed to open history database {}: {err}",
            path.display()
        ))
    })?;
    init_sqlite_history(&connection)?;

    let mut statement = connection
        .prepare(
            r#"
            SELECT record_json
            FROM task_history
            ORDER BY timestamp_ms DESC, rowid DESC
            LIMIT ?1
            "#,
        )
        .map_err(|err| {
            BrokerError::Provider(format!(
                "failed to query history database {}: {err}",
                path.display()
            ))
        })?;
    let rows = statement
        .query_map(params![usize_to_i64(limit)], |row| row.get::<_, String>(0))
        .map_err(|err| {
            BrokerError::Provider(format!(
                "failed to read history database {}: {err}",
                path.display()
            ))
        })?;

    let mut records = Vec::new();
    for row in rows {
        let record_json = row.map_err(|err| {
            BrokerError::Provider(format!(
                "failed to read history row from {}: {err}",
                path.display()
            ))
        })?;
        let record = serde_json::from_str::<HistoryRecord>(&record_json).map_err(|err| {
            BrokerError::Provider(format!(
                "failed to decode history row from {}: {err}",
                path.display()
            ))
        })?;
        records.push(record);
    }

    Ok(records)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePaths {
    pub history_file: PathBuf,
    pub history_db: PathBuf,
    pub output_dir: PathBuf,
}

impl RuntimePaths {
    pub fn from_env() -> BrokerResult<Self> {
        let output_dir = output_dir_from_env(None)?;

        let history_file = env_path("HGRIPE_HISTORY_FILE").unwrap_or_else(|| {
            local_root()
                .join("user")
                .join("hgripe")
                .join("history")
                .join("tasks.jsonl")
        });
        let history_db = env_path("HGRIPE_HISTORY_DB").unwrap_or_else(|| {
            local_root()
                .join("user")
                .join("hgripe")
                .join("history")
                .join("tasks.sqlite3")
        });

        Ok(Self {
            history_file,
            history_db,
            output_dir,
        })
    }
}

fn init_sqlite_history(connection: &Connection) -> BrokerResult<()> {
    connection
        .execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS task_history (
                task_id TEXT PRIMARY KEY,
                schema_version INTEGER NOT NULL,
                timestamp_ms INTEGER NOT NULL,
                provider TEXT NOT NULL,
                operation TEXT NOT NULL,
                output_type TEXT NOT NULL,
                credentials_ref TEXT,
                status TEXT NOT NULL,
                duration_ms INTEGER NOT NULL,
                cache_hit INTEGER NOT NULL,
                provider_request_id TEXT,
                output_dir TEXT NOT NULL,
                output_file_count INTEGER NOT NULL,
                output_files_json TEXT NOT NULL,
                output_json_summary_json TEXT,
                error_json TEXT,
                record_json TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_task_history_timestamp
                ON task_history(timestamp_ms DESC);
            CREATE INDEX IF NOT EXISTS idx_task_history_provider_status
                ON task_history(provider, status);
            CREATE INDEX IF NOT EXISTS idx_task_history_operation
                ON task_history(operation);
            "#,
        )
        .map_err(|err| {
            BrokerError::Provider(format!("failed to initialize history database: {err}"))
        })
}

fn json_scalar_string<T>(value: &T) -> BrokerResult<String>
where
    T: Serialize,
{
    serde_json::to_value(value)
        .map_err(|err| BrokerError::Provider(format!("failed to encode history scalar: {err}")))?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| BrokerError::Provider("history scalar was not a string".to_string()))
}

fn u128_to_i64(value: u128) -> i64 {
    value.min(i64::MAX as u128) as i64
}

fn usize_to_i64(value: usize) -> i64 {
    value.min(i64::MAX as usize) as i64
}

fn history_disabled() -> bool {
    env::var("HGRIPE_HISTORY_DISABLED")
        .ok()
        .map(|value| {
            let value = value.trim().to_ascii_lowercase();
            matches!(value.as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(false)
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn local_root() -> PathBuf {
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn summarize_json(value: &Value) -> Value {
    match value {
        Value::Null => json!({"type": "null"}),
        Value::Bool(_) => json!({"type": "bool"}),
        Value::Number(_) => json!({"type": "number"}),
        Value::String(text) => json!({
            "type": "string",
            "len": text.len(),
            "preview": preview(text),
        }),
        Value::Array(items) => json!({
            "type": "array",
            "len": items.len(),
        }),
        Value::Object(map) => {
            let keys: Vec<_> = map.keys().take(20).cloned().collect();
            let mut summary = serde_json::Map::new();
            summary.insert("type".to_string(), json!("object"));
            summary.insert("keys".to_string(), json!(keys));

            if let Some(text) = map.get("text").and_then(Value::as_str) {
                summary.insert(
                    "text".to_string(),
                    json!({
                        "len": text.len(),
                        "preview": preview(text),
                    }),
                );
            }
            if let Some(images) = map.get("images").and_then(Value::as_array) {
                summary.insert("images_count".to_string(), json!(images.len()));
            }
            if let Some(status_code) = map.get("status_code").and_then(Value::as_u64) {
                summary.insert("status_code".to_string(), json!(status_code));
            }

            Value::Object(summary)
        }
    }
}

fn preview(text: &str) -> String {
    const MAX_PREVIEW_CHARS: usize = 160;
    text.chars().take(MAX_PREVIEW_CHARS).collect()
}
