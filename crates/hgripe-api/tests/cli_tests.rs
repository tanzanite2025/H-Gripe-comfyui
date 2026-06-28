use hgripe_api::ApiTask;
use serde_json::json;
use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn cli_executes_mock_task_from_stdin() {
    let mut task = ApiTask::new("mock", "echo");
    task.inputs.insert("prompt".into(), json!("from cli"));

    let mut child = Command::new(env!("CARGO_BIN_EXE_hgripe-api-broker"))
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
