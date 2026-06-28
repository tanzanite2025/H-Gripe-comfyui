use crate::provider::{BrokerError, BrokerResult};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct CredentialEntry {
    pub provider: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub api_key_env: Option<String>,
    pub headers: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CredentialSummary {
    pub credential_ref: String,
    pub provider: Option<String>,
    pub base_url: Option<String>,
    pub api_key_configured: bool,
    pub api_key_env: Option<String>,
    pub headers_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RedactedCredentialEntry {
    pub provider: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub api_key_env: Option<String>,
    pub headers: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CredentialValidationIssue {
    pub severity: String,
    pub credential_ref: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CredentialsValidation {
    pub credential_count: usize,
    pub ok: bool,
    pub issues: Vec<CredentialValidationIssue>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum CredentialsDocument {
    Profiles {
        profiles: BTreeMap<String, CredentialEntry>,
    },
    Direct(BTreeMap<String, CredentialEntry>),
}

pub fn load_credential_ref(
    credential_ref: &str,
    credentials_file: Option<&str>,
) -> BrokerResult<Option<CredentialEntry>> {
    let entries = load_credentials(credentials_file)?;
    Ok(entries.get(credential_ref).cloned())
}

pub fn load_credentials(
    credentials_file: Option<&str>,
) -> BrokerResult<BTreeMap<String, CredentialEntry>> {
    let path = credential_file_path(credentials_file);
    if !path.exists() {
        if credentials_file.is_some() || env::var("HGRIPE_CREDENTIALS_FILE").is_ok() {
            return Err(BrokerError::Provider(format!(
                "credentials file not found: {}",
                path.display()
            )));
        }
        return Ok(BTreeMap::new());
    }

    load_credentials_from_path(&path)
}

pub fn list_credential_summaries(
    credentials_file: Option<&str>,
) -> BrokerResult<Vec<CredentialSummary>> {
    let credentials = load_credentials(credentials_file)?;
    Ok(credential_summaries(&credentials))
}

pub fn get_redacted_credential_ref(
    credential_ref: &str,
    credentials_file: Option<&str>,
) -> BrokerResult<Option<RedactedCredentialEntry>> {
    Ok(load_credential_ref(credential_ref, credentials_file)?
        .map(|entry| redact_credential(&entry)))
}

pub fn validate_credentials(credentials_file: Option<&str>) -> BrokerResult<CredentialsValidation> {
    let credentials = load_credentials(credentials_file)?;
    Ok(validate_credentials_map(&credentials))
}

pub fn credentials_file_path(credentials_file: Option<&str>) -> PathBuf {
    credential_file_path(credentials_file)
}

fn load_credentials_from_path(path: &Path) -> BrokerResult<BTreeMap<String, CredentialEntry>> {
    let raw = fs::read_to_string(path).map_err(|err| {
        BrokerError::Provider(format!(
            "failed to read credentials file {}: {err}",
            path.display()
        ))
    })?;
    let document: CredentialsDocument = serde_json::from_str(&raw).map_err(|err| {
        BrokerError::Provider(format!(
            "failed to parse credentials file {}: {err}",
            path.display()
        ))
    })?;
    let entries = match document {
        CredentialsDocument::Profiles { profiles } => profiles,
        CredentialsDocument::Direct(entries) => entries,
    };

    Ok(entries)
}

fn credential_summaries(credentials: &BTreeMap<String, CredentialEntry>) -> Vec<CredentialSummary> {
    credentials
        .iter()
        .map(|(credential_ref, entry)| CredentialSummary {
            credential_ref: credential_ref.clone(),
            provider: trimmed_string(entry.provider.as_deref()),
            base_url: trimmed_string(entry.base_url.as_deref()),
            api_key_configured: entry
                .api_key
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_some(),
            api_key_env: trimmed_string(entry.api_key_env.as_deref()),
            headers_count: entry.headers.as_ref().map(BTreeMap::len).unwrap_or(0),
        })
        .collect()
}

fn redact_credential(entry: &CredentialEntry) -> RedactedCredentialEntry {
    RedactedCredentialEntry {
        provider: trimmed_string(entry.provider.as_deref()),
        base_url: trimmed_string(entry.base_url.as_deref()),
        api_key: entry
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|_| "<redacted>".to_string()),
        api_key_env: trimmed_string(entry.api_key_env.as_deref()),
        headers: entry.headers.as_ref().map(redact_headers),
    }
}

fn redact_headers(headers: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    headers
        .iter()
        .map(|(name, value)| {
            if is_sensitive_key(name) {
                (name.clone(), "<redacted>".to_string())
            } else {
                (name.clone(), value.clone())
            }
        })
        .collect()
}

fn validate_credentials_map(
    credentials: &BTreeMap<String, CredentialEntry>,
) -> CredentialsValidation {
    let mut issues = Vec::new();

    for (credential_ref, entry) in credentials {
        validate_credential_ref(credential_ref, &mut issues);
        validate_credential_provider(credential_ref, entry, &mut issues);
        validate_credential_base_url(credential_ref, entry, &mut issues);
        validate_credential_auth(credential_ref, entry, &mut issues);
        validate_credential_headers(credential_ref, entry, &mut issues);
    }

    let ok = !issues.iter().any(|issue| issue.severity == "error");
    CredentialsValidation {
        credential_count: credentials.len(),
        ok,
        issues,
    }
}

fn validate_credential_ref(credential_ref: &str, issues: &mut Vec<CredentialValidationIssue>) {
    if credential_ref.trim().is_empty() {
        push_issue(
            issues,
            "error",
            credential_ref,
            "empty_credential_ref",
            "credential ref must not be empty",
        );
    }
}

fn validate_credential_provider(
    credential_ref: &str,
    entry: &CredentialEntry,
    issues: &mut Vec<CredentialValidationIssue>,
) {
    let provider = entry
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("openai_compatible");

    if provider != "openai_compatible" {
        push_issue(
            issues,
            "error",
            credential_ref,
            "unsupported_provider",
            "only provider 'openai_compatible' is supported by credentials right now",
        );
    }
}

fn validate_credential_base_url(
    credential_ref: &str,
    entry: &CredentialEntry,
    issues: &mut Vec<CredentialValidationIssue>,
) {
    let Some(base_url) = entry
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
            credential_ref,
            "invalid_base_url",
            "base_url must start with http:// or https://",
        );
    }
}

fn validate_credential_auth(
    credential_ref: &str,
    entry: &CredentialEntry,
    issues: &mut Vec<CredentialValidationIssue>,
) {
    let has_api_key = entry
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some();
    let api_key_env = entry
        .api_key_env
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let has_secret_like_header = entry
        .headers
        .as_ref()
        .map(|headers| headers.keys().any(|name| is_sensitive_key(name)))
        .unwrap_or(false);

    if !has_api_key && api_key_env.is_none() && !has_secret_like_header {
        push_issue(
            issues,
            "warning",
            credential_ref,
            "auth_not_configured",
            "credential has no api_key, api_key_env, or secret-like auth header",
        );
    }

    if has_api_key && api_key_env.is_some() {
        push_issue(
            issues,
            "warning",
            credential_ref,
            "multiple_api_key_sources",
            "both api_key and api_key_env are set; inline api_key takes precedence",
        );
    }

    if let Some(api_key_env) = api_key_env {
        if env::var(api_key_env)
            .ok()
            .filter(|value| !value.is_empty())
            .is_none()
        {
            push_issue(
                issues,
                "warning",
                credential_ref,
                "api_key_env_not_set",
                "api_key_env is configured but the environment variable is not currently set",
            );
        }
    }
}

fn validate_credential_headers(
    credential_ref: &str,
    entry: &CredentialEntry,
    issues: &mut Vec<CredentialValidationIssue>,
) {
    let Some(headers) = &entry.headers else {
        return;
    };

    for name in headers.keys() {
        let trimmed = name.trim();
        if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
            push_issue(
                issues,
                "error",
                credential_ref,
                "invalid_header_name",
                "header names must not be empty or contain whitespace",
            );
        }
    }
}

fn push_issue(
    issues: &mut Vec<CredentialValidationIssue>,
    severity: &str,
    credential_ref: &str,
    code: &str,
    message: &str,
) {
    issues.push(CredentialValidationIssue {
        severity: severity.to_string(),
        credential_ref: credential_ref.to_string(),
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

fn credential_file_path(credentials_file: Option<&str>) -> PathBuf {
    if let Some(credentials_file) = credentials_file {
        let credentials_file = credentials_file.trim();
        if !credentials_file.is_empty() {
            return PathBuf::from(credentials_file);
        }
    }

    if let Ok(credentials_file) = env::var("HGRIPE_CREDENTIALS_FILE") {
        let credentials_file = credentials_file.trim();
        if !credentials_file.is_empty() {
            return PathBuf::from(credentials_file);
        }
    }

    env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("user")
        .join("hgripe")
        .join("credentials.json")
}
