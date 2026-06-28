use crate::credentials::load_credential_ref;
use crate::provider::{BrokerError, BrokerResult};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ProviderProfile {
    pub provider: Option<String>,
    pub credentials_ref: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub path: Option<String>,
    pub api_key_env: Option<String>,
    pub no_auth: Option<bool>,
    pub headers: Option<BTreeMap<String, String>>,
    pub params: Option<BTreeMap<String, Value>>,
    pub extra_body: Option<BTreeMap<String, Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderProfileSummary {
    pub profile_ref: String,
    pub provider: Option<String>,
    pub credentials_ref: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub no_auth: Option<bool>,
    pub has_headers: bool,
    pub params_count: usize,
    pub extra_body_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResolvedProviderProfile {
    pub profile_ref: String,
    pub ok: bool,
    pub provider: String,
    pub credentials_ref: Option<String>,
    pub credentials_ref_status: String,
    pub base_url: Option<String>,
    pub base_url_source: Option<String>,
    pub model: Option<String>,
    pub path: Option<String>,
    pub no_auth: bool,
    pub auth_source: String,
    pub api_key_env: Option<String>,
    pub api_key_env_is_set: Option<bool>,
    pub header_names: Vec<String>,
    pub sensitive_header_names: Vec<String>,
    pub params: BTreeMap<String, Value>,
    pub extra_body: BTreeMap<String, Value>,
    pub issues: Vec<ResolvedProviderProfileIssue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolvedProviderProfileIssue {
    pub severity: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderProfileValidationIssue {
    pub severity: String,
    pub profile_ref: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderProfilesValidation {
    pub profile_count: usize,
    pub ok: bool,
    pub issues: Vec<ProviderProfileValidationIssue>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ProviderProfilesDocument {
    Profiles {
        profiles: BTreeMap<String, ProviderProfile>,
    },
    Direct(BTreeMap<String, ProviderProfile>),
}

pub fn load_provider_profile(
    profile_ref: &str,
    profiles_file: Option<&str>,
) -> BrokerResult<Option<ProviderProfile>> {
    let profiles = load_provider_profiles(profiles_file)?;
    Ok(profiles.get(profile_ref).cloned())
}

pub fn load_provider_profiles(
    profiles_file: Option<&str>,
) -> BrokerResult<BTreeMap<String, ProviderProfile>> {
    let path = provider_profiles_path(profiles_file);
    if !path.exists() {
        if profiles_file.is_some() || env::var("HGRIPE_PROVIDER_PROFILES_FILE").is_ok() {
            return Err(BrokerError::Provider(format!(
                "provider profiles file not found: {}",
                path.display()
            )));
        }
        return Ok(BTreeMap::new());
    }

    load_provider_profiles_from_path(&path)
}

pub fn list_provider_profile_summaries(
    profiles_file: Option<&str>,
) -> BrokerResult<Vec<ProviderProfileSummary>> {
    let profiles = load_provider_profiles(profiles_file)?;
    Ok(provider_profile_summaries(&profiles))
}

pub fn get_provider_profile(
    profile_ref: &str,
    profiles_file: Option<&str>,
) -> BrokerResult<Option<ProviderProfile>> {
    load_provider_profile(profile_ref, profiles_file)
}

pub fn resolve_provider_profile(
    profile_ref: &str,
    profiles_file: Option<&str>,
    credentials_file: Option<&str>,
) -> BrokerResult<ResolvedProviderProfile> {
    let profile = load_provider_profile(profile_ref, profiles_file)?.ok_or_else(|| {
        BrokerError::Provider(format!("provider profile '{profile_ref}' was not found"))
    })?;

    resolve_provider_profile_entry(profile_ref, &profile, credentials_file)
}

pub fn validate_provider_profiles(
    profiles_file: Option<&str>,
) -> BrokerResult<ProviderProfilesValidation> {
    let profiles = load_provider_profiles(profiles_file)?;
    Ok(validate_provider_profiles_map(&profiles))
}

pub fn provider_profiles_path(profiles_file: Option<&str>) -> PathBuf {
    profiles_path(profiles_file)
}

fn load_provider_profiles_from_path(
    path: &Path,
) -> BrokerResult<BTreeMap<String, ProviderProfile>> {
    let raw = fs::read_to_string(path).map_err(|err| {
        BrokerError::Provider(format!(
            "failed to read provider profiles file {}: {err}",
            path.display()
        ))
    })?;
    let document: ProviderProfilesDocument = serde_json::from_str(&raw).map_err(|err| {
        BrokerError::Provider(format!(
            "failed to parse provider profiles file {}: {err}",
            path.display()
        ))
    })?;
    let profiles = match document {
        ProviderProfilesDocument::Profiles { profiles } => profiles,
        ProviderProfilesDocument::Direct(profiles) => profiles,
    };

    Ok(profiles)
}

fn provider_profile_summaries(
    profiles: &BTreeMap<String, ProviderProfile>,
) -> Vec<ProviderProfileSummary> {
    profiles
        .iter()
        .map(|(profile_ref, profile)| ProviderProfileSummary {
            profile_ref: profile_ref.clone(),
            provider: trimmed_string(profile.provider.as_deref()),
            credentials_ref: trimmed_string(profile.credentials_ref.as_deref()),
            base_url: trimmed_string(profile.base_url.as_deref()),
            model: trimmed_string(profile.model.as_deref()),
            no_auth: profile.no_auth,
            has_headers: profile
                .headers
                .as_ref()
                .map(|headers| !headers.is_empty())
                .unwrap_or(false),
            params_count: profile.params.as_ref().map(BTreeMap::len).unwrap_or(0),
            extra_body_count: profile.extra_body.as_ref().map(BTreeMap::len).unwrap_or(0),
        })
        .collect()
}

fn resolve_provider_profile_entry(
    profile_ref: &str,
    profile: &ProviderProfile,
    credentials_file: Option<&str>,
) -> BrokerResult<ResolvedProviderProfile> {
    let mut issues = Vec::new();
    let provider = trimmed_string(profile.provider.as_deref())
        .unwrap_or_else(|| "openai_compatible".to_string());
    let no_auth = profile.no_auth.unwrap_or(false);
    let credentials_ref = trimmed_string(profile.credentials_ref.as_deref());
    let mut credentials_ref_status = if credentials_ref.is_some() {
        "not_checked".to_string()
    } else {
        "not_configured".to_string()
    };
    let mut credential = None;

    if provider != "openai_compatible" {
        push_resolved_issue(
            &mut issues,
            "error",
            "unsupported_provider",
            "only provider 'openai_compatible' is supported by provider profiles right now",
        );
    }

    if no_auth {
        if credentials_ref.is_some() {
            credentials_ref_status = "ignored_by_no_auth".to_string();
            push_resolved_issue(
                &mut issues,
                "warning",
                "auth_ignored_by_no_auth",
                "no_auth=true means credentials_ref and api_key_env will be ignored",
            );
        }
    } else if let Some(credentials_ref) = credentials_ref.as_deref() {
        match load_credential_ref(credentials_ref, credentials_file) {
            Ok(Some(entry)) => {
                credentials_ref_status = "found".to_string();
                credential = Some(entry);
            }
            Ok(None) => {
                credentials_ref_status = "missing".to_string();
                push_resolved_issue(
                    &mut issues,
                    "error",
                    "missing_credentials_ref",
                    &format!(
                        "provider profile references credentials_ref '{credentials_ref}' but it was not found"
                    ),
                );
            }
            Err(err) => {
                credentials_ref_status = "unreadable".to_string();
                push_resolved_issue(
                    &mut issues,
                    "error",
                    "credentials_unreadable",
                    &err.to_string(),
                );
            }
        }
    }

    let (base_url, base_url_source) = resolved_base_url(profile, credential.as_ref());
    let (auth_source, api_key_env, api_key_env_is_set) = resolved_auth_source(
        profile,
        credential.as_ref(),
        no_auth,
        credentials_ref.as_deref(),
        &credentials_ref_status,
    );
    let (header_names, sensitive_header_names) =
        resolved_header_names(profile, credential.as_ref());
    let params = redacted_value_map(profile.params.as_ref());
    let extra_body = redacted_value_map(profile.extra_body.as_ref());
    let ok = !issues.iter().any(|issue| issue.severity == "error");

    Ok(ResolvedProviderProfile {
        profile_ref: profile_ref.to_string(),
        ok,
        provider,
        credentials_ref,
        credentials_ref_status,
        base_url,
        base_url_source,
        model: trimmed_string(profile.model.as_deref()),
        path: trimmed_string(profile.path.as_deref()),
        no_auth,
        auth_source,
        api_key_env,
        api_key_env_is_set,
        header_names,
        sensitive_header_names,
        params,
        extra_body,
        issues,
    })
}

fn resolved_base_url(
    profile: &ProviderProfile,
    credential: Option<&crate::credentials::CredentialEntry>,
) -> (Option<String>, Option<String>) {
    if let Some(base_url) = trimmed_string(profile.base_url.as_deref()) {
        return (Some(base_url), Some("profile.base_url".to_string()));
    }

    if let Some(base_url) = credential.and_then(|entry| trimmed_string(entry.base_url.as_deref())) {
        return (Some(base_url), Some("credentials.base_url".to_string()));
    }

    if let Some(base_url) = env_string("HGRIPE_OPENAI_COMPATIBLE_BASE_URL") {
        return (
            Some(base_url),
            Some("env.HGRIPE_OPENAI_COMPATIBLE_BASE_URL".to_string()),
        );
    }

    (
        Some("https://api.openai.com/v1".to_string()),
        Some("runtime_default".to_string()),
    )
}

fn resolved_auth_source(
    profile: &ProviderProfile,
    credential: Option<&crate::credentials::CredentialEntry>,
    no_auth: bool,
    credentials_ref: Option<&str>,
    credentials_ref_status: &str,
) -> (String, Option<String>, Option<bool>) {
    if no_auth {
        return ("profile.no_auth".to_string(), None, None);
    }

    if credentials_ref.is_some() && credentials_ref_status != "found" {
        return ("unresolved_credentials_ref".to_string(), None, None);
    }

    if let Some(api_key_env) = trimmed_string(profile.api_key_env.as_deref()) {
        let is_set = env_var_is_set(&api_key_env);
        return (
            "profile.api_key_env".to_string(),
            Some(api_key_env),
            Some(is_set),
        );
    }

    if let Some(entry) = credential {
        if entry
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_some()
        {
            return ("credentials.api_key".to_string(), None, None);
        }

        if let Some(api_key_env) = trimmed_string(entry.api_key_env.as_deref()) {
            let is_set = env_var_is_set(&api_key_env);
            return (
                "credentials.api_key_env".to_string(),
                Some(api_key_env),
                Some(is_set),
            );
        }

        if entry
            .headers
            .as_ref()
            .map(|headers| headers.keys().any(|name| is_sensitive_key(name)))
            .unwrap_or(false)
        {
            return ("credentials.headers".to_string(), None, None);
        }
    }

    (
        "environment_fallback".to_string(),
        Some("HGRIPE_OPENAI_COMPATIBLE_API_KEY|OPENAI_API_KEY".to_string()),
        Some(
            env_var_is_set("HGRIPE_OPENAI_COMPATIBLE_API_KEY") || env_var_is_set("OPENAI_API_KEY"),
        ),
    )
}

fn resolved_header_names(
    profile: &ProviderProfile,
    credential: Option<&crate::credentials::CredentialEntry>,
) -> (Vec<String>, Vec<String>) {
    let mut header_names = BTreeSet::new();
    if let Some(headers) = credential.and_then(|entry| entry.headers.as_ref()) {
        header_names.extend(headers.keys().cloned());
    }
    if let Some(headers) = &profile.headers {
        header_names.extend(headers.keys().cloned());
    }

    let sensitive_header_names = header_names
        .iter()
        .filter(|name| is_sensitive_key(name))
        .cloned()
        .collect();

    (header_names.into_iter().collect(), sensitive_header_names)
}

fn validate_provider_profiles_map(
    profiles: &BTreeMap<String, ProviderProfile>,
) -> ProviderProfilesValidation {
    let mut issues = Vec::new();

    for (profile_ref, profile) in profiles {
        validate_profile_ref(profile_ref, &mut issues);
        validate_profile_provider(profile_ref, profile, &mut issues);
        validate_profile_base_url(profile_ref, profile, &mut issues);
        validate_profile_auth(profile_ref, profile, &mut issues);
        validate_profile_headers(profile_ref, profile, &mut issues);
        validate_profile_defaults(profile_ref, profile, &mut issues);
    }

    let ok = !issues.iter().any(|issue| issue.severity == "error");
    ProviderProfilesValidation {
        profile_count: profiles.len(),
        ok,
        issues,
    }
}

fn validate_profile_ref(profile_ref: &str, issues: &mut Vec<ProviderProfileValidationIssue>) {
    if profile_ref.trim().is_empty() {
        push_issue(
            issues,
            "error",
            profile_ref,
            "empty_profile_ref",
            "profile ref must not be empty",
        );
    }
}

fn validate_profile_provider(
    profile_ref: &str,
    profile: &ProviderProfile,
    issues: &mut Vec<ProviderProfileValidationIssue>,
) {
    let provider = profile
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("openai_compatible");

    if provider != "openai_compatible" {
        push_issue(
            issues,
            "error",
            profile_ref,
            "unsupported_provider",
            "only provider 'openai_compatible' is supported by provider profiles right now",
        );
    }
}

fn validate_profile_base_url(
    profile_ref: &str,
    profile: &ProviderProfile,
    issues: &mut Vec<ProviderProfileValidationIssue>,
) {
    let Some(base_url) = profile
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return;
    };

    if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
        push_issue(
            issues,
            "error",
            profile_ref,
            "invalid_base_url",
            "base_url must start with http:// or https://",
        );
    }
}

fn validate_profile_auth(
    profile_ref: &str,
    profile: &ProviderProfile,
    issues: &mut Vec<ProviderProfileValidationIssue>,
) {
    if profile.no_auth == Some(true)
        && (profile
            .credentials_ref
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_some()
            || profile
                .api_key_env
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_some())
    {
        push_issue(
            issues,
            "warning",
            profile_ref,
            "auth_ignored_by_no_auth",
            "no_auth=true means credentials_ref and api_key_env will be ignored",
        );
    }

    if profile.no_auth != Some(true)
        && profile
            .credentials_ref
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none()
        && profile
            .api_key_env
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none()
    {
        push_issue(
            issues,
            "warning",
            profile_ref,
            "auth_not_configured",
            "profile has no credentials_ref, api_key_env, or no_auth=true; tasks may rely on credentials outside the profile",
        );
    }
}

fn validate_profile_headers(
    profile_ref: &str,
    profile: &ProviderProfile,
    issues: &mut Vec<ProviderProfileValidationIssue>,
) {
    let Some(headers) = &profile.headers else {
        return;
    };

    for name in headers.keys() {
        let trimmed = name.trim();
        if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
            push_issue(
                issues,
                "error",
                profile_ref,
                "invalid_header_name",
                "header names must not be empty or contain whitespace",
            );
        }
        if is_sensitive_key(trimmed) {
            push_issue(
                issues,
                "warning",
                profile_ref,
                "secret_like_header",
                "provider profiles should not store secret-like headers; prefer credentials_ref",
            );
        }
    }
}

fn validate_profile_defaults(
    profile_ref: &str,
    profile: &ProviderProfile,
    issues: &mut Vec<ProviderProfileValidationIssue>,
) {
    if profile
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
    {
        push_issue(
            issues,
            "warning",
            profile_ref,
            "model_not_configured",
            "profile has no default model; tasks must provide model explicitly",
        );
    }

    if let Some(params) = &profile.params {
        validate_secret_like_value_map(profile_ref, "params", params, issues);
    }
    if let Some(extra_body) = &profile.extra_body {
        validate_secret_like_value_map(profile_ref, "extra_body", extra_body, issues);
    }
}

fn validate_secret_like_value_map(
    profile_ref: &str,
    scope: &str,
    values: &BTreeMap<String, Value>,
    issues: &mut Vec<ProviderProfileValidationIssue>,
) {
    for (key, value) in values {
        validate_secret_like_value(profile_ref, scope, key, value, issues);
    }
}

fn validate_secret_like_value(
    profile_ref: &str,
    scope: &str,
    key: &str,
    value: &Value,
    issues: &mut Vec<ProviderProfileValidationIssue>,
) {
    if is_sensitive_key(key) {
        push_issue(
            issues,
            "warning",
            profile_ref,
            "secret_like_param",
            &format!("{scope}.{key} looks secret-like; prefer credentials_ref"),
        );
    }

    match value {
        Value::Object(map) => {
            for (child_key, child_value) in map {
                validate_secret_like_value(profile_ref, scope, child_key, child_value, issues);
            }
        }
        Value::Array(items) => {
            for item in items {
                validate_secret_like_value(profile_ref, scope, key, item, issues);
            }
        }
        _ => {}
    }
}

fn push_issue(
    issues: &mut Vec<ProviderProfileValidationIssue>,
    severity: &str,
    profile_ref: &str,
    code: &str,
    message: &str,
) {
    issues.push(ProviderProfileValidationIssue {
        severity: severity.to_string(),
        profile_ref: profile_ref.to_string(),
        code: code.to_string(),
        message: message.to_string(),
    });
}

fn push_resolved_issue(
    issues: &mut Vec<ResolvedProviderProfileIssue>,
    severity: &str,
    code: &str,
    message: &str,
) {
    issues.push(ResolvedProviderProfileIssue {
        severity: severity.to_string(),
        code: code.to_string(),
        message: message.to_string(),
    });
}

fn trimmed_string(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn env_string(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_var_is_set(name: &str) -> bool {
    env_string(name).is_some()
}

fn redacted_value_map(values: Option<&BTreeMap<String, Value>>) -> BTreeMap<String, Value> {
    values
        .map(|values| {
            values
                .iter()
                .map(|(key, value)| (key.clone(), redact_secret_like_value(key, value)))
                .collect()
        })
        .unwrap_or_default()
}

fn redact_secret_like_value(key: &str, value: &Value) -> Value {
    if is_sensitive_key(key) {
        return Value::String("<redacted>".to_string());
    }

    match value {
        Value::Object(map) => {
            let mut redacted = Map::new();
            for (child_key, child_value) in map {
                redacted.insert(
                    child_key.clone(),
                    redact_secret_like_value(child_key, child_value),
                );
            }
            Value::Object(redacted)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| redact_secret_like_value(key, item))
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized: String = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect();

    matches!(
        normalized.as_str(),
        "authorization"
            | "proxyauthorization"
            | "apikey"
            | "xapikey"
            | "key"
            | "token"
            | "accesstoken"
            | "refreshtoken"
            | "idtoken"
            | "cookie"
            | "setcookie"
            | "session"
            | "sessionid"
    ) || normalized.ends_with("apikey")
        || normalized.ends_with("token")
        || normalized.contains("password")
        || normalized.contains("secret")
}

fn profiles_path(profiles_file: Option<&str>) -> PathBuf {
    if let Some(profiles_file) = profiles_file {
        let profiles_file = profiles_file.trim();
        if !profiles_file.is_empty() {
            return PathBuf::from(profiles_file);
        }
    }

    if let Ok(profiles_file) = env::var("HGRIPE_PROVIDER_PROFILES_FILE") {
        let profiles_file = profiles_file.trim();
        if !profiles_file.is_empty() {
            return PathBuf::from(profiles_file);
        }
    }

    env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("user")
        .join("hgripe")
        .join("provider_profiles.json")
}
