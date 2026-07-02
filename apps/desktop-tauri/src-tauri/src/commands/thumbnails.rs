//! Thumbnail infrastructure shared by the media commands: base64 encoding,
//! content hashing, cache keys, and the memory-LRU → disk cache → decode
//! pipeline. The Tauri command handlers (and the ingestion pipeline) live in
//! [`super::media`]; this module owns the pixel/cache work behind them.

use std::path::Path;

use serde::Serialize;

use crate::{studio, thumb_cache};

pub(crate) fn base64_encode(bytes: &[u8]) -> String {
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

/// In-memory thumbnail cache key: canonical path + target size + the source's
/// mtime and length, so editing or replacing the file invalidates its entry.
/// Returns `None` if the file's metadata cannot be read (the caller then just
/// skips the memory cache and takes the normal disk/decode path).
fn thumb_mem_key(src: &Path, target: u32) -> Option<String> {
    let meta = std::fs::metadata(src).ok()?;
    let len = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let canon = std::fs::canonicalize(src).unwrap_or_else(|_| src.to_path_buf());
    Some(format!(
        "{}|{target}|{mtime}|{len}",
        canon.to_string_lossy()
    ))
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
pub(crate) struct ThumbnailResult {
    /// `data:` URL of the generated thumbnail, ready for an `<img src>`.
    pub(crate) data_url: String,
    /// On-disk cached thumbnail path (PNG), reused on subsequent calls.
    pub(crate) cache_path: String,
    /// Thumbnail pixel dimensions (already scaled by dpr).
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// Content hash of the source file (the thumbnail cache key).
    pub(crate) source_hash: String,
    pub(crate) mime: String,
}

/// Shared core behind the `generate_thumbnail` command and the ingestion
/// pipeline: memory-LRU → disk cache → decode+resize, populating both caches.
pub(crate) fn generate_thumbnail_inner(
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
    let bytes =
        std::fs::read(src).map_err(|err| format!("failed to read {}: {err}", src.display()))?;
    let source_hash = fnv1a_hex(&bytes);

    let cache_dir = crate::cache_subdir(".thumbnails")?;
    let cache_path = cache_dir.join(format!("{source_hash}_{target}.png"));

    // Disk cache hit: reuse the previously generated thumbnail PNG.
    let (data_url, width, height) = if let Some((cached, decoded)) = std::fs::read(&cache_path)
        .ok()
        .and_then(|c| image::load_from_memory(&c).ok().map(|d| (c, d)))
    {
        let data_url = format!("data:image/png;base64,{}", base64_encode(&cached));
        (data_url, decoded.width(), decoded.height())
    } else {
        // Display decode: identical to a plain decode except that a 16-bit
        // ProPhoto manual output is colour-managed to sRGB for the thumbnail.
        let source = studio::studio_image::decode_display_from_memory(&bytes)?;
        // `resize` preserves aspect ratio, fitting within target x target.
        let thumb = source.resize(target, target, image::imageops::FilterType::Lanczos3);

        let mut png: Vec<u8> = Vec::new();
        thumb
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|err| format!("failed to encode thumbnail: {err}"))?;
        // Best-effort cache write; a failure here should not fail the request.
        let _ = std::fs::write(&cache_path, &png);

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

    let cache_dir = crate::cache_subdir(".thumbnails")?;
    let cache_path = cache_dir.join(format!("{source_hash}_{target}.png"));
    // Best-effort cache write; a failure here should not fail the request.
    let _ = std::fs::write(&cache_path, &png);

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
