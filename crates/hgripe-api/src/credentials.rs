use crate::provider::{BrokerError, BrokerResult};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct CredentialEntry {
    pub provider: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub api_key_env: Option<String>,
    pub headers: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum CredentialsDocument {
    Direct(BTreeMap<String, CredentialEntry>),
    Profiles {
        profiles: BTreeMap<String, CredentialEntry>,
    },
}

pub fn load_credential_ref(
    credential_ref: &str,
    credentials_file: Option<&str>,
) -> BrokerResult<Option<CredentialEntry>> {
    let path = credentials_path(credentials_file);
    if !path.exists() {
        if credentials_file.is_some() || env::var("HGRIPE_CREDENTIALS_FILE").is_ok() {
            return Err(BrokerError::Provider(format!(
                "credentials file not found: {}",
                path.display()
            )));
        }
        return Ok(None);
    }

    let raw = fs::read_to_string(&path).map_err(|err| {
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
        CredentialsDocument::Direct(entries) => entries,
        CredentialsDocument::Profiles { profiles } => profiles,
    };

    Ok(entries.get(credential_ref).cloned())
}

fn credentials_path(credentials_file: Option<&str>) -> PathBuf {
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
