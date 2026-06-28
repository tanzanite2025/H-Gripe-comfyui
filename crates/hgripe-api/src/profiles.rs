use crate::provider::{BrokerError, BrokerResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::PathBuf;

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

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ProviderProfilesDocument {
    Direct(BTreeMap<String, ProviderProfile>),
    Profiles {
        profiles: BTreeMap<String, ProviderProfile>,
    },
}

pub fn load_provider_profile(
    profile_ref: &str,
    profiles_file: Option<&str>,
) -> BrokerResult<Option<ProviderProfile>> {
    let path = profiles_path(profiles_file);
    if !path.exists() {
        if profiles_file.is_some() || env::var("HGRIPE_PROVIDER_PROFILES_FILE").is_ok() {
            return Err(BrokerError::Provider(format!(
                "provider profiles file not found: {}",
                path.display()
            )));
        }
        return Ok(None);
    }

    let raw = fs::read_to_string(&path).map_err(|err| {
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
        ProviderProfilesDocument::Direct(profiles) => profiles,
        ProviderProfilesDocument::Profiles { profiles } => profiles,
    };

    Ok(profiles.get(profile_ref).cloned())
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
