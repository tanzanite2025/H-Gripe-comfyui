use crate::credentials::credentials_file_path;
use crate::profiles::provider_profiles_path;
use crate::provider::{BrokerError, BrokerResult};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct InitOptions {
    pub root_dir: Option<String>,
    pub credentials_file: Option<String>,
    pub profiles_file: Option<String>,
    pub history_file: Option<String>,
    pub history_db: Option<String>,
    pub output_dir: Option<String>,
    pub force: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InitAction {
    pub target_type: String,
    pub path: String,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct InitReport {
    pub dry_run: bool,
    pub root_dir: String,
    pub actions: Vec<InitAction>,
    pub created_count: usize,
    pub skipped_count: usize,
    pub overwritten_count: usize,
    pub would_create_count: usize,
    pub would_skip_count: usize,
    pub would_overwrite_count: usize,
}

pub fn initialize_local_config(options: InitOptions) -> BrokerResult<InitReport> {
    let paths = InitPaths::resolve(&options);
    let mut report = InitReport {
        dry_run: options.dry_run,
        root_dir: paths.root_dir.to_string_lossy().to_string(),
        ..InitReport::default()
    };

    apply_directory(&paths.hgripe_dir, "directory", &options, &mut report)?;
    apply_directory(&paths.history_dir, "directory", &options, &mut report)?;
    apply_directory(&paths.output_dir, "directory", &options, &mut report)?;
    apply_file(
        &paths.credentials_file,
        "credentials_file",
        credentials_template()?,
        &options,
        &mut report,
    )?;
    apply_file(
        &paths.profiles_file,
        "provider_profiles_file",
        provider_profiles_template()?,
        &options,
        &mut report,
    )?;

    Ok(report)
}

#[derive(Debug, Clone)]
struct InitPaths {
    root_dir: PathBuf,
    hgripe_dir: PathBuf,
    history_dir: PathBuf,
    credentials_file: PathBuf,
    profiles_file: PathBuf,
    output_dir: PathBuf,
}

impl InitPaths {
    fn resolve(options: &InitOptions) -> Self {
        let root_dir = options
            .root_dir
            .as_deref()
            .and_then(explicit_path)
            .unwrap_or_else(local_root);
        let root_dir = absolute_path(root_dir);
        let hgripe_dir = root_dir.join("user").join("hgripe");
        let history_dir = options
            .history_file
            .as_deref()
            .and_then(explicit_path)
            .map(absolute_path)
            .or_else(|| {
                options
                    .history_db
                    .as_deref()
                    .and_then(explicit_path)
                    .map(absolute_path)
            })
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| hgripe_dir.join("history"));

        Self {
            credentials_file: options
                .credentials_file
                .as_deref()
                .and_then(explicit_path)
                .map(absolute_path)
                .unwrap_or_else(|| default_credentials_file(&root_dir)),
            profiles_file: options
                .profiles_file
                .as_deref()
                .and_then(explicit_path)
                .map(absolute_path)
                .unwrap_or_else(|| default_profiles_file(&root_dir)),
            output_dir: options
                .output_dir
                .as_deref()
                .and_then(explicit_path)
                .map(absolute_path)
                .unwrap_or_else(|| hgripe_dir.join("outputs")),
            root_dir,
            hgripe_dir,
            history_dir,
        }
    }
}

fn apply_directory(
    path: &Path,
    target_type: &str,
    options: &InitOptions,
    report: &mut InitReport,
) -> BrokerResult<()> {
    let exists = path.exists();
    let status = if exists {
        if !path.is_dir() {
            return Err(BrokerError::Provider(format!(
                "{} exists but is not a directory",
                path.display()
            )));
        }
        if options.dry_run {
            "would_skip_existing"
        } else {
            "skipped_existing"
        }
    } else if options.dry_run {
        "would_create"
    } else {
        fs::create_dir_all(path).map_err(|err| {
            BrokerError::Provider(format!(
                "failed to create directory {}: {err}",
                path.display()
            ))
        })?;
        "created"
    };

    push_action(report, target_type, path, status, directory_message(status));
    Ok(())
}

fn apply_file(
    path: &Path,
    target_type: &str,
    content: String,
    options: &InitOptions,
    report: &mut InitReport,
) -> BrokerResult<()> {
    let exists = path.exists();
    let status = if exists && !options.force {
        if options.dry_run {
            "would_skip_existing"
        } else {
            "skipped_existing"
        }
    } else if options.dry_run {
        if exists {
            "would_overwrite"
        } else {
            "would_create"
        }
    } else {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                BrokerError::Provider(format!(
                    "failed to create directory {}: {err}",
                    parent.display()
                ))
            })?;
        }
        fs::write(path, content).map_err(|err| {
            BrokerError::Provider(format!("failed to write file {}: {err}", path.display()))
        })?;
        if exists {
            "overwritten"
        } else {
            "created"
        }
    };

    push_action(report, target_type, path, status, file_message(status));
    Ok(())
}

