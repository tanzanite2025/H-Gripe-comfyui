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
mod resource;
mod studio;
mod thumb_cache;

use studio::{StudioRunCancels, StudioScheduler};

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

/// The `<img>`-native MIME for a format the webview can render directly from
/// its original bytes (no transcode). Anything else the decoder supports (TIFF,
/// …) is decoded and re-encoded to PNG for display instead.
fn browser_native_mime(format: image::ImageFormat) -> Option<&'static str> {
    match format {
        image::ImageFormat::Png => Some("image/png"),
        image::ImageFormat::Jpeg => Some("image/jpeg"),
        image::ImageFormat::WebP => Some("image/webp"),
        image::ImageFormat::Gif => Some("image/gif"),
        image::ImageFormat::Bmp => Some("image/bmp"),
        _ => None,
    }
}

/// Read an image file and return it as a `data:` URL for inline display. The
/// format is determined by *sniffing the header*, never the extension, so a
/// mislabelled or extension-less file still resolves and the accepted set stays
/// in lock-step with what the decoder can actually read. A browser-native
/// format is inlined byte-for-byte; any other decodable format (e.g. TIFF,
/// which `<img>` cannot render) is decoded and re-encoded to PNG so it still
/// displays.
#[tauri::command]
fn read_image_data_url(path: String) -> Result<String, String> {
    let path = Path::new(path.trim());
    // Guard against accidentally inlining huge files into the webview.
    let metadata =
        fs::metadata(path).map_err(|err| format!("failed to stat {}: {err}", path.display()))?;
    if metadata.len() > 25 * 1024 * 1024 {
        return Err("image is larger than 25 MB".to_string());
    }
    let bytes =
        fs::read(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;

    let format = image::guess_format(&bytes)
        .map_err(|_| format!("unsupported image type: {}", path.display()))?;

    if let Some(mime) = browser_native_mime(format) {
        return Ok(format!("data:{mime};base64,{}", base64_encode(&bytes)));
    }

    // Decodable but not `<img>`-native (TIFF, …): guard the decode size, then
    // decode + re-encode to PNG so the webview can show it.
    let reader = image::ImageReader::with_format(std::io::Cursor::new(&bytes), format);
    let (width, height) = reader
        .into_dimensions()
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    if u64::from(width) * u64::from(height) > MAX_PREVIEW_DECODE_PIXELS {
        return Err("image is too large to decode safely".to_string());
    }
    let decoded = image::load_from_memory_with_format(&bytes, format)
        .map_err(|err| format!("failed to decode {}: {err}", path.display()))?;
    let mut png: Vec<u8> = Vec::new();
    decoded
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|err| format!("failed to encode {}: {err}", path.display()))?;
    Ok(format!("data:image/png;base64,{}", base64_encode(&png)))
}

/// Decode-size ceiling for the [`read_image_data_url`] transcode path, aligned
/// with the compute lane's default budget (see `studio::studio_image`). Guards
/// a decompression bomb before the pixel buffer is allocated.
const MAX_PREVIEW_DECODE_PIXELS: u64 = 96_000_000;

/// Image pixel dimensions, read from the file header only (no full decode).
#[derive(Clone, Serialize)]
struct ImageDims {
    width: u32,
    height: u32,
}

/// Read an image's `width` x `height` from its header without decoding the
/// pixels (the shared core behind the [`probe_image_dims`] command and the
/// ingestion pipeline). Even a 4K/8K source resolves in microseconds because
/// only the header is parsed.
fn probe_image_dims_inner(path: &str) -> Result<ImageDims, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("path is empty".to_string());
    }
    let src = Path::new(trimmed);
    if !src.is_file() {
        return Err(format!("file does not exist: {trimmed}"));
    }
    let (width, height) = image::ImageReader::open(src)
        .map_err(|err| format!("failed to open {}: {err}", src.display()))?
        .with_guessed_format()
        .map_err(|err| format!("failed to read {}: {err}", src.display()))?
        .into_dimensions()
        .map_err(|err| format!("failed to read image dimensions: {err}"))?;
    Ok(ImageDims { width, height })
}

