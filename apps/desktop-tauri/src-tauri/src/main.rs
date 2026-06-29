#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use hgripe_api::providers::custom_http::CustomHttpProvider;
use hgripe_api::providers::mock::MockProvider;
use hgripe_api::providers::openai_compatible::OpenAiCompatibleProvider;
use hgripe_api::providers::replicate::ReplicateProvider;
use hgripe_api::{
    apply_history_cleanup, build_doctor_report, build_rerun_task_from_record,
    credentials_file_path, get_history_detail, get_history_record, list_credential_summaries,
    list_provider_profile_summaries, plan_history_cleanup, provider_profiles_path,
    query_history_records, record_task_failure, record_task_result, validate_credentials,
    validate_provider_profiles, ApiBroker, ApiErrorInfo, ApiResult, ApiStatus, ApiTask,
    BrokerError, CancellationToken, CredentialSummary, CredentialsValidation, DoctorOptions,
    DoctorReport, HistoryCleanupOptions, HistoryCleanupPlan, HistoryCleanupResult, HistoryDetail,
    HistoryQuery, HistoryRecord, HistoryRerunOptions, OutputType, ProviderExecutionContext,
    ProviderProfileSummary, ProviderProfilesValidation, RuntimePaths,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::Emitter;

const STUDIO_GRAPH_RUN_EVENT: &str = "studio:graph-run";

#[derive(Default)]
struct StudioRunCancels(Mutex<HashMap<String, CancellationToken>>);

fn broker() -> ApiBroker {
    let mut broker = ApiBroker::new();
    broker.register_provider(CustomHttpProvider::default());
    broker.register_provider(MockProvider);
    broker.register_provider(OpenAiCompatibleProvider::default());
    broker.register_provider(ReplicateProvider::default());
    broker
}

fn runtime_paths() -> Result<RuntimePaths, String> {
    RuntimePaths::from_env().map_err(|err| err.to_string())
}

fn config_path(kind: &str) -> Result<PathBuf, String> {
    match kind {
        "credentials" => Ok(credentials_file_path(None)),
        "profiles" => Ok(provider_profiles_path(None)),
        other => Err(format!("unknown config kind: {other}")),
    }
}

#[derive(Serialize)]
struct PathInfo {
    path: String,
    exists: bool,
}

impl PathInfo {
    fn new(path: PathBuf) -> Self {
        Self {
            exists: path.exists(),
            path: path.to_string_lossy().to_string(),
        }
    }
}

#[derive(Serialize)]
struct RuntimeInfo {
    providers: Vec<String>,
    credentials_file: PathInfo,
    profiles_file: PathInfo,
    history_file: PathInfo,
    history_db: PathInfo,
    output_dir: PathInfo,
}

#[tauri::command]
fn get_runtime_info() -> Result<RuntimeInfo, String> {
    let paths = runtime_paths()?;
    Ok(RuntimeInfo {
        providers: broker().providers(),
        credentials_file: PathInfo::new(credentials_file_path(None)),
        profiles_file: PathInfo::new(provider_profiles_path(None)),
        history_file: PathInfo::new(paths.history_file),
        history_db: PathInfo::new(paths.history_db),
        output_dir: PathInfo::new(paths.output_dir),
    })
}

#[tauri::command]
fn doctor() -> Result<DoctorReport, String> {
    build_doctor_report(DoctorOptions::default()).map_err(|err| err.to_string())
}

#[tauri::command]
fn get_credentials() -> Result<Vec<CredentialSummary>, String> {
    list_credential_summaries(None).map_err(|err| err.to_string())
}

#[tauri::command]
fn check_credentials() -> Result<CredentialsValidation, String> {
    validate_credentials(None).map_err(|err| err.to_string())
}

#[tauri::command]
fn get_profiles() -> Result<Vec<ProviderProfileSummary>, String> {
    list_provider_profile_summaries(None).map_err(|err| err.to_string())
}

#[tauri::command]
fn check_profiles() -> Result<ProviderProfilesValidation, String> {
    validate_provider_profiles(None).map_err(|err| err.to_string())
}

#[tauri::command]
fn read_config_file(kind: String) -> Result<String, String> {
    let path = config_path(&kind)?;
    if !path.exists() {
        return Ok(String::new());
    }
    fs::read_to_string(&path).map_err(|err| format!("failed to read {}: {err}", path.display()))
}

#[tauri::command]
fn write_config_file(kind: String, content: String) -> Result<(), String> {
    let path = config_path(&kind)?;
    // Validate JSON before persisting so we never write a broken config file.
    if !content.trim().is_empty() {
        serde_json::from_str::<serde_json::Value>(&content)
            .map_err(|err| format!("invalid JSON: {err}"))?;
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
    }
    fs::write(&path, content).map_err(|err| format!("failed to write {}: {err}", path.display()))
}

#[tauri::command]
fn list_history(query: HistoryQuery) -> Result<Vec<HistoryRecord>, String> {
    let paths = runtime_paths()?;
    query_history_records(&paths.history_db, query).map_err(|err| err.to_string())
}

#[tauri::command]
fn history_detail(task_id: String) -> Result<Option<HistoryDetail>, String> {
    let paths = runtime_paths()?;
    get_history_detail(&paths.history_db, &task_id).map_err(|err| err.to_string())
}

#[tauri::command]
fn history_cleanup_preview(options: HistoryCleanupOptions) -> Result<HistoryCleanupPlan, String> {
    let paths = runtime_paths()?;
    plan_history_cleanup(&paths.history_db, &options).map_err(|err| err.to_string())
}

#[tauri::command]
fn history_cleanup_apply(options: HistoryCleanupOptions) -> Result<HistoryCleanupResult, String> {
    let paths = runtime_paths()?;
    apply_history_cleanup(&paths.history_db, &paths.history_file, &options)
        .map_err(|err| err.to_string())
}

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

async fn execute_and_record_cancellable(
    task: ApiTask,
    cancels: &tauri::State<'_, StudioRunCancels>,
    run_id: &str,
) -> Result<ApiResult, String> {
    let history_task = task.clone();
    let context = ProviderExecutionContext::new(studio_run_token(cancels, run_id));

    match broker().execute_with_context(task, context).await {
        Ok(result) => {
            let _ = record_task_result(&history_task, &result);
            Ok(result)
        }
        Err(BrokerError::Cancelled) => {
            let result = ApiResult {
                id: history_task.id.clone(),
                status: ApiStatus::Cancelled,
                output_files: Vec::new(),
                output_json: None,
                metadata: BTreeMap::new(),
                cost: None,
                duration_ms: 0,
                provider_request_id: None,
                cache_hit: false,
                error: Some(ApiErrorInfo {
                    code: "cancelled".to_string(),
                    message: "Studio run cancelled".to_string(),
                    retryable: false,
                }),
            };
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
async fn run_task(task: ApiTask) -> Result<ApiResult, String> {
    execute_and_record(task).await
}

#[tauri::command]
async fn run_task_json(task_json: String) -> Result<ApiResult, String> {
    let task: ApiTask =
        serde_json::from_str(&task_json).map_err(|err| format!("invalid ApiTask JSON: {err}"))?;
    execute_and_record(task).await
}

#[derive(Debug, Deserialize)]
struct StudioWorkflowGraph {
    version: u32,
    #[serde(default)]
    nodes: Vec<StudioGraphNode>,
    #[serde(default)]
    edges: Vec<StudioGraphEdge>,
}

#[derive(Debug, Deserialize)]
struct StudioGraphNode {
    id: String,
    kind: String,
    #[serde(default)]
    params: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StudioGraphEdge {
    id: String,
    source: String,
    source_port: String,
    target: String,
    target_port: String,
}

#[derive(Debug, Serialize)]
struct StudioGraphRunResult {
    version: u32,
    outputs: BTreeMap<String, BTreeMap<String, Value>>,
    statuses: BTreeMap<String, String>,
    node_runs: Vec<StudioNodeRun>,
}

#[derive(Debug, Serialize)]
struct StudioNodeRun {
    node_id: String,
    kind: String,
    status: String,
    duration_ms: Option<u128>,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct StudioGraphRunEvent {
    run_id: String,
    node_id: Option<String>,
    kind: Option<String>,
    status: String,
    duration_ms: Option<u128>,
    error: Option<String>,
    message: Option<String>,
}

fn emit_studio_run_event(app: &tauri::AppHandle, payload: StudioGraphRunEvent) {
    let _ = app.emit(STUDIO_GRAPH_RUN_EVENT, payload);
}

fn studio_node_event(
    run_id: &str,
    node: &StudioGraphNode,
    status: &str,
    duration_ms: Option<u128>,
    error: Option<String>,
) -> StudioGraphRunEvent {
    StudioGraphRunEvent {
        run_id: run_id.to_string(),
        node_id: Some(node.id.clone()),
        kind: Some(node.kind.clone()),
        status: status.to_string(),
        duration_ms,
        error,
        message: None,
    }
}

fn studio_graph_event(run_id: &str, status: &str, message: Option<String>) -> StudioGraphRunEvent {
    StudioGraphRunEvent {
        run_id: run_id.to_string(),
        node_id: None,
        kind: None,
        status: status.to_string(),
        duration_ms: None,
        error: None,
        message,
    }
}

fn studio_run_token(state: &tauri::State<'_, StudioRunCancels>, run_id: &str) -> CancellationToken {
    let mut cancels = state.0.lock().unwrap();
    cancels.entry(run_id.to_string()).or_default().clone()
}

fn is_studio_run_cancelled(state: &tauri::State<'_, StudioRunCancels>, run_id: &str) -> bool {
    state
        .0
        .lock()
        .unwrap()
        .get(run_id)
        .is_some_and(CancellationToken::is_cancelled)
}

fn clear_studio_run_cancel(state: &tauri::State<'_, StudioRunCancels>, run_id: &str) {
    state.0.lock().unwrap().remove(run_id);
}

fn studio_output_map<const N: usize>(entries: [(&str, Value); N]) -> BTreeMap<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

fn studio_value_to_string(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Null) | None => String::new(),
        Some(value) => value.to_string(),
    }
}

fn studio_value_to_number(value: Option<&Value>) -> f64 {
    match value {
        Some(Value::Number(number)) => number.as_f64().unwrap_or(0.0),
        Some(Value::String(value)) => value.parse::<f64>().unwrap_or(0.0),
        Some(Value::Bool(value)) => {
            if *value {
                1.0
            } else {
                0.0
            }
        }
        _ => 0.0,
    }
}

fn studio_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_f64().map(|n| n != 0.0).unwrap_or(false),
        Value::String(value) => !value.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn studio_non_empty(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(value) => !value.is_empty(),
        _ => true,
    }
}

fn studio_task_id(node_id: &str) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    format!("studio-{node_id}-{millis}")
}

fn studio_workspace_dir() -> PathBuf {
    let credentials = credentials_file_path(None);
    let base = credentials
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("user")
                .join("hgripe")
        });
    base.join("studio")
}

