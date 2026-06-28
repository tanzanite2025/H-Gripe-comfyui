use hgripe_api::providers::custom_http::CustomHttpProvider;
use hgripe_api::providers::mock::MockProvider;
use hgripe_api::providers::openai_compatible::OpenAiCompatibleProvider;
use hgripe_api::{
    apply_history_cleanup, build_rerun_task_from_record, get_history_detail, get_history_record,
    plan_history_cleanup, query_history_records, record_task_failure, record_task_result,
    ApiBroker, ApiStatus, ApiTask, HistoryCleanupOptions, HistoryQuery, HistoryRecord,
    HistoryRerunOptions, RuntimePaths,
};
use serde::Serialize;
use serde_json::json;
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize)]
struct HistoryListItem {
    task_id: String,
    provider: String,
    operation: String,
    status: ApiStatus,
    duration_ms: u128,
    provider_request_id: Option<String>,
    output_file_count: usize,
    rerunnable: bool,
}

#[derive(Debug, Clone, Serialize)]
struct HistoryListResponse {
    history_db: String,
    records: Vec<HistoryListItem>,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}

async fn run() -> Result<(), String> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || matches!(args[0].as_str(), "-h" | "--help") {
        print_help();
        return Ok(());
    }

    let command = args.remove(0);
    match command.as_str() {
        "list" => run_list(args),
        "show" => run_show(args),
        "rerun-task" => run_rerun_task(args),
        "rerun" => run_rerun(args).await,
        "cleanup" => run_cleanup(args),
        _ => Err(format!(
            "unknown command '{command}'. Run `hgripe-api-history --help`."
        )),
    }
}

fn run_list(args: Vec<String>) -> Result<(), String> {
    let mut history_db = None;
    let mut query = HistoryQuery::recent(20);
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--history-db" => {
                history_db = Some(PathBuf::from(option_value(&args, index)?));
                index += 2;
            }
            "--limit" => {
                query.limit = option_value(&args, index)?
                    .parse::<usize>()
                    .map_err(|err| format!("invalid --limit value: {err}"))?;
                index += 2;
            }
            "--provider" => {
                query.provider = Some(option_value(&args, index)?);
                index += 2;
            }
            "--operation" => {
                query.operation = Some(option_value(&args, index)?);
                index += 2;
            }
            "--status" => {
                query.status = Some(parse_status(&option_value(&args, index)?)?);
                index += 2;
            }
            "--has-output-files" => {
                query.has_output_files = Some(parse_yes_no(&option_value(&args, index)?)?);
                index += 2;
            }
            other => return Err(format!("unknown list option '{other}'")),
        }
    }

    let history_db = history_db_path(history_db)?;
    let records = query_history_records(&history_db, query).map_err(|err| err.to_string())?;
    let response = HistoryListResponse {
        history_db: history_db.to_string_lossy().to_string(),
        records: records.iter().map(history_list_item).collect(),
    };
    print_json(&response)
}

fn run_show(args: Vec<String>) -> Result<(), String> {
    let ParsedTaskCommand {
        history_db,
        task_id,
        new_task_id: _,
        keep_cache: _,
    } = parse_task_command(args, false)?;
    let history_db = history_db_path(history_db)?;
    let detail = get_history_detail(&history_db, &task_id)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("history record not found for task_id='{task_id}'"))?;

    print_json(&json!({
        "history_db": history_db,
        "detail": detail,
    }))
}

fn run_rerun_task(args: Vec<String>) -> Result<(), String> {
    let ParsedTaskCommand {
        history_db,
        task_id,
        new_task_id,
        keep_cache,
    } = parse_task_command(args, true)?;
    let history_db = history_db_path(history_db)?;
    let record = get_history_record(&history_db, &task_id)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("history record not found for task_id='{task_id}'"))?;
    let rerun_task = build_rerun_task_from_record(
        &record,
        HistoryRerunOptions {
            new_task_id,
            disable_cache: !keep_cache,
        },
    )
    .map_err(|err| err.to_string())?;

    print_json(&json!({
        "history_db": history_db,
        "source_task_id": task_id,
        "rerun_task": rerun_task,
    }))
}

