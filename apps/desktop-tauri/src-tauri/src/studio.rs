//! Studio graph editor backend: the workflow graph schema, the topological
//! execution engine (generate / PSD-export / compare / logic nodes), run-event
//! emission with per-run cancellation, and on-disk persistence for autosave,
//! workflow files, recents, snapshots, and run history.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use hgripe_api::{
    credentials_file_path, record_task_failure, record_task_result, ApiErrorInfo, ApiResult,
    ApiStatus, ApiTask, BrokerError, CancellationToken, OutputType, ProviderExecutionContext,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::Emitter;

use crate::psd::compose_psd;
use crate::{broker, modified_ms, runtime_paths};

const STUDIO_GRAPH_RUN_EVENT: &str = "studio:graph-run";

/// Per-run cancellation tokens for in-flight Studio graph executions, keyed by
/// run id. Shared with the front-end as Tauri managed state.
#[derive(Default)]
pub(crate) struct StudioRunCancels(Mutex<HashMap<String, CancellationToken>>);

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
pub(crate) struct StudioGraphRunResult {
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

/// Booster tags appended (deduped) per local preset. Mirrors the TS
/// `PRESET_TAGS` in `runtime/promptOptimize.ts`; keep both in sync.
fn studio_preset_tags(preset: &str) -> &'static [&'static str] {
    match preset {
        "photographic" => &[
            "photorealistic",
            "high detail",
            "sharp focus",
            "natural lighting",
            "8k",
        ],
        "anime" => &["anime style", "vibrant colors", "clean lineart", "highly detailed"],
        "cinematic" => &[
            "cinematic lighting",
            "dramatic composition",
            "depth of field",
            "film grain",
        ],
        "detailed" => &["highly detailed", "intricate", "ultra quality", "masterpiece"],
        _ => &[],
    }
}

/// Model-free prompt optimisation for the `local` mode. Mirrors the TS
/// `optimizePromptLocally`: collapse whitespace, split on commas, drop empties,
/// case-insensitively dedupe (keeping first occurrence), then append the
/// preset's booster tags (also deduped). An empty input yields an empty string.
fn studio_optimize_prompt_locally(text: &str, preset: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut segments: Vec<String> = collapsed
        .split(',')
        .map(|segment| segment.trim().to_string())
        .filter(|segment| !segment.is_empty())
        .collect();
    if !segments.is_empty() {
        for tag in studio_preset_tags(preset) {
            segments.push((*tag).to_string());
        }
    }

    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for segment in segments {
        if seen.insert(segment.to_lowercase()) {
            out.push(segment);
        }
    }
    out.join(", ")
}

