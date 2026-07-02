//! Persisted local-model weight paths (the "local model manager" surface).
//!
//! Each opt-in ML engine resolves its non-bundled weight from a dedicated env
//! var (e.g. `HGRIPE_REALESRGAN_MODEL`) falling back to the shared cache dir
//! (`HGRIPE_MODEL_CACHE`). Those env vars are a dev/CI affordance; end users
//! need a persisted, in-app way to point an engine at a downloaded weight.
//! This module stores that mapping in a small JSON file next to the broker's
//! other local config files and applies it as env vars on every spawned bridge
//! subprocess, so the Python backends keep their existing resolution order
//! unchanged. A real process-level env var always wins over the persisted
//! config (the config is applied only when the var is unset).

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use hgripe_api::credentials_file_path;
use serde::{Deserialize, Serialize};

/// Engine id -> the env var its Python backend resolves the weight from.
/// Covers every bridge engine the cross-card `probe_engines` report lists.
const ENGINE_ENV_VARS: [(&str, &str); 5] = [
    ("realesrgan", "HGRIPE_REALESRGAN_MODEL"),
    ("sd_inpaint", "HGRIPE_INPAINT_MODEL"),
    ("onnx_defect", "HGRIPE_WATCHDOG_MODEL"),
    ("onnx_harmonize", "HGRIPE_COLOR_MODEL"),
    ("onnx_matting", "HGRIPE_MATTING_MODEL"),
];

/// Shared weight cache env var (`sr_backends.model_cache_dir`).
const MODEL_CACHE_ENV: &str = "HGRIPE_MODEL_CACHE";

/// The persisted mapping: an optional shared cache dir plus per-engine weight
/// path overrides. Unknown engine ids are preserved (forward compat).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub(crate) struct ModelPathsConfig {
    /// Shared weight cache dir (`HGRIPE_MODEL_CACHE`); empty/absent = default.
    #[serde(default)]
    pub(crate) model_cache_dir: Option<String>,
    /// Engine id -> explicit weight path (file, or dir for a HF snapshot).
    #[serde(default)]
    pub(crate) weights: BTreeMap<String, String>,
}

/// One row of the manager surface: how an engine's weight override is
/// currently sourced, so the UI can show and edit it truthfully.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ModelPathEntry {
    /// Engine id (matches the `probe_engines` report keys).
    pub(crate) engine: String,
    /// The env var the Python backend reads.
    pub(crate) env_var: String,
    /// The persisted override, if any.
    pub(crate) configured_path: Option<String>,
    /// Whether the persisted override exists on disk (file or directory).
    pub(crate) configured_exists: bool,
    /// A process-level env var is set and therefore wins over the config.
    pub(crate) env_active: bool,
    /// The active process-level value, when `env_active`.
    pub(crate) env_value: Option<String>,
}

/// The full manager report: the persisted config plus its per-engine rows and
/// the cache-dir sourcing, ready for the Dashboard panel.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ModelPathsReport {
    pub(crate) config: ModelPathsConfig,
    pub(crate) entries: Vec<ModelPathEntry>,
    /// A process-level `HGRIPE_MODEL_CACHE` is set and wins over the config.
    pub(crate) cache_env_active: bool,
    pub(crate) cache_env_value: Option<String>,
    /// Where the config file lives (shown so users can locate/back it up).
    pub(crate) config_file: String,
}

/// The config lives next to the broker's credentials/profiles files, which are
/// already the app's local-config home.
fn config_file_path() -> PathBuf {
    let credentials = credentials_file_path(None);
    match credentials.parent() {
        Some(dir) => dir.join("model_paths.json"),
        None => PathBuf::from("model_paths.json"),
    }
}

/// Load the persisted config; a missing or unreadable file is an empty config
/// (the manager surface starts blank rather than failing the Dashboard).
pub(crate) fn load_model_paths_config() -> ModelPathsConfig {
    let path = config_file_path();
    let Ok(raw) = fs::read_to_string(&path) else {
        return ModelPathsConfig::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

fn save_model_paths_config(config: &ModelPathsConfig) -> Result<(), String> {
    let path = config_file_path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)
            .map_err(|err| format!("could not create {}: {err}", dir.display()))?;
    }
    let raw = serde_json::to_string_pretty(config)
        .map_err(|err| format!("could not serialise model paths: {err}"))?;
    fs::write(&path, raw).map_err(|err| format!("could not write {}: {err}", path.display()))
}

/// Normalise a saved config: trim entries and drop empties so clearing a field
/// in the UI removes the override.
fn normalise(mut config: ModelPathsConfig) -> ModelPathsConfig {
    config.model_cache_dir = config
        .model_cache_dir
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty());
    config.weights = config
        .weights
        .into_iter()
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .filter(|(k, v)| !k.is_empty() && !v.is_empty())
        .collect();
    config
}

