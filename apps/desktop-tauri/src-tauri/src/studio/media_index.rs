//! Persistent media index/cache for Studio graph runs.
//!
//! The runner used to re-execute every node on every run, even when nothing
//! feeding a node had changed — re-running an expensive compute chain or, for
//! seeded `generate` nodes, re-billing the provider for a byte-identical image.
//! This module gives the runner a durable index of *what each node produced
//! under which exact conditions*, so an unchanged node can be served from the
//! previous run's media instead of executing again.
//!
//! **Fingerprint.** A node's cache key is the SHA-256 of a canonical JSON
//! fingerprint: the node `kind`, its `params`, its resolved input values, and —
//! for every input value that is a path to a real file — that file's
//! `(mtime, len)` stamp. Any change to a parameter, an upstream value, or the
//! bytes of an upstream media file changes the key.
//!
//! **Validation.** A hit is only served when every file-backed output recorded
//! at store time still exists with the same `(mtime, len)` stamp (mirroring the
//! thumbnail LRU / decoded-buffer freshness rule), so deleted or edited outputs
//! invalidate their own entry.
//!
//! **Policy.** Only deterministic, expensive lanes participate: `Compute` and
//! `Local` (python-bridge CLI) nodes are always cacheable; `Api` nodes are only
//! cacheable when the call is pinned by an explicit `seed` (a seeded generation
//! is a deterministic request — an unseeded one is a deliberate re-roll). A
//! boolean `cache` node param overrides both directions. `Graph` nodes are free
//! to recompute and `Hybrid` prompt optimisation already caches in the broker.
//!
//! The index itself is a bounded JSON file under the runtime output dir
//! (`.media-index/index.json`), pruned least-recently-used, and doubles as a
//! queryable record of the media the runner has produced.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::exec::StudioExecutor;
use super::graph::StudioGraphNode;
use super::node_registry::node_class;

/// Cap on retained entries; the least-recently-used entries are pruned first.
const MEDIA_INDEX_CAP: usize = 256;

const MEDIA_INDEX_DIR: &str = ".media-index";
const MEDIA_INDEX_FILE: &str = "index.json";

/// `(mtime, len)` stamp of a file-backed value, used both in fingerprints
/// (inputs) and in hit validation (outputs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FileStamp {
    pub path: String,
    pub mtime_ms: Option<u64>,
    pub len: u64,
}

impl FileStamp {
    fn capture(path: &Path) -> Option<Self> {
        let metadata = fs::metadata(path).ok()?;
        if !metadata.is_file() {
            return None;
        }
        Some(Self {
            path: path.to_string_lossy().to_string(),
            mtime_ms: crate::modified_ms(&metadata),
            len: metadata.len(),
        })
    }

    fn still_fresh(&self) -> bool {
        FileStamp::capture(Path::new(&self.path)).as_ref() == Some(self)
    }
}

