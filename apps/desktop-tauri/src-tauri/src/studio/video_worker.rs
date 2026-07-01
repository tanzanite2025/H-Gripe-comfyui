//! Long-lived Python video worker — the first backend behind the media
//! engine's decoder seam (`Media` lane, staged-rollout step 5 of
//! `docs/cards/editor-resource-model.md`).
//!
//! Rust has no video decoder, so probing a clip and pulling a scrub frame goes
//! through PyAV. The old path (`video_probe_cli.py`) spawned a fresh `python`
//! subprocess per poster and reopened+re-demuxed the file from the start every
//! time. This module instead spawns **one** persistent worker
//! (`python/bridge/video_worker.py`) and keeps it alive for the life of the
//! process, so the ffmpeg container/stream is opened once per file and reused
//! across every probe and seek — the warm-decode analogue of the torch worker
//! (step 4) and the ONNX pool (step 3).
//!
//! Crucially this is a **separate** worker and `Mutex` from the torch worker:
//! the resource model puts media playback/scrub on its own lane, independent of
//! the GPU `Semaphore(1)` compute queue, so a decode never queues behind an
//! inference job (and vice-versa). Requests to *this* worker are still
//! serialised (one process, one pipe), but they do not contend with compute.
//!
//! The transport mirrors the torch worker exactly — newline-delimited JSON,
//! one request/response per line — except the request carries a structured
//! `args` object instead of a CLI `argv`, and `stdout` carries the result
//! payload as a JSON string. Failure is always recoverable: a spawn failure,
//! broken pipe, or dead worker returns `Err` (retried once with a fresh worker)
//! and the caller falls back to the one-shot `video_probe_cli.py`.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::Deserialize;
use serde_json::{json, Value};

/// One response from the worker. `stdout` carries the result payload as a JSON
/// string (the same shape `video_probe_cli.py` prints); `ok` is `code == 0`.
#[derive(Debug, Deserialize)]
struct WorkerResponse {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    code: i64,
    #[serde(default)]
    stdout: String,
    #[serde(default)]
    error: String,
}

/// A running worker: the child process plus its framed stdin/stdout. Pinned to
/// the `(python, dir)` it was spawned for so a project/interpreter change forces
/// a respawn (its cwd is fixed at spawn, matching the one-shot's `current_dir`).
struct Worker {
    python: PathBuf,
    dir: PathBuf,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Worker {
    /// Send one request line and read its single response line. Any I/O or
    /// decode failure here means the worker/pipe is unusable and the caller
    /// should drop and (once) respawn it.
    fn request(&mut self, cmd: &str, args: &Value) -> Result<WorkerResponse, String> {
        let id = next_request_id();
        let mut line = json!({ "id": id, "cmd": cmd, "args": args }).to_string();
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|_| self.stdin.flush())
            .map_err(|err| format!("video worker write failed: {err}"))?;

        let mut resp_line = String::new();
        let read = self
            .stdout
            .read_line(&mut resp_line)
            .map_err(|err| format!("video worker read failed: {err}"))?;
        if read == 0 {
            return Err("video worker closed its stdout".to_string());
        }
        serde_json::from_str::<WorkerResponse>(resp_line.trim()).map_err(|err| {
            format!(
                "video worker sent invalid json: {err} (raw: {})",
                resp_line.trim()
            )
        })
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Best-effort graceful stop: ask the loop to exit, then reap the child
        // so we never leak a zombie when the worker is replaced or the app ends.
        if let Ok(mut line) = serde_json::to_string(&json!({ "cmd": "shutdown" })) {
            line.push('\n');
            let _ = self.stdin.write_all(line.as_bytes());
            let _ = self.stdin.flush();
        }
        let _ = self.child.wait();
    }
}

/// The single warm video worker (`None` until first use / after a crash). A
/// distinct static from the torch worker so decode never shares that lane.
static WORKER: OnceLock<Mutex<Option<Worker>>> = OnceLock::new();
/// Monotonic request ids, purely for correlating a response with its request.
static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

fn worker_cell() -> &'static Mutex<Option<Worker>> {
    WORKER.get_or_init(|| Mutex::new(None))
}

fn next_request_id() -> u64 {
    REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}

