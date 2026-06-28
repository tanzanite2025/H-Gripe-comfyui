use crate::model::{ApiErrorInfo, ApiResult, ApiStatus, ApiTask, OutputFile, OutputType};
use crate::outputs::output_dir_from_env;
use crate::provider::{BrokerError, BrokerResult};
use rusqlite::{params, Connection, ToSql};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const HISTORY_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryRecord {
    pub schema_version: u32,
    pub timestamp_ms: u128,
    pub task_id: String,
    pub provider: String,
    pub operation: String,
    pub output_type: OutputType,
    pub credentials_ref: Option<String>,
    #[serde(default)]
    pub task_snapshot: Option<ApiTask>,
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryQuery {
    pub limit: usize,
    pub provider: Option<String>,
    pub operation: Option<String>,
    pub status: Option<ApiStatus>,
    pub has_output_files: Option<bool>,
}

impl HistoryQuery {
    pub fn recent(limit: usize) -> Self {
        Self {
            limit,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryDetail {
    pub record: HistoryRecord,
    pub rerunnable: bool,
    pub output_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryRerunOptions {
    pub new_task_id: Option<String>,
    pub disable_cache: bool,
}

impl Default for HistoryRerunOptions {
    fn default() -> Self {
        Self {
            new_task_id: None,
            disable_cache: true,
        }
    }
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
        task_snapshot: Some(sanitized_task_snapshot(task)),
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
        task_snapshot: Some(sanitized_task_snapshot(task)),
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
    let task_snapshot_json = record
        .task_snapshot
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|err| {
            BrokerError::Provider(format!("failed to encode history task snapshot: {err}"))
        })?;
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
                task_snapshot_json,
                record_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
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
                task_snapshot_json,
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
    query_history_records(path, HistoryQuery::recent(limit))
}

pub fn get_history_record(path: &Path, task_id: &str) -> BrokerResult<Option<HistoryRecord>> {
    let task_id = task_id.trim();
    if task_id.is_empty() || !path.exists() {
        return Ok(None);
    }

    let connection = Connection::open(path).map_err(|err| {
        BrokerError::Provider(format!(
            "failed to open history database {}: {err}",
            path.display()
        ))
    })?;
    init_sqlite_history(&connection)?;

    let mut statement = connection
        .prepare("SELECT record_json FROM task_history WHERE task_id = ?1")
        .map_err(|err| {
            BrokerError::Provider(format!(
                "failed to query history database {}: {err}",
                path.display()
            ))
        })?;
    let mut rows = statement.query(params![task_id]).map_err(|err| {
        BrokerError::Provider(format!(
            "failed to read history database {}: {err}",
            path.display()
        ))
    })?;

    let Some(row) = rows.next().map_err(|err| {
        BrokerError::Provider(format!(
            "failed to read history row from {}: {err}",
            path.display()
        ))
    })?
    else {
        return Ok(None);
    };

    let record_json = row.get::<_, String>(0).map_err(|err| {
        BrokerError::Provider(format!(
            "failed to read history row from {}: {err}",
            path.display()
        ))
    })?;
    Ok(Some(decode_history_record_json(path, &record_json)?))
}

pub fn get_history_detail(path: &Path, task_id: &str) -> BrokerResult<Option<HistoryDetail>> {
    get_history_record(path, task_id).map(|record| record.map(history_detail_from_record))
}

pub fn history_detail_from_record(record: HistoryRecord) -> HistoryDetail {
    let output_paths = record
        .output_files
        .iter()
        .map(|file| file.path.clone())
        .collect();
    let rerunnable = record.task_snapshot.is_some();

    HistoryDetail {
        record,
        rerunnable,
        output_paths,
    }
}

pub fn build_rerun_task_from_record(
    record: &HistoryRecord,
    options: HistoryRerunOptions,
) -> BrokerResult<ApiTask> {
    let mut task = record.task_snapshot.clone().ok_or_else(|| {
        BrokerError::Provider(format!(
            "history record '{}' has no task_snapshot; run the task again with the current broker before rerunning it from history",
            record.task_id
        ))
    })?;

    task.id = options
        .new_task_id
        .filter(|id| !id.trim().is_empty())
        .unwrap_or_else(|| generated_rerun_task_id(&task.id));

    if options.disable_cache {
        task.cache_policy.enabled = false;
    }

    Ok(task)
}

pub fn query_history_records(path: &Path, query: HistoryQuery) -> BrokerResult<Vec<HistoryRecord>> {
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

    let (sql, sql_params) = build_history_query_sql(&query)?;
    let sql_param_refs: Vec<&dyn ToSql> = sql_params
        .iter()
        .map(|param| param.as_ref() as &dyn ToSql)
        .collect();
    let mut statement = connection.prepare(&sql).map_err(|err| {
        BrokerError::Provider(format!(
            "failed to query history database {}: {err}",
            path.display()
        ))
    })?;
    let rows = statement
        .query_map(sql_param_refs.as_slice(), |row| row.get::<_, String>(0))
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
        let record = decode_history_record_json(path, &record_json)?;
        records.push(record);
    }

    Ok(records)
}

fn build_history_query_sql(query: &HistoryQuery) -> BrokerResult<(String, Vec<Box<dyn ToSql>>)> {
    let mut sql = String::from("SELECT record_json FROM task_history");
    let mut clauses: Vec<&'static str> = Vec::new();
    let mut params: Vec<Box<dyn ToSql>> = Vec::new();

    if let Some(provider) = query
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        clauses.push("provider = ?");
        params.push(Box::new(provider.to_string()));
    }

    if let Some(operation) = query
        .operation
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        clauses.push("operation = ?");
        params.push(Box::new(operation.to_string()));
    }

    if let Some(status) = &query.status {
        clauses.push("status = ?");
        params.push(Box::new(json_scalar_string(status)?));
    }

    if let Some(has_output_files) = query.has_output_files {
        if has_output_files {
            clauses.push("output_file_count > 0");
        } else {
            clauses.push("output_file_count = 0");
        }
    }

    if !clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&clauses.join(" AND "));
    }

    sql.push_str(" ORDER BY timestamp_ms DESC, rowid DESC LIMIT ?");
    params.push(Box::new(usize_to_i64(normalized_limit(query.limit))));

    Ok((sql, params))
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
                task_snapshot_json TEXT,
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
        })?;

    ensure_sqlite_history_column(connection, "task_snapshot_json", "TEXT")?;
    Ok(())
}

