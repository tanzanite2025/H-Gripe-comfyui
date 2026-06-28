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

#[test]
fn config_cli_resolves_provider_profile_without_secret_values() {
    let profiles_file = temp_profiles_path();
    let credentials_file = temp_credentials_path();
    fs::write(
        &profiles_file,
        serde_json::to_string_pretty(&json!({
            "profiles": {
                "openai-main": {
                    "provider": "openai_compatible",
                    "credentials_ref": "openai-main",
                    "base_url": "https://profile.example/v1",
                    "model": "gpt-4.1-mini",
                    "headers": {
                        "X-Profile": "visible"
                    },
                    "params": {
                        "api_key": "do-not-leak"
                    }
                }
            }
        }))
        .unwrap(),
    )
    .expect("profiles file should write");
    fs::write(
        &credentials_file,
        serde_json::to_string_pretty(&json!({
            "openai-main": {
                "provider": "openai_compatible",
                "base_url": "https://credentials.example/v1",
                "api_key": "sk-do-not-leak",
                "headers": {
                    "Authorization": "Bearer do-not-leak",
                    "X-Team": "visible"
                }
            }
        }))
        .unwrap(),
    )
    .expect("credentials file should write");

    let resolve_output = Command::new(env!("CARGO_BIN_EXE_hgripe-api-config"))
        .arg("profiles")
        .arg("resolve")
        .arg("openai-main")
        .arg("--profiles-file")
        .arg(&profiles_file)
        .arg("--credentials-file")
        .arg(&credentials_file)
        .output()
        .expect("config CLI profiles resolve should run");
    assert!(resolve_output.status.success());
    let resolve_text = String::from_utf8_lossy(&resolve_output.stdout);
    let resolve_json: serde_json::Value =
        serde_json::from_slice(&resolve_output.stdout).expect("resolve output should be JSON");

    assert_eq!(resolve_json["resolved"]["ok"], true);
    assert_eq!(resolve_json["resolved"]["credentials_ref_status"], "found");
    assert_eq!(
        resolve_json["resolved"]["auth_source"],
        "credentials.api_key"
    );
    assert_eq!(
        resolve_json["resolved"]["params"]["api_key"],
        json!("<redacted>")
    );
    assert!(resolve_json["resolved"]["header_names"]
        .as_array()
        .unwrap()
        .iter()
        .any(|name| name.as_str() == Some("Authorization")));
    assert!(!resolve_text.contains("sk-do-not-leak"));
    assert!(!resolve_text.contains("Bearer do-not-leak"));

    let _ = fs::remove_file(profiles_file);
    let _ = fs::remove_file(credentials_file);
}

#[test]
fn config_cli_lists_shows_and_validates_credentials_redacted() {
    let credentials_file = temp_credentials_path();
    fs::write(
        &credentials_file,
        serde_json::to_string_pretty(&json!({
            "profiles": {
                "openai-main": {
                    "provider": "openai_compatible",
                    "base_url": "https://api.openai.com/v1",
                    "api_key": "sk-do-not-leak",
                    "headers": {
                        "Authorization": "Bearer do-not-leak",
                        "X-Org": "visible"
                    }
                }
            }
        }))
        .unwrap(),
    )
    .expect("credentials file should write");

    let list_output = Command::new(env!("CARGO_BIN_EXE_hgripe-api-config"))
        .arg("credentials")
        .arg("list")
        .arg("--credentials-file")
        .arg(&credentials_file)
        .output()
        .expect("config CLI credentials list should run");
    assert!(list_output.status.success());
    let list_text = String::from_utf8_lossy(&list_output.stdout);
    let list_json: serde_json::Value =
        serde_json::from_slice(&list_output.stdout).expect("list output should be JSON");
    assert_eq!(list_json["credentials"][0]["credential_ref"], "openai-main");
    assert_eq!(list_json["credentials"][0]["api_key_configured"], true);
    assert!(!list_text.contains("sk-do-not-leak"));

    let show_output = Command::new(env!("CARGO_BIN_EXE_hgripe-api-config"))
        .arg("credentials")
        .arg("show")
        .arg("openai-main")
        .arg("--credentials-file")
        .arg(&credentials_file)
        .output()
        .expect("config CLI credentials show should run");
    assert!(show_output.status.success());
    let show_text = String::from_utf8_lossy(&show_output.stdout);
    let show_json: serde_json::Value =
        serde_json::from_slice(&show_output.stdout).expect("show output should be JSON");
    assert_eq!(show_json["credential_ref"], "openai-main");
    assert_eq!(show_json["credential"]["api_key"], "<redacted>");
    assert_eq!(
        show_json["credential"]["headers"]["Authorization"],
        "<redacted>"
    );
    assert_eq!(show_json["credential"]["headers"]["X-Org"], "visible");
    assert!(!show_text.contains("sk-do-not-leak"));
    assert!(!show_text.contains("Bearer do-not-leak"));

    let validate_output = Command::new(env!("CARGO_BIN_EXE_hgripe-api-config"))
        .arg("credentials")
        .arg("validate")
        .arg("--credentials-file")
        .arg(&credentials_file)
        .output()
        .expect("config CLI credentials validate should run");
    assert!(validate_output.status.success());
    let validate_json: serde_json::Value =
        serde_json::from_slice(&validate_output.stdout).expect("validate output should be JSON");
    assert_eq!(validate_json["validation"]["ok"], true);
    assert_eq!(validate_json["validation"]["credential_count"], 1);

    let _ = fs::remove_file(credentials_file);
}

