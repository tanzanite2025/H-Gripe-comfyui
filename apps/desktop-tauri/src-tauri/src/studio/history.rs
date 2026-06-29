//! Project-scoped JSON stores for editor state that should travel with the
//! project folder rather than living only in browser `localStorage`: named
//! graph snapshots and run history. Each is a single JSON-array file; the
//! renderer owns the shape and the backend just reads/writes it via the store
//! primitives in [`super::persist`].

use super::persist::{read_studio_store, write_studio_store};

const SNAPSHOTS_FILE: &str = ".hgripe-snapshots.json";
const RUN_HISTORY_FILE: &str = ".hgripe-runhistory.json";

/// Read the project folder's persisted snapshots file (raw JSON array text).
#[tauri::command]
pub(crate) fn read_studio_snapshots(dir: String) -> Result<String, String> {
    read_studio_store(&dir, SNAPSHOTS_FILE)
}

/// Write the project folder's snapshots file (renderer's serialized array).
#[tauri::command]
pub(crate) fn write_studio_snapshots(dir: String, snapshots_json: String) -> Result<(), String> {
    write_studio_store(&dir, SNAPSHOTS_FILE, &snapshots_json)
}

/// Read the project folder's run-history file (raw JSON array text).
#[tauri::command]
pub(crate) fn read_studio_run_history(dir: String) -> Result<String, String> {
    read_studio_store(&dir, RUN_HISTORY_FILE)
}

/// Write the project folder's run-history file (renderer's serialized array).
#[tauri::command]
pub(crate) fn write_studio_run_history(dir: String, history_json: String) -> Result<(), String> {
    write_studio_store(&dir, RUN_HISTORY_FILE, &history_json)
}
