//! Per-run cancellation state for Studio graph executions: the Tauri-managed
//! token map plus the helpers the engine uses to mint, query, and clear a
//! run's [`CancellationToken`].

use std::collections::HashMap;
use std::sync::Mutex;

use hgripe_api::CancellationToken;

/// Per-run cancellation tokens for in-flight Studio graph executions, keyed by
/// run id. Shared with the front-end as Tauri managed state.
#[derive(Default)]
pub(crate) struct StudioRunCancels(Mutex<HashMap<String, CancellationToken>>);

pub(super) fn studio_run_token(
    state: &tauri::State<'_, StudioRunCancels>,
    run_id: &str,
) -> CancellationToken {
    let mut cancels = state.0.lock().unwrap();
    cancels.entry(run_id.to_string()).or_default().clone()
}

pub(super) fn is_studio_run_cancelled(
    state: &tauri::State<'_, StudioRunCancels>,
    run_id: &str,
) -> bool {
    state
        .0
        .lock()
        .unwrap()
        .get(run_id)
        .is_some_and(CancellationToken::is_cancelled)
}

pub(super) fn clear_studio_run_cancel(state: &tauri::State<'_, StudioRunCancels>, run_id: &str) {
    state.0.lock().unwrap().remove(run_id);
}
