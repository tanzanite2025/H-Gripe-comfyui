#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use std::fs;
use std::path::PathBuf;

use hgripe_api::providers::custom_http::CustomHttpProvider;
use hgripe_api::providers::mock::MockProvider;
use hgripe_api::providers::openai_compatible::OpenAiCompatibleProvider;
use hgripe_api::providers::replicate::ReplicateProvider;
use hgripe_api::{
    apply_history_cleanup, build_doctor_report, build_rerun_task_from_record,
    credentials_file_path, get_history_detail, get_history_record, list_credential_summaries,
    list_provider_profile_summaries, plan_history_cleanup, provider_profiles_path,
    query_history_records, record_task_failure, record_task_result, validate_credentials,
    validate_provider_profiles, ApiBroker, ApiResult, ApiTask, CredentialSummary,
    CredentialsValidation, DoctorOptions, DoctorReport, HistoryCleanupOptions, HistoryCleanupPlan,
    HistoryCleanupResult, HistoryDetail, HistoryQuery, HistoryRecord, HistoryRerunOptions,
    ProviderProfileSummary, ProviderProfilesValidation, RuntimePaths,
};
use serde::Serialize;

fn broker() -> ApiBroker {
    let mut broker = ApiBroker::new();
    broker.register_provider(CustomHttpProvider::default());
    broker.register_provider(MockProvider);
    broker.register_provider(OpenAiCompatibleProvider::default());
    broker.register_provider(ReplicateProvider::default());
    broker
}

fn runtime_paths() -> Result<RuntimePaths, String> {
    RuntimePaths::from_env().map_err(|err| err.to_string())
}

fn config_path(kind: &str) -> Result<PathBuf, String> {
    match kind {
        "credentials" => Ok(credentials_file_path(None)),
        "profiles" => Ok(provider_profiles_path(None)),
        other => Err(format!("unknown config kind: {other}")),
    }
}

#[derive(Serialize)]
struct PathInfo {
    path: String,
    exists: bool,
}

impl PathInfo {
    fn new(path: PathBuf) -> Self {
        Self {
            exists: path.exists(),
            path: path.to_string_lossy().to_string(),
        }
    }
}

#[derive(Serialize)]
struct RuntimeInfo {
    providers: Vec<String>,
    credentials_file: PathInfo,
    profiles_file: PathInfo,
    history_file: PathInfo,
    history_db: PathInfo,
    output_dir: PathInfo,
}

#[tauri::command]
fn get_runtime_info() -> Result<RuntimeInfo, String> {
    let paths = runtime_paths()?;
    Ok(RuntimeInfo {
        providers: broker().providers(),
        credentials_file: PathInfo::new(credentials_file_path(None)),
        profiles_file: PathInfo::new(provider_profiles_path(None)),
        history_file: PathInfo::new(paths.history_file),
        history_db: PathInfo::new(paths.history_db),
        output_dir: PathInfo::new(paths.output_dir),
    })
}

#[tauri::command]
fn doctor() -> Result<DoctorReport, String> {
    build_doctor_report(DoctorOptions::default()).map_err(|err| err.to_string())
}

#[tauri::command]
fn get_credentials() -> Result<Vec<CredentialSummary>, String> {
    list_credential_summaries(None).map_err(|err| err.to_string())
}

#[tauri::command]
fn check_credentials() -> Result<CredentialsValidation, String> {
    validate_credentials(None).map_err(|err| err.to_string())
}

#[tauri::command]
fn get_profiles() -> Result<Vec<ProviderProfileSummary>, String> {
    list_provider_profile_summaries(None).map_err(|err| err.to_string())
}

#[tauri::command]
fn check_profiles() -> Result<ProviderProfilesValidation, String> {
    validate_provider_profiles(None).map_err(|err| err.to_string())
}

#[tauri::command]
fn read_config_file(kind: String) -> Result<String, String> {
    let path = config_path(&kind)?;
    if !path.exists() {
        return Ok(String::new());
    }
    fs::read_to_string(&path).map_err(|err| format!("failed to read {}: {err}", path.display()))
}

#[tauri::command]
fn write_config_file(kind: String, content: String) -> Result<(), String> {
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

#[tauri::command]
fn list_history(query: HistoryQuery) -> Result<Vec<HistoryRecord>, String> {
    let paths = runtime_paths()?;
    query_history_records(&paths.history_db, query).map_err(|err| err.to_string())
}

#[tauri::command]
fn history_detail(task_id: String) -> Result<Option<HistoryDetail>, String> {
    let paths = runtime_paths()?;
    get_history_detail(&paths.history_db, &task_id).map_err(|err| err.to_string())
}

#[tauri::command]
fn history_cleanup_preview(options: HistoryCleanupOptions) -> Result<HistoryCleanupPlan, String> {
    let paths = runtime_paths()?;
    plan_history_cleanup(&paths.history_db, &options).map_err(|err| err.to_string())
}

#[tauri::command]
fn history_cleanup_apply(options: HistoryCleanupOptions) -> Result<HistoryCleanupResult, String> {
    let paths = runtime_paths()?;
    apply_history_cleanup(&paths.history_db, &paths.history_file, &options)
        .map_err(|err| err.to_string())
}

async fn execute_and_record(task: ApiTask) -> Result<ApiResult, String> {
    let history_task = task.clone();
    match broker().execute(task).await {
        Ok(result) => {
            let _ = record_task_result(&history_task, &result);
            Ok(result)
        }
        Err(err) => {
            let message = err.to_string();
            let _ = record_task_failure(&history_task, message.clone());
            Err(message)
        }
    }
}

#[tauri::command]
async fn run_task(task: ApiTask) -> Result<ApiResult, String> {
    execute_and_record(task).await
}

#[tauri::command]
async fn run_task_json(task_json: String) -> Result<ApiResult, String> {
    let task: ApiTask =
        serde_json::from_str(&task_json).map_err(|err| format!("invalid ApiTask JSON: {err}"))?;
    execute_and_record(task).await
}

#[tauri::command]
async fn rerun_task(task_id: String, disable_cache: bool) -> Result<ApiResult, String> {
    let paths = runtime_paths()?;
    let record = get_history_record(&paths.history_db, &task_id)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("no history record for task {task_id}"))?;
    let options = HistoryRerunOptions {
        new_task_id: None,
        disable_cache,
    };
    let task = build_rerun_task_from_record(&record, options).map_err(|err| err.to_string())?;
    execute_and_record(task).await
}

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("only http(s) URLs are allowed".to_string());
    }
    open_external(&url)
}

#[cfg(target_os = "windows")]
fn open_external(url: &str) -> Result<(), String> {
    std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn()
        .map(|_| ())
        .map_err(|err| err.to_string())
}

#[cfg(target_os = "macos")]
fn open_external(url: &str) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|err| err.to_string())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_external(url: &str) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|err| err.to_string())
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_runtime_info,
            doctor,
            get_credentials,
            check_credentials,
            get_profiles,
            check_profiles,
            read_config_file,
            write_config_file,
            list_history,
            history_detail,
            history_cleanup_preview,
            history_cleanup_apply,
            run_task,
            run_task_json,
            rerun_task,
            open_url
        ])
        .run(tauri::generate_context!())
        .expect("error while running H-Gripe Desktop");
}
