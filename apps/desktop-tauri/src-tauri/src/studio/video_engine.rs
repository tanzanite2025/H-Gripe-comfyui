//! Media engine: the decoder seam + frame cache + playback/seek thread for the
//! clip editor (`Media` lane, step 5 of `docs/cards/editor-resource-model.md`).
//!
//! This is the Rust foundation the manual video editor will sit on. Three
//! pieces, each independent of the GPU `Semaphore(1)` compute queue so a scrub
//! never stalls on an inference job (and vice-versa):
//!
//! * [`FrameSource`] — the **decoder seam**. Any backend that can probe a clip
//!   and render a frame at a timestamp fits behind it. The first impl,
//!   [`PyAvFrameSource`], delegates to the long-lived PyAV worker
//!   ([`super::video_worker`]); a native-Rust ffmpeg decoder can replace it
//!   later without touching the engine or the cache.
//! * [`super::frame_cache::FrameCache`] — a small LRU of recently decoded frame
//!   PNGs, so scrubbing back over a timestamp is a cache hit, not a re-decode.
//! * [`PlaybackEngine`] — a **dedicated decode thread** fed by a channel. Seek
//!   requests are *latest-wins*: while the thread is busy decoding, queued older
//!   positions are superseded by the newest (the playhead has moved on), so the
//!   preview keeps up with a fast drag instead of grinding through every stale
//!   position.
//!
//! The engine only produces frame *paths* (PNGs the video card already renders
//! through the thumbnail pipeline); it does not itself paint, so it stays off
//! the UI thread entirely. Any decoder failure surfaces as `Err` to the caller,
//! which falls back to the one-shot `video_probe_cli.py`.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use serde::Deserialize;
use serde_json::json;

use super::frame_cache::{frame_key, FrameCache};

/// How many decoded frames the playback thread keeps warm. Sized for scrubbing
/// a short neighbourhood of the playhead back and forth without re-decoding.
const SCRUB_CACHE_FRAMES: usize = 24;

/// Metadata about a probed clip (mirrors `video_worker`'s `probe` payload).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct VideoMeta {
    #[serde(default)]
    pub(crate) width: u32,
    #[serde(default)]
    pub(crate) height: u32,
    #[serde(default)]
    pub(crate) duration_sec: Option<f64>,
    #[serde(default)]
    pub(crate) fps: Option<f64>,
    #[serde(default)]
    pub(crate) codec: Option<String>,
}

/// A decoder backend: probe a clip, render one frame at a timestamp to a PNG.
///
/// This is the pluggable seam of the media engine. `Send` so a boxed source can
/// live on the playback thread; not `Sync` because a decoder holds mutable
/// per-file state (the open container) and is only touched from that one thread.
pub(crate) trait FrameSource: Send {
    /// Read a clip's metadata (resolution, duration, fps, codec).
    fn probe(&mut self, video: &Path) -> Result<VideoMeta, String>;
    /// Decode the frame nearest `timestamp_sec`, writing it to `poster_out`.
    /// Returns the on-disk path actually written (normally `poster_out`).
    fn decode_frame(
        &mut self,
        video: &Path,
        timestamp_sec: f64,
        poster_out: &Path,
    ) -> Result<PathBuf, String>;
}

/// [`FrameSource`] backed by the long-lived PyAV worker. Holds the `(python,
/// dir)` context the worker needs; the worker itself keeps the ffmpeg container
/// open across calls, so this struct is a thin request builder.
pub(crate) struct PyAvFrameSource {
    python: PathBuf,
    dir: PathBuf,
}

impl PyAvFrameSource {
    pub(crate) fn new(python: PathBuf, dir: PathBuf) -> Self {
        Self { python, dir }
    }
}

/// Shape of the worker's `frame` payload; only the written path is needed here.
#[derive(Debug, Deserialize)]
struct FramePayload {
    #[serde(default)]
    poster_path: String,
}