#[test]
fn config_cli_doctor_reports_paths_and_validation() {
    let root = temp_doctor_dir();
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
            "openai-main": {
                "provider": "openai_compatible",
                "credentials_ref": "openai-main",
                "base_url": "https://api.openai.com/v1",
                "model": "gpt-4.1-mini"
            }
        }))
        .unwrap(),
    )
    .expect("profiles file should write");

    let doctor_output = Command::new(env!("CARGO_BIN_EXE_hgripe-api-config"))
        .arg("doctor")
        .arg("--credentials-file")
        .arg(&credentials_file)
        .arg("--profiles-file")
        .arg(&profiles_file)
        .arg("--history-file")
        .arg(&history_file)
        .arg("--history-db")
        .arg(&history_db)
        .arg("--output-dir")
        .arg(&output_dir)
        .arg("--broker")
        .arg(&broker)
        .output()
        .expect("config CLI doctor should run");
    assert!(doctor_output.status.success());
    let doctor_text = String::from_utf8_lossy(&doctor_output.stdout);
    let doctor_json: serde_json::Value =
        serde_json::from_slice(&doctor_output.stdout).expect("doctor output should be JSON");

    assert_eq!(doctor_json["ok"], true);
    assert_eq!(doctor_json["credentials"]["configured_count"], 1);
    assert_eq!(doctor_json["provider_profiles"]["configured_count"], 1);
    assert_eq!(doctor_json["runtime"]["broker"]["exists"], true);
    assert!(!doctor_text.contains("sk-do-not-leak"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn config_cli_doctor_reports_missing_profile_credentials_ref() {
    let root = temp_doctor_dir();
    let credentials_file = root.join("credentials.json");
    let profiles_file = root.join("provider_profiles.json");
    let output_dir = root.join("outputs");
    let broker = root.join(if cfg!(windows) {
        "hgripe-api-broker.exe"
    } else {
        "hgripe-api-broker"
    });

    fs::create_dir_all(&output_dir).expect("output dir should be created");
    fs::write(&broker, "fake broker").expect("broker file should write");
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
            "broken-profile": {
                "provider": "openai_compatible",
                "credentials_ref": "missing-ref",
                "base_url": "https://api.openai.com/v1",
                "model": "gpt-4.1-mini"
            }
        }))
        .unwrap(),
    )
    .expect("profiles file should write");

    let doctor_output = Command::new(env!("CARGO_BIN_EXE_hgripe-api-config"))
        .arg("doctor")
        .arg("--credentials-file")
        .arg(&credentials_file)
        .arg("--profiles-file")
        .arg(&profiles_file)
        .arg("--output-dir")
        .arg(&output_dir)
        .arg("--broker")
        .arg(&broker)
        .output()
        .expect("config CLI doctor should run");
    assert!(doctor_output.status.success());
    let doctor_text = String::from_utf8_lossy(&doctor_output.stdout);
    let doctor_json: serde_json::Value =
        serde_json::from_slice(&doctor_output.stdout).expect("doctor output should be JSON");

    assert_eq!(doctor_json["ok"], false);
    assert_eq!(doctor_json["provider_profiles"]["ok"], false);
    assert_eq!(doctor_json["provider_profiles"]["error_count"], 1);
    assert!(doctor_json["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| {
            issue["scope"] == "provider_profile:broken-profile"
                && issue["code"] == "missing_credentials_ref"
        }));
    assert!(!doctor_text.contains("sk-do-not-leak"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn config_cli_init_dry_run_and_apply() {
    let root = temp_init_dir();

    let dry_run_output = Command::new(env!("CARGO_BIN_EXE_hgripe-api-config"))
        .arg("init")
        .arg("--root")
        .arg(&root)
        .arg("--dry-run")
        .output()
        .expect("config CLI init dry-run should run");
    assert!(dry_run_output.status.success());
    let dry_run_json: serde_json::Value =
        serde_json::from_slice(&dry_run_output.stdout).expect("dry-run output should be JSON");
    assert_eq!(dry_run_json["dry_run"], true);
    assert!(dry_run_json["would_create_count"].as_u64().unwrap_or(0) >= 5);
    assert!(!root.exists());

    let init_output = Command::new(env!("CARGO_BIN_EXE_hgripe-api-config"))
        .arg("init")
        .arg("--root")
        .arg(&root)
        .output()
        .expect("config CLI init should run");
    assert!(init_output.status.success());
    let init_json: serde_json::Value =
        serde_json::from_slice(&init_output.stdout).expect("init output should be JSON");
    assert_eq!(init_json["dry_run"], false);
    assert!(init_json["created_count"].as_u64().unwrap_or(0) >= 5);

    assert!(root
        .join("user")
        .join("hgripe")
        .join("credentials.json")
        .exists());
    assert!(root
        .join("user")
        .join("hgripe")
        .join("provider_profiles.json")
        .exists());

    let _ = fs::remove_dir_all(root);
}

fn temp_profiles_path() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be valid")
        .as_nanos();
    std::env::temp_dir().join(format!("hgripe-config-cli-profiles-test-{nonce}.json"))
}

fn temp_credentials_path() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be valid")
        .as_nanos();
    std::env::temp_dir().join(format!("hgripe-config-cli-credentials-test-{nonce}.json"))
}

fn temp_doctor_dir() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be valid")
        .as_nanos();
    std::env::temp_dir().join(format!("hgripe-config-cli-doctor-test-{nonce}"))
}

fn temp_init_dir() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be valid")
        .as_nanos();
    std::env::temp_dir().join(format!("hgripe-config-cli-init-test-{nonce}"))
}