fn studio_autosave_path() -> PathBuf {
    studio_workspace_dir().join("autosave.workflow.json")
}

fn studio_topo_order(graph: &StudioWorkflowGraph) -> Result<Vec<String>, String> {
    if graph.version != 1 {
        return Err(format!(
            "unsupported Studio graph version: {} (expected 1)",
            graph.version
        ));
    }

    let mut seen = HashSet::new();
    let mut indegree: HashMap<String, usize> = HashMap::new();
    let mut outgoing: HashMap<String, Vec<String>> = HashMap::new();
    for node in &graph.nodes {
        if !seen.insert(node.id.clone()) {
            return Err(format!("duplicate node id: {}", node.id));
        }
        indegree.insert(node.id.clone(), 0);
        outgoing.insert(node.id.clone(), Vec::new());
    }

    for edge in &graph.edges {
        if !indegree.contains_key(&edge.source) {
            return Err(format!(
                "edge {} references missing source node {}",
                edge.id, edge.source
            ));
        }
        if !indegree.contains_key(&edge.target) {
            return Err(format!(
                "edge {} references missing target node {}",
                edge.id, edge.target
            ));
        }
        *indegree.entry(edge.target.clone()).or_insert(0) += 1;
        outgoing
            .entry(edge.source.clone())
            .or_default()
            .push(edge.target.clone());
    }

    let mut queue: VecDeque<String> = graph
        .nodes
        .iter()
        .filter(|node| indegree.get(&node.id).copied().unwrap_or(0) == 0)
        .map(|node| node.id.clone())
        .collect();
    let mut order = Vec::with_capacity(graph.nodes.len());

    while let Some(id) = queue.pop_front() {
        order.push(id.clone());
        for target in outgoing.get(&id).into_iter().flatten() {
            let degree = indegree
                .get_mut(target)
                .ok_or_else(|| format!("missing target node during topo sort: {target}"))?;
            *degree = degree.saturating_sub(1);
            if *degree == 0 {
                queue.push_back(target.clone());
            }
        }
    }

    if order.len() != graph.nodes.len() {
        return Err("graph contains a cycle".to_string());
    }
    Ok(order)
}

fn studio_node_inputs(
    node_id: &str,
    graph: &StudioWorkflowGraph,
    outputs: &BTreeMap<String, BTreeMap<String, Value>>,
) -> BTreeMap<String, Value> {
    let mut inputs = BTreeMap::new();
    for edge in graph.edges.iter().filter(|edge| edge.target == node_id) {
        if let Some(value) = outputs
            .get(&edge.source)
            .and_then(|source_outputs| source_outputs.get(&edge.source_port))
        {
            inputs.insert(edge.target_port.clone(), value.clone());
        }
    }
    inputs
}