async fn execute_studio_prompt_optimize(
    node: &StudioGraphNode,
    inputs: &BTreeMap<String, Value>,
    cancels: &tauri::State<'_, StudioRunCancels>,
    run_id: &str,
) -> Result<BTreeMap<String, Value>, String> {
    let raw = if inputs.contains_key("text") {
        studio_value_to_string(inputs.get("text"))
    } else {
        studio_value_to_string(node.params.get("text"))
    };
    let mode = studio_value_to_string(node.params.get("mode"));

    match mode.as_str() {
        "local" => {
            let preset = studio_value_to_string(node.params.get("preset"));
            let optimized = studio_optimize_prompt_locally(&raw, preset.trim());
            Ok(studio_output_map([("text", json!(optimized))]))
        }
        "api" => {
            if raw.trim().is_empty() {
                return Ok(studio_output_map([("text", json!(raw))]));
            }
            let mut provider = studio_value_to_string(node.params.get("provider"))
                .trim()
                .to_string();
            if provider.is_empty() {
                provider = "openai_compatible".to_string();
            }
            let mut task = ApiTask::new(provider, "text.generate".to_string());
            task.id = studio_task_id(&node.id);
            task.output_type = OutputType::Text;
            task.cache_policy.enabled = false;
            task.retry_policy.max_attempts = 1;
            task.retry_policy.backoff_ms = 200;
            task.retry_policy.timeout_ms = Some(60_000);
            task.inputs.insert("prompt".to_string(), json!(raw));

            let model = studio_value_to_string(node.params.get("model"));
            if !model.trim().is_empty() {
                task.params.insert("model".to_string(), json!(model));
            }
            let instruction = studio_value_to_string(node.params.get("instruction"));
            if !instruction.trim().is_empty() {
                task.params
                    .insert("system_prompt".to_string(), json!(instruction));
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
                    .unwrap_or_else(|| "prompt optimization failed".to_string());
                return Err(message);
            }
            let optimized = result
                .output_json
                .as_ref()
                .and_then(|value| value.get("text"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| raw.clone());
            let result_json = serde_json::to_value(result)
                .map_err(|err| format!("failed to encode ApiResult: {err}"))?;
            Ok(studio_output_map([
                ("text", json!(optimized)),
                ("result", result_json),
            ]))
        }
        _ => Ok(studio_output_map([("text", json!(raw))])),
    }
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
        "promptOptimize" => execute_studio_prompt_optimize(node, &inputs, cancels, run_id).await,
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
pub(crate) async fn run_studio_graph(
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
pub(crate) fn read_studio_autosave() -> Result<Option<String>, String> {
    let path = studio_autosave_path();
    if !path.exists() {
        return Ok(None);
    }
    fs::read_to_string(&path)
        .map(Some)
        .map_err(|err| format!("failed to read Studio autosave {}: {err}", path.display()))
}

#[tauri::command]
pub(crate) fn write_studio_autosave(graph_json: String) -> Result<(), String> {
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
pub(crate) fn clear_studio_autosave() -> Result<(), String> {
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
pub(crate) struct StudioWorkflowFile {
    /// File name including extension (e.g. `poster.workflow.json`).
    name: String,
    path: String,
    modified_ms: Option<u64>,
    size_bytes: u64,
}

/// Persisted editor session pointers: the active project folder and the
/// most-recently-opened workflow files (newest first).
#[derive(Serialize, Deserialize, Default)]
pub(crate) struct StudioRecents {
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
pub(crate) fn pick_workflow_save_path(
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
pub(crate) fn pick_workflow_open_path(app: tauri::AppHandle, dir: Option<String>) -> Option<String> {
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
pub(crate) fn pick_project_folder(app: tauri::AppHandle, dir: Option<String>) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let mut builder = app.dialog().file().set_title("Choose Project Folder");
    if let Some(dir) = dir.as_deref().filter(|d| !d.trim().is_empty()) {
        builder = builder.set_directory(dir);
    }
    builder.blocking_pick_folder().map(|path| path.to_string())
}

/// Read a workflow file from disk, validating it parses as a Studio graph.
#[tauri::command]
pub(crate) fn read_studio_workflow(path: String) -> Result<String, String> {
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
pub(crate) fn write_studio_workflow(path: String, graph_json: String) -> Result<(), String> {
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
pub(crate) fn list_studio_workflows(dir: String) -> Result<Vec<StudioWorkflowFile>, String> {
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
pub(crate) fn read_studio_recents() -> Result<StudioRecents, String> {
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
pub(crate) fn write_studio_recents(recents: StudioRecents) -> Result<(), String> {
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

/// Reject a user-supplied base file name that could escape the directory it is
/// later joined onto (path separators, or a `.`/`..` component). Used for
/// export targets where a downstream helper does `directory / name`, so an
/// untrusted workflow cannot redirect the write outside the chosen folder.
pub(crate) fn studio_reject_unsafe_basename(name: &str) -> Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("filename is empty".to_string());
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err("filename must not contain path separators".to_string());
    }
    if trimmed == "." || trimmed == ".." {
        return Err("filename is not a valid name".to_string());
    }
    Ok(())
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
pub(crate) fn rename_studio_workflow(path: String, new_name: String) -> Result<String, String> {
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
pub(crate) fn delete_studio_workflow(path: String) -> Result<(), String> {
    let target = Path::new(path.trim());
    if !studio_is_workflow_file(target) {
        return Err(format!("not a workflow file: {}", target.display()));
    }
    fs::remove_file(target)
        .map_err(|err| format!("failed to delete {}: {err}", target.display()))
}

/// Copy a workflow file to a fresh `"… copy.json"` sibling; returns its path.
#[tauri::command]
pub(crate) fn duplicate_studio_workflow(path: String) -> Result<String, String> {
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

// --- Project-scoped JSON stores ---------------------------------------------
// Some renderer state (named graph snapshots, run history) can be persisted
// into the active project folder as a single JSON file so it travels with the
// project and survives a cache wipe / machine change, instead of living only in
// browser localStorage. The renderer owns the JSON shape (an array); the
// backend just reads/writes one file per store, mirroring the autosave slot.

/// Resolve `<dir>/<filename>`, validating that `dir` is a real directory.
fn studio_store_path(dir: &str, filename: &str) -> Result<PathBuf, String> {
    let dir = dir.trim();
    if dir.is_empty() {
        return Err("project folder is empty".to_string());
    }
    let path = Path::new(dir);
    if !path.is_dir() {
        return Err(format!("not a directory: {dir}"));
    }
    Ok(path.join(filename))
}

/// Read a project-scoped store file as raw JSON text, or `"[]"` if absent.
fn read_studio_store(dir: &str, filename: &str) -> Result<String, String> {
    let path = studio_store_path(dir, filename)?;
    if !path.exists() {
        return Ok("[]".to_string());
    }
    fs::read_to_string(&path).map_err(|err| format!("failed to read {}: {err}", path.display()))
}

/// Write `json` to a project-scoped store file, validating it as JSON first.
fn write_studio_store(dir: &str, filename: &str, json: &str) -> Result<(), String> {
    serde_json::from_str::<serde_json::Value>(json)
        .map_err(|err| format!("invalid JSON for {filename}: {err}"))?;
    let path = studio_store_path(dir, filename)?;
    fs::write(&path, json).map_err(|err| format!("failed to write {}: {err}", path.display()))
}

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

#[tauri::command]
pub(crate) fn cancel_studio_run(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reject_unsafe_basename_accepts_plain_names() {
        assert!(studio_reject_unsafe_basename("final").is_ok());
        assert!(studio_reject_unsafe_basename("  result  ").is_ok());
        assert!(studio_reject_unsafe_basename("my.output").is_ok());
    }

    #[test]
    fn reject_unsafe_basename_rejects_traversal_and_separators() {
        assert!(studio_reject_unsafe_basename("").is_err());
        assert!(studio_reject_unsafe_basename("   ").is_err());
        assert!(studio_reject_unsafe_basename(".").is_err());
        assert!(studio_reject_unsafe_basename("..").is_err());
        assert!(studio_reject_unsafe_basename("../evil").is_err());
        assert!(studio_reject_unsafe_basename("..\\evil").is_err());
        assert!(studio_reject_unsafe_basename("sub/dir").is_err());
        assert!(studio_reject_unsafe_basename("/etc/passwd").is_err());
    }

    #[test]
    fn optimize_prompt_locally_normalises_and_dedupes() {
        assert_eq!(
            studio_optimize_prompt_locally("  a fox ,   running  \n , river ", "cleanup"),
            "a fox, running, river"
        );
        assert_eq!(
            studio_optimize_prompt_locally("Fox, fox, FOX, river", "cleanup"),
            "Fox, river"
        );
    }

    #[test]
    fn optimize_prompt_locally_appends_preset_tags_deduped() {
        assert_eq!(
            studio_optimize_prompt_locally("a cat", "photographic"),
            "a cat, photorealistic, high detail, sharp focus, natural lighting, 8k"
        );
        assert_eq!(
            studio_optimize_prompt_locally("a cat, masterpiece", "detailed"),
            "a cat, masterpiece, highly detailed, intricate, ultra quality"
        );
    }

    #[test]
    fn optimize_prompt_locally_empty_stays_empty() {
        assert_eq!(studio_optimize_prompt_locally("   ", "photographic"), "");
        assert_eq!(studio_optimize_prompt_locally("", "cleanup"), "");
    }
}
