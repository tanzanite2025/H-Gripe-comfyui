use hgripe_api::{
    get_redacted_credential_ref, list_credential_summaries, load_credentials, validate_credentials,
};
use serde_json::json;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn credentials_load_profiles_wrapper_document() {
    let path = temp_credentials_path();
    write_credentials_file(
        &path,
        json!({
            "profiles": {
                "openai-main": {
                    "provider": "openai_compatible",
                    "base_url": "https://api.openai.com/v1",
                    "api_key_env": "OPENAI_API_KEY"
                }
            }
        }),
    );

    let credentials =
        load_credentials(Some(path.to_str().unwrap())).expect("credentials should load");

    assert!(credentials.contains_key("openai-main"));
    assert_eq!(
        credentials["openai-main"].base_url.as_deref(),
        Some("https://api.openai.com/v1")
    );

    let _ = fs::remove_file(path);
}

#[test]
fn credentials_list_summaries_without_secret_values() {
    let path = temp_credentials_path();
    write_credentials_file(
        &path,
        json!({
            "openai-main": {
                "provider": "openai_compatible",
                "base_url": "https://api.openai.com/v1",
                "api_key": "sk-do-not-leak",
                "headers": {
                    "X-Org": "visible"
                }
            }
        }),
    );

    let summaries = list_credential_summaries(Some(path.to_str().unwrap()))
        .expect("credential summaries should load");
    let encoded = serde_json::to_string(&summaries).expect("summaries should encode");

    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].credential_ref, "openai-main");
    assert!(summaries[0].api_key_configured);
    assert_eq!(summaries[0].headers_count, 1);
    assert!(!encoded.contains("sk-do-not-leak"));

    let _ = fs::remove_file(path);
}

#[test]
fn credentials_show_redacts_secret_values() {
    let path = temp_credentials_path();
    write_credentials_file(
        &path,
        json!({
            "openai-main": {
                "provider": "openai_compatible",
                "base_url": "https://api.openai.com/v1",
                "api_key": "sk-do-not-leak",
                "headers": {
                    "Authorization": "Bearer do-not-leak",
                    "X-Org": "visible"
                }
            }
        }),
    );

    let credential = get_redacted_credential_ref("openai-main", Some(path.to_str().unwrap()))
        .expect("credential lookup should run")
        .expect("credential should exist");
    let encoded = serde_json::to_string(&credential).expect("credential should encode");

    assert_eq!(credential.api_key.as_deref(), Some("<redacted>"));
    assert_eq!(
        credential
            .headers
            .as_ref()
            .expect("headers should be present")
            .get("Authorization")
            .map(String::as_str),
        Some("<redacted>")
    );
    assert_eq!(
        credential
            .headers
            .as_ref()
            .expect("headers should be present")
            .get("X-Org")
            .map(String::as_str),
        Some("visible")
    );
    assert!(!encoded.contains("sk-do-not-leak"));
    assert!(!encoded.contains("Bearer do-not-leak"));

    let _ = fs::remove_file(path);
}

#[test]
fn credentials_validate_reports_errors_and_warnings() {
    let path = temp_credentials_path();
    write_credentials_file(
        &path,
        json!({
            "bad-credential": {
                "provider": "unknown_provider",
                "base_url": "localhost:1234",
                "api_key_env": "HGRIPE_TEST_ENV_THAT_SHOULD_NOT_EXIST",
                "headers": {
                    "Bad Header": "value"
                }
            }
        }),
    );

    let validation = validate_credentials(Some(path.to_str().unwrap()))
        .expect("credential validation should run");

    assert_eq!(validation.credential_count, 1);
    assert!(!validation.ok);
    assert!(validation
        .issues
        .iter()
        .any(|issue| issue.code == "unsupported_provider"));
    assert!(validation
        .issues
        .iter()
        .any(|issue| issue.code == "invalid_base_url"));
    assert!(validation
        .issues
        .iter()
        .any(|issue| issue.code == "api_key_env_not_set"));
    assert!(validation
        .issues
        .iter()
        .any(|issue| issue.code == "invalid_header_name"));

    let _ = fs::remove_file(path);
}

fn write_credentials_file(path: &std::path::Path, document: serde_json::Value) {
    fs::write(path, serde_json::to_string_pretty(&document).unwrap())
        .expect("credentials file should write");
}

fn temp_credentials_path() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be valid")
        .as_nanos();
    std::env::temp_dir().join(format!("hgripe-credentials-test-{nonce}.json"))
}