/// One cached node result: the exact output map the node produced plus the
/// stamps of every file-backed output for hit validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct MediaIndexEntry {
    pub node_kind: String,
    pub outputs: BTreeMap<String, Value>,
    pub files: Vec<FileStamp>,
    pub created_ms: u64,
    pub last_used_ms: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct MediaIndexData {
    entries: BTreeMap<String, MediaIndexEntry>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn index_path() -> Result<PathBuf, String> {
    Ok(crate::cache_subdir(MEDIA_INDEX_DIR)?.join(MEDIA_INDEX_FILE))
}

fn load_data(path: &Path) -> MediaIndexData {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_data(path: &Path, data: &MediaIndexData) {
    if let Ok(text) = serde_json::to_string(data) {
        let _ = fs::write(path, text);
    }
}

fn prune_lru(data: &mut MediaIndexData, cap: usize) {
    while data.entries.len() > cap {
        let oldest = data
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.last_used_ms)
            .map(|(key, _)| key.clone());
        match oldest {
            Some(key) => {
                data.entries.remove(&key);
            }
            None => break,
        }
    }
}

fn index_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// True when an explicit truthy/falsy `cache` param is present.
fn cache_param(node: &StudioGraphNode) -> Option<bool> {
    match node.params.get("cache") {
        Some(Value::Bool(flag)) => Some(*flag),
        Some(Value::String(text)) => match text.trim() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn has_seed(node: &StudioGraphNode, inputs: &BTreeMap<String, Value>) -> bool {
    let non_empty = |value: &Value| match value {
        Value::Null => false,
        Value::String(text) => !text.trim().is_empty(),
        _ => true,
    };
    inputs.get("seed").map(non_empty).unwrap_or(false)
        || node.params.get("seed").map(non_empty).unwrap_or(false)
}

/// Whether this node participates in the media cache (see module policy).
fn is_cacheable(node: &StudioGraphNode, inputs: &BTreeMap<String, Value>) -> bool {
    if let Some(flag) = cache_param(node) {
        return flag;
    }
    match node_class(&node.kind).map(|class| class.executor) {
        Some(StudioExecutor::Compute) | Some(StudioExecutor::Local) => true,
        Some(StudioExecutor::Api) => has_seed(node, inputs),
        _ => false,
    }
}

/// Compute the cache key for a node given its resolved inputs, or `None` when
/// the node does not participate in the cache.
pub(crate) fn media_index_key(
    node: &StudioGraphNode,
    inputs: &BTreeMap<String, Value>,
) -> Option<String> {
    if !is_cacheable(node, inputs) {
        return None;
    }
    // File-backed inputs contribute their content stamp so edited upstream
    // media invalidates the key even when the path string is unchanged.
    let mut input_stamps: BTreeMap<String, Value> = BTreeMap::new();
    for (port, value) in inputs {
        if let Value::String(text) = value {
            if let Some(stamp) = FileStamp::capture(Path::new(text)) {
                input_stamps.insert(
                    port.clone(),
                    json!({ "mtime_ms": stamp.mtime_ms, "len": stamp.len }),
                );
            }
        }
    }
    let fingerprint = json!({
        "kind": node.kind,
        "params": node.params,
        "inputs": inputs,
        "input_stamps": input_stamps,
    });
    let canonical = serde_json::to_string(&fingerprint).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    Some(format!("{:x}", hasher.finalize()))
}

/// Look up a cached result. Returns the stored output map only when every
/// file-backed output is still fresh; a stale entry is removed on the spot.
pub(crate) fn media_index_lookup(key: &str) -> Option<BTreeMap<String, Value>> {
    let _guard = index_lock().lock().ok()?;
    let path = index_path().ok()?;
    let mut data = load_data(&path);
    let fresh = match data.entries.get(key) {
        Some(entry) => entry.files.iter().all(FileStamp::still_fresh),
        None => return None,
    };
    if !fresh {
        data.entries.remove(key);
        save_data(&path, &data);
        return None;
    }
    let entry = data.entries.get_mut(key)?;
    entry.last_used_ms = now_ms();
    let outputs = entry.outputs.clone();
    save_data(&path, &data);
    Some(outputs)
}

/// Record a node's successful result. Every top-level string output that is an
/// absolute path must exist as a file (it gets a freshness stamp) — a producer
/// that skipped its PNG write (deferred in-process buffer) is not cached, since
/// a later run must be able to serve the media from disk. Entries past the cap
/// are pruned LRU.
pub(crate) fn media_index_store(key: &str, node_kind: &str, outputs: &BTreeMap<String, Value>) {
    let Ok(_guard) = index_lock().lock() else {
        return;
    };
    let Ok(path) = index_path() else {
        return;
    };
    let mut files = Vec::new();
    for value in outputs.values() {
        if let Value::String(text) = value {
            let value_path = Path::new(text);
            match FileStamp::capture(value_path) {
                Some(stamp) => files.push(stamp),
                None if value_path.is_absolute() => return,
                None => {}
            }
        }
    }
    let now = now_ms();
    let mut data = load_data(&path);
    data.entries.insert(
        key.to_string(),
        MediaIndexEntry {
            node_kind: node_kind.to_string(),
            outputs: outputs.clone(),
            files,
            created_ms: now,
            last_used_ms: now,
        },
    );
    prune_lru(&mut data, MEDIA_INDEX_CAP);
    save_data(&path, &data);
}

/// Queryable view of the index: entry summaries, newest first.
#[derive(Debug, Serialize)]
pub(crate) struct MediaIndexSummary {
    pub node_kind: String,
    pub files: Vec<String>,
    pub created_ms: u64,
    pub last_used_ms: u64,
}

#[tauri::command]
pub(crate) fn list_studio_media_index() -> Result<Vec<MediaIndexSummary>, String> {
    let _guard = index_lock().lock().map_err(|err| err.to_string())?;
    let data = load_data(&index_path()?);
    let mut summaries: Vec<MediaIndexSummary> = data
        .entries
        .values()
        .map(|entry| MediaIndexSummary {
            node_kind: entry.node_kind.clone(),
            files: entry.files.iter().map(|f| f.path.clone()).collect(),
            created_ms: entry.created_ms,
            last_used_ms: entry.last_used_ms,
        })
        .collect();
    summaries.sort_by(|a, b| b.created_ms.cmp(&a.created_ms));
    Ok(summaries)
}

/// Drop every cached entry (the media files themselves are untouched).
#[tauri::command]
pub(crate) fn clear_studio_media_index() -> Result<(), String> {
    let _guard = index_lock().lock().map_err(|err| err.to_string())?;
    save_data(&index_path()?, &MediaIndexData::default());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(kind: &str, params: &[(&str, Value)]) -> StudioGraphNode {
        StudioGraphNode {
            id: "n1".to_string(),
            kind: kind.to_string(),
            params: params
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        }
    }

    #[test]
    fn compute_and_local_nodes_are_cacheable() {
        let inputs = BTreeMap::new();
        assert!(is_cacheable(&node("crop", &[]), &inputs));
        assert!(is_cacheable(&node("subjectMask", &[]), &inputs));
        assert!(is_cacheable(&node("imageEnhance", &[]), &inputs));
    }

    #[test]
    fn api_nodes_require_a_seed() {
        let inputs = BTreeMap::new();
        assert!(!is_cacheable(&node("generate", &[]), &inputs));
        assert!(is_cacheable(
            &node("generate", &[("seed", json!(42))]),
            &inputs,
        ));
        assert!(!is_cacheable(
            &node("generate", &[("seed", json!("  "))]),
            &inputs,
        ));
        let mut seeded_inputs = BTreeMap::new();
        seeded_inputs.insert("seed".to_string(), json!(7));
        assert!(is_cacheable(&node("generate", &[]), &seeded_inputs));
    }

    #[test]
    fn cache_param_overrides_the_lane_policy() {
        let inputs = BTreeMap::new();
        assert!(!is_cacheable(
            &node("crop", &[("cache", json!(false))]),
            &inputs
        ));
        assert!(is_cacheable(
            &node("generate", &[("cache", json!(true))]),
            &inputs,
        ));
        assert!(is_cacheable(
            &node("generate", &[("cache", json!("true"))]),
            &inputs,
        ));
        // Graph nodes stay non-cacheable without an explicit override.
        assert!(!is_cacheable(&node("prompt", &[]), &inputs));
    }

    #[test]
    fn key_changes_with_params_and_inputs() {
        let inputs = BTreeMap::new();
        let a = media_index_key(&node("crop", &[("width", json!(100))]), &inputs).unwrap();
        let b = media_index_key(&node("crop", &[("width", json!(200))]), &inputs).unwrap();
        assert_ne!(a, b);
        let mut other_inputs = BTreeMap::new();
        other_inputs.insert("image".to_string(), json!("does-not-exist.png"));
        let c = media_index_key(&node("crop", &[("width", json!(100))]), &other_inputs).unwrap();
        assert_ne!(a, c);
        // Deterministic: the same fingerprint yields the same key.
        let a2 = media_index_key(&node("crop", &[("width", json!(100))]), &inputs).unwrap();
        assert_eq!(a, a2);
    }

    #[test]
    fn key_tracks_input_file_content() {
        let dir = std::env::temp_dir().join(format!("hgripe-media-index-{}", now_ms()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("input.png");
        fs::write(&file, b"one").unwrap();
        let mut inputs = BTreeMap::new();
        inputs.insert(
            "image".to_string(),
            json!(file.to_string_lossy().to_string()),
        );
        let before = media_index_key(&node("crop", &[]), &inputs).unwrap();
        fs::write(&file, b"different length").unwrap();
        let after = media_index_key(&node("crop", &[]), &inputs).unwrap();
        assert_ne!(before, after);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_file_stamp_is_not_fresh() {
        let dir = std::env::temp_dir().join(format!("hgripe-media-stamp-{}", now_ms()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("out.png");
        fs::write(&file, b"payload").unwrap();
        let stamp = FileStamp::capture(&file).unwrap();
        assert!(stamp.still_fresh());
        fs::write(&file, b"changed payload!").unwrap();
        assert!(!stamp.still_fresh());
        fs::remove_file(&file).unwrap();
        assert!(!stamp.still_fresh());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_drops_least_recently_used_first() {
        let mut data = MediaIndexData::default();
        for i in 0..4 {
            data.entries.insert(
                format!("k{i}"),
                MediaIndexEntry {
                    node_kind: "crop".to_string(),
                    outputs: BTreeMap::new(),
                    files: Vec::new(),
                    created_ms: i,
                    last_used_ms: i,
                },
            );
        }
        prune_lru(&mut data, 2);
        assert_eq!(
            data.entries.keys().cloned().collect::<Vec<_>>(),
            vec!["k2".to_string(), "k3".to_string()],
        );
    }
}
