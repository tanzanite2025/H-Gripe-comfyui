use hgripe_api::{
    append_history_record, apply_history_cleanup, build_history_cleanup_plan, build_history_record,
    build_rerun_task_from_record, get_history_detail, get_history_record,
    history_detail_from_record, list_recent_history_records, query_history_records,
    upsert_sqlite_history_record, ApiResult, ApiStatus, ApiTask, HistoryCleanupOptions,
    HistoryQuery, HistoryRecord, HistoryRerunOptions, OutputFile, OutputType,
};
use rusqlite::Connection;
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
fn history_record_stores_sanitized_task_snapshot() {
    let mut task = ApiTask::new("openai_compatible", "chat.completions");
    task.credentials_ref = Some("openai-main".to_string());
    task.inputs
        .insert("prompt".to_string(), json!("keep this prompt"));
    task.inputs
        .insert("password".to_string(), json!("do-not-store"));
    task.params
        .insert("api_key".to_string(), json!("sk-do-not-store"));
    task.params.insert("max_tokens".to_string(), json!(100));
    task.params.insert(
        "headers".to_string(),
        json!({
            "Authorization": "Bearer do-not-store",
            "X-Request-ID": "keep-this-header"
        }),
    );

    let result = ApiResult::succeeded(task.id.clone(), Some(json!({"ok": true})));
    let record = build_history_record(&task, &result, std::path::Path::new("outputs"));
    let snapshot = record
        .task_snapshot
        .as_ref()
        .expect("task snapshot should be present");

    assert_eq!(snapshot.credentials_ref.as_deref(), Some("openai-main"));
    assert_eq!(snapshot.inputs["prompt"], json!("keep this prompt"));
    assert!(snapshot.inputs.get("password").is_none());
    assert!(snapshot.params.get("api_key").is_none());
    assert_eq!(snapshot.params["max_tokens"], json!(100));

    let headers = snapshot.params["headers"]
        .as_object()
        .expect("headers should stay an object");
    assert!(headers.get("Authorization").is_none());
    assert_eq!(headers["X-Request-ID"], json!("keep-this-header"));

    let encoded = serde_json::to_string(&record).expect("record should encode");
    assert!(!encoded.contains("sk-do-not-store"));
    assert!(!encoded.contains("Bearer do-not-store"));
    assert!(!encoded.contains("do-not-store"));
}

#[test]
fn old_history_json_without_task_snapshot_still_decodes() {
    let raw = json!({
        "schema_version": 1,
        "timestamp_ms": 1,
        "task_id": "old-task",
        "provider": "mock",
        "operation": "echo",
        "output_type": "text",
        "credentials_ref": null,
        "status": "succeeded",
        "duration_ms": 0,
        "cache_hit": false,
        "provider_request_id": null,
        "output_dir": "outputs",
        "output_file_count": 0,
        "output_files": [],
        "output_json_summary": null,
        "error": null
    });

    let record: HistoryRecord =
        serde_json::from_value(raw).expect("old history record should still decode");

    assert_eq!(record.task_id, "old-task");
    assert!(record.task_snapshot.is_none());
}

#[test]
fn history_detail_marks_rerunnable_records_and_outputs() {
    let task = ApiTask::new("mock", "echo");
    let mut result = ApiResult::succeeded(task.id.clone(), Some(json!({"ok": true})));
    result.output_files.push(OutputFile {
        path: "user/hgripe/outputs/mock.txt".to_string(),
        mime_type: Some("text/plain".to_string()),
        size_bytes: Some(4),
        sha256: None,
    });

    let record = build_history_record(&task, &result, std::path::Path::new("outputs"));
    let detail = history_detail_from_record(record);

    assert!(detail.rerunnable);
    assert_eq!(
        detail.output_paths,
        vec!["user/hgripe/outputs/mock.txt".to_string()]
    );
}