fn studio_batch_items(items: Option<&Value>) -> Vec<String> {
    studio_value_to_string(items)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn studio_compare_result(
    params: &BTreeMap<String, Value>,
    inputs: &BTreeMap<String, Value>,
) -> bool {
    let a = inputs.get("a");
    let b = inputs.get("b");
    let sa = studio_value_to_string(a);
    let sb = studio_value_to_string(b);
    let an = sa.parse::<f64>();
    let bn = sb.parse::<f64>();
    let numeric = !sa.is_empty() && !sb.is_empty() && an.is_ok() && bn.is_ok();
    let op = studio_value_to_string(params.get("op"));

    if numeric {
        let a = an.unwrap_or(0.0);
        let b = bn.unwrap_or(0.0);
        match op.as_str() {
            "==" => a == b,
            "!=" => a != b,
            ">" => a > b,
            ">=" => a >= b,
            "<" => a < b,
            "<=" => a <= b,
            _ => false,
        }
    } else {
        match op.as_str() {
            "==" => sa == sb,
            "!=" => sa != sb,
            ">" => sa > sb,
            ">=" => sa >= sb,
            "<" => sa < sb,
            "<=" => sa <= sb,
            _ => false,
        }
    }
}

fn studio_logic_result(params: &BTreeMap<String, Value>, inputs: &BTreeMap<String, Value>) -> bool {
    let a = inputs.get("a").map(studio_truthy).unwrap_or(false);
    let b = inputs.get("b").map(studio_truthy).unwrap_or(false);
    match studio_value_to_string(params.get("op")).as_str() {
        "and" => a && b,
        "or" => a || b,
        "xor" => a != b,
        "not" => !a,
        _ => false,
    }
}

async fn execute_studio_generate(
    node: &StudioGraphNode,
    inputs: &BTreeMap<String, Value>,
    cancels: &tauri::State<'_, StudioRunCancels>,
    run_id: &str,
) -> Result<BTreeMap<String, Value>, String> {
    let mut task = ApiTask::new(
        studio_value_to_string(node.params.get("provider"))
            .trim()
            .to_string(),
        studio_value_to_string(node.params.get("operation"))
            .trim()
            .to_string(),
    );
    if task.provider.is_empty() {
        task.provider = "mock".to_string();
    }
    if task.operation.is_empty() {
        task.operation = "image.generate".to_string();
    }
    task.id = studio_task_id(&node.id);
    task.output_type = OutputType::Image;
    task.cache_policy.enabled = false;
    task.retry_policy.max_attempts = 1;
    task.retry_policy.backoff_ms = 200;
    task.retry_policy.timeout_ms = Some(60_000);

    let prompt = studio_value_to_string(inputs.get("prompt"));
    if !prompt.is_empty() {
        task.inputs.insert("prompt".to_string(), json!(prompt));
    }
    let reference = studio_value_to_string(inputs.get("reference"));
    if !reference.is_empty() {
        task.inputs
            .insert("image_path".to_string(), json!(reference));
    }

    for (key, value) in &node.params {
        if matches!(key.as_str(), "provider" | "operation" | "credentials_ref") {
            continue;
        }
        if studio_non_empty(value) {
            task.params.insert(key.clone(), value.clone());
        }
    }
    if let Some(seed) = inputs.get("seed") {
        task.params.insert("seed".to_string(), seed.clone());
    }

    let credentials_ref = studio_value_to_string(node.params.get("credentials_ref"));
    if !credentials_ref.is_empty() {
        task.credentials_ref = Some(credentials_ref);
    }

    let result = execute_and_record_cancellable(task, cancels, run_id).await?;
    if !matches!(result.status, ApiStatus::Succeeded | ApiStatus::Cached) {
        let message = result
            .error
            .as_ref()
            .map(|error| error.message.clone())
            .unwrap_or_else(|| "generation failed".to_string());
        return Err(message);
    }
    let image = result
        .output_files
        .first()
        .map(|file| json!(file.path.clone()))
        .unwrap_or(Value::Null);
    let result_json =
        serde_json::to_value(result).map_err(|err| format!("failed to encode ApiResult: {err}"))?;
    Ok(studio_output_map([
        ("image", image),
        ("result", result_json),
    ]))
}

fn execute_studio_psd_export(
    node: &StudioGraphNode,
    inputs: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, String> {
    let image = studio_value_to_string(inputs.get("image"));
    if image.is_empty() {
        return Err("PSD Export needs a connected image input".to_string());
    }
    let template = studio_value_to_string(inputs.get("template"));
    if template.is_empty() {
        return Err("PSD Export needs a connected PSD template input".to_string());
    }

    let output_dir = {
        let configured = studio_value_to_string(node.params.get("output_dir"));
        if configured.trim().is_empty() {
            runtime_paths()?.output_dir.to_string_lossy().to_string()
        } else {
            configured
        }
    };
    let filename = {
        let configured = studio_value_to_string(node.params.get("filename"));
        if configured.trim().is_empty() {
            "final".to_string()
        } else {
            configured
        }
    };
    let placeholder_name = studio_value_to_string(node.params.get("placeholder"));
    let placeholder = if placeholder_name.trim().is_empty() {
        None
    } else {
        Some(json!({ "name": placeholder_name }).to_string())
    };

    let result = compose_psd(
        None,
        template,
        image,
        output_dir,
        Some(filename),
        placeholder,
        Some(
            studio_value_to_string(node.params.get("fit_mode"))
                .trim()
                .to_string(),
        )
        .filter(|value| !value.is_empty()),
        None,
        Some(
            studio_value_to_string(node.params.get("smart_object_mode"))
                .trim()
                .to_string(),
        )
        .filter(|value| !value.is_empty()),
        None,
        None,
        None,
    )?;

    if result.status != "succeeded" {
        return Err(format!("PSD export failed: {}", result.status));
    }

    let result_json = serde_json::to_value(&result)
        .map_err(|err| format!("failed to encode ComposePsdResult: {err}"))?;
    Ok(studio_output_map([
        ("psdPath", json!(result.psd_path)),
        ("previewPath", json!(result.preview_path)),
        ("metadataPath", json!(result.metadata_path)),
        ("placeholderKind", json!(result.placeholder_kind)),
        ("smartObjectMode", json!(result.smart_object_mode)),
        ("result", result_json),
    ]))
}

async fn execute_studio_node(
    node: &StudioGraphNode,
    inputs: BTreeMap<String, Value>,
    cancels: &tauri::State<'_, StudioRunCancels>,
    run_id: &str,
) -> Result<BTreeMap<String, Value>, String> {
    match node.kind.as_str() {
        "prompt" => Ok(studio_output_map([(
            "text",
            json!(studio_value_to_string(node.params.get("text"))),
        )])),
        "batch" => {
            let items = studio_batch_items(node.params.get("items"));
            let index = studio_value_to_number(node.params.get("index")).max(0.0) as usize;
            Ok(studio_output_map([(
                "item",
                json!(items
                    .get(index)
                    .or_else(|| items.first())
                    .cloned()
                    .unwrap_or_default()),
            )]))
        }
        "imageSource" => {
            let path = studio_value_to_string(node.params.get("path"));
            let image = if path.is_empty() {
                Value::Null
            } else {
                json!(path)
            };
            Ok(studio_output_map([("image", image)]))
        }
        "psdTemplate" => {
            let path = studio_value_to_string(node.params.get("path"));
            let template = if path.is_empty() {
                Value::Null
            } else {
                json!(path)
            };
            Ok(studio_output_map([("template", template)]))
        }
        "number" => Ok(studio_output_map([(
            "value",
            json!(studio_value_to_number(node.params.get("value"))),
        )])),
        "reroute" => Ok(studio_output_map([(
            "out",
            inputs.get("in").cloned().unwrap_or(Value::Null),
        )])),
        "group" => Ok(BTreeMap::new()),
        "compare" => Ok(studio_output_map([(
            "result",
            json!(if studio_compare_result(&node.params, &inputs) {
                1
            } else {
                0
            }),
        )])),
        "logic" => Ok(studio_output_map([(
            "result",
            json!(if studio_logic_result(&node.params, &inputs) {
                1
            } else {
                0
            }),
        )])),
        "if" => {
            let active = inputs
                .get("cond")
                .map(studio_truthy)
                .unwrap_or_else(|| studio_value_to_string(node.params.get("cond")) == "true");
            let port = if active { "true" } else { "false" };
            Ok(studio_output_map([(
                port,
                inputs.get("value").cloned().unwrap_or(Value::Null),
            )]))
        }
        "switch" => {
            let index = inputs
                .get("index")
                .map(|value| studio_value_to_number(Some(value)))
                .unwrap_or_else(|| studio_value_to_number(node.params.get("index")))
                as i64;
            let port = match index {
                0 => "0",
                1 => "1",
                2 => "2",
                _ => "default",
            };
            Ok(studio_output_map([(
                port,
                inputs.get("value").cloned().unwrap_or(Value::Null),
            )]))
        }
        "generate" => execute_studio_generate(node, &inputs, cancels, run_id).await,
        "preview" => Ok(studio_output_map([(
            "image",
            inputs.get("image").cloned().unwrap_or(Value::Null),
        )])),
        "save" => Ok(studio_output_map([
            ("image", inputs.get("image").cloned().unwrap_or(Value::Null)),
            (
                "template",
                inputs.get("template").cloned().unwrap_or(Value::Null),
            ),
            (
                "filename",
                json!(studio_value_to_string(node.params.get("filename"))),
            ),
        ])),
        "psdExport" => execute_studio_psd_export(node, &inputs),
        other => Err(format!("unsupported Studio node kind: {other}")),
    }
}

#[tauri::command]
async fn run_studio_graph(
    app: tauri::AppHandle,
    cancels: tauri::State<'_, StudioRunCancels>,
    graph_json: String,
    run_id: Option<String>,
) -> Result<StudioGraphRunResult, String> {
    let run_id = run_id.unwrap_or_else(|| studio_task_id("graph"));
    let _ = studio_run_token(&cancels, &run_id);
    emit_studio_run_event(
        &app,
        studio_graph_event(&run_id, "running", Some("Studio graph started".to_string())),
    );
    let graph: StudioWorkflowGraph = serde_json::from_str(&graph_json)
        .map_err(|err| format!("invalid Studio graph JSON: {err}"))?;
    let order = studio_topo_order(&graph)?;
    let nodes_by_id: HashMap<String, &StudioGraphNode> = graph
        .nodes
        .iter()
        .map(|node| (node.id.clone(), node))
        .collect();
    let mut outputs: BTreeMap<String, BTreeMap<String, Value>> = BTreeMap::new();
    let mut statuses: BTreeMap<String, String> = BTreeMap::new();
    let mut node_runs: Vec<StudioNodeRun> = Vec::new();
    let mut pruned: HashSet<String> = HashSet::new();

    for node in &graph.nodes {
        statuses.insert(node.id.clone(), "queued".to_string());
        emit_studio_run_event(&app, studio_node_event(&run_id, node, "queued", None, None));
    }

    for node_id in order {
        if is_studio_run_cancelled(&cancels, &run_id) {
            clear_studio_run_cancel(&cancels, &run_id);
            emit_studio_run_event(
                &app,
                studio_graph_event(
                    &run_id,
                    "cancelled",
                    Some("Studio graph cancelled".to_string()),
                ),
            );
            return Err("Studio run cancelled".to_string());
        }
        let node = nodes_by_id
            .get(&node_id)
            .ok_or_else(|| format!("missing node during run: {node_id}"))?;
        let incoming_edges: Vec<&StudioGraphEdge> = graph
            .edges
            .iter()
            .filter(|edge| edge.target == node.id)
            .collect();
        let dead_edge = |edge: &&StudioGraphEdge| {
            pruned.contains(&edge.source)
                || outputs
                    .get(&edge.source)
                    .map(|source_outputs| !source_outputs.contains_key(&edge.source_port))
                    .unwrap_or(true)
        };
        if !incoming_edges.is_empty() && incoming_edges.iter().all(dead_edge) {
            pruned.insert(node.id.clone());
            statuses.insert(node.id.clone(), "skipped".to_string());
            emit_studio_run_event(
                &app,
                studio_node_event(&run_id, node, "skipped", None, None),
            );
            node_runs.push(StudioNodeRun {
                node_id: node.id.clone(),
                kind: node.kind.clone(),
                status: "skipped".to_string(),
                duration_ms: None,
                error: None,
            });
            continue;
        }
        statuses.insert(node.id.clone(), "running".to_string());
        emit_studio_run_event(
            &app,
            studio_node_event(&run_id, node, "running", None, None),
        );
        let started_at = Instant::now();
        let inputs = studio_node_inputs(&node.id, &graph, &outputs);
        match execute_studio_node(node, inputs, &cancels, &run_id).await {
            Ok(node_outputs) => {
                let duration_ms = started_at.elapsed().as_millis();
                outputs.insert(node.id.clone(), node_outputs);
                statuses.insert(node.id.clone(), "succeeded".to_string());
                emit_studio_run_event(
                    &app,
                    studio_node_event(&run_id, node, "succeeded", Some(duration_ms), None),
                );
                node_runs.push(StudioNodeRun {
                    node_id: node.id.clone(),
                    kind: node.kind.clone(),
                    status: "succeeded".to_string(),
                    duration_ms: Some(duration_ms),
                    error: None,
                });
            }
            Err(error) => {
                let cancelled = error.to_ascii_lowercase().contains("cancel");
                let duration_ms = started_at.elapsed().as_millis();
                let status = if cancelled { "cancelled" } else { "failed" };
                statuses.insert(node.id.clone(), status.to_string());
                emit_studio_run_event(
                    &app,
                    studio_node_event(
                        &run_id,
                        node,
                        status,
                        Some(duration_ms),
                        Some(error.clone()),
                    ),
                );
                emit_studio_run_event(
                    &app,
                    studio_graph_event(
                        &run_id,
                        if cancelled { "cancelled" } else { "failed" },
                        Some(error.clone()),
                    ),
                );
                clear_studio_run_cancel(&cancels, &run_id);
                node_runs.push(StudioNodeRun {
                    node_id: node.id.clone(),
                    kind: node.kind.clone(),
                    status: status.to_string(),
                    duration_ms: Some(duration_ms),
                    error: Some(error.clone()),
                });
                return Err(if cancelled {
                    "Studio run cancelled".to_string()
                } else {
                    format!("Studio node {} failed: {error}", node.id)
                });
            }
        }
        if is_studio_run_cancelled(&cancels, &run_id) {
            clear_studio_run_cancel(&cancels, &run_id);
            emit_studio_run_event(
                &app,
                studio_graph_event(
                    &run_id,
                    "cancelled",
                    Some("Studio graph cancelled".to_string()),
                ),
            );
            return Err("Studio run cancelled".to_string());
        }
    }

    emit_studio_run_event(
        &app,
        studio_graph_event(
            &run_id,
            "succeeded",
            Some("Studio graph finished".to_string()),
        ),
    );
    clear_studio_run_cancel(&cancels, &run_id);
    Ok(StudioGraphRunResult {
        version: graph.version,
        outputs,
        statuses,
        node_runs,
    })
}

#[tauri::command]
fn read_studio_autosave() -> Result<Option<String>, String> {
    let path = studio_autosave_path();
    if !path.exists() {
        return Ok(None);
    }
    fs::read_to_string(&path)
        .map(Some)
        .map_err(|err| format!("failed to read Studio autosave {}: {err}", path.display()))
}

#[tauri::command]
fn write_studio_autosave(graph_json: String) -> Result<(), String> {
    let graph: StudioWorkflowGraph = serde_json::from_str(&graph_json)
        .map_err(|err| format!("invalid Studio graph JSON: {err}"))?;
    if graph.version != 1 {
        return Err(format!(
            "unsupported Studio graph version: {} (expected 1)",
            graph.version
        ));
    }

    let path = studio_autosave_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
    }
    fs::write(&path, graph_json)
        .map_err(|err| format!("failed to write Studio autosave {}: {err}", path.display()))
}

#[tauri::command]
fn clear_studio_autosave() -> Result<(), String> {
    let path = studio_autosave_path();
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!(
            "failed to remove Studio autosave {}: {err}",
            path.display()
        )),
    }
}

