//! Credential / provider-profile summaries and validation, plus raw read/write
//! of the on-disk config files backing them.

use std::fs;
use std::path::PathBuf;

use hgripe_api::{
    credentials_file_path, list_credential_summaries, list_provider_profile_summaries,
    provider_profiles_path, validate_credentials, validate_provider_profiles, CredentialSummary,
    CredentialsValidation, ProviderProfileSummary, ProviderProfilesValidation,
};

fn config_path(kind: &str) -> Result<PathBuf, String> {
    match kind {
        "credentials" => Ok(credentials_file_path(None)),
        "profiles" => Ok(provider_profiles_path(None)),
        other => Err(format!("unknown config kind: {other}")),
    }
}

#[tauri::command]
pub(crate) fn get_credentials() -> Result<Vec<CredentialSummary>, String> {
    list_credential_summaries(None).map_err(|err| err.to_string())
}

#[tauri::command]
pub(crate) fn check_credentials() -> Result<CredentialsValidation, String> {
    validate_credentials(None).map_err(|err| err.to_string())
}

#[tauri::command]
pub(crate) fn get_profiles() -> Result<Vec<ProviderProfileSummary>, String> {
    list_provider_profile_summaries(None).map_err(|err| err.to_string())
}

#[tauri::command]
pub(crate) fn check_profiles() -> Result<ProviderProfilesValidation, String> {
    validate_provider_profiles(None).map_err(|err| err.to_string())
}

#[tauri::command]
pub(crate) fn read_config_file(kind: String) -> Result<String, String> {
    let path = config_path(&kind)?;
    if !path.exists() {
        return Ok(String::new());
    }
    fs::read_to_string(&path).map_err(|err| format!("failed to read {}: {err}", path.display()))
}

#[tauri::command]
pub(crate) fn write_config_file(kind: String, content: String) -> Result<(), String> {
    let path = config_path(&kind)?;
    // Validate JSON before persisting so we never write a broken config file.
    if !content.trim().is_empty() {
        serde_json::from_str::<serde_json::Value>(&content)
            .map_err(|err| format!("invalid JSON: {err}"))?;
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
    }
    fs::write(&path, content).map_err(|err| format!("failed to write {}: {err}", path.display()))
}
