//! Write-skip analysis for compute-node outputs: decides which output ports
//! never need a PNG on disk because every consumer is served from the shared
//! in-memory [`image_buffer`](super::image_buffer).

use std::collections::{HashMap, HashSet};

use super::exec::{studio_executor_for_kind, StudioExecutor};
use super::graph::{StudioGraphNode, StudioWorkflowGraph};

/// Whether a consumer of a compute output can be served *without a file on
/// disk*, so the producer may skip that output's PNG write:
///
/// * another `Compute` card — it loads the surface in-process through the
///   shared [`image_buffer`], so the file is never read; or
/// * a *leaf* `preview` — a pure display sink whose thumbnail / large-view both
///   resolve through `image_buffer::lookup_dynamic` (see `generate_thumbnail`).
///   The leaf requirement (no edge leaves the preview) is what keeps this safe:
///   `preview` echoes its `image` through, so a *chained* preview could forward
///   the path to a file-reading `save` / export and must keep the file.
///
/// Every other consumer — a `Local` python-bridge card, an `Api` upload, a
/// `save` / export sink, or a non-leaf `preview` — reads the file and forces a
/// materialised output.
///
/// [`image_buffer`]: super::image_buffer
fn studio_consumer_permits_write_skip(
    consumer: &StudioGraphNode,
    graph: &StudioWorkflowGraph,
) -> bool {
    match studio_executor_for_kind(consumer.kind.as_str()) {
        Some(StudioExecutor::Compute) => true,
        _ => {
            consumer.kind == "preview" && !graph.edges.iter().any(|edge| edge.source == consumer.id)
        }
    }
}

/// The set of a compute node's output ports whose PNG write may be skipped: an
/// output is skippable when it has at least one consumer and *every* consumer
/// [permits a write-skip](studio_consumer_permits_write_skip) (all in-process
/// compute cards and/or leaf previews, never a file reader). Only compute nodes
/// can skip; every other kind returns empty.
pub(super) fn studio_skippable_output_ports(
    node: &StudioGraphNode,
    graph: &StudioWorkflowGraph,
    nodes_by_id: &HashMap<String, &StudioGraphNode>,
) -> HashSet<String> {
    let mut skippable = HashSet::new();
    if studio_executor_for_kind(node.kind.as_str()) != Some(StudioExecutor::Compute) {
        return skippable;
    }
    let ports: HashSet<&str> = graph
        .edges
        .iter()
        .filter(|edge| edge.source == node.id)
        .map(|edge| edge.source_port.as_str())
        .collect();
    for port in ports {
        let mut has_consumer = false;
        let mut all_skippable = true;
        for edge in graph
            .edges
            .iter()
            .filter(|edge| edge.source == node.id && edge.source_port == port)
        {
            has_consumer = true;
            let consumer_ok = nodes_by_id
                .get(&edge.target)
                .map(|target| studio_consumer_permits_write_skip(target, graph))
                .unwrap_or(false);
            if !consumer_ok {
                all_skippable = false;
                break;
            }
        }
        if has_consumer && all_skippable {
            skippable.insert(port.to_string());
        }
    }
    skippable
}

#[cfg(test)]
mod tests {
    use super::super::graph::StudioGraphEdge;
    use super::*;
    use std::collections::BTreeMap;

    fn node(id: &str, kind: &str) -> StudioGraphNode {
        StudioGraphNode {
            id: id.to_string(),
            kind: kind.to_string(),
            params: BTreeMap::new(),
        }
    }

    fn edge(id: &str, source: &str, source_port: &str, target: &str) -> StudioGraphEdge {
        StudioGraphEdge {
            id: id.to_string(),
            source: source.to_string(),
            source_port: source_port.to_string(),
            target: target.to_string(),
            target_port: "image".to_string(),
        }
    }

    #[test]
    fn skippable_ports_feed_only_compute_or_leaf_previews() {
        // crop1.image -> a second crop (Compute): skippable.
        // crop1.crop_report -> a leaf preview (display sink, no outgoing edge):
        //   skippable too — the preview resolves it from the buffer and forwards
        //   it nowhere.
        // crop2.image fans out to a compute card *and* a Local imageEnhance: the
        //   Local card reads the file, so nothing on crop2 is skippable.
        let graph = StudioWorkflowGraph {
            version: 1,
            nodes: vec![
                node("crop1", "crop"),
                node("crop2", "crop"),
                node("prev", "preview"),
                node("enh", "imageEnhance"),
            ],
            edges: vec![
                edge("e1", "crop1", "image", "crop2"),
                edge("e2", "crop1", "crop_report", "prev"),
                edge("e3", "crop2", "image", "crop1"),
                edge("e4", "crop2", "image", "enh"),
            ],
        };
        let nodes_by_id: HashMap<String, &StudioGraphNode> =
            graph.nodes.iter().map(|n| (n.id.clone(), n)).collect();

        let crop1 = nodes_by_id.get("crop1").unwrap();
        assert_eq!(
            studio_skippable_output_ports(crop1, &graph, &nodes_by_id),
            HashSet::from(["image".to_string(), "crop_report".to_string()]),
            "an output feeding only a compute card or a leaf preview is skippable"
        );

        // crop2.image fans out to a compute card *and* a Local imageEnhance, so
        // the file must stay — nothing is skippable.
        let crop2 = nodes_by_id.get("crop2").unwrap();
        assert!(
            studio_skippable_output_ports(crop2, &graph, &nodes_by_id).is_empty(),
            "a mixed fan-out (compute + local) always keeps the file"
        );

        // A non-compute node never skips, even when its consumer is compute.
        let prev = nodes_by_id.get("prev").unwrap();
        assert!(studio_skippable_output_ports(prev, &graph, &nodes_by_id).is_empty());
    }

    #[test]
    fn a_forwarding_preview_keeps_the_file() {
        // A preview that forwards its echoed `image` onward (here to a Local
        // imageEnhance that reads the file) is not a leaf, so an output feeding
        // it must keep its PNG — the buffer can't serve the downstream reader.
        let graph = StudioWorkflowGraph {
            version: 1,
            nodes: vec![
                node("crop1", "crop"),
                node("prev", "preview"),
                node("enh", "imageEnhance"),
            ],
            edges: vec![
                edge("e1", "crop1", "image", "prev"),
                edge("e2", "prev", "image", "enh"),
            ],
        };
        let nodes_by_id: HashMap<String, &StudioGraphNode> =
            graph.nodes.iter().map(|n| (n.id.clone(), n)).collect();
        let crop1 = nodes_by_id.get("crop1").unwrap();
        assert!(
            studio_skippable_output_ports(crop1, &graph, &nodes_by_id).is_empty(),
            "an output feeding a forwarding (non-leaf) preview keeps its file"
        );
    }

    #[test]
    fn an_output_with_no_consumer_is_not_skippable() {
        // A terminal-ish crop whose image port has no outgoing edge must keep
        // its file: it is the run's returned artifact / a thumbnail source.
        let graph = StudioWorkflowGraph {
            version: 1,
            nodes: vec![node("crop1", "crop")],
            edges: vec![],
        };
        let nodes_by_id: HashMap<String, &StudioGraphNode> =
            graph.nodes.iter().map(|n| (n.id.clone(), n)).collect();
        let crop1 = nodes_by_id.get("crop1").unwrap();
        assert!(studio_skippable_output_ports(crop1, &graph, &nodes_by_id).is_empty());
    }
}