// --- Explicit workflow save/open + project folder ---------------------------
// Beyond the single-slot autosave, the editor can save/open named workflow
// files anywhere on disk and browse a chosen "project folder" of workflows.
// Recents (last project folder + recently opened files) persist next to the
// autosave so the editor reopens where the user left off.

fn studio_recents_path() -> PathBuf {
    studio_workspace_dir().join("recents.workflow.json")
}

/// A `.workflow.json` (or `.json`) file discovered in a project folder.
#[derive(Serialize)]
struct StudioWorkflowFile {
    /// File name including extension (e.g. `poster.workflow.json`).
    name: String,
    path: String,
    modified_ms: Option<u64>,
    size_bytes: u64,
}

/// Persisted editor session pointers: the active project folder and the
/// most-recently-opened workflow files (newest first).
#[derive(Serialize, Deserialize, Default)]
struct StudioRecents {
    #[serde(default)]
    project_dir: Option<String>,
    #[serde(default)]
    current_file: Option<String>,
    #[serde(default)]
    files: Vec<String>,
}

fn studio_is_workflow_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("json"))
        .unwrap_or(false)
}

/// Open a native save dialog scoped to workflow JSON and return the chosen
/// path, or `None` if cancelled.
#[tauri::command]
fn pick_workflow_save_path(
    app: tauri::AppHandle,
    default_name: Option<String>,
    dir: Option<String>,
) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let mut builder = app
        .dialog()
        .file()
        .set_title("Save Workflow")
        .add_filter("Workflow", &["json"])
        .set_file_name(default_name.unwrap_or_else(|| "workflow.json".to_string()));
    if let Some(dir) = dir.as_deref().filter(|d| !d.trim().is_empty()) {
        builder = builder.set_directory(dir);
    }
    builder.blocking_save_file().map(|path| path.to_string())
}

