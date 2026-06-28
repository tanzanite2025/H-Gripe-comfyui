use hgripe_api::{
    get_provider_profile, list_provider_profile_summaries, load_provider_profiles,
    validate_provider_profiles,
};
use serde_json::json;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn provider_profiles_load_profiles_wrapper_document() {
    let path = temp_profiles_path();
    write_profiles_file(
        &path,
        json!({
            "profiles": {
                "local-profile": {
                    "provider": "openai_compatible",
                    "base_url": "http://127.0.0.1:1234/v1",
                    "model": "local-model",
                    "no_auth": true
                }
            }
        }),
    );

    let profiles = load_provider_profiles(Some(path.to_str().unwrap()))
        .expect("provider profiles should load");

    assert!(profiles.contains_key("local-profile"));
    assert_eq!(
        profiles["local-profile"].base_url.as_deref(),
        Some("http://127.0.0.1:1234/v1")
    );

    let _ = fs::remove_file(path);
}

#[test]
fn provider_profiles_list_summaries() {
    let path = temp_profiles_path();
    write_profiles_file(
        &path,
        json!({
            "openai-main": {
                "provider": "openai_compatible",
                "credentials_ref": "openai-main",
                "base_url": "https://api.openai.com/v1",
                "model": "gpt-4.1-mini",
                "params": {
                    "temperature": 0.7
                }
            }
        }),
    );

    let summaries = list_provider_profile_summaries(Some(path.to_str().unwrap()))
        .expect("profile summaries should load");

    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].profile_ref, "openai-main");
    assert_eq!(summaries[0].params_count, 1);
    assert_eq!(summaries[0].model.as_deref(), Some("gpt-4.1-mini"));

    let _ = fs::remove_file(path);
}

#[test]
fn provider_profiles_validate_reports_errors_and_warnings() {
    let path = temp_profiles_path();
    write_profiles_file(
        &path,
        json!({
            "bad-profile": {
                "provider": "unknown_provider",
                "base_url": "localhost:1234",
                "headers": {
                    "Authorization": "Bearer do-not-store"
                },
                "params": {
                    "api_key": "do-not-store"
                }
            }
        }),
    );

    let validation = validate_provider_profiles(Some(path.to_str().unwrap()))
        .expect("profile validation should run");

    assert_eq!(validation.profile_count, 1);
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
        .any(|issue| issue.code == "secret_like_header"));
    assert!(validation
        .issues
        .iter()
        .any(|issue| issue.code == "secret_like_param"));

    let _ = fs::remove_file(path);
}

#[test]
fn provider_profiles_get_profile_by_ref() {
    let path = temp_profiles_path();
    write_profiles_file(
        &path,
        json!({
            "local": {
                "provider": "openai_compatible",
                "base_url": "http://127.0.0.1:1234/v1",
                "model": "local-model",
                "no_auth": true
            }
        }),
    );

    let profile = get_provider_profile("local", Some(path.to_str().unwrap()))
        .expect("profile lookup should run")
        .expect("profile should exist");
    let missing = get_provider_profile("missing", Some(path.to_str().unwrap()))
        .expect("missing profile lookup should run");

    assert_eq!(profile.model.as_deref(), Some("local-model"));
    assert!(missing.is_none());

    let _ = fs::remove_file(path);
}

fn write_profiles_file(path: &std::path::Path, document: serde_json::Value) {
    fs::write(path, serde_json::to_string_pretty(&document).unwrap())
        .expect("profiles file should write");
}

fn temp_profiles_path() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be valid")
        .as_nanos();
    std::env::temp_dir().join(format!("hgripe-provider-profiles-test-{nonce}.json"))
}