/// Resolve the worker script under a project's vendored bridge, erroring if the
/// helper is missing (older checkout / bundle) so the caller falls back.
fn worker_script(dir: &Path) -> Result<PathBuf, String> {
    let script = dir.join("python").join("bridge").join("video_worker.py");
    if !script.is_file() {
        return Err(format!("video_worker.py not found at {}", script.display()));
    }
    Ok(script)
}

/// Spawn a fresh worker for `(python, dir)` with piped stdin/stdout. The child's
/// cwd is the project dir and its stderr is discarded — errors are reported
/// per-request in the response `error`.
fn spawn(python: &Path, dir: &Path) -> Result<Worker, String> {
    let script = worker_script(dir)?;
    let mut cmd = Command::new(python);
    cmd.arg(&script)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: don't pop a console window for the child.
        cmd.creation_flags(0x0800_0000);
    }

    let mut child = cmd
        .spawn()
        .map_err(|err| format!("failed to launch video worker {}: {err}", python.display()))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "video worker stdin unavailable".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "video worker stdout unavailable".to_string())?;
    Ok(Worker {
        python: python.to_path_buf(),
        dir: dir.to_path_buf(),
        child,
        stdin,
        stdout: BufReader::new(stdout),
    })
}

/// Whether the current worker (if any) can serve this `(python, dir)`; a
/// mismatch means a different project/interpreter and forces a respawn.
fn matches(worker: &Option<Worker>, python: &Path, dir: &Path) -> bool {
    worker
        .as_ref()
        .is_some_and(|w| w.python == python && w.dir == dir)
}

/// Run one video-worker command and return its stdout (the result payload as a
/// JSON string), keeping the decoder warm across calls.
///
/// `cmd` is `"probe"` / `"frame"` / `"close"` and `args` the structured request
/// body (`{"video": ..}`, plus `timestamp`/`poster_out` for `frame`).
///
/// Returns `Err` on two distinct conditions, both of which the caller treats as
/// "use the one-shot fallback": the worker infrastructure is unavailable (spawn
/// failed, pipe broke — retried once with a fresh worker), **or** the worker
/// reported the request itself failed (surfaced via its captured `error`).
pub(crate) fn run(python: &Path, dir: &Path, cmd: &str, args: &Value) -> Result<String, String> {
    let mut guard = worker_cell()
        .lock()
        .map_err(|_| "video worker mutex poisoned".to_string())?;

    let mut last_transport_err = String::new();
    // Two attempts: the first may hit a stale/dead pipe from an earlier crash;
    // dropping it and respawning gives one clean retry before we give up.
    for _ in 0..2 {
        if !matches(&guard, python, dir) {
            *guard = None; // drop (and gracefully stop) any mismatched worker
            *guard = Some(spawn(python, dir)?);
        }

        let worker = guard
            .as_mut()
            .expect("worker present after spawn/match check");
        match worker.request(cmd, args) {
            Ok(resp) if resp.ok => return Ok(resp.stdout),
            Ok(resp) => {
                // The request ran but failed (bad file, missing arg). This is a
                // real error, not a transport fault, so surface it immediately
                // rather than respawning — a retry would fail identically.
                return Err(if resp.error.is_empty() {
                    format!("video worker command {cmd} failed (exit {})", resp.code)
                } else {
                    resp.error
                });
            }
            Err(transport) => {
                // Pipe broke / worker died: discard it so the next iteration
                // (or the next call) spawns a fresh one.
                *guard = None;
                last_transport_err = transport;
            }
        }
    }
    Err(last_transport_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn worker_script_errors_when_missing() {
        let dir = Path::new("Z:/definitely/missing-project");
        let err = worker_script(dir).unwrap_err();
        assert!(err.contains("video_worker.py not found"));
    }

    #[test]
    fn matches_is_false_without_a_worker() {
        let none: Option<Worker> = None;
        assert!(!matches(&none, Path::new("python"), Path::new("/proj")));
    }

    #[test]
    fn request_ids_are_monotonic() {
        let a = next_request_id();
        let b = next_request_id();
        assert!(b > a);
    }

    #[test]
    fn response_defaults_are_a_failure() {
        // A worker reply missing every field must decode as not-ok so a garbled
        // response is treated as a failure rather than a silent success.
        let resp: WorkerResponse = serde_json::from_str("{}").unwrap();
        assert!(!resp.ok);
        assert_eq!(resp.code, 0);
        assert!(resp.stdout.is_empty());
    }
}