fn push_action(
    report: &mut InitReport,
    target_type: &str,
    path: &Path,
    status: &str,
    message: &str,
) {
    match status {
        "created" => report.created_count += 1,
        "skipped_existing" => report.skipped_count += 1,
        "overwritten" => report.overwritten_count += 1,
        "would_create" => report.would_create_count += 1,
        "would_skip_existing" => report.would_skip_count += 1,
        "would_overwrite" => report.would_overwrite_count += 1,
        _ => {}
    }

    report.actions.push(InitAction {
        target_type: target_type.to_string(),
        path: path.to_string_lossy().to_string(),
        status: status.to_string(),
        message: message.to_string(),
    });
}

fn directory_message(status: &str) -> &'static str {
    match status {
        "created" => "directory created",
        "skipped_existing" => "directory already exists",
        "would_create" => "directory would be created",
        "would_skip_existing" => "directory already exists",
        _ => "directory action recorded",
    }
}

fn file_message(status: &str) -> &'static str {
    match status {
        "created" => "template file created",
        "overwritten" => "template file overwritten",
        "skipped_existing" => "file already exists; use --force to overwrite",
        "would_create" => "template file would be created",
        "would_overwrite" => "template file would be overwritten",
        "would_skip_existing" => "file already exists; use --force to overwrite",
        _ => "file action recorded",
    }
}

fn credentials_template() -> BrokerResult<String> {
    encode_template(json!({
        "profiles": {
            "openai-main": {
                "provider": "openai_compatible",
                "base_url": "https://api.openai.com/v1",
                "api_key_env": "OPENAI_API_KEY"
            }
        }
    }))
}

fn provider_profiles_template() -> BrokerResult<String> {
    encode_template(json!({
        "profiles": {
            "openai-default": {
                "provider": "openai_compatible",
                "credentials_ref": "openai-main",
                "base_url": "https://api.openai.com/v1",
                "model": "gpt-4.1-mini",
                "params": {
                    "temperature": 0.7,
                    "max_tokens": 1024
                }
            },
            "local-openai-compatible": {
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
}

fn encode_template(value: serde_json::Value) -> BrokerResult<String> {
    serde_json::to_string_pretty(&value)
        .map(|encoded| format!("{encoded}\n"))
        .map_err(|err| BrokerError::Provider(format!("failed to encode config template: {err}")))
}

fn default_credentials_file(root_dir: &Path) -> PathBuf {
    if root_dir != local_root() {
        return root_dir
            .join("user")
            .join("hgripe")
            .join("credentials.json");
    }

    if let Ok(credentials_file) = env::var("HGRIPE_CREDENTIALS_FILE") {
        let credentials_file = credentials_file.trim();
        if !credentials_file.is_empty() {
            return PathBuf::from(credentials_file);
        }
    }

    credentials_file_path(None)
}

fn default_profiles_file(root_dir: &Path) -> PathBuf {
    if root_dir != local_root() {
        return root_dir
            .join("user")
            .join("hgripe")
            .join("provider_profiles.json");
    }

    if let Ok(profiles_file) = env::var("HGRIPE_PROVIDER_PROFILES_FILE") {
        let profiles_file = profiles_file.trim();
        if !profiles_file.is_empty() {
            return PathBuf::from(profiles_file);
        }
    }

    provider_profiles_path(None)
}

fn explicit_path(value: &str) -> Option<PathBuf> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(PathBuf::from(value))
    }
}

fn absolute_path(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        local_root().join(path)
    }
}

fn local_root() -> PathBuf {
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}