impl FrameSource for PyAvFrameSource {
    fn probe(&mut self, video: &Path) -> Result<VideoMeta, String> {
        let args = json!({ "video": video.to_string_lossy() });
        let stdout = super::video_worker::run(&self.python, &self.dir, "probe", &args)?;
        serde_json::from_str::<VideoMeta>(stdout.trim())
            .map_err(|err| format!("could not parse video probe: {err} (raw: {})", stdout.trim()))
    }

    fn decode_frame(
        &mut self,
        video: &Path,
        timestamp_sec: f64,
        poster_out: &Path,
    ) -> Result<PathBuf, String> {
        let args = json!({
            "video": video.to_string_lossy(),
            "timestamp": timestamp_sec,
            "poster_out": poster_out.to_string_lossy(),
        });
        let stdout = super::video_worker::run(&self.python, &self.dir, "frame", &args)?;
        let payload: FramePayload = serde_json::from_str(stdout.trim())
            .map_err(|err| format!("could not parse video frame: {err} (raw: {})", stdout.trim()))?;
        if payload.poster_path.is_empty() {
            return Ok(poster_out.to_path_buf());
        }
        Ok(PathBuf::from(payload.poster_path))
    }
}

/// Build the decoder backend for `(python, dir)`.
///
/// Default build: the PyAV worker ([`PyAvFrameSource`]). With `native-ffmpeg`:
/// the in-process libav decoder, wrapped so a per-clip decode failure falls back
/// to the PyAV worker rather than erroring the scrub. Either way the returned
/// box is what the playback thread and the one-shot poster path decode through.
pub(crate) fn make_frame_source(python: &Path, dir: &Path) -> Box<dyn FrameSource> {
    #[cfg(feature = "native-ffmpeg")]
    {
        Box::new(super::ffmpeg_native::FfmpegWithPyAvFallback::new(
            python.to_path_buf(),
            dir.to_path_buf(),
        ))
    }
    #[cfg(not(feature = "native-ffmpeg"))]
    {
        Box::new(PyAvFrameSource::new(python.to_path_buf(), dir.to_path_buf()))
    }
}

/// Return the frame path for `timestamp_sec`, decoding + caching on a miss.
///
/// The cache key quantises the time to milliseconds, so two seeks to the same
/// position share a slot. On a miss the frame is decoded to `poster_dir` under a
/// key-derived name and inserted; an eviction just drops the map entry (the
/// poster files live in a project cache dir that is cleared wholesale).
pub(crate) fn resolve_frame(
    source: &mut dyn FrameSource,
    cache: &mut FrameCache,
    video: &Path,
    timestamp_sec: f64,
    poster_dir: &Path,
) -> Result<PathBuf, String> {
    let key = frame_key(timestamp_sec);
    if let Some(path) = cache.get(key) {
        return Ok(path.to_path_buf());
    }
    let poster_out = poster_dir.join(format!("scrub_{key}.png"));
    let written = source.decode_frame(video, timestamp_sec, &poster_out)?;
    cache.insert(key, written.clone());
    Ok(written)
}

/// One seek request handed to the playback thread, with a one-shot `reply`.
struct ScrubRequest {
    video: PathBuf,
    timestamp_sec: f64,
    poster_dir: PathBuf,
    reply: Sender<Result<PathBuf, String>>,
}

/// Collapse a burst of queued seeks to the newest (latest-wins).
///
/// While the decode thread was busy, any positions that piled up behind it are
/// stale — the playhead is wherever the *last* request points — so we keep only
/// that one and answer every skipped request with `superseded` so its caller
/// never blocks waiting for a frame that will never be decoded.
fn coalesce_latest(first: ScrubRequest, rx: &Receiver<ScrubRequest>) -> ScrubRequest {
    let mut newest = first;
    while let Ok(next) = rx.try_recv() {
        let stale = std::mem::replace(&mut newest, next);
        let _ = stale.reply.send(Err("superseded by a newer seek".to_string()));
    }
    newest
}

/// A dedicated decode thread + its warm frame cache. Pinned to the `(python,
/// dir)` its source was built for, so a project/interpreter change respawns it.
pub(crate) struct PlaybackEngine {
    python: PathBuf,
    dir: PathBuf,
    tx: Option<Sender<ScrubRequest>>,
    handle: Option<JoinHandle<()>>,
}

