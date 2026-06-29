//! The Studio graph execution engine: topological ordering, per-node execution
//! (prompt / batch / logic / generate / promptOptimize / psdExport / …), dead-
//! branch pruning, run-event emission to the webview, and per-run cancellation.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use hgripe_api::{
    record_task_failure, record_task_result, ApiErrorInfo, ApiResult, ApiStatus, ApiTask,
    BrokerError, CancellationToken, OutputType, ProviderExecutionContext,
};
use serde::Serialize;
use serde_json::{json, Value};
use tauri::Emitter;

use super::color_match::execute_studio_match_light_color;
use super::detail_watchdog::execute_studio_detail_watchdog;
use super::edge_refine::execute_studio_refine_mask_edge;
use super::graph::{
    studio_non_empty, studio_output_map, studio_truthy, studio_value_to_number,
    studio_value_to_string, StudioGraphEdge, StudioGraphNode, StudioWorkflowGraph,
};
use super::image_enhance::execute_studio_image_enhance;
use super::psd_analyze::execute_studio_psd_context_analyze;
use super::psd_export::execute_studio_psd_export;
use crate::broker;
use crate::psd::{composite_repaint, prepare_repaint_regions};

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

fn studio_task_id(node_id: &str) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    format!("studio-{node_id}-{millis}")
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