/// Open a native open dialog scoped to workflow JSON and return the chosen
/// path, or `None` if cancelled.
#[tauri::command]
fn pick_workflow_open_path(app: tauri::AppHandle, dir: Option<String>) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let mut builder = app
        .dialog()
        .file()
        .set_title("Open Workflow")
        .add_filter("Workflow", &["json"]);
    if let Some(dir) = dir.as_deref().filter(|d| !d.trim().is_empty()) {
        builder = builder.set_directory(dir);
    }
    builder.blocking_pick_file().map(|path| path.to_string())
}

/// Open a native folder-picker and return the chosen directory, or `None`.
#[tauri::command]
fn pick_project_folder(app: tauri::AppHandle, dir: Option<String>) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let mut builder = app.dialog().file().set_title("Choose Project Folder");
    if let Some(dir) = dir.as_deref().filter(|d| !d.trim().is_empty()) {
        builder = builder.set_directory(dir);
    }
    builder.blocking_pick_folder().map(|path| path.to_string())
}

/// Read a workflow file from disk, validating it parses as a Studio graph.
#[tauri::command]
fn read_studio_workflow(path: String) -> Result<String, String> {
    let path = Path::new(path.trim());
    let text = fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    serde_json::from_str::<StudioWorkflowGraph>(&text)
        .map_err(|err| format!("not a valid Studio workflow ({}): {err}", path.display()))?;
    Ok(text)
}

/// Write a workflow file to disk, validating the payload first and creating
/// parent directories as needed.
#[tauri::command]
fn write_studio_workflow(path: String, graph_json: String) -> Result<(), String> {
    let graph: StudioWorkflowGraph = serde_json::from_str(&graph_json)
        .map_err(|err| format!("invalid Studio graph JSON: {err}"))?;
    if graph.version != 1 {
        return Err(format!(
            "unsupported Studio graph version: {} (expected 1)",
            graph.version
        ));
    }
    let path = Path::new(path.trim());
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
    }
    fs::write(path, graph_json)
        .map_err(|err| format!("failed to write {}: {err}", path.display()))
}

/// List workflow JSON files in a project folder (non-recursive), newest first.
#[tauri::command]
fn list_studio_workflows(dir: String) -> Result<Vec<StudioWorkflowFile>, String> {
    let dir = dir.trim();
    if dir.is_empty() {
        return Err("project folder is empty".to_string());
    }
    let path = Path::new(dir);
    if !path.is_dir() {
        return Err(format!("not a directory: {dir}"));
    }

    let mut files = Vec::new();
    for entry in
        fs::read_dir(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let file_path = entry.path();
        if !file_path.is_file() || !studio_is_workflow_file(&file_path) {
            continue;
        }
        let name = match file_path.file_name().and_then(|s| s.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };
        let metadata = entry.metadata().ok();
        files.push(StudioWorkflowFile {
            name,
            path: file_path.to_string_lossy().to_string(),
            modified_ms: metadata.as_ref().and_then(modified_ms),
            size_bytes: metadata.as_ref().map(|m| m.len()).unwrap_or(0),
        });
    }

    files.sort_by(|a, b| {
        b.modified_ms
            .cmp(&a.modified_ms)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(files)
}

/// Read the persisted editor session pointers (project folder + recent files).
#[tauri::command]
fn read_studio_recents() -> Result<StudioRecents, String> {
    let path = studio_recents_path();
    if !path.exists() {
        return Ok(StudioRecents::default());
    }
    let text = fs::read_to_string(&path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|err| format!("invalid Studio recents {}: {err}", path.display()))
}

/// Persist the editor session pointers (project folder + recent files).
#[tauri::command]
fn write_studio_recents(recents: StudioRecents) -> Result<(), String> {
    let path = studio_recents_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(&recents)
        .map_err(|err| format!("failed to serialize Studio recents: {err}"))?;
    fs::write(&path, text)
        .map_err(|err| format!("failed to write {}: {err}", path.display()))
}

/// Normalize a user-supplied workflow file name: reject empties and path
/// separators, and ensure a `.json` extension.
fn studio_normalize_workflow_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("name is empty".to_string());
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err("name must not contain path separators".to_string());
    }
    let has_json = Path::new(trimmed)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("json"))
        .unwrap_or(false);
    Ok(if has_json {
        trimmed.to_string()
    } else {
        format!("{trimmed}.json")
    })
}

/// Find an unused `"{stem} copy[.N].json"` path next to a source workflow.
fn studio_unique_copy_path(parent: &Path, stem: &str) -> Result<PathBuf, String> {
    let first = parent.join(format!("{stem} copy.json"));
    if !first.exists() {
        return Ok(first);
    }
    for n in 2..1000 {
        let candidate = parent.join(format!("{stem} copy {n}.json"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err("too many copies of this workflow".to_string())
}

/// Rename a workflow file within its folder; returns the new path.
#[tauri::command]
fn rename_studio_workflow(path: String, new_name: String) -> Result<String, String> {
    let from = Path::new(path.trim());
    if !studio_is_workflow_file(from) {
        return Err(format!("not a workflow file: {}", from.display()));
    }
    let parent = from
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| "file has no parent directory".to_string())?;
    let file_name = studio_normalize_workflow_name(&new_name)?;
    let to = parent.join(&file_name);
    if to == from {
        return Ok(from.to_string_lossy().to_string());
    }
    if to.exists() {
        return Err(format!("{file_name} already exists"));
    }
    fs::rename(from, &to)
        .map_err(|err| format!("failed to rename {}: {err}", from.display()))?;
    Ok(to.to_string_lossy().to_string())
}

/// Delete a workflow file from disk.
#[tauri::command]
fn delete_studio_workflow(path: String) -> Result<(), String> {
    let target = Path::new(path.trim());
    if !studio_is_workflow_file(target) {
        return Err(format!("not a workflow file: {}", target.display()));
    }
    fs::remove_file(target)
        .map_err(|err| format!("failed to delete {}: {err}", target.display()))
}

/// Copy a workflow file to a fresh `"… copy.json"` sibling; returns its path.
#[tauri::command]
fn duplicate_studio_workflow(path: String) -> Result<String, String> {
    let from = Path::new(path.trim());
    if !studio_is_workflow_file(from) {
        return Err(format!("not a workflow file: {}", from.display()));
    }
    let parent = from
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| "file has no parent directory".to_string())?;
    let stem = from
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("workflow");
    let to = studio_unique_copy_path(parent, stem)?;
    fs::copy(from, &to).map_err(|err| format!("failed to copy {}: {err}", from.display()))?;
    Ok(to.to_string_lossy().to_string())
}

// --- Project-scoped snapshots -----------------------------------------------
// Named graph snapshots can be persisted into the active project folder (as a
// single `.hgripe-snapshots.json`) so they travel with the project and survive
// a cache wipe / machine change, instead of living only in browser
// localStorage. The renderer owns the JSON shape (an array of snapshots); the
// backend just reads/writes the file, mirroring the autosave slot.

fn studio_snapshots_path(dir: &str) -> Result<PathBuf, String> {
    let dir = dir.trim();
    if dir.is_empty() {
        return Err("project folder is empty".to_string());
    }
    let path = Path::new(dir);
    if !path.is_dir() {
        return Err(format!("not a directory: {dir}"));
    }
    Ok(path.join(".hgripe-snapshots.json"))
}

/// Read the project folder's persisted snapshots file, returning its raw JSON
/// text (an array). Returns `"[]"` when the file does not exist yet.
#[tauri::command]
fn read_studio_snapshots(dir: String) -> Result<String, String> {
    let path = studio_snapshots_path(&dir)?;
    if !path.exists() {
        return Ok("[]".to_string());
    }
    fs::read_to_string(&path).map_err(|err| format!("failed to read {}: {err}", path.display()))
}

/// Write the project folder's snapshots file. `snapshots_json` is the renderer's
/// serialized array; it is validated as JSON before writing.
#[tauri::command]
fn write_studio_snapshots(dir: String, snapshots_json: String) -> Result<(), String> {
    serde_json::from_str::<serde_json::Value>(&snapshots_json)
        .map_err(|err| format!("invalid snapshots JSON: {err}"))?;
    let path = studio_snapshots_path(&dir)?;
    fs::write(&path, snapshots_json)
        .map_err(|err| format!("failed to write {}: {err}", path.display()))
}

#[tauri::command]
fn cancel_studio_run(
    app: tauri::AppHandle,
    cancels: tauri::State<'_, StudioRunCancels>,
    run_id: String,
) -> Result<(), String> {
    let run_id = run_id.trim();
    if run_id.is_empty() {
        return Err("run_id is empty".to_string());
    }
    studio_run_token(&cancels, run_id).cancel();
    emit_studio_run_event(
        &app,
        studio_graph_event(
            run_id,
            "cancelling",
            Some("Studio graph cancellation requested".to_string()),
        ),
    );
    Ok(())
}

#[tauri::command]
async fn rerun_task(task_id: String, disable_cache: bool) -> Result<ApiResult, String> {
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

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("only http(s) URLs are allowed".to_string());
    }
    open_external(&url)
}

