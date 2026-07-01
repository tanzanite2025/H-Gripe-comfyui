//! API task execution: run an [`ApiTask`] through the broker and record its
//! result/failure in history, plus re-running a prior task from its record.

use hgripe_api::{
    build_rerun_task_from_record, get_history_record, record_task_failure, record_task_result,
    ApiResult, ApiTask, HistoryRerunOptions,
};

use crate::{broker, runtime_paths};

async fn execute_and_record(task: ApiTask) -> Result<ApiResult, String> {
    let history_task = task.clone();
    match broker().execute(task).await {
        Ok(result) => {
            let _ = record_task_result(&history_task, &result);
            Ok(result)
        }
        Err(err) => {
            let message = err.to_string();
            let _ = record_task_failure(&history_task, message.clone());
            Err(message)
        }
    }
}

#[tauri::command]
pub(crate) async fn run_task(task: ApiTask) -> Result<ApiResult, String> {
    execute_and_record(task).await
}

#[tauri::command]
pub(crate) async fn run_task_json(task_json: String) -> Result<ApiResult, String> {
    let task: ApiTask =
        serde_json::from_str(&task_json).map_err(|err| format!("invalid ApiTask JSON: {err}"))?;
    execute_and_record(task).await
}

#[tauri::command]
pub(crate) async fn rerun_task(task_id: String, disable_cache: bool) -> Result<ApiResult, String> {
    let paths = runtime_paths()?;
    let record = get_history_record(&paths.history_db, &task_id)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("no history record for task {task_id}"))?;
    let options = HistoryRerunOptions {
        new_task_id: None,
        disable_cache,
    };
    let task = build_rerun_task_from_record(&record, options).map_err(|err| err.to_string())?;
    execute_and_record(task).await
}
