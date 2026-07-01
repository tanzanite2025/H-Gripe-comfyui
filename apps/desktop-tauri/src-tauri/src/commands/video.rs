//! Video card commands: probe a dropped clip for a poster + metadata, and
//! scrub to a timestamp for the manual clip editor. Both decode through the
//! shared [`crate::studio::video_engine`] `FrameSource` seam (PyAV worker, or
//! the native ffmpeg decoder under `native-ffmpeg`), falling back to the
//! one-shot `video_probe_cli.py` subprocess when the engine is unavailable.
//!
//! These are the desktop bridge surface only — the decode/cache logic lives in
//! `studio::video_engine`; this module just resolves the project interpreter,
//! picks the poster cache location, and shapes the TS-facing result.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::psd::{no_window, project_python, resolve_project_dir};

/// Metadata + poster-frame path for a dropped video, surfaced on the generic
/// video card. Fields are `snake_case` to match the TS `VideoProbeResult`.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct VideoProbeResult {
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// Clip length in seconds; `None` when the container reports none.
    pub(crate) duration_sec: Option<f64>,
    /// Frame rate; `None` when unknown rather than guessed.
    pub(crate) fps: Option<f64>,
    pub(crate) codec: Option<String>,
    /// On-disk PNG of the poster frame (rendered via the image thumbnail path).
    pub(crate) poster_path: String,
}

/// Shape of `video_probe_cli.py`'s stdout JSON. The poster path is decided by
/// Rust (the cache location), so the CLI only echoes the metadata back.
#[derive(Debug, Deserialize)]
struct VideoProbeCli {
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
    #[serde(default)]
    duration_sec: Option<f64>,
    #[serde(default)]
    fps: Option<f64>,
    #[serde(default)]
    codec: Option<String>,
}

/// Probe a dropped video and extract a poster frame for the video card.
///
/// Rust has no video decoder, so this shells out to the bundled Python's
/// `video_probe_cli.py` (PyAV, which ships ffmpeg) to read the metadata and
/// decode one frame to a cached PNG. The card then renders that PNG through the
/// existing `generate_thumbnail` pipeline, and the original `path` stays the
/// source of truth for the workflow. The poster is cached under the project
/// output dir keyed by `path + timestamp`.
#[tauri::command]
pub(crate) fn video_probe(
    path: String,
    timestamp: Option<f64>,
    dir: Option<String>,
) -> Result<VideoProbeResult, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("path is empty".to_string());
    }
    let video = Path::new(trimmed);
    if !video.is_file() {
        return Err(format!("file does not exist: {trimmed}"));
    }
    let dir = resolve_project_dir(&dir)?;
    let python = project_python(&dir);

    let ts = timestamp.unwrap_or(0.0).max(0.0);
    let poster_path = poster_cache_path(trimmed, ts)?;

    // Prefer the long-lived PyAV worker (the ffmpeg container stays open across
    // calls); fall back to the one-shot `video_probe_cli.py` if the worker is
    // unavailable or errors, so behaviour is identical to the pre-worker path.
    match video_probe_worker(&python, &dir, video, ts, &poster_path) {
        Ok(result) => Ok(result),
        Err(_) => video_probe_oneshot(&python, &dir, video, ts, &poster_path),
    }
}

/// The cached poster PNG path for a `(video, timestamp)` pair, under the project
/// output dir's `.posters` cache (created on demand). Keyed by `path + ts` so
/// re-probing the same frame reuses the file.
fn poster_cache_path(video_path: &str, ts: f64) -> Result<PathBuf, String> {
    use std::hash::{Hash, Hasher};
    let poster_dir = crate::cache_subdir(".posters")?;
    let key = format!("{video_path}|{}", (ts * 1000.0).round() as i64);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    Ok(poster_dir.join(format!("{:016x}.png", hasher.finish())))
}

/// Worker-backed probe: read metadata and decode the poster through the warm
/// PyAV worker (open container reused across calls), building the card result.
fn video_probe_worker(
    python: &Path,
    dir: &Path,
    video: &Path,
    ts: f64,
    poster_path: &Path,
) -> Result<VideoProbeResult, String> {
    use crate::studio::video_engine::FrameSource;
    let mut source = crate::studio::video_engine::make_frame_source(python, dir);
    let meta = source.probe(video)?;
    source.decode_frame(video, ts, poster_path)?;
    Ok(VideoProbeResult {
        width: meta.width,
        height: meta.height,
        duration_sec: meta.duration_sec,
        fps: meta.fps,
        codec: meta.codec,
        poster_path: poster_path.to_string_lossy().to_string(),
    })
}

/// One-shot fallback: the original per-call `video_probe_cli.py` subprocess that
/// reads metadata and decodes one poster frame. Behaviour is unchanged from the
/// pre-worker path.
fn video_probe_oneshot(
    python: &Path,
    dir: &Path,
    video: &Path,
    ts: f64,
    poster_path: &Path,
) -> Result<VideoProbeResult, String> {
    let script = dir
        .join("python")
        .join("bridge")
        .join("video_probe_cli.py");
    if !script.is_file() {
        return Err(format!("video_probe_cli.py not found at {}", script.display()));
    }
    let mut cmd = std::process::Command::new(python);
    cmd.arg(&script)
        .arg("--video")
        .arg(video)
        .arg("--poster-out")
        .arg(poster_path)
        .arg("--timestamp")
        .arg(format!("{ts}"))
        .current_dir(dir);
    no_window(&mut cmd);
    let output = cmd
        .output()
        .map_err(|err| format!("failed to launch {}: {err}", python.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("video probe failed: {}", stderr.trim()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: VideoProbeCli = serde_json::from_str(stdout.trim())
        .map_err(|err| format!("could not parse video probe: {err} (raw: {})", stdout.trim()))?;
    Ok(VideoProbeResult {
        width: parsed.width,
        height: parsed.height,
        duration_sec: parsed.duration_sec,
        fps: parsed.fps,
        codec: parsed.codec,
        poster_path: poster_path.to_string_lossy().to_string(),
    })
}

/// Scrub to `timestamp` in a video and return the decoded frame's poster path,
/// reusing the media engine's dedicated decode thread + warm frame cache
/// ([`crate::studio::video_engine`]) so repeated seeks over the same
/// neighbourhood are cache hits rather than re-decodes. Falls back to a one-shot
/// poster extraction when the engine/worker is unavailable. This backs the
/// manual clip editor's timeline scrubbing (Media lane, step 5).
#[tauri::command]
pub(crate) fn video_scrub(
    path: String,
    timestamp: f64,
    dir: Option<String>,
) -> Result<String, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("path is empty".to_string());
    }
    let video = Path::new(trimmed);
    if !video.is_file() {
        return Err(format!("file does not exist: {trimmed}"));
    }
    let dir = resolve_project_dir(&dir)?;
    let python = project_python(&dir);
    let ts = timestamp.max(0.0);
    let poster_dir = crate::cache_subdir(".posters")?;

    match crate::studio::video_engine::scrub_frame(&python, &dir, &poster_dir, video, ts) {
        Ok(frame) => Ok(frame.to_string_lossy().to_string()),
        Err(_) => {
            let poster_path = poster_cache_path(trimmed, ts)?;
            video_probe_oneshot(&python, &dir, video, ts, &poster_path).map(|r| r.poster_path)
        }
    }
}
