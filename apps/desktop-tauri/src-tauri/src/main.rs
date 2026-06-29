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

mod contracts;
mod psd;
mod studio;

use studio::StudioRunCancels;

pub(crate) fn broker() -> ApiBroker {
    let mut broker = ApiBroker::new();
    broker.register_provider(CustomHttpProvider::default());
    broker.register_provider(MockProvider);
    broker.register_provider(OpenAiCompatibleProvider::default());
    broker.register_provider(ReplicateProvider::default());
    broker
}

pub(crate) fn runtime_paths() -> Result<RuntimePaths, String> {
    RuntimePaths::from_env().map_err(|err| err.to_string())
}

fn config_path(kind: &str) -> Result<PathBuf, String> {
    match kind {
        "credentials" => Ok(credentials_file_path(None)),
        "profiles" => Ok(provider_profiles_path(None)),
        other => Err(format!("unknown config kind: {other}")),
    }
}

/// File modification time in milliseconds since the Unix epoch, if available.
pub(crate) fn modified_ms(metadata: &fs::Metadata) -> Option<u64> {
    metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
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

/// Open a native file-open dialog and return the chosen path, or `None` if the
/// user cancelled. `filter_name` + `extensions` optionally scope the picker
/// (e.g. images, or `.psd` templates); extensions are bare (no leading dot).
#[tauri::command]
fn pick_file(
    app: tauri::AppHandle,
    title: Option<String>,
    filter_name: Option<String>,
    extensions: Option<Vec<String>>,
) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let mut builder = app.dialog().file();
    if let Some(title) = title {
        builder = builder.set_title(title);
    }
    if let Some(exts) = extensions.as_ref().filter(|e| !e.is_empty()) {
        let refs: Vec<&str> = exts.iter().map(String::as_str).collect();
        builder = builder.add_filter(filter_name.unwrap_or_else(|| "Files".to_string()), &refs);
    }
    builder.blocking_pick_file().map(|path| path.to_string())
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

/// FNV-1a 64-bit hash, used to key the thumbnail cache by source content.
fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[derive(Serialize)]
struct ThumbnailResult {
    /// `data:` URL of the generated thumbnail, ready for an `<img src>`.
    data_url: String,
    /// On-disk cached thumbnail path (PNG), reused on subsequent calls.
    cache_path: String,
    /// Thumbnail pixel dimensions (already scaled by dpr).
    width: u32,
    height: u32,
    /// Content hash of the source file (the thumbnail cache key).
    source_hash: String,
    mime: String,
}

/// Generate (or fetch from cache) a crisp thumbnail for an image file.
///
/// The thumbnail is produced at `size * dpr` pixels with Lanczos3 resampling so
/// it stays sharp on high-DPI displays, cached on disk keyed by
/// `source_hash + target_size`, and returned as a `data:` URL for display. The
/// original `path` is never downscaled in the webview and remains the source of
/// truth for execution/export.
#[tauri::command]
fn generate_thumbnail(
    path: String,
    size: u32,
    dpr: Option<f64>,
) -> Result<ThumbnailResult, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("path is empty".to_string());
    }
    let src = Path::new(trimmed);
    if !src.is_file() {
        return Err(format!("file does not exist: {trimmed}"));
    }

    // Target edge in physical pixels, clamped to a sane range.
    let dpr = dpr.unwrap_or(1.0);
    let dpr = if dpr.is_finite() && dpr > 0.0 {
        dpr
    } else {
        1.0
    };
    let target = ((size as f64) * dpr).round() as u32;
    let target = target.clamp(16, 4096);

    let bytes = fs::read(src).map_err(|err| format!("failed to read {}: {err}", src.display()))?;
    let source_hash = fnv1a_hex(&bytes);

    let cache_dir = runtime_paths()?.output_dir.join(".thumbnails");
    fs::create_dir_all(&cache_dir)
        .map_err(|err| format!("failed to create {}: {err}", cache_dir.display()))?;
    let cache_path = cache_dir.join(format!("{source_hash}_{target}.png"));

    // Cache hit: reuse the previously generated thumbnail.
    if let Ok(cached) = fs::read(&cache_path) {
        if let Ok(decoded) = image::load_from_memory(&cached) {
            return Ok(ThumbnailResult {
                data_url: format!("data:image/png;base64,{}", base64_encode(&cached)),
                cache_path: cache_path.to_string_lossy().to_string(),
                width: decoded.width(),
                height: decoded.height(),
                source_hash,
                mime: "image/png".to_string(),
            });
        }
    }

    let source =
        image::load_from_memory(&bytes).map_err(|err| format!("failed to decode image: {err}"))?;
    // `resize` preserves aspect ratio, fitting within target x target.
    let thumb = source.resize(target, target, image::imageops::FilterType::Lanczos3);

    let mut png: Vec<u8> = Vec::new();
    thumb
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|err| format!("failed to encode thumbnail: {err}"))?;
    // Best-effort cache write; a failure here should not fail the request.
    let _ = fs::write(&cache_path, &png);

    Ok(ThumbnailResult {
        data_url: format!("data:image/png;base64,{}", base64_encode(&png)),
        cache_path: cache_path.to_string_lossy().to_string(),
        width: thumb.width(),
        height: thumb.height(),
        source_hash,
        mime: "image/png".to_string(),
    })
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