/// Open a native file-open dialog and return the chosen path, or `None` if the
/// user cancelled. `filter_name` + `extensions` optionally scope the picker
/// (e.g. images, or `.psd` templates); extensions are bare (no leading dot).
#[tauri::command]
fn pick_file(
    app: tauri::AppHandle,
    title: Option<String>,
    filter_name: Option<String>,
    extensions: Option<Vec<String>>,
) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let mut builder = app.dialog().file();
    if let Some(title) = title {
        builder = builder.set_title(title);
    }
    if let Some(exts) = extensions.as_ref().filter(|e| !e.is_empty()) {
        let refs: Vec<&str> = exts.iter().map(String::as_str).collect();
        builder = builder.add_filter(filter_name.unwrap_or_else(|| "Files".to_string()), &refs);
    }
    builder.blocking_pick_file().map(|path| path.to_string())
}

#[derive(Serialize)]
struct PsdOutputFile {
    /// Base name shared by the triplet (e.g. `final` for `final.psd`).
    name: String,
    psd_path: String,
    preview_path: Option<String>,
    metadata_path: Option<String>,
    /// PSD file modification time in milliseconds since the Unix epoch.
    modified_ms: Option<u64>,
    size_bytes: u64,
    /// True when the export's metadata records a true smart-object content
    /// replacement (`smart_object_mode == "replace_content"`).
    smart_object: bool,
}

/// Cheap check for whether a `_metadata.json` records a smart-object content
/// replacement, without pulling in a JSON parser.
fn metadata_has_smart_object(metadata_path: &Option<String>) -> bool {
    let Some(path) = metadata_path else {
        return false;
    };
    match fs::read_to_string(path) {
        Ok(text) => text.contains("\"smart_object_mode\"") && text.contains("\"replace_content\""),
        Err(_) => false,
    }
}

fn modified_ms(metadata: &fs::Metadata) -> Option<u64> {
    metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}

/// Scan a directory (non-recursively) for PSD exports produced by the PSD
/// nodes and group each `<base>.psd` with its `<base>_preview.png` and
/// `<base>_metadata.json` siblings when present.
#[tauri::command]
fn list_psd_outputs(dir: String) -> Result<Vec<PsdOutputFile>, String> {
    let dir = dir.trim();
    if dir.is_empty() {
        return Err("output directory is empty".to_string());
    }
    let path = Path::new(dir);
    if !path.is_dir() {
        return Err(format!("not a directory: {dir}"));
    }

    let mut outputs = Vec::new();
    for entry in
        fs::read_dir(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let psd_path = entry.path();
        let is_psd = psd_path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("psd"))
            .unwrap_or(false);
        if !is_psd {
            continue;
        }
        let base = match psd_path.file_stem().and_then(|s| s.to_str()) {
            Some(stem) => stem.to_string(),
            None => continue,
        };

        let sibling = |suffix: &str| {
            let candidate = path.join(format!("{base}{suffix}"));
            candidate
                .is_file()
                .then(|| candidate.to_string_lossy().to_string())
        };
        let preview_path = sibling("_preview.png");
        let metadata_path = sibling("_metadata.json");
        let smart_object = metadata_has_smart_object(&metadata_path);

        let metadata = entry.metadata().ok();
        outputs.push(PsdOutputFile {
            name: base,
            psd_path: psd_path.to_string_lossy().to_string(),
            preview_path,
            metadata_path,
            modified_ms: metadata.as_ref().and_then(modified_ms),
            size_bytes: metadata.as_ref().map(|m| m.len()).unwrap_or(0),
            smart_object,
        });
    }

    // Newest first, falling back to name for stable ordering.
    outputs.sort_by(|a, b| {
        b.modified_ms
            .cmp(&a.modified_ms)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(outputs)
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
        let b2 = chunk.get(2).copied().unwrap_or(0) as usize;
        out.push(TABLE[b0 >> 2] as char);
        out.push(TABLE[((b0 & 0x03) << 4) | (b1 >> 4)] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((b1 & 0x0f) << 2) | (b2 >> 6)] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[b2 & 0x3f] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Read an image file and return it as a `data:` URL for inline display.
#[tauri::command]
fn read_image_data_url(path: String) -> Result<String, String> {
    let path = Path::new(path.trim());
    let mime = match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("bmp") => "image/bmp",
        other => return Err(format!("unsupported image type: {}", other.unwrap_or(""))),
    };
    // Guard against accidentally inlining huge files into the webview.
    let metadata =
        fs::metadata(path).map_err(|err| format!("failed to stat {}: {err}", path.display()))?;
    if metadata.len() > 25 * 1024 * 1024 {
        return Err("image is larger than 25 MB".to_string());
    }
    let bytes =
        fs::read(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    Ok(format!("data:{mime};base64,{}", base64_encode(&bytes)))
}

/// FNV-1a 64-bit hash, used to key the thumbnail cache by source content.
fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[derive(Serialize)]
struct ThumbnailResult {
    /// `data:` URL of the generated thumbnail, ready for an `<img src>`.
    data_url: String,
    /// On-disk cached thumbnail path (PNG), reused on subsequent calls.
    cache_path: String,
    /// Thumbnail pixel dimensions (already scaled by dpr).
    width: u32,
    height: u32,
    /// Content hash of the source file (the thumbnail cache key).
    source_hash: String,
    mime: String,
}

/// Generate (or fetch from cache) a crisp thumbnail for an image file.
///
/// The thumbnail is produced at `size * dpr` pixels with Lanczos3 resampling so
/// it stays sharp on high-DPI displays, cached on disk keyed by
/// `source_hash + target_size`, and returned as a `data:` URL for display. The
/// original `path` is never downscaled in the webview and remains the source of
/// truth for execution/export.
#[tauri::command]
fn generate_thumbnail(
    path: String,
    size: u32,
    dpr: Option<f64>,
) -> Result<ThumbnailResult, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("path is empty".to_string());
    }
    let src = Path::new(trimmed);
    if !src.is_file() {
        return Err(format!("file does not exist: {trimmed}"));
    }

    // Target edge in physical pixels, clamped to a sane range.
    let dpr = dpr.unwrap_or(1.0);
    let dpr = if dpr.is_finite() && dpr > 0.0 {
        dpr
    } else {
        1.0
    };
    let target = ((size as f64) * dpr).round() as u32;
    let target = target.clamp(16, 4096);

    let bytes = fs::read(src).map_err(|err| format!("failed to read {}: {err}", src.display()))?;
    let source_hash = fnv1a_hex(&bytes);

    let cache_dir = runtime_paths()?.output_dir.join(".thumbnails");
    fs::create_dir_all(&cache_dir)
        .map_err(|err| format!("failed to create {}: {err}", cache_dir.display()))?;
    let cache_path = cache_dir.join(format!("{source_hash}_{target}.png"));

    // Cache hit: reuse the previously generated thumbnail.
    if let Ok(cached) = fs::read(&cache_path) {
        if let Ok(decoded) = image::load_from_memory(&cached) {
            return Ok(ThumbnailResult {
                data_url: format!("data:image/png;base64,{}", base64_encode(&cached)),
                cache_path: cache_path.to_string_lossy().to_string(),
                width: decoded.width(),
                height: decoded.height(),
                source_hash,
                mime: "image/png".to_string(),
            });
        }
    }

    let source =
        image::load_from_memory(&bytes).map_err(|err| format!("failed to decode image: {err}"))?;
    // `resize` preserves aspect ratio, fitting within target x target.
    let thumb = source.resize(target, target, image::imageops::FilterType::Lanczos3);

    let mut png: Vec<u8> = Vec::new();
    thumb
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|err| format!("failed to encode thumbnail: {err}"))?;
    // Best-effort cache write; a failure here should not fail the request.
    let _ = fs::write(&cache_path, &png);

    Ok(ThumbnailResult {
        data_url: format!("data:image/png;base64,{}", base64_encode(&png)),
        cache_path: cache_path.to_string_lossy().to_string(),
        width: thumb.width(),
        height: thumb.height(),
        source_hash,
        mime: "image/png".to_string(),
    })
}