#[test]
fn rerun_task_from_history_snapshot_gets_new_id_and_disables_cache() {
    let mut task = ApiTask::new("mock", "echo");
    task.id = "source-task".to_string();
    task.cache_policy.enabled = true;
    task.inputs.insert("prompt".into(), json!("rerun me"));
    task.params
        .insert("api_key".to_string(), json!("do-not-keep"));

    let result = ApiResult::succeeded(task.id.clone(), Some(json!({"ok": true})));
    let record = build_history_record(&task, &result, std::path::Path::new("outputs"));
    let rerun_task = build_rerun_task_from_record(&record, HistoryRerunOptions::default()).unwrap();

    assert_ne!(rerun_task.id, "source-task");
    assert!(rerun_task.id.starts_with("source-task-rerun-"));
    assert!(!rerun_task.cache_policy.enabled);
    assert_eq!(rerun_task.inputs["prompt"], json!("rerun me"));
    assert!(rerun_task.params.get("api_key").is_none());
}

#[test]
fn rerun_task_can_keep_cache_and_use_explicit_id() {
    let task = ApiTask::new("mock", "echo");
    let result = ApiResult::succeeded(task.id.clone(), Some(json!({"ok": true})));
    let record = build_history_record(&task, &result, std::path::Path::new("outputs"));
    let rerun_task = build_rerun_task_from_record(
        &record,
        HistoryRerunOptions {
            new_task_id: Some("manual-rerun-id".to_string()),
            disable_cache: false,
        },
    )
    .unwrap();

    assert_eq!(rerun_task.id, "manual-rerun-id");
    assert!(rerun_task.cache_policy.enabled);
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

#[test]
fn sqlite_history_gets_record_by_task_id() {
    let path = temp_history_path().with_extension("sqlite3");

    let first = record_for("task-1", "mock", "echo", 100);
    let second = record_for("task-2", "openai_compatible", "image.generate", 200);

    upsert_sqlite_history_record(&path, &first).expect("first sqlite record should write");
    upsert_sqlite_history_record(&path, &second).expect("second sqlite record should write");

    let record = get_history_record(&path, "task-2")
        .expect("sqlite record should read")
        .expect("task-2 should exist");
    assert_eq!(record.task_id, "task-2");
    assert_eq!(record.operation, "image.generate");
    assert!(record.task_snapshot.is_some());

    let missing = get_history_record(&path, "missing").expect("missing query should work");
    assert!(missing.is_none());

    let detail = get_history_detail(&path, "task-2")
        .expect("sqlite detail should read")
        .expect("task-2 detail should exist");
    assert!(detail.rerunnable);
    assert_eq!(detail.record.task_id, "task-2");

    let _ = fs::remove_file(path);
}

#[test]
fn sqlite_history_migrates_task_snapshot_column() {
    let path = temp_history_path().with_extension("sqlite3");
    fs::create_dir_all(path.parent().expect("temp path should have parent"))
        .expect("temp directory should be created");

    let connection = Connection::open(&path).expect("sqlite database should open");
    connection
        .execute_batch(
            r#"
            CREATE TABLE task_history (
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
            "#,
        )
        .expect("old schema should be created");
    drop(connection);

    let record = record_for("task-migrated", "mock", "echo", 100);
    upsert_sqlite_history_record(&path, &record).expect("upsert should migrate old schema");

    let connection = Connection::open(&path).expect("sqlite database should reopen");
    let mut statement = connection
        .prepare("PRAGMA table_info(task_history)")
        .expect("schema should be inspectable");
    let columns: Vec<String> = statement
        .query_map([], |row| row.get::<_, String>(1))
        .expect("schema rows should query")
        .map(|row| row.expect("schema row should decode"))
        .collect();
    assert!(columns.iter().any(|column| column == "task_snapshot_json"));

    let task_snapshot_json: Option<String> = connection
        .query_row(
            "SELECT task_snapshot_json FROM task_history WHERE task_id = ?1",
            ["task-migrated"],
            |row| row.get(0),
        )
        .expect("snapshot column should be queryable");
    assert!(task_snapshot_json
        .expect("snapshot json should be stored")
        .contains("\"provider\":\"mock\""));

    let _ = fs::remove_file(path);
}

#[test]
fn sqlite_history_filters_records() {
    let path = temp_history_path().with_extension("sqlite3");

    let mock = record_for("task-mock", "mock", "echo", 100);
    let mut image = record_for("task-image", "openai_compatible", "image.generate", 200);
    image.output_file_count = 1;
    let mut failed = record_for("task-failed", "openai_compatible", "chat.completions", 300);
    failed.status = ApiStatus::Failed;

    upsert_sqlite_history_record(&path, &mock).expect("mock record should write");
    upsert_sqlite_history_record(&path, &image).expect("image record should write");
    upsert_sqlite_history_record(&path, &failed).expect("failed record should write");

    let provider_records = query_history_records(
        &path,
        HistoryQuery {
            limit: 10,
            provider: Some("openai_compatible".to_string()),
            ..HistoryQuery::default()
        },
    )
    .expect("provider query should work");
    assert_eq!(provider_records.len(), 2);
    assert!(provider_records
        .iter()
        .all(|record| record.provider == "openai_compatible"));

    let failed_records = query_history_records(
        &path,
        HistoryQuery {
            limit: 10,
            status: Some(ApiStatus::Failed),
            ..HistoryQuery::default()
        },
    )
    .expect("status query should work");
    assert_eq!(failed_records.len(), 1);
    assert_eq!(failed_records[0].task_id, "task-failed");

    let output_records = query_history_records(
        &path,
        HistoryQuery {
            limit: 10,
            has_output_files: Some(true),
            ..HistoryQuery::default()
        },
    )
    .expect("output query should work");
    assert_eq!(output_records.len(), 1);
    assert_eq!(output_records[0].task_id, "task-image");

    let _ = fs::remove_file(path);
}

#[test]
fn history_cleanup_plan_keeps_latest_matching_records() {
    let records = vec![
        record_for("old-mock", "mock", "echo", 100),
        record_for("new-mock", "mock", "echo", 300),
        record_for("middle-mock", "mock", "echo", 200),
        record_for(
            "other-provider",
            "openai_compatible",
            "chat.completions",
            50,
        ),
    ];

    let plan = build_history_cleanup_plan(
        &records,
        &HistoryCleanupOptions {
            keep_latest: Some(1),
            provider: Some("mock".to_string()),
            ..HistoryCleanupOptions::default()
        },
    );

    assert_eq!(plan.total_records, 4);
    assert_eq!(plan.matched_records, 3);
    assert_eq!(plan.protected_records, 1);
    assert_eq!(plan.delete_count, 2);
    assert_eq!(
        plan.delete_task_ids,
        vec!["middle-mock".to_string(), "old-mock".to_string()]
    );
}

#[test]
fn history_cleanup_applies_to_sqlite_jsonl_and_optional_outputs() {
    let history_file = temp_history_path();
    let history_db = history_file.with_extension("sqlite3");
    let output_dir = history_file
        .parent()
        .expect("temp path should have parent")
        .join("outputs");
    fs::create_dir_all(&output_dir).expect("output dir should be created");

    let keep = record_for("keep", "mock", "echo", 300);
    let mut delete = record_for("delete", "mock", "echo", 100);
    let output_path = output_dir.join("delete.txt");
    fs::write(&output_path, "delete me").expect("output file should write");
    delete.output_file_count = 1;
    delete.output_files.push(OutputFile {
        path: output_path.to_string_lossy().to_string(),
        mime_type: Some("text/plain".to_string()),
        size_bytes: Some(9),
        sha256: None,
    });

    upsert_sqlite_history_record(&history_db, &keep).expect("keep record should write sqlite");
    upsert_sqlite_history_record(&history_db, &delete).expect("delete record should write sqlite");
    append_history_record(&history_file, &keep).expect("keep record should write jsonl");
    append_history_record(&history_file, &delete).expect("delete record should write jsonl");

    let result = apply_history_cleanup(
        &history_db,
        &history_file,
        &HistoryCleanupOptions {
            keep_latest: Some(1),
            provider: Some("mock".to_string()),
            delete_output_files: true,
            ..HistoryCleanupOptions::default()
        },
    )
    .expect("cleanup should apply");

    assert_eq!(result.plan.delete_task_ids, vec!["delete".to_string()]);
    assert_eq!(result.sqlite_deleted, 1);
    assert_eq!(result.jsonl_removed, 1);
    assert_eq!(result.output_files_deleted, 1);
    assert!(!output_path.exists());

    let remaining =
        list_recent_history_records(&history_db, 10).expect("remaining sqlite should query");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].task_id, "keep");

    let jsonl = fs::read_to_string(&history_file).expect("history file should read");
    assert!(jsonl.contains("\"task_id\":\"keep\""));
    assert!(!jsonl.contains("\"task_id\":\"delete\""));

    let _ = fs::remove_dir_all(
        history_file
            .parent()
            .expect("temp history path should have parent"),
    );
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