// NOTE: Long term this should move to the official `tauri-plugin-opener`
// (Tauri 2) so opening files/URLs goes through a vetted, permissioned path
// rather than spawning a child process here. Until then we invoke the OS
// handler directly without going through `cmd /C start`, whose shell re-parses
// metacharacters (`&`, `^`, `%`, …) in the target. `rundll32 url.dll,
// FileProtocolHandler` opens http(s) URLs, files, and folders via the default
// handler and receives the target as a single, un-reparsed argv element.
#[cfg(target_os = "windows")]
fn open_external(url: &str) -> Result<(), String> {
    std::process::Command::new("rundll32.exe")
        .args(["url.dll,FileProtocolHandler", url])
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
        .plugin(tauri_plugin_dialog::init())
        .manage(StudioRunCancels::default())
        .setup(|app| {
            // Capture the bundled resource directory so the PSD nodes can fall
            // back to the `main.py` + `python/bridge` subtree shipped via
            // `bundle.resources` when running from a packaged install.
            use tauri::Manager;
            psd::set_resource_dir(app.path().resource_dir().ok());
            Ok(())
        })
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
            studio::run_studio_graph,
            studio::read_studio_autosave,
            studio::write_studio_autosave,
            studio::clear_studio_autosave,
            studio::pick_workflow_save_path,
            studio::pick_workflow_open_path,
            studio::pick_project_folder,
            studio::read_studio_workflow,
            studio::write_studio_workflow,
            studio::list_studio_workflows,
            studio::rename_studio_workflow,
            studio::delete_studio_workflow,
            studio::duplicate_studio_workflow,
            studio::read_studio_snapshots,
            studio::write_studio_snapshots,
            studio::read_studio_run_history,
            studio::write_studio_run_history,
            studio::read_studio_recents,
            studio::write_studio_recents,
            studio::cancel_studio_run,
            rerun_task,
            open_url,
            pick_file,
            psd::list_psd_outputs,
            read_image_data_url,
            generate_thumbnail,
            read_text_file,
            open_path,
            psd::compose_psd,
            psd::inspect_psd,
            psd::analyze_psd_context,
            psd::match_light_color,
            psd::refine_mask_edge,
            psd::enhance_image,
            psd::detect_quality_issues,
            psd::prepare_repaint_regions,
            psd::composite_repaint
        ])
        .run(tauri::generate_context!())
        .expect("error while running H-Gripe Desktop");
}