impl PlaybackEngine {
    /// Spawn the decode thread around `source`, keeping up to `cache_frames`
    /// decoded frames warm. `python`/`dir` are recorded only for [`matches`].
    fn spawn(
        source: Box<dyn FrameSource>,
        cache_frames: usize,
        python: PathBuf,
        dir: PathBuf,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<ScrubRequest>();
        let handle = std::thread::spawn(move || {
            let mut source = source;
            let mut cache = FrameCache::new(cache_frames);
            // Ends when every sender is dropped (engine dropped / respawned).
            while let Ok(req) = rx.recv() {
                let req = coalesce_latest(req, &rx);
                let result = resolve_frame(
                    source.as_mut(),
                    &mut cache,
                    &req.video,
                    req.timestamp_sec,
                    &req.poster_dir,
                );
                let _ = req.reply.send(result);
            }
        });
        Self {
            python,
            dir,
            tx: Some(tx),
            handle: Some(handle),
        }
    }

    fn matches(&self, python: &Path, dir: &Path) -> bool {
        self.python == python && self.dir == dir
    }

    /// Queue a seek and block until the decode thread answers. Returns the frame
    /// path, or `Err` if the frame was superseded, the decode failed, or the
    /// thread is gone.
    fn scrub_blocking(
        &self,
        video: PathBuf,
        timestamp_sec: f64,
        poster_dir: PathBuf,
    ) -> Result<PathBuf, String> {
        let tx = self
            .tx
            .as_ref()
            .ok_or_else(|| "playback engine stopped".to_string())?;
        let (reply, out) = mpsc::channel();
        tx.send(ScrubRequest {
            video,
            timestamp_sec,
            poster_dir,
            reply,
        })
        .map_err(|_| "playback engine stopped".to_string())?;
        out.recv()
            .map_err(|_| "playback engine dropped the request".to_string())?
    }
}

