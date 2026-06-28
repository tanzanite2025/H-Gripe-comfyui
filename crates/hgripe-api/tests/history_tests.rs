use hgripe_api::{
    append_history_record, build_history_record, list_recent_history_records,
    upsert_sqlite_history_record, ApiResult, ApiStatus, ApiTask, OutputType,
};
use serde_json::json;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn history_record_summarizes_output_json() {
    let mut task = ApiTask::new("openai_compatible", "chat.completions");
    task.output_type = OutputType::Text;
    task.credentials_ref = Some("openai-main".to_string());

    let mut result = ApiResult::succeeded(
        task.id.clone(),
        Some(json!({
            "text": "a useful answer",
            "raw": {
                "very_large_field": "x".repeat(4096)
            }
        })),
    );
    result.status = ApiStatus::Succeeded;
    result.duration_ms = 123;
    result.provider_request_id = Some("request-123".to_string());

    let record = build_history_record(&task, &result, std::path::Path::new("outputs"));

    assert_eq!(record.provider, "openai_compatible");
    assert_eq!(record.operation, "chat.completions");
    assert_eq!(record.credentials_ref.as_deref(), Some("openai-main"));
    assert_eq!(record.duration_ms, 123);
    assert_eq!(
        record.output_json_summary.as_ref().unwrap()["type"],
        "object"
    );
    assert_eq!(
        record.output_json_summary.as_ref().unwrap()["text"]["preview"],
        "a useful answer"
    );
    assert!(!serde_json::to_string(&record)
        .unwrap()
        .contains("very_large_field"));
}

#[test]
fn append_history_record_writes_jsonl() {
    let path = temp_history_path();
    let task = ApiTask::new("mock", "echo");
    let result = ApiResult::succeeded(task.id.clone(), Some(json!({"ok": true})));
    let record = build_history_record(&task, &result, std::path::Path::new("outputs"));

    append_history_record(&path, &record).expect("history record should write");

    let raw = fs::read_to_string(&path).expect("history file should be readable");
    let parsed: serde_json::Value =
        serde_json::from_str(raw.lines().next().unwrap()).expect("history line should be JSON");
    assert_eq!(parsed["provider"], "mock");
    assert_eq!(parsed["operation"], "echo");

    let _ = fs::remove_file(path);
}

#[test]
fn sqlite_history_upserts_and_lists_recent_records() {
    let path = temp_history_path().with_extension("sqlite3");

    let first = record_for("task-1", "mock", "echo", 100);
    let second = record_for("task-2", "openai_compatible", "image.generate", 200);

    upsert_sqlite_history_record(&path, &first).expect("first sqlite record should write");
    upsert_sqlite_history_record(&path, &second).expect("second sqlite record should write");

    let records = list_recent_history_records(&path, 2).expect("sqlite records should list");

    assert_eq!(records.len(), 2);
    assert_eq!(records[0].task_id, "task-2");
    assert_eq!(records[0].provider, "openai_compatible");
    assert_eq!(records[1].task_id, "task-1");

    let _ = fs::remove_file(path);
}

fn record_for(
    task_id: &str,
    provider: &str,
    operation: &str,
    timestamp_ms: u128,
) -> hgripe_api::HistoryRecord {
    let mut task = ApiTask::new(provider, operation);
    task.id = task_id.to_string();
    let result = ApiResult::succeeded(task.id.clone(), Some(json!({"ok": true})));
    let mut record = build_history_record(&task, &result, std::path::Path::new("outputs"));
    record.timestamp_ms = timestamp_ms;
    record
}

fn temp_history_path() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be valid")
        .as_nanos();
    std::env::temp_dir()
        .join(format!("hgripe-history-test-{nonce}"))
        .join("tasks.jsonl")
}
