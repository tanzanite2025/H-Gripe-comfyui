//! The Studio graph execution engine: topological ordering, executor-lane
//! dispatch (graph / local / compute / API), dead-branch pruning, media-index
//! cache serving, and the `run_studio_graph` / `cancel_studio_run` commands.
//!
//! The engine's supporting concerns live in sibling modules: run events and
//! error details in [`run_events`](super::run_events), cancellation state in
//! [`run_cancel`](super::run_cancel), broker-call plumbing in
//! [`api_call`](super::api_call), and PNG write-skip analysis in
//! [`write_skip`](super::write_skip). API-lane node executors live in
//! [`generate`](super::generate), [`detail_repaint`](super::detail_repaint),
//! and [`prompt_optimize`](super::prompt_optimize).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::time::Instant;

use serde::Serialize;
use serde_json::{json, Value};

use super::api_call::studio_task_id;
use super::color_match::execute_studio_match_light_color;
use super::crop::execute_studio_crop;
use super::detail_repaint::execute_studio_detail_repaint;
use super::detail_watchdog::execute_studio_detail_watchdog;
use super::edge_refine::execute_studio_refine_mask_edge;
use super::generate::execute_studio_generate;
use super::graph::{
    studio_output_map, studio_truthy, studio_value_to_number, studio_value_to_string,
    StudioGraphEdge, StudioGraphNode, StudioWorkflowGraph,
};
use super::image_enhance::execute_studio_image_enhance;
use super::media_index::{media_index_key, media_index_lookup, media_index_store};
use super::prompt_optimize::execute_studio_prompt_optimize;
use super::psd_analyze::execute_studio_psd_context_analyze;
use super::psd_export::execute_studio_psd_export;
use super::run_cancel::{clear_studio_run_cancel, is_studio_run_cancelled, studio_run_token};
use super::run_events::{
    emit_studio_run_event, studio_graph_event, studio_node_event, StudioRunLogger,
};
use super::schedule::{category_for_kind, JobCategory, StudioScheduler};
use super::subject_mask::execute_studio_subject_mask;
use super::write_skip::studio_skippable_output_ports;

pub(crate) use super::run_cancel::StudioRunCancels;
pub(crate) use super::run_events::StudioNodeErrorDetail;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    error_detail: Option<StudioNodeErrorDetail>,
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
    /// In-process native-Rust image / model work; must not touch the network.
    /// (No broker handle, exactly like `Local`, so a `Compute` card can never
    /// make a provider call.)
    Compute,
    /// Always a provider call through the broker.
    Api,
    /// User picks per-node via a `mode` param (`promptOptimize`).
    Hybrid,
}

/// Classify a node kind. Returns `None` for an unknown kind (the single
/// gate for unsupported kinds). Delegates to the shared
/// [`node_registry`](super::node_registry), the single source of truth pairing
/// each kind with its executor + resource lane.
pub(crate) fn studio_executor_for_kind(kind: &str) -> Option<StudioExecutor> {
    super::node_registry::node_class(kind).map(|class| class.executor)
}