impl Drop for PlaybackEngine {
    fn drop(&mut self) {
        // Dropping the sender ends the thread's recv loop; then join it so we
        // never leak the decode thread when the engine is replaced or the app
        // ends.
        self.tx = None;
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

use std::sync::{Mutex, OnceLock};

/// The single process-global playback engine (its own lane, distinct from the
/// torch worker / GPU compute queue). `None` until the first scrub.
static ENGINE: OnceLock<Mutex<Option<PlaybackEngine>>> = OnceLock::new();

fn engine_cell() -> &'static Mutex<Option<PlaybackEngine>> {
    ENGINE.get_or_init(|| Mutex::new(None))
}

/// Scrub to `timestamp_sec` in `video` and return the decoded frame's path,
/// (re)spawning the playback engine for `(python, dir)` on demand and reusing
/// its warm frame cache across calls. Errors are the caller's cue to fall back
/// to the one-shot poster extraction.
pub(crate) fn scrub_frame(
    python: &Path,
    dir: &Path,
    poster_dir: &Path,
    video: &Path,
    timestamp_sec: f64,
) -> Result<PathBuf, String> {
    let mut guard = engine_cell()
        .lock()
        .map_err(|_| "playback engine mutex poisoned".to_string())?;
    if !guard.as_ref().is_some_and(|e| e.matches(python, dir)) {
        *guard = None; // drop (join) any engine bound to a different project
        let source = make_frame_source(python, dir);
        *guard = Some(PlaybackEngine::spawn(
            source,
            SCRUB_CACHE_FRAMES,
            python.to_path_buf(),
            dir.to_path_buf(),
        ));
    }
    guard
        .as_ref()
        .expect("engine present after spawn/match check")
        .scrub_blocking(video.to_path_buf(), timestamp_sec, poster_dir.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// A `FrameSource` that decodes nothing: it records how many decodes it was
    /// asked for and echoes a synthetic path, so cache behaviour is observable
    /// without ffmpeg or the filesystem.
    struct MockSource {
        decodes: Arc<AtomicUsize>,
    }

    impl FrameSource for MockSource {
        fn probe(&mut self, _video: &Path) -> Result<VideoMeta, String> {
            Ok(VideoMeta {
                width: 640,
                height: 480,
                duration_sec: Some(10.0),
                fps: Some(24.0),
                codec: Some("h264".into()),
            })
        }

        fn decode_frame(
            &mut self,
            _video: &Path,
            _timestamp_sec: f64,
            poster_out: &Path,
        ) -> Result<PathBuf, String> {
            self.decodes.fetch_add(1, Ordering::SeqCst);
            Ok(poster_out.to_path_buf())
        }
    }

    #[test]
    fn resolve_frame_decodes_on_miss_and_hits_cache() {
        let decodes = Arc::new(AtomicUsize::new(0));
        let mut source = MockSource {
            decodes: decodes.clone(),
        };
        let mut cache = FrameCache::new(8);
        let video = Path::new("clip.mp4");
        let dir = Path::new("/posters");

        let a = resolve_frame(&mut source, &mut cache, video, 1.0, dir).unwrap();
        let b = resolve_frame(&mut source, &mut cache, video, 1.0, dir).unwrap();
        assert_eq!(a, b);
        assert_eq!(a, PathBuf::from("/posters/scrub_1000.png"));
        // Second seek to the same time is a cache hit — only one decode.
        assert_eq!(decodes.load(Ordering::SeqCst), 1);

        resolve_frame(&mut source, &mut cache, video, 2.0, dir).unwrap();
        assert_eq!(decodes.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn coalesce_latest_supersedes_older_requests() {
        let (tx, rx) = mpsc::channel::<ScrubRequest>();
        let (r1, out1) = mpsc::channel();
        let (r2, out2) = mpsc::channel();
        let (r3, out3) = mpsc::channel();
        let mk = |ts: f64, reply: Sender<Result<PathBuf, String>>| ScrubRequest {
            video: PathBuf::from("clip.mp4"),
            timestamp_sec: ts,
            poster_dir: PathBuf::from("/posters"),
            reply,
        };
        tx.send(mk(2.0, r2)).unwrap();
        tx.send(mk(3.0, r3)).unwrap();

        let first = mk(1.0, r1);
        let newest = coalesce_latest(first, &rx);
        assert_eq!(newest.timestamp_sec, 3.0);
        // The two older ones were answered with a superseded error.
        assert!(out1.recv().unwrap().is_err());
        assert!(out2.recv().unwrap().is_err());
        // The newest was NOT answered by coalesce — the caller decodes it.
        assert!(out3.try_recv().is_err());
    }

    #[test]
    fn playback_engine_reuses_the_cache_across_scrubs() {
        let decodes = Arc::new(AtomicUsize::new(0));
        let source = Box::new(MockSource {
            decodes: decodes.clone(),
        });
        let engine = PlaybackEngine::spawn(
            source,
            8,
            PathBuf::from("python"),
            PathBuf::from("/proj"),
        );
        let video = PathBuf::from("clip.mp4");
        let posters = PathBuf::from("/posters");

        let p1 = engine
            .scrub_blocking(video.clone(), 1.0, posters.clone())
            .unwrap();
        let p2 = engine
            .scrub_blocking(video.clone(), 1.0, posters.clone())
            .unwrap();
        assert_eq!(p1, p2);
        assert_eq!(decodes.load(Ordering::SeqCst), 1);

        engine
            .scrub_blocking(video, 5.0, posters)
            .unwrap();
        assert_eq!(decodes.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn engine_matches_only_its_own_python_and_dir() {
        let engine = PlaybackEngine::spawn(
            Box::new(MockSource {
                decodes: Arc::new(AtomicUsize::new(0)),
            }),
            2,
            PathBuf::from("python"),
            PathBuf::from("/proj"),
        );
        assert!(engine.matches(Path::new("python"), Path::new("/proj")));
        assert!(!engine.matches(Path::new("python3"), Path::new("/proj")));
        assert!(!engine.matches(Path::new("python"), Path::new("/other")));
    }
}