async fn run_rerun(args: Vec<String>) -> Result<(), String> {
    let ParsedTaskCommand {
        history_db,
        task_id,
        new_task_id,
        keep_cache,
    } = parse_task_command(args, true)?;
    let history_db = history_db_path(history_db)?;
    let record = get_history_record(&history_db, &task_id)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("history record not found for task_id='{task_id}'"))?;
    let rerun_task = build_rerun_task_from_record(
        &record,
        HistoryRerunOptions {
            new_task_id,
            disable_cache: !keep_cache,
        },
    )
    .map_err(|err| err.to_string())?;
    let rerun_task_id = rerun_task.id.clone();
    let result = execute_and_record(rerun_task).await?;

    print_json(&json!({
        "history_db": history_db,
        "source_task_id": task_id,
        "rerun_task_id": rerun_task_id,
        "result": result,
    }))
}

fn run_cleanup(args: Vec<String>) -> Result<(), String> {
    let CleanupCommand {
        history_db,
        history_file,
        options,
        apply,
    } = parse_cleanup_command(args)?;
    let paths = history_paths(history_db, history_file)?;

    if apply {
        let result = apply_history_cleanup(&paths.history_db, &paths.history_file, &options)
            .map_err(|err| err.to_string())?;
        print_json(&json!({
            "dry_run": false,
            "history_db": paths.history_db,
            "history_file": paths.history_file,
            "result": result,
        }))
    } else {
        let plan =
            plan_history_cleanup(&paths.history_db, &options).map_err(|err| err.to_string())?;
        print_json(&json!({
            "dry_run": true,
            "history_db": paths.history_db,
            "history_file": paths.history_file,
            "plan": plan,
        }))
    }
}

#[derive(Debug, Clone)]
struct ParsedTaskCommand {
    history_db: Option<PathBuf>,
    task_id: String,
    new_task_id: Option<String>,
    keep_cache: bool,
}

fn parse_task_command(
    args: Vec<String>,
    allow_rerun_options: bool,
) -> Result<ParsedTaskCommand, String> {
    let mut history_db = None;
    let mut task_id = None;
    let mut new_task_id = None;
    let mut keep_cache = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--history-db" => {
                history_db = Some(PathBuf::from(option_value(&args, index)?));
                index += 2;
            }
            "--new-id" if allow_rerun_options => {
                new_task_id = Some(option_value(&args, index)?);
                index += 2;
            }
            "--keep-cache" if allow_rerun_options => {
                keep_cache = true;
                index += 1;
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown option '{value}'"));
            }
            value => {
                if task_id.is_some() {
                    return Err(format!("unexpected extra argument '{value}'"));
                }
                task_id = Some(value.to_string());
                index += 1;
            }
        }
    }

    Ok(ParsedTaskCommand {
        history_db,
        task_id: task_id.ok_or_else(|| "missing task_id".to_string())?,
        new_task_id,
        keep_cache,
    })
}

#[derive(Debug, Clone)]
struct CleanupCommand {
    history_db: Option<PathBuf>,
    history_file: Option<PathBuf>,
    options: HistoryCleanupOptions,
    apply: bool,
}

fn parse_cleanup_command(args: Vec<String>) -> Result<CleanupCommand, String> {
    let mut history_db = None;
    let mut history_file = None;
    let mut options = HistoryCleanupOptions::default();
    let mut apply = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--history-db" => {
                history_db = Some(PathBuf::from(option_value(&args, index)?));
                index += 2;
            }
            "--history-file" => {
                history_file = Some(PathBuf::from(option_value(&args, index)?));
                index += 2;
            }
            "--keep-latest" => {
                options.keep_latest = Some(
                    option_value(&args, index)?
                        .parse::<usize>()
                        .map_err(|err| format!("invalid --keep-latest value: {err}"))?,
                );
                index += 2;
            }
            "--older-than-days" => {
                let days = option_value(&args, index)?
                    .parse::<u64>()
                    .map_err(|err| format!("invalid --older-than-days value: {err}"))?;
                options.older_than_timestamp_ms = Some(cutoff_timestamp_ms_for_days(days));
                index += 2;
            }
            "--provider" => {
                options.provider = Some(option_value(&args, index)?);
                index += 2;
            }
            "--operation" => {
                options.operation = Some(option_value(&args, index)?);
                index += 2;
            }
            "--status" => {
                options.status = Some(parse_status(&option_value(&args, index)?)?);
                index += 2;
            }
            "--has-output-files" => {
                options.has_output_files = Some(parse_yes_no(&option_value(&args, index)?)?);
                index += 2;
            }
            "--all-matched" => {
                options.delete_all_matched = true;
                index += 1;
            }
            "--delete-output-files" => {
                options.delete_output_files = true;
                index += 1;
            }
            "--apply" => {
                apply = true;
                index += 1;
            }
            other => return Err(format!("unknown cleanup option '{other}'")),
        }
    }

    if options.keep_latest.is_none()
        && options.older_than_timestamp_ms.is_none()
        && !options.delete_all_matched
    {
        return Err(
            "cleanup requires --keep-latest, --older-than-days, or --all-matched".to_string(),
        );
    }

    Ok(CleanupCommand {
        history_db,
        history_file,
        options,
        apply,
    })
}