/// Read an image's `width` x `height` from its header. This is the fast first
/// phase of media-card ingestion: the info row can render `W×H` near-instantly
/// while the (much heavier) thumbnail decode runs separately.
#[tauri::command]
fn probe_image_dims(path: String) -> Result<ImageDims, String> {
    probe_image_dims_inner(&path)
}

/// In-memory thumbnail cache key: canonical path + target size + the source's
/// mtime and length, so editing or replacing the file invalidates its entry.
/// Returns `None` if the file's metadata cannot be read (the caller then just
/// skips the memory cache and takes the normal disk/decode path).
fn thumb_mem_key(src: &Path, target: u32) -> Option<String> {
    let meta = fs::metadata(src).ok()?;
    let len = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let canon = fs::canonicalize(src).unwrap_or_else(|_| src.to_path_buf());
    Some(format!("{}|{target}|{mtime}|{len}", canon.to_string_lossy()))
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

#[derive(Clone, Serialize)]
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
    generate_thumbnail_inner(&path, size, dpr)
}

/// Shared core behind the [`generate_thumbnail`] command and the ingestion
/// pipeline: memory-LRU → disk cache → decode+resize, populating both caches.
fn generate_thumbnail_inner(
    path: &str,
    size: u32,
    dpr: Option<f64>,
) -> Result<ThumbnailResult, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("path is empty".to_string());
    }
    let src = Path::new(trimmed);

    // Target edge in physical pixels, clamped to a sane range.
    let dpr = dpr.unwrap_or(1.0);
    let dpr = if dpr.is_finite() && dpr > 0.0 {
        dpr
    } else {
        1.0
    };
    let target = ((size as f64) * dpr).round() as u32;
    let target = target.clamp(16, 4096);

    // Fast path: an in-memory hit returns the finished thumbnail without reading
    // (let alone decoding) the source at all. Keyed by path + size + mtime/len,
    // so an edited source misses and regenerates.
    let mem_key = thumb_mem_key(src, target);
    if let Some(key) = &mem_key {
        if let Some(hit) = thumb_cache::get(key) {
            return Ok(ThumbnailResult {
                data_url: hit.data_url,
                cache_path: hit.cache_path,
                width: hit.width,
                height: hit.height,
                source_hash: hit.source_hash,
                mime: hit.mime,
            });
        }
    }

    // Buffer fast path: a compute card published this output's decoded surface,
    // so resize it directly — no re-read, no PNG re-decode. This is the display
    // half of the in-process buffer handoff (item 5, option 2): a compute
    // output's thumbnail comes from the buffer the card already produced, and it
    // works even if the file is absent (groundwork for dropping the producer
    // PNG write). The file is still written today, so a miss (never published /
    // evicted / stale) falls through to the disk decode below.
    if let Some(decoded) = studio::image_buffer::lookup_dynamic(src) {
        return finish_thumbnail_from_decoded(decoded, target, mem_key);
    }

    if !src.is_file() {
        return Err(format!("file does not exist: {trimmed}"));
    }
    let bytes = fs::read(src).map_err(|err| format!("failed to read {}: {err}", src.display()))?;
    let source_hash = fnv1a_hex(&bytes);

    let cache_dir = runtime_paths()?.output_dir.join(".thumbnails");
    fs::create_dir_all(&cache_dir)
        .map_err(|err| format!("failed to create {}: {err}", cache_dir.display()))?;
    let cache_path = cache_dir.join(format!("{source_hash}_{target}.png"));

    // Disk cache hit: reuse the previously generated thumbnail PNG.
    let (data_url, width, height) = if let Some((cached, decoded)) = fs::read(&cache_path)
        .ok()
        .and_then(|c| image::load_from_memory(&c).ok().map(|d| (c, d)))
    {
        let data_url = format!("data:image/png;base64,{}", base64_encode(&cached));
        (data_url, decoded.width(), decoded.height())
    } else {
        let source = image::load_from_memory(&bytes)
            .map_err(|err| format!("failed to decode image: {err}"))?;
        // `resize` preserves aspect ratio, fitting within target x target.
        let thumb = source.resize(target, target, image::imageops::FilterType::Lanczos3);

        let mut png: Vec<u8> = Vec::new();
        thumb
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|err| format!("failed to encode thumbnail: {err}"))?;
        // Best-effort cache write; a failure here should not fail the request.
        let _ = fs::write(&cache_path, &png);

        let data_url = format!("data:image/png;base64,{}", base64_encode(&png));
        (data_url, thumb.width(), thumb.height())
    };

    let cache_path = cache_path.to_string_lossy().to_string();
    let mime = "image/png".to_string();

    if let Some(key) = mem_key {
        thumb_cache::put(
            key,
            thumb_cache::CachedThumb {
                data_url: data_url.clone(),
                cache_path: cache_path.clone(),
                width,
                height,
                source_hash: source_hash.clone(),
                mime: mime.clone(),
            },
        );
    }

    Ok(ThumbnailResult {
        data_url,
        cache_path,
        width,
        height,
        source_hash,
        mime,
    })
}