/// Read a text file, truncating to `max_bytes` so large files cannot freeze
/// the UI. A truncation marker is appended when the file is clipped.
#[tauri::command]
fn read_text_file(path: String, max_bytes: usize) -> Result<String, String> {
    let path = Path::new(path.trim());
    let bytes =
        fs::read(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let limit = if max_bytes == 0 {
        bytes.len()
    } else {
        max_bytes
    };
    if bytes.len() > limit {
        let mut end = limit;
        // Avoid slicing in the middle of a UTF-8 sequence.
        while end > 0 && (bytes[end] & 0xC0) == 0x80 {
            end -= 1;
        }
        let mut text = String::from_utf8_lossy(&bytes[..end]).to_string();
        text.push_str("\n… (truncated)");
        Ok(text)
    } else {
        Ok(String::from_utf8_lossy(&bytes).to_string())
    }
}

/// Open a local file or folder with the OS default handler.
#[tauri::command]
fn open_path(path: String) -> Result<(), String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("path is empty".to_string());
    }
    if !Path::new(trimmed).exists() {
        return Err(format!("path does not exist: {trimmed}"));
    }
    open_external(trimmed)
}

// NOTE: Long term this should move to the official `tauri-plugin-opener`
// (Tauri 2) so opening files/URLs goes through a vetted, permissioned path
// rather than spawning a child process here. Until then we invoke the OS
// handler directly without going through `cmd /C start`, whose shell re-parses
// metacharacters (`&`, `^`, `%`, …) in the target. `rundll32 url.dll,
// FileProtocolHandler` opens http(s) URLs, files, and folders via the default
// handler and receives the target as a single, un-reparsed argv element.
#[cfg(target_os = "windows")]
fn open_external(url: &str) -> Result<(), String> {
    std::process::Command::new("rundll32.exe")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn()
        .map(|_| ())
        .map_err(|err| err.to_string())
}

#[cfg(target_os = "macos")]
fn open_external(url: &str) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|err| err.to_string())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_external(url: &str) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|err| err.to_string())
}

/// Holds the locally spawned ComfyUI server process, if any, so the desktop
/// shell can act as a launcher (start / stop) for the embedded UI.
#[derive(Default)]
struct ComfyServer(Mutex<Option<Child>>);

/// Resolve the ComfyUI project directory: the caller-provided path, else the
/// process working directory (the repo root in dev / the install dir packaged).
fn resolve_comfy_dir(dir: &Option<String>) -> Result<PathBuf, String> {
    let base = match dir {
        Some(d) if !d.trim().is_empty() => PathBuf::from(d.trim()),
        _ => std::env::current_dir().map_err(|err| err.to_string())?,
    };
    if !base.join("main.py").is_file() {
        return Err(format!(
            "ComfyUI main.py not found in {} (set the ComfyUI folder)",
            base.display()
        ));
    }
    Ok(base)
}

/// Pick a Python interpreter: prefer the bundled `python_embeded` shipped with
/// the ComfyUI Windows distribution, otherwise fall back to PATH `python`.
fn comfy_python(dir: &Path) -> PathBuf {
    for candidate in [
        dir.join("python_embeded").join("python.exe"),
        dir.join("python_embeded").join("python"),
    ] {
        if candidate.is_file() {
            return candidate;
        }
    }
    PathBuf::from(if cfg!(windows) { "python" } else { "python3" })
}

#[tauri::command]
fn comfyui_reachable(port: Option<u16>) -> bool {
    let port = port.unwrap_or(8188);
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_millis(400),
    )
    .is_ok()
}

#[tauri::command]
fn comfyui_status(state: tauri::State<'_, ComfyServer>) -> bool {
    let mut guard = state.0.lock().unwrap();
    match guard.as_mut() {
        Some(child) => match child.try_wait() {
            Ok(Some(_)) => {
                // Process has exited; clear the slot.
                *guard = None;
                false
            }
            Ok(None) => true,
            Err(_) => false,
        },
        None => false,
    }
}

#[tauri::command]
fn start_comfyui(
    state: tauri::State<'_, ComfyServer>,
    dir: Option<String>,
    port: Option<u16>,
    args: Option<String>,
) -> Result<String, String> {
    let mut guard = state.0.lock().unwrap();
    if let Some(child) = guard.as_mut() {
        if matches!(child.try_wait(), Ok(None)) {
            return Err("ComfyUI is already running".to_string());
        }
    }
    let dir = resolve_comfy_dir(&dir)?;
    let python = comfy_python(&dir);
    let port = port.unwrap_or(8188);

    // Bootstrap that injects the project dir onto sys.path at runtime before
    // running main.py as __main__. This works even with the restrictive
    // `._pth` of embeddable Python builds (which ignore PYTHONPATH and do not
    // auto-add the script directory), as well as normal/standalone Python.
    // Extra CLI args (e.g. `--cpu`, `--listen`, `--lowvram`) are passed through
    // HG_COMFY_ARGS and split on whitespace.
    let bootstrap = "import os, sys, runpy; d = os.environ['HG_COMFY_DIR']; \
sys.argv = ['main.py', '--port', os.environ['HG_COMFY_PORT']] + os.environ.get('HG_COMFY_ARGS', '').split(); \
sys.path.insert(0, d); \
runpy.run_path(os.path.join(d, 'main.py'), run_name='__main__')";
    let mut cmd = std::process::Command::new(&python);
    cmd.arg("-c")
        .arg(bootstrap)
        .current_dir(&dir)
        .env("HG_COMFY_DIR", &dir)
        .env("HG_COMFY_PORT", port.to_string())
        .env("HG_COMFY_ARGS", args.unwrap_or_default());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: don't pop a console window for the child.
        cmd.creation_flags(0x0800_0000);
    }

    let child = cmd
        .spawn()
        .map_err(|err| format!("failed to launch {}: {err}", python.display()))?;
    *guard = Some(child);
    Ok(format!("started ComfyUI on port {port}"))
}

