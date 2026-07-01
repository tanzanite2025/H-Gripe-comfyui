//! Process-global warm pool of ONNX Runtime sessions (`Compute` lane).
//!
//! The `Compute` segmenters (`subject_model`'s salient nets and `subject_sam2`)
//! previously rebuilt an `ort::Session` from the weight file on *every* run —
//! reading and re-parsing hundreds of megabytes (BiRefNet ~224 MB, the SAM 2
//! encoder ~134 MB) per invocation. This module keeps the parsed sessions warm:
//! the first load of a given weight path builds the session, every subsequent
//! request for the same path shares it. This is staged-rollout step 3 of
//! `docs/cards/editor-resource-model.md` ("cache `ort::Session` in a warm pool;
//! kill per-call model reload").
//!
//! Each session is wrapped in a `Mutex` because `Session::run` takes `&mut self`
//! (ort ≥ 2.0.0-rc.10 made concurrent runs unsound), so callers serialise their
//! inference through it — which matches both the ONNX Runtime team's guidance
//! against concurrent inference and the GPU `Semaphore(1)` policy from step 2.
//! The pool lives for the life of the process and, like `RESOURCE_DIR`, is a
//! plain `static` rather than Tauri managed state so the handle-free `Compute`
//! segmenters can reach it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use ort::session::Session;

/// A warm ONNX session shared across runs. `Mutex` because `Session::run` needs
/// `&mut self`; `Arc` so the pool and every in-flight segmenter share one copy.
pub(super) type SharedSession = Arc<Mutex<Session>>;

/// Weight path → warm session. Keyed by the canonicalised path so the same
/// weight resolved via different relative spellings maps to one session.
static POOL: OnceLock<Mutex<HashMap<PathBuf, SharedSession>>> = OnceLock::new();

fn pool() -> &'static Mutex<HashMap<PathBuf, SharedSession>> {
    POOL.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The cache key for a weight path: its canonical form when resolvable (so
/// distinct spellings of the same file collapse), otherwise the path as given.
fn cache_key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Build an `ort::Session` from an ONNX weight file on disk. Kept private so the
/// only way to obtain a session is through the warm pool.
fn build_session(path: &Path) -> Result<Session, String> {
    let bytes = std::fs::read(path)
        .map_err(|err| format!("failed to read onnx model {}: {err}", path.display()))?;
    Session::builder()
        .and_then(|mut builder| builder.commit_from_memory(&bytes))
        .map_err(|err| format!("failed to load onnx model {}: {err}", path.display()))
}

/// Get the warm session for `path`, building and caching it on first use. The
/// returned handle is shared: repeated calls for the same weight hand back the
/// same `Arc`, so the heavy weight parse happens once per process.
///
/// The weight file is read only on a cache miss, outside the pool lock, so a
/// slow first load of one model does not block sessions for others. A race that
/// builds the same session twice is resolved by keeping whichever landed first.
pub(super) fn cached_session(path: &Path) -> Result<SharedSession, String> {
    let key = cache_key(path);
    {
        let map = pool()
            .lock()
            .map_err(|_| "onnx session pool poisoned".to_string())?;
        if let Some(existing) = map.get(&key) {
            return Ok(existing.clone());
        }
    }

    let built: SharedSession = Arc::new(Mutex::new(build_session(path)?));
    let mut map = pool()
        .lock()
        .map_err(|_| "onnx session pool poisoned".to_string())?;
    // If another thread inserted while we were building, keep theirs so every
    // caller for this weight converges on a single shared session.
    Ok(map.entry(key).or_insert(built).clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_of_missing_path_is_unchanged() {
        // A path that can't be canonicalised (does not exist) is used verbatim,
        // so resolution never panics and unresolved weights still key sanely.
        let missing = Path::new("Z:/definitely/missing-model.onnx");
        assert_eq!(cache_key(missing), missing.to_path_buf());
    }
}
