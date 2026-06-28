use hgripe_api::ApiTask;
use serde_json::json;
use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn cli_executes_mock_task_from_stdin() {
    let mut task = ApiTask::new("mock", "echo");
    task.inputs.insert("prompt".into(), json!("from cli"));

    let mut child = Command::new(env!("CARGO_BIN_EXE_hgripe-api-broker"))
        .env("HGRIPE_HISTORY_DISABLED", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("broker binary should spawn");

    child
        .stdin
        .as_mut()
        .expect("stdin should be open")
        .write_all(serde_json::to_string(&task).unwrap().as_bytes())
        .expect("task JSON should be written");

    let output = child.wait_with_output().expect("broker should finish");
    assert!(output.status.success());

    let result: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout should be JSON");
    assert_eq!(result["status"], "succeeded");
    assert_eq!(result["output_json"]["inputs"]["prompt"], "from cli");
}

#[test]
fn history_cli_lists_shows_and_builds_rerun_task() {
    let temp_dir = temp_cli_dir();
    let history_db = temp_dir.join("tasks.sqlite3");
    let history_file = temp_dir.join("tasks.jsonl");
    let output_dir = temp_dir.join("outputs");

    let mut task = ApiTask::new("mock", "echo");
    task.id = "cli-history-task".to_string();
    task.inputs
        .insert("prompt".into(), json!("from history cli"));

    let mut child = Command::new(env!("CARGO_BIN_EXE_hgripe-api-broker"))
        .env("HGRIPE_HISTORY_DB", &history_db)
        .env("HGRIPE_HISTORY_FILE", &history_file)
        .env("HGRIPE_OUTPUT_DIR", &output_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("broker binary should spawn");

    child
        .stdin
        .as_mut()
        .expect("stdin should be open")
        .write_all(serde_json::to_string(&task).unwrap().as_bytes())
        .expect("task JSON should be written");

    let output = child.wait_with_output().expect("broker should finish");
    assert!(output.status.success());

    let list_output = Command::new(env!("CARGO_BIN_EXE_hgripe-api-history"))
        .arg("list")
        .arg("--history-db")
        .arg(&history_db)
        .arg("--provider")
        .arg("mock")
        .output()
        .expect("history CLI should run");
    assert!(list_output.status.success());
    let list_json: serde_json::Value =
        serde_json::from_slice(&list_output.stdout).expect("list output should be JSON");
    assert_eq!(list_json["records"][0]["task_id"], "cli-history-task");
    assert_eq!(list_json["records"][0]["rerunnable"], true);

    let show_output = Command::new(env!("CARGO_BIN_EXE_hgripe-api-history"))
        .arg("show")
        .arg("cli-history-task")
        .arg("--history-db")
        .arg(&history_db)
        .output()
        .expect("history CLI show should run");
    assert!(show_output.status.success());
    let show_json: serde_json::Value =
        serde_json::from_slice(&show_output.stdout).expect("show output should be JSON");
    assert_eq!(show_json["detail"]["record"]["task_id"], "cli-history-task");
    assert_eq!(show_json["detail"]["rerunnable"], true);

    let rerun_output = Command::new(env!("CARGO_BIN_EXE_hgripe-api-history"))
        .arg("rerun-task")
        .arg("cli-history-task")
        .arg("--new-id")
        .arg("cli-history-rerun")
        .arg("--history-db")
        .arg(&history_db)
        .output()
        .expect("history CLI rerun-task should run");
    assert!(rerun_output.status.success());
    let rerun_json: serde_json::Value =
        serde_json::from_slice(&rerun_output.stdout).expect("rerun-task output should be JSON");
    assert_eq!(rerun_json["rerun_task"]["id"], "cli-history-rerun");
    assert_eq!(rerun_json["rerun_task"]["cache_policy"]["enabled"], false);
    assert_eq!(
        rerun_json["rerun_task"]["inputs"]["prompt"],
        "from history cli"
    );

    let executed_rerun_output = Command::new(env!("CARGO_BIN_EXE_hgripe-api-history"))
        .arg("rerun")
        .arg("cli-history-task")
        .arg("--new-id")
        .arg("cli-history-executed-rerun")
        .arg("--history-db")
        .arg(&history_db)
        .env("HGRIPE_HISTORY_DB", &history_db)
        .env("HGRIPE_HISTORY_FILE", &history_file)
        .env("HGRIPE_OUTPUT_DIR", &output_dir)
        .output()
        .expect("history CLI rerun should run");
    assert!(executed_rerun_output.status.success());
    let executed_rerun_json: serde_json::Value =
        serde_json::from_slice(&executed_rerun_output.stdout)
            .expect("executed rerun output should be JSON");
    assert_eq!(
        executed_rerun_json["rerun_task_id"],
        "cli-history-executed-rerun"
    );
    assert_eq!(executed_rerun_json["result"]["status"], "succeeded");

    let cleanup_preview_output = Command::new(env!("CARGO_BIN_EXE_hgripe-api-history"))
        .arg("cleanup")
        .arg("--history-db")
        .arg(&history_db)
        .arg("--history-file")
        .arg(&history_file)
        .arg("--provider")
        .arg("mock")
        .arg("--keep-latest")
        .arg("1")
        .output()
        .expect("history CLI cleanup preview should run");
    assert!(cleanup_preview_output.status.success());
    let cleanup_preview_json: serde_json::Value =
        serde_json::from_slice(&cleanup_preview_output.stdout)
            .expect("cleanup preview output should be JSON");
    assert_eq!(cleanup_preview_json["dry_run"], true);
    assert_eq!(cleanup_preview_json["plan"]["delete_count"], 1);

    let cleanup_apply_output = Command::new(env!("CARGO_BIN_EXE_hgripe-api-history"))
        .arg("cleanup")
        .arg("--history-db")
        .arg(&history_db)
        .arg("--history-file")
        .arg(&history_file)
        .arg("--provider")
        .arg("mock")
        .arg("--keep-latest")
        .arg("1")
        .arg("--apply")
        .output()
        .expect("history CLI cleanup apply should run");
    assert!(cleanup_apply_output.status.success());
    let cleanup_apply_json: serde_json::Value =
        serde_json::from_slice(&cleanup_apply_output.stdout)
            .expect("cleanup apply output should be JSON");
    assert_eq!(cleanup_apply_json["dry_run"], false);
    assert_eq!(cleanup_apply_json["result"]["sqlite_deleted"], 1);
    assert_eq!(cleanup_apply_json["result"]["jsonl_removed"], 1);

    let list_after_cleanup_output = Command::new(env!("CARGO_BIN_EXE_hgripe-api-history"))
        .arg("list")
        .arg("--history-db")
        .arg(&history_db)
        .arg("--provider")
        .arg("mock")
        .output()
        .expect("history CLI list after cleanup should run");
    assert!(list_after_cleanup_output.status.success());
    let list_after_cleanup_json: serde_json::Value =
        serde_json::from_slice(&list_after_cleanup_output.stdout)
            .expect("list after cleanup output should be JSON");
    assert_eq!(
        list_after_cleanup_json["records"]
            .as_array()
            .expect("records should be an array")
            .len(),
        1
    );

    let _ = fs::remove_dir_all(temp_dir);
}

fn temp_cli_dir() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be valid")
        .as_nanos();
    std::env::temp_dir().join(format!("hgripe-cli-test-{nonce}"))
}