/// The `detailRepaint` node executor: localized issue-region repaint built on
/// top of a Detail Watchdog `QualityReport`. Crops each repaintable issue (via
/// `prepare_repaint_regions`), sends each crop + inpaint mask + repaint prompt
/// through the broker's `image.edit` operation (the same provider/credentials
/// path as `generate`), then pastes the results back with a feathered seam
/// (`composite_repaint`). Outputs the fixed image and a `RepaintReport`.
///
/// When no `image.edit`-capable provider is configured (empty or `mock`), the
/// provider loop is skipped and the node passes the image through unchanged
/// (`repaint_report.status == "unchanged"`), mirroring the mock behaviour of
/// the other production nodes.
async fn execute_studio_detail_repaint(
    node: &StudioGraphNode,
    inputs: &BTreeMap<String, Value>,
    cancels: &tauri::State<'_, StudioRunCancels>,
    run_id: &str,
) -> Result<BTreeMap<String, Value>, String> {
    let image = studio_value_to_string(inputs.get("image"));
    if image.trim().is_empty() {
        return Err("Detail Repaint needs a connected image input".to_string());
    }

    // The QualityReport from Detail Watchdog, forwarded to the CLI as JSON.
    let quality_report = match inputs.get("quality_report") {
        Some(value) if !value.is_null() => Some(
            serde_json::to_string(value)
                .map_err(|err| format!("failed to encode quality_report input: {err}"))?,
        ),
        _ => None,
    };

    let output_dir = {
        let configured = studio_value_to_string(node.params.get("output_dir"));
        if configured.trim().is_empty() {
            crate::runtime_paths()?
                .output_dir
                .to_string_lossy()
                .to_string()
        } else {
            configured
        }
    };
    let output_name = {
        let configured = studio_value_to_string(node.params.get("output_name"));
        let trimmed = configured.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    };

    let repaint_actions = {
        let configured = studio_value_to_string(node.params.get("repaint_actions"));
        let trimmed = configured.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    };
    let min_confidence = studio_param_f64(node, "min_confidence");
    let padding = studio_param_i64(node, "region_padding");
    let max_regions = studio_param_i64(node, "max_regions");
    let feather_px = studio_param_f64(node, "feather_px");

    let prepared = prepare_repaint_regions(
        None,
        image.clone(),
        quality_report,
        repaint_actions,
        min_confidence,
        padding,
        max_regions,
        Some(false),
        Some(output_dir.clone()),
        None,
    )?;
    let manifest = serde_json::to_string(&prepared)
        .map_err(|err| format!("failed to encode repaint manifest: {err}"))?;

    let provider = studio_value_to_string(node.params.get("provider"))
        .trim()
        .to_string();
    let operation = {
        let configured = studio_value_to_string(node.params.get("operation"));
        let trimmed = configured.trim();
        if trimmed.is_empty() {
            "image.edit".to_string()
        } else {
            trimmed.to_string()
        }
    };
    let credentials_ref = studio_value_to_string(node.params.get("credentials_ref"))
        .trim()
        .to_string();
    let prompt_base = studio_value_to_string(node.params.get("repaint_prompt_base"))
        .trim()
        .to_string();

    // Only call a real provider; mock/empty means no `image.edit` capability,
    // so we leave every region unrepainted and pass the image through.
    let provider_can_edit = !provider.is_empty() && provider != "mock";
    let mut repainted: Vec<Value> = Vec::new();
    if provider_can_edit {
        for region in &prepared.regions {
            let mut task = ApiTask::new(provider.clone(), operation.clone());
            task.id = studio_task_id(&node.id);
            task.output_type = OutputType::Image;
            task.cache_policy.enabled = false;
            task.retry_policy.max_attempts = 1;
            task.retry_policy.backoff_ms = 200;
            task.retry_policy.timeout_ms = Some(120_000);

            task.inputs
                .insert("image_path".to_string(), json!(region.crop_path));
            task.inputs
                .insert("mask_path".to_string(), json!(region.mask_path));
            let issue = region.issue_type.clone().unwrap_or_default();
            let prompt = if prompt_base.is_empty() {
                let label = if issue.is_empty() { "flagged" } else { &issue };
                format!(
                    "Repaint and restore this {label} region with clean, realistic detail; \
                     keep the style, lighting and colours consistent with the surroundings."
                )
            } else if issue.is_empty() {
                prompt_base.clone()
            } else {
                format!("{prompt_base} (issue: {issue})")
            };
            task.inputs.insert("prompt".to_string(), json!(prompt));
            task.params.insert("save_outputs".to_string(), json!(true));

            for (key, value) in &node.params {
                if matches!(
                    key.as_str(),
                    "provider"
                        | "operation"
                        | "credentials_ref"
                        | "repaint_prompt_base"
                        | "repaint_actions"
                        | "min_confidence"
                        | "region_padding"
                        | "max_regions"
                        | "feather_px"
                        | "output_dir"
                        | "output_name"
                ) {
                    continue;
                }
                if studio_non_empty(value) {
                    task.params.insert(key.clone(), value.clone());
                }
            }
            if !credentials_ref.is_empty() {
                task.credentials_ref = Some(credentials_ref.clone());
            }

            let result = execute_and_record_cancellable(task, cancels, run_id).await?;
            if matches!(result.status, ApiStatus::Succeeded | ApiStatus::Cached) {
                if let Some(file) = result.output_files.first() {
                    repainted.push(json!({ "index": region.index, "path": file.path.clone() }));
                }
            }
            // A per-region provider failure leaves that region unrepainted
            // rather than aborting the whole node.
        }
    }

    let repainted_json = serde_json::to_string(&repainted)
        .map_err(|err| format!("failed to encode repainted list: {err}"))?;
    let composed = composite_repaint(
        None,
        image,
        manifest,
        repainted_json,
        feather_px,
        Some(output_dir),
        output_name,
    )?;

    let report = serde_json::to_value(&composed.repaint_report)
        .map_err(|err| format!("failed to encode RepaintReport: {err}"))?;
    Ok(studio_output_map([
        ("fixed_image", json!(composed.fixed_image)),
        ("repaint_report", report),
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
        "anime" => &[
            "anime style",
            "vibrant colors",
            "clean lineart",
            "highly detailed",
        ],
        "cinematic" => &[
            "cinematic lighting",
            "dramatic composition",
            "depth of field",
            "film grain",
        ],
        "detailed" => &[
            "highly detailed",
            "intricate",
            "ultra quality",
            "masterpiece",
        ],
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

// Providers whose broker `supports("text.generate")` returns true. Mirrors the
// capability declared in crates/hgripe-api (openai_compatible + mock) and the
// TS `promptOptimizeProviderSupported`; keep all three in sync.
fn studio_prompt_optimize_provider_supported(provider: &str) -> bool {
    matches!(provider.trim(), "openai_compatible" | "mock")
}

/// Read an optional numeric param (accepts a JSON number or a numeric string;
/// blank/non-numeric yields `None` so the field is omitted from the task).
fn studio_param_f64(node: &StudioGraphNode, key: &str) -> Option<f64> {
    match node.params.get(key) {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                trimmed.parse::<f64>().ok()
            }
        }
        _ => None,
    }
}

/// Like [`studio_param_f64`] but truncates to an integer (for `max_tokens`/`seed`).
fn studio_param_i64(node: &StudioGraphNode, key: &str) -> Option<i64> {
    studio_param_f64(node, key).map(|value| value as i64)
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
            if !studio_prompt_optimize_provider_supported(&provider) {
                return Err(format!(
                    "Provider \"{provider}\" can't optimize prompts (no text.generate support). \
                     Pick an OpenAI-compatible chat profile, or switch mode to \"local\"/\"off\"."
                ));
            }
            let mut task = ApiTask::new(provider, "text.generate".to_string());
            task.id = studio_task_id(&node.id);
            task.output_type = OutputType::Text;
            // Cache identical optimisations (same text+instruction+model+sampling)
            // so re-runs don't re-bill the LLM; the broker derives the key.
            task.cache_policy.enabled = true;
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
            // Optional sampling controls (forwarded to the chat call when set).
            if let Some(temperature) = studio_param_f64(node, "temperature") {
                task.params
                    .insert("temperature".to_string(), json!(temperature));
            }
            if let Some(max_tokens) = studio_param_i64(node, "max_tokens") {
                task.params
                    .insert("max_tokens".to_string(), json!(max_tokens));
            }
            if let Some(seed) = studio_param_i64(node, "seed") {
                task.params.insert("seed".to_string(), json!(seed));
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

/// Where a Studio node runs. Authoritative server-side mirror of the
/// `executor` field on the TS `NodeSpec` (studio-ui/src/graph/nodeSpecs.ts).
/// Routing is driven by this classification (never by a client-supplied
/// field), so a `local` card can only reach `python/bridge` handlers and an
/// `api` card can only reach the broker — they can't be swapped by a crafted
/// graph. See docs/card-executor-split-and-psd-chain-hardening.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StudioExecutor {
    /// Pure in-process node (no backend call, no network).
    Graph,
    /// Always a `python/bridge` CLI; must not touch the network.
    Local,
    /// Always a provider call through the broker.
    Api,
    /// User picks per-node via a `mode` param (`promptOptimize`).
    Hybrid,
}

/// Classify a node kind. Returns `None` for an unknown kind (the single
/// gate for unsupported kinds). Keep in sync with `nodeSpecs.ts`.
pub(crate) fn studio_executor_for_kind(kind: &str) -> Option<StudioExecutor> {
    use StudioExecutor::*;
    Some(match kind {
        "prompt" | "batch" | "imageSource" | "psdTemplate" | "number" | "reroute" | "group"
        | "compare" | "logic" | "if" | "switch" | "preview" | "save" => Graph,
        "psdContextAnalyze" | "matchLightColor" | "refineMaskEdge" | "imageEnhance"
        | "detailWatchdog" | "psdExport" => Local,
        "generate" | "detailRepaint" => Api,
        "promptOptimize" => Hybrid,
        _ => return None,
    })
}

async fn execute_studio_node(
    node: &StudioGraphNode,
    inputs: BTreeMap<String, Value>,
    cancels: &tauri::State<'_, StudioRunCancels>,
    run_id: &str,
) -> Result<BTreeMap<String, Value>, String> {
    // Route on the executor first, then dispatch by kind inside that class.
    // Each class-handler only has access to the resources its executor is
    // allowed to use, so the local/API boundary is enforced structurally.
    match studio_executor_for_kind(node.kind.as_str()) {
        Some(StudioExecutor::Graph) => execute_studio_graph_node(node, &inputs),
        Some(StudioExecutor::Local) => execute_studio_local_node(node, &inputs),
        Some(StudioExecutor::Api) => execute_studio_api_node(node, &inputs, cancels, run_id).await,
        Some(StudioExecutor::Hybrid) => {
            execute_studio_prompt_optimize(node, &inputs, cancels, run_id).await
        }
        None => Err(format!("unsupported Studio node kind: {}", node.kind)),
    }
}

/// Pure-graph nodes: no backend call, no network.
fn execute_studio_graph_node(
    node: &StudioGraphNode,
    inputs: &BTreeMap<String, Value>,
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
            json!(if studio_compare_result(&node.params, inputs) {
                1
            } else {
                0
            }),
        )])),
        "logic" => Ok(studio_output_map([(
            "result",
            json!(if studio_logic_result(&node.params, inputs) {
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
        other => Err(format!("node kind is not a graph node: {other}")),
    }
}

/// Local nodes: every arm shells out to a `python/bridge` CLI via `psd.rs`.
/// This handler is intentionally given no broker/network access, so a local
/// card can never make a provider call.
fn execute_studio_local_node(
    node: &StudioGraphNode,
    inputs: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, String> {
    match node.kind.as_str() {
        "psdContextAnalyze" => execute_studio_psd_context_analyze(node, inputs),
        "matchLightColor" => execute_studio_match_light_color(node, inputs),
        "refineMaskEdge" => execute_studio_refine_mask_edge(node, inputs),
        "imageEnhance" => execute_studio_image_enhance(node, inputs),
        "detailWatchdog" => execute_studio_detail_watchdog(node, inputs),
        "psdExport" => execute_studio_psd_export(node, inputs),
        other => Err(format!("node kind is not a local node: {other}")),
    }
}

/// API nodes: every arm goes through the broker (`execute_and_record_cancellable`).
async fn execute_studio_api_node(
    node: &StudioGraphNode,
    inputs: &BTreeMap<String, Value>,
    cancels: &tauri::State<'_, StudioRunCancels>,
    run_id: &str,
) -> Result<BTreeMap<String, Value>, String> {
    match node.kind.as_str() {
        "generate" => execute_studio_generate(node, inputs, cancels, run_id).await,
        "detailRepaint" => execute_studio_detail_repaint(node, inputs, cancels, run_id).await,
        other => Err(format!("node kind is not an API node: {other}")),
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

    #[test]
    fn prompt_optimize_provider_support_matches_broker() {
        assert!(studio_prompt_optimize_provider_supported(
            "openai_compatible"
        ));
        assert!(studio_prompt_optimize_provider_supported("  mock  "));
        assert!(!studio_prompt_optimize_provider_supported("custom_http"));
        assert!(!studio_prompt_optimize_provider_supported("replicate"));
        assert!(!studio_prompt_optimize_provider_supported(""));
    }

    fn node_with_kind(kind: &str) -> StudioGraphNode {
        StudioGraphNode {
            id: "n1".to_string(),
            kind: kind.to_string(),
            params: BTreeMap::new(),
        }
    }

    #[test]
    fn executor_classification_partitions_kinds() {
        use StudioExecutor::*;
        for kind in [
            "prompt", "batch", "imageSource", "psdTemplate", "number", "reroute", "group",
            "compare", "logic", "if", "switch", "preview", "save",
        ] {
            assert_eq!(studio_executor_for_kind(kind), Some(Graph), "{kind}");
        }
        for kind in [
            "psdContextAnalyze",
            "matchLightColor",
            "refineMaskEdge",
            "imageEnhance",
            "detailWatchdog",
            "psdExport",
        ] {
            assert_eq!(studio_executor_for_kind(kind), Some(Local), "{kind}");
        }
        assert_eq!(studio_executor_for_kind("generate"), Some(Api));
        assert_eq!(studio_executor_for_kind("detailRepaint"), Some(Api));
        assert_eq!(studio_executor_for_kind("promptOptimize"), Some(Hybrid));
        assert_eq!(studio_executor_for_kind("nope"), None);
    }

    #[test]
    fn class_handlers_reject_foreign_kinds() {
        let inputs = BTreeMap::new();
        // An API kind must never be runnable through the local (python-only)
        // path, and a local kind must never run through the graph path.
        let err = execute_studio_local_node(&node_with_kind("generate"), &inputs).unwrap_err();
        assert!(err.contains("not a local node"), "{err}");
        let err = execute_studio_graph_node(&node_with_kind("psdExport"), &inputs).unwrap_err();
        assert!(err.contains("not a graph node"), "{err}");
        // A genuine graph node still resolves through its own handler.
        assert!(execute_studio_graph_node(&node_with_kind("prompt"), &inputs).is_ok());
    }

    #[test]
    fn numeric_params_parse_from_number_or_string() {
        let mut params = BTreeMap::new();
        params.insert("temperature".to_string(), json!(0.7));
        params.insert("max_tokens".to_string(), json!("128"));
        params.insert("seed".to_string(), json!(42));
        params.insert("blank".to_string(), json!("  "));
        let node = StudioGraphNode {
            id: "n1".to_string(),
            kind: "promptOptimize".to_string(),
            params,
        };
        assert_eq!(studio_param_f64(&node, "temperature"), Some(0.7));
        assert_eq!(studio_param_i64(&node, "max_tokens"), Some(128));
        assert_eq!(studio_param_i64(&node, "seed"), Some(42));
        assert_eq!(studio_param_f64(&node, "blank"), None);
        assert_eq!(studio_param_f64(&node, "missing"), None);
    }
}
