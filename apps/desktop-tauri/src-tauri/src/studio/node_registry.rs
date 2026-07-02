//! The single source of truth mapping a Studio node `kind` to its execution
//! class: which [`StudioExecutor`] runs it and which [`JobCategory`] resource
//! lane its work contends on.
//!
//! Both [`super::exec::studio_executor_for_kind`] and
//! [`super::schedule::category_for_kind`] delegate here, so onboarding a new
//! node kind is a single row edit rather than keeping two parallel `match kind`
//! tables in step. An unknown kind returns `None` — the single gate for
//! unsupported kinds. Keep in sync with `nodeSpecs.ts`.
//!
//! Note this classifies *what resources* a kind is allowed to touch; it does
//! **not** dispatch to the executor function. That dispatch stays in `exec`'s
//! per-lane handlers (`execute_studio_{graph,local,compute,api}_node`), each of
//! which is handed only the resources its lane may use, so the local / native /
//! broker boundary remains enforced structurally rather than by a lookup table.

use super::exec::StudioExecutor;
use super::schedule::JobCategory;

/// The execution class of a node kind: the executor that runs it paired with
/// the resource lane it is scheduled on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NodeClass {
    pub(crate) executor: StudioExecutor,
    pub(crate) category: JobCategory,
}

/// Classify a node kind, or `None` when the kind is unsupported.
pub(crate) fn node_class(kind: &str) -> Option<NodeClass> {
    use JobCategory::*;
    use StudioExecutor::*;
    // Pure in-process graph logic: routing, comparisons, sources, sinks.
    let (executor, category) = match kind {
        "prompt" | "batch" | "imageSource" | "videoSource" | "psdTemplate" | "number"
        | "reroute" | "group" | "compare" | "logic" | "if" | "switch" | "preview" | "save" => {
            (Graph, CpuLight)
        }
        // `python/bridge` CLI cards: CPU-bound subprocess work.
        "psdContextAnalyze" | "matchLightColor" | "refineMaskEdge" | "imageEnhance"
        | "detailWatchdog" | "psdExport" | "videoAssemble" => (Local, CpuBound),
        // Native-Rust compute cards split by device use: the ONNX matte runs on
        // the GPU (serialised), plain crop geometry is CPU-only.
        "subjectMask" => (Compute, Gpu),
        "crop" => (Compute, CpuBound),
        // Broker / hybrid calls await a (possibly remote) provider; they are
        // network-bound and never hold the local GPU permit.
        "generate" | "detailRepaint" => (Api, Network),
        "promptOptimize" => (Hybrid, Network),
        _ => return None,
    };
    Some(NodeClass { executor, category })
}
