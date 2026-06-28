use hgripe_api::{build_doctor_report, DoctorOptions};
use serde_json::json;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn doctor_report_summarizes_runtime_without_secret_values() {
    let root = temp_diag_dir();
    let credentials_file = root.join("credentials.json");
    let profiles_file = root.join("provider_profiles.json");
    let history_file = root.join("tasks.jsonl");
    let history_db = root.join("tasks.sqlite3");
    let output_dir = root.join("outputs");
    let broker = root.join(if cfg!(windows) {
        "hgripe-api-broker.exe"
    } else {
        "hgripe-api-broker"
    });

    fs::create_dir_all(&output_dir).expect("output dir should be created");
    fs::write(&broker, "fake broker").expect("broker file should write");
    fs::write(&history_file, "").expect("history file should write");
    fs::write(&history_db, "").expect("history db file should write");
    fs::write(
        &credentials_file,
        serde_json::to_string_pretty(&json!({
            "openai-main": {
                "provider": "openai_compatible",
                "base_url": "https://api.openai.com/v1",
                "api_key": "sk-do-not-leak"
            }
        }))
        .unwrap(),
    )
    .expect("credentials file should write");
    fs::write(
        &profiles_file,
        serde_json::to_string_pretty(&json!({
            "profiles": {
                "openai-main": {
                    "provider": "openai_compatible",
                    "credentials_ref": "openai-main",
                    "base_url": "https://api.openai.com/v1",
                    "model": "gpt-4.1-mini"
                }
            }
        }))
        .unwrap(),
    )
    .expect("profiles file should write");

    let report = build_doctor_report(DoctorOptions {
        credentials_file: Some(credentials_file.to_string_lossy().to_string()),
        profiles_file: Some(profiles_file.to_string_lossy().to_string()),
        history_file: Some(history_file.to_string_lossy().to_string()),
        history_db: Some(history_db.to_string_lossy().to_string()),
        output_dir: Some(output_dir.to_string_lossy().to_string()),
        broker_path: Some(broker.to_string_lossy().to_string()),
    })
    .expect("doctor report should build");
    let encoded = serde_json::to_string(&report).expect("doctor report should encode");

    assert!(report.ok);
    assert_eq!(report.credentials.configured_count, 1);
    assert_eq!(report.provider_profiles.configured_count, 1);
    assert!(report.runtime.broker.exists);
    assert_eq!(report.runtime.output_dir.kind, "directory");
    assert!(!encoded.contains("sk-do-not-leak"));

    let _ = fs::remove_dir_all(root);
}

fn temp_diag_dir() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be valid")
        .as_nanos();
    std::env::temp_dir().join(format!("hgripe-doctor-test-{nonce}"))
}
