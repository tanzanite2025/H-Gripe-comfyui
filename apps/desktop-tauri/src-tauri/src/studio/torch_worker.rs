//! Long-lived Python worker that keeps the torch models warm (`Compute` lane).
//!
//! The torch bridge CLIs (`image_enhance_cli.py` with `--engine realesrgan`,
//! `detail_repaint_cli.py repaint --engine sd_inpaint`) previously ran as a
//! fresh `python` subprocess per call, and each rebuilt its model from disk —
//! the ~64 MB Real-ESRGAN weight, the multi-GB Stable Diffusion inpaint
//! pipeline — on *every* invocation. This module spawns **one** persistent
//! worker (`python/bridge/torch_worker.py`) and keeps it alive for the life of
//! the process, sending it one request per torch call over piped stdin/stdout.
//! Because the worker process is long-lived, the Python backends' process-global
//! warm caches survive across requests, so those weights load once per
//! `(weight, device, precision)` instead of on every run. This is staged-rollout
//! step 4 of `docs/cards/editor-resource-model.md` ("torch long-lived Python
//! worker: Rust spawns and keeps it alive; replaces per-call subprocess + model
//! reload for realesrgan / sd_inpaint").
//!
//! The worker is guarded by a single `Mutex`, so torch calls are serialised
//! through it — one request is written and its response read before the next
//! begins. That matches the "don't run torch inference concurrently" rule and
//! the GPU `Semaphore(1)` policy from step 2. Like the ONNX warm pool it is a
//! plain process-global `static` (not Tauri managed state) so the handle-free
//! `psd.rs` commands can reach it.
//!
//! Failure is always recoverable: if the worker cannot be spawned, its pipe
//! breaks, or it dies, [`run_cli`] returns `Err` and the caller falls back to
//! the original one-shot subprocess. A broken worker is dropped and respawned
//! on the next call (with one in-call retry across a dead pipe), so a single
//! crash never wedges the feature.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::Deserialize;
use serde_json::json;

/// One request/response the worker understands. `stdout` carries the hosted
/// CLI's JSON (exactly what the one-shot subprocess would have printed) and
/// `code` its POSIX exit status; `ok` is `code == 0`.
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
/// a respawn (the worker's cwd is fixed at spawn, and relative image/output
/// paths resolve against it just as the one-shot's `current_dir` did).
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
    fn request(&mut self, cmd: &str, argv: &[String]) -> Result<WorkerResponse, String> {
        let id = next_request_id();
        let mut line = json!({ "id": id, "cmd": cmd, "argv": argv }).to_string();
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|_| self.stdin.flush())
            .map_err(|err| format!("torch worker write failed: {err}"))?;

        let mut resp_line = String::new();
        let read = self
            .stdout
            .read_line(&mut resp_line)
            .map_err(|err| format!("torch worker read failed: {err}"))?;
        if read == 0 {
            return Err("torch worker closed its stdout".to_string());
        }
        serde_json::from_str::<WorkerResponse>(resp_line.trim())
            .map_err(|err| format!("torch worker sent invalid json: {err} (raw: {})", resp_line.trim()))
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

/// The single warm worker (`None` until first use / after a crash).
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
    let script = dir.join("python").join("bridge").join("torch_worker.py");
    if !script.is_file() {
        return Err(format!("torch_worker.py not found at {}", script.display()));
    }
    Ok(script)
}

/// Spawn a fresh worker for `(python, dir)` with piped stdin/stdout. The child's
/// cwd is the project dir (matching the one-shot `current_dir`) and its stderr
/// is discarded — errors are reported per-request in the response `error`.
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
        .map_err(|err| format!("failed to launch torch worker {}: {err}", python.display()))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "torch worker stdin unavailable".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "torch worker stdout unavailable".to_string())?;
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

/// Run a torch bridge CLI through the warm worker and return its stdout (the
/// hosted CLI's JSON), keeping the models warm across calls.
///
/// `cmd` is the worker command (`"image_enhance"` or `"detail_repaint"`) and
/// `argv` the exact argument vector the one-shot CLI would have received (for
/// `detail_repaint` that includes the `prepare`/`repaint`/`composite`
/// subcommand as `argv[0]`).
///
/// Returns `Err` on two distinct conditions, both of which the caller treats as
/// "use the one-shot fallback": the worker infrastructure is unavailable (spawn
/// failed, pipe broke — retried once with a fresh worker), **or** the hosted CLI
/// itself exited non-zero (surfaced via its captured `error`). Either way the
/// authoritative one-shot subprocess then runs, so behaviour is unchanged when
/// the worker is absent and a genuine CLI error still surfaces to the user.
pub(crate) fn run_cli(python: &Path, dir: &Path, cmd: &str, argv: &[String]) -> Result<String, String> {
    let mut guard = worker_cell()
        .lock()
        .map_err(|_| "torch worker mutex poisoned".to_string())?;

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
        match worker.request(cmd, argv) {
            Ok(resp) if resp.ok => return Ok(resp.stdout),
            Ok(resp) => {
                // The CLI ran but failed (bad args, backend error). This is a
                // real error, not a transport fault, so surface it immediately
                // rather than respawning — a retry would fail identically.
                return Err(if resp.error.is_empty() {
                    format!("torch worker command {cmd} failed (exit {})", resp.code)
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
        assert!(err.contains("torch_worker.py not found"));
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