/// Finish a thumbnail from an already-decoded surface (the buffer fast path in
/// [`generate_thumbnail_inner`]): resize, encode PNG, warm the disk + memory
/// caches, and build the [`ThumbnailResult`]. No source file is read, so the
/// disk cache is keyed by the thumbnail's own content hash rather than the
/// source file hash.
fn finish_thumbnail_from_decoded(
    decoded: image::DynamicImage,
    target: u32,
    mem_key: Option<String>,
) -> Result<ThumbnailResult, String> {
    // `resize` preserves aspect ratio, fitting within target x target.
    let thumb = decoded.resize(target, target, image::imageops::FilterType::Lanczos3);
    let mut png: Vec<u8> = Vec::new();
    thumb
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|err| format!("failed to encode thumbnail: {err}"))?;
    let source_hash = fnv1a_hex(&png);

    let cache_dir = runtime_paths()?.output_dir.join(".thumbnails");
    fs::create_dir_all(&cache_dir)
        .map_err(|err| format!("failed to create {}: {err}", cache_dir.display()))?;
    let cache_path = cache_dir.join(format!("{source_hash}_{target}.png"));
    // Best-effort cache write; a failure here should not fail the request.
    let _ = fs::write(&cache_path, &png);

    let data_url = format!("data:image/png;base64,{}", base64_encode(&png));
    let (width, height) = (thumb.width(), thumb.height());
    let cache_path = cache_path.to_string_lossy().to_string();
    let mime = "image/png".to_string();

    if let Some(key) = mem_key {
        thumb_cache::put(
            key,
            thumb_cache::CachedThumb {
                data_url: data_url.clone(),
                cache_path: cache_path.clone(),
                width,
                height,
                source_hash: source_hash.clone(),
                mime: mime.clone(),
            },
        );
    }

    Ok(ThumbnailResult {
        data_url,
        cache_path,
        width,
        height,
        source_hash,
        mime,
    })
}

/// A registered media resource handed to the webview: a stable [`resource`] id
/// plus the canonical path and header dims. Cards hold the `id` and pass it back
/// to [`resource_info`] / [`resource_thumbnail`] instead of shuttling the path
/// (and never the pixels) around — the heavy data stays in Rust.
#[derive(Clone, Serialize)]
struct ResourceRef {
    id: String,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<u32>,
}

/// Shared core behind [`register_resource`]: canonicalize `path`, derive its
/// stable id, probe header dims (best effort), and record it in the registry.
fn register_resource_inner(path: &str) -> Result<ResourceRef, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("path is empty".to_string());
    }
    let src = Path::new(trimmed);
    if !src.is_file() {
        return Err(format!("file does not exist: {trimmed}"));
    }
    let canonical = fs::canonicalize(src)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| trimmed.to_string());
    let id = resource::id_for(&canonical);
    // Header-only dims; a non-image (or unreadable) source just registers
    // without dimensions and the card falls back to its own probe.
    let (width, height) = match probe_image_dims_inner(trimmed) {
        Ok(d) => (Some(d.width), Some(d.height)),
        Err(_) => (None, None),
    };
    resource::put(
        &id,
        resource::ResourceEntry { path: canonical.clone(), width, height },
    );
    Ok(ResourceRef { id, path: canonical, width, height })
}

