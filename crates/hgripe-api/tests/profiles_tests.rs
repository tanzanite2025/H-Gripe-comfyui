use hgripe_api::{
    get_provider_profile, list_provider_profile_summaries, load_provider_profiles,
    resolve_provider_profile, validate_provider_profiles,
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
fn provider_profiles_validate_accepts_custom_http_without_model() {
    let path = temp_profiles_path();
    write_profiles_file(
        &path,
        json!({
            "custom-http": {
                "provider": "custom_http",
                "credentials_ref": "custom-http-main",
                "params": {
                    "method": "POST",
                    "url": "/jobs"
                }
            }
        }),
    );

    let validation = validate_provider_profiles(Some(path.to_str().unwrap()))
        .expect("profile validation should run");

    assert_eq!(validation.profile_count, 1);
    assert!(validation.ok);
    assert!(!validation
        .issues
        .iter()
        .any(|issue| issue.code == "unsupported_provider"));
    assert!(!validation
        .issues
        .iter()
        .any(|issue| issue.code == "model_not_configured"));

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

#[test]
fn provider_profiles_resolve_redacts_secret_like_values() {
    let profiles_path = temp_profiles_path();
    let credentials_path = temp_credentials_path();
    write_profiles_file(
        &profiles_path,
        json!({
            "openai-main": {
                "provider": "openai_compatible",
                "credentials_ref": "openai-main",
                "base_url": "https://profile.example/v1",
                "model": "gpt-4.1-mini",
                "headers": {
                    "X-Profile": "visible"
                },
                "params": {
                    "temperature": 0.7,
                    "api_key": "do-not-leak"
                },
                "extra_body": {
                    "metadata": {
                        "safe": "visible",
                        "token": "do-not-leak"
                    }
                }
            }
        }),
    );
    write_credentials_file(
        &credentials_path,
        json!({
            "openai-main": {
                "provider": "openai_compatible",
                "base_url": "https://credentials.example/v1",
                "api_key": "sk-do-not-leak",
                "headers": {
                    "Authorization": "Bearer do-not-leak",
                    "X-Team": "visible"
                }
            }
        }),
    );

    let resolved = resolve_provider_profile(
        "openai-main",
        Some(profiles_path.to_str().unwrap()),
        Some(credentials_path.to_str().unwrap()),
    )
    .expect("profile should resolve");

    assert!(resolved.ok);
    assert_eq!(resolved.credentials_ref_status, "found");
    assert_eq!(
        resolved.base_url.as_deref(),
        Some("https://profile.example/v1")
    );
    assert_eq!(
        resolved.base_url_source.as_deref(),
        Some("profile.base_url")
    );
    assert_eq!(resolved.auth_source, "credentials.api_key");
    assert!(resolved.header_names.contains(&"Authorization".to_string()));
    assert!(resolved
        .sensitive_header_names
        .contains(&"Authorization".to_string()));
    assert_eq!(resolved.params["api_key"], json!("<redacted>"));
    assert_eq!(
        resolved.extra_body["metadata"]["token"],
        json!("<redacted>")
    );
    assert_eq!(resolved.extra_body["metadata"]["safe"], json!("visible"));

    let encoded = serde_json::to_string(&resolved).expect("resolved profile should encode");
    assert!(!encoded.contains("sk-do-not-leak"));
    assert!(!encoded.contains("Bearer do-not-leak"));

    let _ = fs::remove_file(profiles_path);
    let _ = fs::remove_file(credentials_path);
}

#[test]
fn provider_profiles_resolve_reports_missing_credentials_ref() {
    let profiles_path = temp_profiles_path();
    let credentials_path = temp_credentials_path();
    write_profiles_file(
        &profiles_path,
        json!({
            "broken-profile": {
                "provider": "openai_compatible",
                "credentials_ref": "missing-ref",
                "base_url": "https://api.openai.com/v1",
                "model": "gpt-4.1-mini"
            }
        }),
    );
    write_credentials_file(
        &credentials_path,
        json!({
            "openai-main": {
                "provider": "openai_compatible",
                "api_key": "sk-do-not-leak"
            }
        }),
    );

    let resolved = resolve_provider_profile(
        "broken-profile",
        Some(profiles_path.to_str().unwrap()),
        Some(credentials_path.to_str().unwrap()),
    )
    .expect("profile should resolve with issue");

    assert!(!resolved.ok);
    assert_eq!(resolved.credentials_ref_status, "missing");
    assert_eq!(resolved.auth_source, "unresolved_credentials_ref");
    assert!(resolved
        .issues
        .iter()
        .any(|issue| issue.code == "missing_credentials_ref"));

    let _ = fs::remove_file(profiles_path);
    let _ = fs::remove_file(credentials_path);
}

#[test]
fn provider_profiles_resolve_custom_http_credentials() {
    let profiles_path = temp_profiles_path();
    let credentials_path = temp_credentials_path();
    write_profiles_file(
        &profiles_path,
        json!({
            "custom-http": {
                "provider": "custom_http",
                "credentials_ref": "custom-http-main",
                "params": {
                    "method": "POST",
                    "url": "/jobs"
                }
            }
        }),
    );
    write_credentials_file(
        &credentials_path,
        json!({
            "custom-http-main": {
                "provider": "custom_http",
                "base_url": "https://api.example.test",
                "api_key": "do-not-leak",
                "headers": {
                    "X-Team": "visible"
                }
            }
        }),
    );

    let resolved = resolve_provider_profile(
        "custom-http",
        Some(profiles_path.to_str().unwrap()),
        Some(credentials_path.to_str().unwrap()),
    )
    .expect("custom HTTP profile should resolve");

    assert!(resolved.ok);
    assert_eq!(resolved.provider, "custom_http");
    assert_eq!(resolved.credentials_ref_status, "found");
    assert_eq!(
        resolved.base_url.as_deref(),
        Some("https://api.example.test")
    );
    assert_eq!(
        resolved.base_url_source.as_deref(),
        Some("credentials.base_url")
    );
    assert_eq!(resolved.auth_source, "credentials.api_key");
    assert_eq!(resolved.params["url"], json!("/jobs"));

    let encoded = serde_json::to_string(&resolved).expect("resolved profile should encode");
    assert!(!encoded.contains("do-not-leak"));

    let _ = fs::remove_file(profiles_path);
    let _ = fs::remove_file(credentials_path);
}

fn write_profiles_file(path: &std::path::Path, document: serde_json::Value) {
    fs::write(path, serde_json::to_string_pretty(&document).unwrap())
        .expect("profiles file should write");
}

fn write_credentials_file(path: &std::path::Path, document: serde_json::Value) {
    fs::write(path, serde_json::to_string_pretty(&document).unwrap())
        .expect("credentials file should write");
}

fn temp_profiles_path() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be valid")
        .as_nanos();
    std::env::temp_dir().join(format!("hgripe-provider-profiles-test-{nonce}.json"))
}

fn temp_credentials_path() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be valid")
        .as_nanos();
    std::env::temp_dir().join(format!("hgripe-provider-credentials-test-{nonce}.json"))
}