#[tauri::command]
fn stop_comfyui(state: tauri::State<'_, ComfyServer>) -> Result<(), String> {
    let mut guard = state.0.lock().unwrap();
    if let Some(mut child) = guard.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    Ok(())
}

/// Result of a `compose_psd` run, mirroring the JSON printed by the
/// `compose_psd_cli.py` helper.
#[derive(Serialize, Deserialize)]
struct ComposePsdResult {
    status: String,
    psd_path: String,
    /// Empty string when preview generation was disabled.
    preview_path: String,
    metadata_path: String,
    placeholder_kind: Option<String>,
    smart_object_mode: String,
}

/// Compose a generated image into a PSD template's placeholder (true
/// smart-object content replacement when applicable) and export
/// `<filename>.psd` + `<filename>_preview.png` + `<filename>_metadata.json`.
///
/// This shells out to `python/bridge/compose_psd_cli.py` using the same Python
/// interpreter resolution as ComfyUI (`python_embeded` when present), so it
/// reuses the proven, vendored psd-tools pipeline without requiring a running
/// ComfyUI server. `dir` is the ComfyUI/project root (defaults to the process
/// working dir); the rest map 1:1 onto the CLI flags.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn compose_psd(
    dir: Option<String>,
    template: String,
    image: String,
    output_dir: String,
    filename: Option<String>,
    placeholder: Option<String>,
    fit_mode: Option<String>,
    z_order: Option<String>,
    smart_object_mode: Option<String>,
    hide_placeholder: Option<String>,
    metadata: Option<String>,
    save_preview: Option<bool>,
) -> Result<ComposePsdResult, String> {
    let dir = resolve_comfy_dir(&dir)?;
    let python = comfy_python(&dir);
    let script = dir.join("python").join("bridge").join("compose_psd_cli.py");
    if !script.is_file() {
        return Err(format!(
            "compose_psd_cli.py not found at {}",
            script.display()
        ));
    }

    let mut cmd = std::process::Command::new(&python);
    cmd.arg(&script)
        .arg("--template")
        .arg(&template)
        .arg("--image")
        .arg(&image)
        .arg("--output-dir")
        .arg(&output_dir)
        .arg("--filename")
        .arg(filename.as_deref().unwrap_or("final"))
        .arg("--placeholder")
        .arg(placeholder.as_deref().unwrap_or("{}"))
        .arg("--fit-mode")
        .arg(fit_mode.as_deref().unwrap_or("contain"))
        .arg("--z-order")
        .arg(z_order.as_deref().unwrap_or("above_background"))
        .arg("--smart-object-mode")
        .arg(smart_object_mode.as_deref().unwrap_or("disable"))
        .arg("--hide-placeholder")
        .arg(hide_placeholder.as_deref().unwrap_or("enable"))
        .arg("--metadata")
        .arg(metadata.as_deref().unwrap_or("{}"))
        .arg("--save-preview")
        .arg(if save_preview.unwrap_or(true) {
            "enable"
        } else {
            "disable"
        })
        .current_dir(&dir);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: don't pop a console window for the child.
        cmd.creation_flags(0x0800_0000);
    }

    let output = cmd
        .output()
        .map_err(|err| format!("failed to launch {}: {err}", python.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("compose_psd failed: {}", stderr.trim()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str::<ComposePsdResult>(stdout.trim()).map_err(|err| {
        format!(
            "could not parse compose_psd output: {err} (raw: {})",
            stdout.trim()
        )
    })
}

/// A single PSD layer, mirroring the rows printed by `inspect_psd_cli.py`.
#[derive(Serialize, Deserialize)]
struct PsdLayerInfo {
    name: String,
    /// "group" | "smartobject" | "pixel".
    kind: String,
}

/// Result of an `inspect_psd` run, mirroring the JSON printed by the
/// `inspect_psd_cli.py` helper.
#[derive(Serialize, Deserialize)]
struct InspectPsdResult {
    status: String,
    /// `false` when the template path does not point at a file on disk.
    exists: bool,
    width: u32,
    height: u32,
    /// Flat list of every layer (groups and their children), newest-first as
    /// PSD stores them.
    layers: Vec<PsdLayerInfo>,
    /// Subset of the requested `names` that were not found in the PSD.
    missing: Vec<String>,
}

/// Inspect a PSD template: report whether it exists on disk, its canvas size,
/// and the names/kinds of its layers, plus which of the requested placeholder
/// `names` are missing. This lets the editor validate a real PSD before a run
/// (file present, placeholder layer name actually exists) instead of only
/// surfacing the problem mid-compose.
///
/// Like `compose_psd`, this shells out to `python/bridge/inspect_psd_cli.py`
/// using the same Python interpreter resolution as ComfyUI, reusing the
/// vendored psd-tools pipeline without a running ComfyUI server.
#[tauri::command]
fn inspect_psd(
    dir: Option<String>,
    template: String,
    names: Option<Vec<String>>,
) -> Result<InspectPsdResult, String> {
    let dir = resolve_comfy_dir(&dir)?;
    let python = comfy_python(&dir);
    let script = dir.join("python").join("bridge").join("inspect_psd_cli.py");
    if !script.is_file() {
        return Err(format!("inspect_psd_cli.py not found at {}", script.display()));
    }
    let names_json =
        serde_json::to_string(&names.unwrap_or_default()).map_err(|err| err.to_string())?;

    let mut cmd = std::process::Command::new(&python);
    cmd.arg(&script)
        .arg("--template")
        .arg(&template)
        .arg("--names")
        .arg(&names_json)
        .current_dir(&dir);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: don't pop a console window for the child.
        cmd.creation_flags(0x0800_0000);
    }

    let output = cmd
        .output()
        .map_err(|err| format!("failed to launch {}: {err}", python.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("inspect_psd failed: {}", stderr.trim()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str::<InspectPsdResult>(stdout.trim()).map_err(|err| {
        format!(
            "could not parse inspect_psd output: {err} (raw: {})",
            stdout.trim()
        )
    })
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(ComfyServer::default())
        .manage(StudioRunCancels::default())
        .invoke_handler(tauri::generate_handler![
            get_runtime_info,
            doctor,
            get_credentials,
            check_credentials,
            get_profiles,
            check_profiles,
            read_config_file,
            write_config_file,
            list_history,
            history_detail,
            history_cleanup_preview,
            history_cleanup_apply,
            run_task,
            run_task_json,
            run_studio_graph,
            read_studio_autosave,
            write_studio_autosave,
            clear_studio_autosave,
            pick_workflow_save_path,
            pick_workflow_open_path,
            pick_project_folder,
            read_studio_workflow,
            write_studio_workflow,
            list_studio_workflows,
            rename_studio_workflow,
            delete_studio_workflow,
            duplicate_studio_workflow,
            read_studio_snapshots,
            write_studio_snapshots,
            read_studio_recents,
            write_studio_recents,
            cancel_studio_run,
            rerun_task,
            open_url,
            pick_file,
            list_psd_outputs,
            read_image_data_url,
            generate_thumbnail,
            read_text_file,
            open_path,
            comfyui_reachable,
            comfyui_status,
            start_comfyui,
            stop_comfyui,
            compose_psd,
            inspect_psd
        ])
        .run(tauri::generate_context!())
        .expect("error while running H-Gripe Desktop");
}
