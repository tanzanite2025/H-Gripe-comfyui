//! Task-history queries and retention cleanup, backed by the history DB whose
//! location comes from [`crate::runtime_paths`].

use hgripe_api::{
    apply_history_cleanup, get_history_detail, plan_history_cleanup, query_history_records,
    HistoryCleanupOptions, HistoryCleanupPlan, HistoryCleanupResult, HistoryDetail, HistoryQuery,
    HistoryRecord,
};

use crate::runtime_paths;

#[tauri::command]
pub(crate) fn list_history(query: HistoryQuery) -> Result<Vec<HistoryRecord>, String> {
    let paths = runtime_paths()?;
    query_history_records(&paths.history_db, query).map_err(|err| err.to_string())
}

#[tauri::command]
pub(crate) fn history_detail(task_id: String) -> Result<Option<HistoryDetail>, String> {
    let paths = runtime_paths()?;
    get_history_detail(&paths.history_db, &task_id).map_err(|err| err.to_string())
}

#[tauri::command]
pub(crate) fn history_cleanup_preview(
    options: HistoryCleanupOptions,
) -> Result<HistoryCleanupPlan, String> {
    let paths = runtime_paths()?;
    plan_history_cleanup(&paths.history_db, &options).map_err(|err| err.to_string())
}

#[tauri::command]
pub(crate) fn history_cleanup_apply(
    options: HistoryCleanupOptions,
) -> Result<HistoryCleanupResult, String> {
    let paths = runtime_paths()?;
    apply_history_cleanup(&paths.history_db, &paths.history_file, &options)
        .map_err(|err| err.to_string())
}
