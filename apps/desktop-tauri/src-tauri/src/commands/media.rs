//! Image-media bridge commands: inline preview data URLs, header dimension
//! probes, thumbnail generation, the session resource registry, and the batch
//! ingestion pipeline for freshly dropped files. The thumbnail/base64
//! infrastructure lives in [`super::thumbnails`]. All the heavy pixel work
//! stays in Rust; the webview only ever gets `data:` URLs, dimensions, and
//! stable resource ids.

use std::path::Path;

use serde::Serialize;

use super::thumbnails::{base64_encode, generate_thumbnail_inner, ThumbnailResult};
use crate::resource;

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

/// Decode-size ceiling for the [`read_image_data_url`] transcode path, aligned
/// with the compute lane's default budget (see `studio::studio_image`). Guards
/// a decompression bomb before the pixel buffer is allocated.
const MAX_PREVIEW_DECODE_PIXELS: u64 = 96_000_000;

/// Read an image file and return it as a `data:` URL for inline display. The
/// format is determined by *sniffing the header*, never the extension, so a
/// mislabelled or extension-less file still resolves and the accepted set stays
/// in lock-step with what the decoder can actually read. A browser-native
/// format is inlined byte-for-byte; any other decodable format (e.g. TIFF,
/// which `<img>` cannot render) is decoded and re-encoded to PNG so it still
/// displays.
#[tauri::command]
pub(crate) fn read_image_data_url(path: String) -> Result<String, String> {
    let path = Path::new(path.trim());
    // Guard against accidentally inlining huge files into the webview.
    let metadata = std::fs::metadata(path)
        .map_err(|err| format!("failed to stat {}: {err}", path.display()))?;
    if metadata.len() > 25 * 1024 * 1024 {
        return Err("image is larger than 25 MB".to_string());
    }
    let bytes =
        std::fs::read(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;

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

/// Image pixel dimensions, read from the file header only (no full decode).
#[derive(Clone, Serialize)]
pub(crate) struct ImageDims {
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
pub(crate) fn probe_image_dims(path: String) -> Result<ImageDims, String> {
    probe_image_dims_inner(&path)
}

/// Generate (or fetch from cache) a crisp thumbnail for an image file.
///
/// The thumbnail is produced at `size * dpr` pixels with Lanczos3 resampling so
/// it stays sharp on high-DPI displays, cached on disk keyed by
/// `source_hash + target_size`, and returned as a `data:` URL for display. The
/// original `path` is never downscaled in the webview and remains the source of
/// truth for execution/export.
#[tauri::command]
pub(crate) fn generate_thumbnail(
    path: String,
    size: u32,
    dpr: Option<f64>,
) -> Result<ThumbnailResult, String> {
    generate_thumbnail_inner(&path, size, dpr)
}

/// A registered media resource handed to the webview: a stable [`resource`] id
/// plus the canonical path and header dims. Cards hold the `id` and pass it back
/// to [`resource_info`] / [`resource_thumbnail`] instead of shuttling the path
/// (and never the pixels) around — the heavy data stays in Rust.
#[derive(Clone, Serialize)]
pub(crate) struct ResourceRef {
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
    let canonical = std::fs::canonicalize(src)
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
        resource::ResourceEntry {
            path: canonical.clone(),
            width,
            height,
        },
    );
    Ok(ResourceRef {
        id,
        path: canonical,
        width,
        height,
    })
}

/// Register a dropped/selected media `path` and return its lightweight
/// [`ResourceRef`]. The id is stable across sessions (a hash of the canonical
/// path), so a card can re-register on project load and get the same handle
/// without any persisted mapping.
#[tauri::command]
pub(crate) fn register_resource(path: String) -> Result<ResourceRef, String> {
    register_resource_inner(&path)
}

/// Resolve a previously [`register_resource`]-ed id back to its
/// [`ResourceRef`], or error if the id was never registered this session.
#[tauri::command]
pub(crate) fn resource_info(id: String) -> Result<ResourceRef, String> {
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
pub(crate) fn resource_thumbnail(
    id: String,
    size: u32,
    dpr: Option<f64>,
) -> Result<ThumbnailResult, String> {
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
pub(crate) async fn prime_ingest(
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
                tokio::task::spawn_blocking(move || generate_thumbnail_inner(&path, size, dpr))
                    .await
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
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn browser_native_formats_are_inlined_verbatim() {
        let dir = tmp_dir("dataurl_png");
        let path = dir.join("scene.png");
        RgbaImage::from_pixel(4, 4, Rgba([10, 20, 30, 255]))
            .save(&path)
            .unwrap();
        let raw = std::fs::read(&path).unwrap();

        let url = read_image_data_url(path.to_string_lossy().to_string()).unwrap();
        assert!(url.starts_with("data:image/png;base64,"), "{url}");
        // A browser-native format is passed through byte-for-byte (no transcode).
        assert_eq!(
            url,
            format!("data:image/png;base64,{}", base64_encode(&raw))
        );

        let _ = std::fs::remove_dir_all(&dir);
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
            image::guess_format(&std::fs::read(&path).unwrap()).unwrap(),
            image::ImageFormat::Tiff
        );

        let url = read_image_data_url(path.to_string_lossy().to_string()).unwrap();
        assert!(
            url.starts_with("data:image/png;base64,"),
            "a TIFF must be transcoded to a PNG data URL, got: {}",
            &url[..url.len().min(40)]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_non_image_file_is_rejected() {
        let dir = tmp_dir("dataurl_bogus");
        let path = dir.join("notes.txt");
        std::fs::write(&path, b"this is not an image").unwrap();

        let err = read_image_data_url(path.to_string_lossy().to_string()).unwrap_err();
        assert!(err.contains("unsupported image type"), "{err}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
