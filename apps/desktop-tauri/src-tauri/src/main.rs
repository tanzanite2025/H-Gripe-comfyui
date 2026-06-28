#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use std::fs;
use std::path::{Path, PathBuf};

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

#[derive(Serialize)]
struct PsdOutputFile {
    /// Base name shared by the triplet (e.g. `final` for `final.psd`).
    name: String,
    psd_path: String,
    preview_path: Option<String>,
    metadata_path: Option<String>,
    /// PSD file modification time in milliseconds since the Unix epoch.
    modified_ms: Option<u64>,
    size_bytes: u64,
}

fn modified_ms(metadata: &fs::Metadata) -> Option<u64> {
    metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}

/// Scan a directory (non-recursively) for PSD exports produced by the PSD
/// nodes and group each `<base>.psd` with its `<base>_preview.png` and
/// `<base>_metadata.json` siblings when present.
#[tauri::command]
fn list_psd_outputs(dir: String) -> Result<Vec<PsdOutputFile>, String> {
    let dir = dir.trim();
    if dir.is_empty() {
        return Err("output directory is empty".to_string());
    }
    let path = Path::new(dir);
    if !path.is_dir() {
        return Err(format!("not a directory: {dir}"));
    }

    let mut outputs = Vec::new();
    for entry in
        fs::read_dir(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let psd_path = entry.path();
        let is_psd = psd_path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("psd"))
            .unwrap_or(false);
        if !is_psd {
            continue;
        }
        let base = match psd_path.file_stem().and_then(|s| s.to_str()) {
            Some(stem) => stem.to_string(),
            None => continue,
        };

        let sibling = |suffix: &str| {
            let candidate = path.join(format!("{base}{suffix}"));
            candidate
                .is_file()
                .then(|| candidate.to_string_lossy().to_string())
        };
        let preview_path = sibling("_preview.png");
        let metadata_path = sibling("_metadata.json");

        let metadata = entry.metadata().ok();
        outputs.push(PsdOutputFile {
            name: base,
            psd_path: psd_path.to_string_lossy().to_string(),
            preview_path,
            metadata_path,
            modified_ms: metadata.as_ref().and_then(modified_ms),
            size_bytes: metadata.as_ref().map(|m| m.len()).unwrap_or(0),
        });
    }

    // Newest first, falling back to name for stable ordering.
    outputs.sort_by(|a, b| {
        b.modified_ms
            .cmp(&a.modified_ms)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(outputs)
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
        let b2 = chunk.get(2).copied().unwrap_or(0) as usize;
        out.push(TABLE[b0 >> 2] as char);
        out.push(TABLE[((b0 & 0x03) << 4) | (b1 >> 4)] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((b1 & 0x0f) << 2) | (b2 >> 6)] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[b2 & 0x3f] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Read an image file and return it as a `data:` URL for inline display.
#[tauri::command]
fn read_image_data_url(path: String) -> Result<String, String> {
    let path = Path::new(path.trim());
    let mime = match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("bmp") => "image/bmp",
        other => return Err(format!("unsupported image type: {}", other.unwrap_or(""))),
    };
    // Guard against accidentally inlining huge files into the webview.
    let metadata =
        fs::metadata(path).map_err(|err| format!("failed to stat {}: {err}", path.display()))?;
    if metadata.len() > 25 * 1024 * 1024 {
        return Err("image is larger than 25 MB".to_string());
    }
    let bytes =
        fs::read(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    Ok(format!("data:{mime};base64,{}", base64_encode(&bytes)))
}

/// Read a text file, truncating to `max_bytes` so large files cannot freeze
/// the UI. A truncation marker is appended when the file is clipped.
#[tauri::command]
fn read_text_file(path: String, max_bytes: usize) -> Result<String, String> {
    let path = Path::new(path.trim());
    let bytes =
        fs::read(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let limit = if max_bytes == 0 {
        bytes.len()
    } else {
        max_bytes
    };
    if bytes.len() > limit {
        let mut end = limit;
        // Avoid slicing in the middle of a UTF-8 sequence.
        while end > 0 && (bytes[end] & 0xC0) == 0x80 {
            end -= 1;
        }
        let mut text = String::from_utf8_lossy(&bytes[..end]).to_string();
        text.push_str("\n… (truncated)");
        Ok(text)
    } else {
        Ok(String::from_utf8_lossy(&bytes).to_string())
    }
}

/// Open a local file or folder with the OS default handler.
#[tauri::command]
fn open_path(path: String) -> Result<(), String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("path is empty".to_string());
    }
    if !Path::new(trimmed).exists() {
        return Err(format!("path does not exist: {trimmed}"));
    }
    open_external(trimmed)
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
            open_url,
            list_psd_outputs,
            read_image_data_url,
            read_text_file,
            open_path
        ])
        .run(tauri::generate_context!())
        .expect("error while running H-Gripe Desktop");
}