fn history_list_item(record: &HistoryRecord) -> HistoryListItem {
    HistoryListItem {
        task_id: record.task_id.clone(),
        provider: record.provider.clone(),
        operation: record.operation.clone(),
        status: record.status.clone(),
        duration_ms: record.duration_ms,
        provider_request_id: record.provider_request_id.clone(),
        output_file_count: record.output_file_count,
        rerunnable: record.task_snapshot.is_some(),
    }
}

fn history_db_path(path: Option<PathBuf>) -> Result<PathBuf, String> {
    path.map(Ok)
        .unwrap_or_else(|| RuntimePaths::from_env().map(|paths| paths.history_db))
        .map_err(|err| err.to_string())
}

#[derive(Debug, Clone)]
struct HistoryPaths {
    history_db: PathBuf,
    history_file: PathBuf,
}

fn history_paths(
    history_db: Option<PathBuf>,
    history_file: Option<PathBuf>,
) -> Result<HistoryPaths, String> {
    let runtime_paths = RuntimePaths::from_env().map_err(|err| err.to_string())?;
    Ok(HistoryPaths {
        history_db: history_db.unwrap_or(runtime_paths.history_db),
        history_file: history_file.unwrap_or(runtime_paths.history_file),
    })
}

fn option_value(args: &[String], index: usize) -> Result<String, String> {
    args.get(index + 1)
        .filter(|value| !value.starts_with('-'))
        .cloned()
        .ok_or_else(|| format!("missing value for {}", args[index]))
}

fn parse_status(value: &str) -> Result<ApiStatus, String> {
    serde_json::from_value(json!(value.trim().to_ascii_lowercase()))
        .map_err(|err| format!("invalid --status value '{value}': {err}"))
}

fn parse_yes_no(value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "yes" | "true" | "1" => Ok(true),
        "no" | "false" | "0" => Ok(false),
        _ => Err(format!("invalid --has-output-files value '{value}'")),
    }
}

fn cutoff_timestamp_ms_for_days(days: u64) -> u128 {
    let age_ms = (days as u128)
        .saturating_mul(24)
        .saturating_mul(60)
        .saturating_mul(60)
        .saturating_mul(1000);
    current_timestamp_ms().saturating_sub(age_ms)
}

fn current_timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn print_json<T: Serialize>(value: &T) -> Result<(), String> {
    let encoded = serde_json::to_string_pretty(value)
        .map_err(|err| format!("failed to encode JSON output: {err}"))?;
    println!("{encoded}");
    Ok(())
}

async fn execute_and_record(task: ApiTask) -> Result<hgripe_api::ApiResult, String> {
    let history_task = task.clone();
    let broker = broker_with_default_providers();

    match broker.execute(task).await {
        Ok(result) => {
            if let Err(err) = record_task_result(&history_task, &result) {
                eprintln!("warning: failed to record task history: {err}");
            }
            Ok(result)
        }
        Err(err) => {
            if let Err(history_err) = record_task_failure(&history_task, err.to_string()) {
                eprintln!("warning: failed to record task history: {history_err}");
            }
            Err(err.to_string())
        }
    }
}

fn broker_with_default_providers() -> ApiBroker {
    let mut broker = ApiBroker::new();
    broker.register_provider(CustomHttpProvider::default());
    broker.register_provider(MockProvider);
    broker.register_provider(OpenAiCompatibleProvider::default());
    broker
}

fn print_help() {
    println!(
        r#"Usage:
  hgripe-api-history list [--limit N] [--provider NAME] [--operation OP] [--status STATUS] [--has-output-files yes|no] [--history-db PATH]
  hgripe-api-history show <task_id> [--history-db PATH]
  hgripe-api-history rerun-task <task_id> [--new-id ID] [--keep-cache] [--history-db PATH]
  hgripe-api-history rerun <task_id> [--new-id ID] [--keep-cache] [--history-db PATH]
  hgripe-api-history cleanup [--keep-latest N] [--older-than-days N] [--provider NAME] [--operation OP] [--status STATUS] [--has-output-files yes|no] [--all-matched] [--delete-output-files] [--apply] [--history-db PATH] [--history-file PATH]
"#
    );
}