fn ensure_sqlite_history_column(
    connection: &Connection,
    column_name: &str,
    column_type: &str,
) -> BrokerResult<()> {
    let mut statement = connection
        .prepare("PRAGMA table_info(task_history)")
        .map_err(|err| {
            BrokerError::Provider(format!("failed to inspect history database schema: {err}"))
        })?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|err| {
            BrokerError::Provider(format!("failed to read history database schema: {err}"))
        })?;

    for row in rows {
        let existing_column = row.map_err(|err| {
            BrokerError::Provider(format!("failed to read history database schema row: {err}"))
        })?;
        if existing_column == column_name {
            return Ok(());
        }
    }

    connection
        .execute(
            &format!("ALTER TABLE task_history ADD COLUMN {column_name} {column_type}"),
            [],
        )
        .map_err(|err| {
            BrokerError::Provider(format!(
                "failed to migrate history database column {column_name}: {err}"
            ))
        })?;

    Ok(())
}

fn decode_history_record_json(path: &Path, record_json: &str) -> BrokerResult<HistoryRecord> {
    serde_json::from_str::<HistoryRecord>(record_json).map_err(|err| {
        BrokerError::Provider(format!(
            "failed to decode history row from {}: {err}",
            path.display()
        ))
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

fn normalized_limit(limit: usize) -> usize {
    if limit == 0 {
        20
    } else {
        limit.min(500)
    }
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

fn generated_rerun_task_id(source_task_id: &str) -> String {
    let nonce = Uuid::new_v4().simple().to_string();
    format!("{source_task_id}-rerun-{}", &nonce[..8])
}

fn sanitized_task_snapshot(task: &ApiTask) -> ApiTask {
    let mut snapshot = task.clone();
    snapshot.inputs = sanitize_value_map(&snapshot.inputs);
    snapshot.params = sanitize_value_map(&snapshot.params);
    snapshot
}

fn sanitize_value_map(map: &BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    map.iter()
        .filter_map(|(key, value)| {
            if is_sensitive_key(key) {
                None
            } else {
                Some((key.clone(), sanitize_value(value)))
            }
        })
        .collect()
}

fn sanitize_json_object(map: &serde_json::Map<String, Value>) -> serde_json::Map<String, Value> {
    map.iter()
        .filter_map(|(key, value)| {
            if is_sensitive_key(key) {
                None
            } else {
                Some((key.clone(), sanitize_value(value)))
            }
        })
        .collect()
}

fn sanitize_value(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(sanitize_value).collect()),
        Value::Object(map) => Value::Object(sanitize_json_object(map)),
        _ => value.clone(),
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized: String = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect();

    matches!(
        normalized.as_str(),
        "authorization"
            | "proxyauthorization"
            | "apikey"
            | "xapikey"
            | "key"
            | "token"
            | "accesstoken"
            | "refreshtoken"
            | "idtoken"
            | "cookie"
            | "setcookie"
            | "session"
            | "sessionid"
    ) || normalized.ends_with("apikey")
        || normalized.ends_with("token")
        || normalized.contains("password")
        || normalized.contains("secret")
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
