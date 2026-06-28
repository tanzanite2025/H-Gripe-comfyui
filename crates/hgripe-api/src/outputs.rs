use crate::model::{ApiTask, OutputFile};
use crate::provider::{BrokerError, BrokerResult};
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::PathBuf;

pub fn output_dir_from_env(override_dir: Option<&str>) -> BrokerResult<PathBuf> {
    let output_dir = override_dir
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| env_path("HGRIPE_OUTPUT_DIR"))
        .unwrap_or_else(default_output_dir);
    let output_dir = absolute_path(output_dir);

    fs::create_dir_all(&output_dir).map_err(|err| {
        BrokerError::Provider(format!(
            "failed to create output directory {}: {err}",
            output_dir.display()
        ))
    })?;

    Ok(output_dir)
}

pub fn write_task_output_bytes(
    override_dir: Option<&str>,
    task: &ApiTask,
    index: usize,
    bytes: &[u8],
    mime_type: Option<&str>,
    extension: &str,
) -> BrokerResult<OutputFile> {
    let root = output_dir_from_env(override_dir)?;
    let subdir = root
        .join(sanitize_path_component(&task.provider))
        .join(sanitize_path_component(&task.operation));
    fs::create_dir_all(&subdir).map_err(|err| {
        BrokerError::Provider(format!(
            "failed to create output directory {}: {err}",
            subdir.display()
        ))
    })?;

    let extension = sanitize_extension(extension);
    let filename = format!(
        "{}-{:03}.{}",
        sanitize_path_component(&task.id),
        index,
        extension
    );
    let path = subdir.join(filename);
    fs::write(&path, bytes).map_err(|err| {
        BrokerError::Provider(format!(
            "failed to write output file {}: {err}",
            path.display()
        ))
    })?;

    let sha256 = Sha256::digest(bytes);
    Ok(OutputFile {
        path: path.to_string_lossy().to_string(),
        mime_type: mime_type.map(str::to_string),
        size_bytes: Some(bytes.len() as u64),
        sha256: Some(format!("{sha256:x}")),
    })
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn default_output_dir() -> PathBuf {
    env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("user")
        .join("hgripe")
        .join("outputs")
}

fn absolute_path(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        return path;
    }

    env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(path)
}

fn sanitize_path_component(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|character| match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => character,
            _ => '_',
        })
        .collect();
    let sanitized = sanitized.trim_matches('_');
    if sanitized.is_empty() {
        "item".to_string()
    } else {
        sanitized.to_string()
    }
}

fn sanitize_extension(extension: &str) -> String {
    let extension = extension.trim().trim_start_matches('.');
    let extension: String = extension
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect();
    if extension.is_empty() {
        "bin".to_string()
    } else {
        extension.to_ascii_lowercase()
    }
}