async fn execute_studio_node(
    node: &StudioGraphNode,
    inputs: BTreeMap<String, Value>,
    cancels: &tauri::State<'_, StudioRunCancels>,
    run_id: &str,
    skip_write_ports: &HashSet<String>,
    logger: &StudioRunLogger<'_>,
) -> Result<BTreeMap<String, Value>, StudioNodeErrorDetail> {
    // Route on the executor first, then dispatch by kind inside that class.
    // Each class-handler only has access to the resources its executor is
    // allowed to use, so the local/API boundary is enforced structurally.
    match studio_executor_for_kind(node.kind.as_str()) {
        Some(StudioExecutor::Graph) => execute_studio_graph_node(node, &inputs).map_err(Into::into),
        Some(StudioExecutor::Local) => execute_studio_local_node(node, &inputs).map_err(Into::into),
        Some(StudioExecutor::Compute) => {
            execute_studio_compute_node(node, &inputs, skip_write_ports).map_err(Into::into)
        }
        Some(StudioExecutor::Api) => {
            execute_studio_api_node(node, &inputs, cancels, run_id, logger).await
        }
        Some(StudioExecutor::Hybrid) => {
            execute_studio_prompt_optimize(node, &inputs, cancels, run_id, logger).await
        }
        None => Err(format!("unsupported Studio node kind: {}", node.kind).into()),
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
        "videoSource" => {
            let path = studio_value_to_string(node.params.get("path"));
            let video = if path.is_empty() {
                Value::Null
            } else {
                json!(path)
            };
            Ok(studio_output_map([("video", video)]))
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
        "videoAssemble" => super::video_assemble::execute_studio_video_assemble(node, inputs),
        "videoTrim" => super::video_trim::execute_studio_video_trim(node, inputs),
        other => Err(format!("node kind is not a local node: {other}")),
    }
}

/// Compute nodes: every arm runs in-process in native Rust (the `image` crate +
/// the shared `studio_image` decode guard). Like `Local`, this handler is given
/// no broker/network access, so a compute card can never make a provider call.
fn execute_studio_compute_node(
    node: &StudioGraphNode,
    inputs: &BTreeMap<String, Value>,
    skip_write_ports: &HashSet<String>,
) -> Result<BTreeMap<String, Value>, String> {
    match node.kind.as_str() {
        "subjectMask" => execute_studio_subject_mask(node, inputs, skip_write_ports),
        "crop" => execute_studio_crop(node, inputs, skip_write_ports),
        other => Err(format!("node kind is not a compute node: {other}")),
    }
}

/// API nodes: every arm goes through the broker (`execute_and_record_cancellable`).
async fn execute_studio_api_node(
    node: &StudioGraphNode,
    inputs: &BTreeMap<String, Value>,
    cancels: &tauri::State<'_, StudioRunCancels>,
    run_id: &str,
    logger: &StudioRunLogger<'_>,
) -> Result<BTreeMap<String, Value>, StudioNodeErrorDetail> {
    match node.kind.as_str() {
        "generate" => execute_studio_generate(node, inputs, cancels, run_id, logger).await,
        "detailRepaint" => {
            execute_studio_detail_repaint(node, inputs, cancels, run_id, logger).await
        }
        other => Err(format!("node kind is not an API node: {other}").into()),
    }
}

#[tauri::command]
pub(crate) async fn run_studio_graph(
    app: tauri::AppHandle,
    cancels: tauri::State<'_, StudioRunCancels>,
    scheduler: tauri::State<'_, StudioScheduler>,
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

    let logger = StudioRunLogger {
        app: &app,
        run_id: &run_id,
    };

    for node in &graph.nodes {
        statuses.insert(node.id.clone(), "queued".to_string());
        emit_studio_run_event(
            &app,
            studio_node_event(&run_id, node, "queued", None, None, None),
        );
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
                studio_node_event(&run_id, node, "skipped", None, None, None),
            );
            node_runs.push(StudioNodeRun {
                node_id: node.id.clone(),
                kind: node.kind.clone(),
                status: "skipped".to_string(),
                duration_ms: None,
                error: None,
                error_detail: None,
            });
            continue;
        }
        statuses.insert(node.id.clone(), "running".to_string());
        emit_studio_run_event(
            &app,
            studio_node_event(&run_id, node, "running", None, None, None),
        );
        let started_at = Instant::now();
        let inputs = studio_node_inputs(&node.id, &graph, &outputs);
        // Media index/cache: an unchanged node (same kind/params/inputs and
        // untouched upstream + output media files) is served from the previous
        // run's result instead of executing again.
        let cache_key = media_index_key(node, &inputs);
        if let Some(key) = cache_key.as_deref() {
            if let Some(cached_outputs) = media_index_lookup(key) {
                let duration_ms = started_at.elapsed().as_millis();
                logger.node(node, "served from media index (inputs unchanged)");
                outputs.insert(node.id.clone(), cached_outputs);
                statuses.insert(node.id.clone(), "cached".to_string());
                emit_studio_run_event(
                    &app,
                    studio_node_event(&run_id, node, "cached", Some(duration_ms), None, None),
                );
                node_runs.push(StudioNodeRun {
                    node_id: node.id.clone(),
                    kind: node.kind.clone(),
                    status: "cached".to_string(),
                    duration_ms: Some(duration_ms),
                    error: None,
                    error_detail: None,
                });
                continue;
            }
        }
        // Hold the lane permit for the node's resource category across its
        // execution: `Gpu` work is serialised by the `Semaphore(1)`, `CpuBound`
        // work by the bounded CPU pool; `CpuLight`/`Network` are ungated. The
        // run loop is still sequential, so this can't change results — it makes
        // the (previously accidental) GPU serialisation explicit policy and is
        // the shared gate a parallel scheduler will contend on.
        let category = category_for_kind(node.kind.as_str()).unwrap_or(JobCategory::CpuLight);
        let _lane_permit = scheduler.acquire(category).await;
        // Outputs consumed exclusively by other in-process compute cards never
        // need a file on disk (the consumer loads them from the shared buffer),
        // so the producer may skip the PNG write for those ports.
        let skip_write_ports = studio_skippable_output_ports(node, &graph, &nodes_by_id);
        match execute_studio_node(node, inputs, &cancels, &run_id, &skip_write_ports, &logger).await
        {
            Ok(node_outputs) => {
                let duration_ms = started_at.elapsed().as_millis();
                if let Some(key) = cache_key.as_deref() {
                    media_index_store(key, &node.kind, &node_outputs);
                }
                outputs.insert(node.id.clone(), node_outputs);
                statuses.insert(node.id.clone(), "succeeded".to_string());
                emit_studio_run_event(
                    &app,
                    studio_node_event(&run_id, node, "succeeded", Some(duration_ms), None, None),
                );
                node_runs.push(StudioNodeRun {
                    node_id: node.id.clone(),
                    kind: node.kind.clone(),
                    status: "succeeded".to_string(),
                    duration_ms: Some(duration_ms),
                    error: None,
                    error_detail: None,
                });
            }
            Err(detail) => {
                let error = detail.message.clone();
                let cancelled = error.to_ascii_lowercase().contains("cancel")
                    || detail.code.as_deref() == Some("cancelled");
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
                        Some(detail.clone()),
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
                    error_detail: Some(detail),
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
            "prompt",
            "batch",
            "imageSource",
            "videoSource",
            "psdTemplate",
            "number",
            "reroute",
            "group",
            "compare",
            "logic",
            "if",
            "switch",
            "preview",
            "save",
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
        assert_eq!(studio_executor_for_kind("subjectMask"), Some(Compute));
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
        // A native-Rust compute kind must never run through the local (python)
        // path, and a python-bridge kind must never run through compute.
        let err = execute_studio_local_node(&node_with_kind("subjectMask"), &inputs).unwrap_err();
        assert!(err.contains("not a local node"), "{err}");
        let err =
            execute_studio_compute_node(&node_with_kind("psdExport"), &inputs, &HashSet::new())
                .unwrap_err();
        assert!(err.contains("not a compute node"), "{err}");
        // A genuine graph node still resolves through its own handler.
        assert!(execute_studio_graph_node(&node_with_kind("prompt"), &inputs).is_ok());
    }
}
