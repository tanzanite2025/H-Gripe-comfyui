use serde_json::json;
use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn config_cli_lists_shows_and_validates_provider_profiles() {
    let profiles_file = temp_profiles_path();
    fs::write(
        &profiles_file,
        serde_json::to_string_pretty(&json!({
            "profiles": {
                "local-profile": {
                    "provider": "openai_compatible",
                    "base_url": "http://127.0.0.1:1234/v1",
                    "model": "local-model",
                    "no_auth": true,
                    "params": {
                        "temperature": 0.2
                    }
                }
            }
        }))
        .unwrap(),
    )
    .expect("profiles file should write");

    let list_output = Command::new(env!("CARGO_BIN_EXE_hgripe-api-config"))
        .arg("profiles")
        .arg("list")
        .arg("--profiles-file")
        .arg(&profiles_file)
        .output()
        .expect("config CLI profiles list should run");
    assert!(list_output.status.success());
    let list_json: serde_json::Value =
        serde_json::from_slice(&list_output.stdout).expect("list output should be JSON");
    assert_eq!(list_json["profiles"][0]["profile_ref"], "local-profile");
    assert_eq!(list_json["profiles"][0]["model"], "local-model");

    let show_output = Command::new(env!("CARGO_BIN_EXE_hgripe-api-config"))
        .arg("profiles")
        .arg("show")
        .arg("local-profile")
        .arg("--profiles-file")
        .arg(&profiles_file)
        .output()
        .expect("config CLI profiles show should run");
    assert!(show_output.status.success());
    let show_json: serde_json::Value =
        serde_json::from_slice(&show_output.stdout).expect("show output should be JSON");
    assert_eq!(show_json["profile_ref"], "local-profile");
    assert_eq!(show_json["profile"]["no_auth"], true);

    let validate_output = Command::new(env!("CARGO_BIN_EXE_hgripe-api-config"))
        .arg("profiles")
        .arg("validate")
        .arg("--profiles-file")
        .arg(&profiles_file)
        .output()
        .expect("config CLI profiles validate should run");
    assert!(validate_output.status.success());
    let validate_json: serde_json::Value =
        serde_json::from_slice(&validate_output.stdout).expect("validate output should be JSON");
    assert_eq!(validate_json["validation"]["ok"], true);
    assert_eq!(validate_json["validation"]["profile_count"], 1);

    let _ = fs::remove_file(profiles_file);
}

fn temp_profiles_path() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be valid")
        .as_nanos();
    std::env::temp_dir().join(format!("hgripe-config-cli-profiles-test-{nonce}.json"))
}