/// Register a dropped/selected media `path` and return its lightweight
/// [`ResourceRef`]. The id is stable across sessions (a hash of the canonical
/// path), so a card can re-register on project load and get the same handle
/// without any persisted mapping.
#[tauri::command]
fn register_resource(path: String) -> Result<ResourceRef, String> {
    register_resource_inner(&path)
}

/// Resolve a previously [`register_resource`]-ed id back to its
/// [`ResourceRef`], or error if the id was never registered this session.
#[tauri::command]
fn resource_info(id: String) -> Result<ResourceRef, String> {
    match resource::get(&id) {
        Some(entry) => Ok(ResourceRef {
            id,
            path: entry.path,
            width: entry.width,
            height: entry.height,
        }),
        None => Err(format!("unknown resource id: {id}")),
    }
}

/// Generate (or fetch from cache) a thumbnail for a registered resource id,
/// resolving the id to its path and reusing [`generate_thumbnail_inner`] so the
/// same disk + in-memory caches back both the id and path entry points.
#[tauri::command]
fn resource_thumbnail(id: String, size: u32, dpr: Option<f64>) -> Result<ThumbnailResult, String> {
    let entry = resource::get(&id).ok_or_else(|| format!("unknown resource id: {id}"))?;
    generate_thumbnail_inner(&entry.path, size, dpr)
}

/// Tauri event name for ingestion progress pushed by [`prime_ingest`].
const INGEST_EVENT: &str = "ingest://progress";

/// How many thumbnails the ingestion pipeline decodes at once. Header probes
/// are cheap and run unbounded, but decoding is CPU/RAM heavy, so a batch drop
/// of many 4K/8K sources warms the cache a few at a time instead of thrashing.
const INGEST_CONCURRENCY: usize = 3;