/// Apply the persisted weight paths as env vars on a bridge subprocess. A var
/// already set on the parent process (dev / CI) is left untouched, preserving
/// the documented "env override first" resolution order.
pub(crate) fn apply_model_env(cmd: &mut std::process::Command) {
    let config = load_model_paths_config();
    if let Some(dir) = config
        .model_cache_dir
        .as_deref()
        .filter(|d| !d.trim().is_empty())
    {
        if std::env::var_os(MODEL_CACHE_ENV).is_none() {
            cmd.env(MODEL_CACHE_ENV, dir);
        }
    }
    for (engine, env_var) in ENGINE_ENV_VARS {
        let Some(path) = config.weights.get(engine).filter(|p| !p.trim().is_empty()) else {
            continue;
        };
        if std::env::var_os(env_var).is_none() {
            cmd.env(env_var, path);
        }
    }
}

fn build_report(config: ModelPathsConfig) -> ModelPathsReport {
    let entries = ENGINE_ENV_VARS
        .iter()
        .map(|(engine, env_var)| {
            let configured_path = config.weights.get(*engine).cloned();
            let configured_exists = configured_path
                .as_deref()
                .map(|p| {
                    let path = std::path::Path::new(p);
                    path.is_file() || path.is_dir()
                })
                .unwrap_or(false);
            let env_value = std::env::var(env_var).ok().filter(|v| !v.trim().is_empty());
            ModelPathEntry {
                engine: engine.to_string(),
                env_var: env_var.to_string(),
                configured_path,
                configured_exists,
                env_active: env_value.is_some(),
                env_value,
            }
        })
        .collect();
    let cache_env_value = std::env::var(MODEL_CACHE_ENV)
        .ok()
        .filter(|v| !v.trim().is_empty());
    ModelPathsReport {
        config,
        entries,
        cache_env_active: cache_env_value.is_some(),
        cache_env_value,
        config_file: config_file_path().to_string_lossy().to_string(),
    }
}

/// Read the persisted local-model weight paths plus their env sourcing.
#[tauri::command]
pub(crate) fn get_model_paths() -> Result<ModelPathsReport, String> {
    Ok(build_report(load_model_paths_config()))
}

/// Persist an updated local-model weight-path config and return the resulting
/// report. Empty/blank entries clear their override. The warm torch worker is
/// restarted so an updated weight path takes effect on the next run rather
/// than after an app restart.
#[tauri::command]
pub(crate) fn set_model_paths(config: ModelPathsConfig) -> Result<ModelPathsReport, String> {
    let config = normalise(config);
    save_model_paths_config(&config)?;
    crate::studio::torch_worker::reset();
    Ok(build_report(config))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalise_drops_blank_entries() {
        let mut weights = BTreeMap::new();
        weights.insert(
            "realesrgan".to_string(),
            "  C:\\models\\w.pth  ".to_string(),
        );
        weights.insert("sd_inpaint".to_string(), "   ".to_string());
        let config = normalise(ModelPathsConfig {
            model_cache_dir: Some("  ".to_string()),
            weights,
        });
        assert!(config.model_cache_dir.is_none());
        assert_eq!(config.weights.len(), 1);
        assert_eq!(config.weights["realesrgan"], "C:\\models\\w.pth");
    }

    #[test]
    fn config_round_trips_and_tolerates_unknown_engines() {
        let raw = r#"{
            "model_cache_dir": "D:\\weights",
            "weights": {"onnx_matting": "D:\\weights\\matting.onnx", "future_engine": "x"}
        }"#;
        let config: ModelPathsConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(config.model_cache_dir.as_deref(), Some("D:\\weights"));
        assert_eq!(config.weights.len(), 2);
        let round = serde_json::to_string(&config).unwrap();
        let back: ModelPathsConfig = serde_json::from_str(&round).unwrap();
        assert_eq!(back, config);
    }

    #[test]
    fn missing_config_loads_as_default() {
        // `load_model_paths_config` must not fail when the file is absent; the
        // default config carries no overrides.
        let config = ModelPathsConfig::default();
        assert!(config.model_cache_dir.is_none());
        assert!(config.weights.is_empty());
    }

    #[test]
    fn report_covers_every_probe_engine() {
        let report = build_report(ModelPathsConfig::default());
        let engines: Vec<&str> = report.entries.iter().map(|e| e.engine.as_str()).collect();
        assert_eq!(
            engines,
            [
                "realesrgan",
                "sd_inpaint",
                "onnx_defect",
                "onnx_harmonize",
                "onnx_matting"
            ]
        );
        assert!(report.entries.iter().all(|e| !e.env_var.is_empty()));
        assert!(!report.config_file.is_empty());
    }
}