/// One ingestion progress message pushed to the webview. `phase` is
/// `"dims"` (header W×H known), `"thumb"` (thumbnail ready), or `"error"`;
/// the other fields are populated per phase.
#[derive(Clone, Serialize)]
struct IngestEvent {
    path: String,
    phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl IngestEvent {
    fn new(path: &str, phase: &str) -> Self {
        Self {
            path: path.to_string(),
            phase: phase.to_string(),
            width: None,
            height: None,
            data_url: None,
            cache_path: None,
            source_hash: None,
            mime: None,
            error: None,
        }
    }
}

/// Warm the media-card ingestion pipeline for freshly dropped image `paths`.
///
/// Returns immediately after spawning background work — nothing here blocks the
/// UI thread. For each path a task first probes header dimensions (cheap) and
/// pushes a `dims` [`IngestEvent`] so the card's info row renders `W×H` at once,
/// then (gated by a small [`INGEST_CONCURRENCY`] semaphore) generates the
/// thumbnail, populating the in-memory LRU + disk cache and pushing a `thumb`
/// event. Cards subscribe to `ingest://progress`; on a cache-warm hit their own
/// `generate_thumbnail` call then returns instantly. Non-image or unreadable
/// paths simply emit an `error` (or no `dims`) and the card falls back.
#[tauri::command]
async fn prime_ingest(
    app: tauri::AppHandle,
    paths: Vec<String>,
    size: u32,
    dpr: Option<f64>,
) -> Result<(), String> {
    use tauri::Emitter;

    let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(INGEST_CONCURRENCY));
    for path in paths {
        let app = app.clone();
        let gate = gate.clone();
        tokio::spawn(async move {
            // Phase 1: header-only dimensions. Skip the event for anything that
            // is not a readable image (the card keeps its placeholder).
            let dims = {
                let path = path.clone();
                tokio::task::spawn_blocking(move || probe_image_dims_inner(&path)).await
            };
            if let Ok(Ok(d)) = dims {
                let mut ev = IngestEvent::new(&path, "dims");
                ev.width = Some(d.width);
                ev.height = Some(d.height);
                let _ = app.emit(INGEST_EVENT, ev);
            }

            // Phase 2: thumbnail decode, bounded so a big batch cannot thrash.
            let _permit = gate.acquire_owned().await;
            let thumb = {
                let path = path.clone();
                tokio::task::spawn_blocking(move || generate_thumbnail_inner(&path, size, dpr)).await
            };
            match thumb {
                Ok(Ok(t)) => {
                    let mut ev = IngestEvent::new(&path, "thumb");
                    ev.width = Some(t.width);
                    ev.height = Some(t.height);
                    ev.data_url = Some(t.data_url);
                    ev.cache_path = Some(t.cache_path);
                    ev.source_hash = Some(t.source_hash);
                    ev.mime = Some(t.mime);
                    let _ = app.emit(INGEST_EVENT, ev);
                }
                Ok(Err(message)) => {
                    let mut ev = IngestEvent::new(&path, "error");
                    ev.error = Some(message);
                    let _ = app.emit(INGEST_EVENT, ev);
                }
                Err(join_err) => {
                    let mut ev = IngestEvent::new(&path, "error");
                    ev.error = Some(join_err.to_string());
                    let _ = app.emit(INGEST_EVENT, ev);
                }
            }
        });
    }
    Ok(())
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
        .manage(StudioScheduler::default())
        .setup(|app| {
            // Capture the bundled resource directory so the PSD nodes can fall
            // back to the `h-gripe.project.json` + `python/bridge` subtree
            // shipped via `bundle.resources` when running from a packaged
            // install.
            use tauri::Manager;
            let resource_dir = app.path().resource_dir().ok();
            psd::set_resource_dir(resource_dir.clone());
            // The auto-subject model is bundled under the same resource dir; the
            // handle-free `Compute` segmenter needs it captured here to resolve
            // the weight in a packaged install.
            studio::set_subject_model_resource_dir(resource_dir);
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
            probe_image_dims,
            prime_ingest,
            register_resource,
            resource_info,
            resource_thumbnail,
            read_text_file,
            open_path,
            psd::compose_psd,
            psd::inspect_psd,
            psd::analyze_psd_context,
            psd::match_light_color,
            psd::refine_mask_edge,
            psd::enhance_image,
            psd::detect_quality_issues,
            psd::probe_engines,
            psd::video_probe,
            psd::video_scrub,
            psd::prepare_repaint_regions,
            psd::local_repaint_regions,
            psd::composite_repaint
        ])
        .run(tauri::generate_context!())
        .expect("error while running H-Gripe Desktop");
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("hgripe_{tag}_{}_{nanos}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn browser_native_formats_are_inlined_verbatim() {
        let dir = tmp_dir("dataurl_png");
        let path = dir.join("scene.png");
        RgbaImage::from_pixel(4, 4, Rgba([10, 20, 30, 255]))
            .save(&path)
            .unwrap();
        let raw = fs::read(&path).unwrap();

        let url = read_image_data_url(path.to_string_lossy().to_string()).unwrap();
        assert!(url.starts_with("data:image/png;base64,"), "{url}");
        // A browser-native format is passed through byte-for-byte (no transcode).
        assert_eq!(url, format!("data:image/png;base64,{}", base64_encode(&raw)));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tiff_is_transcoded_to_png_for_display() {
        // The extension is deliberately `.tiff` (not browser-native): the header
        // is sniffed and the image is re-encoded to PNG so `<img>` can show it,
        // instead of being rejected as an "unsupported image type".
        let dir = tmp_dir("dataurl_tiff");
        let path = dir.join("scene.tiff");
        RgbaImage::from_pixel(4, 4, Rgba([200, 100, 50, 255]))
            .save(&path)
            .unwrap();
        assert_eq!(
            image::guess_format(&fs::read(&path).unwrap()).unwrap(),
            image::ImageFormat::Tiff
        );

        let url = read_image_data_url(path.to_string_lossy().to_string()).unwrap();
        assert!(
            url.starts_with("data:image/png;base64,"),
            "a TIFF must be transcoded to a PNG data URL, got: {}",
            &url[..url.len().min(40)]
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_non_image_file_is_rejected() {
        let dir = tmp_dir("dataurl_bogus");
        let path = dir.join("notes.txt");
        fs::write(&path, b"this is not an image").unwrap();

        let err = read_image_data_url(path.to_string_lossy().to_string()).unwrap_err();
        assert!(err.contains("unsupported image type"), "{err}");

        let _ = fs::remove_dir_all(&dir);
    }
}
