//! Explicit execution-lane policy for the Studio run engine.
//!
//! `exec.rs` walks the topological order and `.await`s each node in turn, so
//! the GPU has historically been serialised *by accident* — nothing declared
//! that only one heavy job may touch the device at a time. This module makes
//! that policy **explicit** (see `docs/cards/editor-resource-model.md`
//! § "Concurrency policy"): every node kind is classified into a
//! [`JobCategory`], and a process-wide [`StudioScheduler`] hands out permits so
//! GPU work is gated by a `Semaphore(1)` while CPU-bound work may fan out on a
//! bounded pool.
//!
//! This is the *skeleton* half of staged-rollout step 2: the run loop still
//! executes nodes sequentially, so acquiring a permit around a node does not
//! change results — it establishes the shared gate that a future parallel
//! scheduler (and the front-end preview lane) will contend on. Everything here
//! is deliberately pure / cheap so the classification is unit-testable without
//! standing up a GPU.

use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::node_registry::node_class;

/// The resource lane a node's work runs in. Distinct from
/// [`StudioExecutor`](super::exec::StudioExecutor) (which decides *who* runs the
/// node — graph / python / native / broker):
/// this decides *what limited resource* the work contends for, which is what
/// the concurrency policy gates on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JobCategory {
    /// In-process graph logic (routing, comparisons, passthroughs). Effectively
    /// free — never gated.
    CpuLight,
    /// CPU-bound native or `python/bridge` work (geometry, PSD, matting CLIs).
    /// May run in parallel up to a bounded pool.
    CpuBound,
    /// Local GPU / model inference (native ONNX). Serialised to one at a time.
    Gpu,
    /// A remote provider call through the broker. Bounded by the network / the
    /// provider, not the local GPU, so it does not take the GPU permit.
    Network,
}

/// Classify a node kind into its resource lane, or `None` for an unknown kind
/// (the single unsupported-kind gate). Delegates to the shared
/// [`node_registry`](super::node_registry) so the lane travels with the kind's
/// executor classification. Keep in sync with `nodeSpecs.ts`.
pub(crate) fn category_for_kind(kind: &str) -> Option<JobCategory> {
    node_class(kind).map(|class| class.category)
}

/// The number of concurrent jobs allowed in a lane, given the CPU pool size.
/// `Gpu` is always 1 (the `Semaphore(1)` policy); light and network work are
/// not locally gated.
pub(crate) fn concurrency_limit(category: JobCategory, cpu_pool: usize) -> usize {
    match category {
        JobCategory::CpuLight | JobCategory::Network => usize::MAX,
        JobCategory::CpuBound => cpu_pool.max(1),
        JobCategory::Gpu => 1,
    }
}

/// The default CPU-pool size: the machine's parallelism, floored at 1.
fn default_cpu_pool() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .max(1)
}

/// Process-wide gate for Studio compute lanes. Held as Tauri managed state so a
/// full graph Run and (later) preview jobs contend on the *same* GPU permit
/// rather than fighting the device independently.
pub(crate) struct StudioScheduler {
    gpu: Arc<Semaphore>,
    cpu: Arc<Semaphore>,
    cpu_pool: usize,
}

impl StudioScheduler {
    /// Build a scheduler with a `Semaphore(1)` GPU gate and a CPU pool of the
    /// given size (floored at 1).
    pub(crate) fn with_cpu_pool(cpu_pool: usize) -> Self {
        let cpu_pool = cpu_pool.max(1);
        Self {
            gpu: Arc::new(Semaphore::new(concurrency_limit(
                JobCategory::Gpu,
                cpu_pool,
            ))),
            cpu: Arc::new(Semaphore::new(cpu_pool)),
            cpu_pool,
        }
    }

    /// Configured CPU-pool size (the `CpuBound` concurrency limit).
    pub(crate) fn cpu_pool(&self) -> usize {
        self.cpu_pool
    }

    /// Acquire a permit for a node's lane, holding it for the duration of the
    /// node's execution. `CpuLight` / `Network` are ungated and return `None`;
    /// `Gpu` and `CpuBound` return a permit that must be kept alive until the
    /// work finishes. The semaphores are never closed, so acquisition only
    /// fails if the runtime is torn down mid-await — treated as ungated.
    pub(crate) async fn acquire(&self, category: JobCategory) -> Option<OwnedSemaphorePermit> {
        let sem = match category {
            JobCategory::Gpu => &self.gpu,
            JobCategory::CpuBound => &self.cpu,
            JobCategory::CpuLight | JobCategory::Network => return None,
        };
        sem.clone().acquire_owned().await.ok()
    }
}

impl Default for StudioScheduler {
    fn default() -> Self {
        Self::with_cpu_pool(default_cpu_pool())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_mirrors_executor_split() {
        use JobCategory::*;
        for kind in ["prompt", "reroute", "if", "switch", "preview", "save"] {
            assert_eq!(category_for_kind(kind), Some(CpuLight), "{kind}");
        }
        for kind in [
            "psdContextAnalyze",
            "matchLightColor",
            "refineMaskEdge",
            "imageEnhance",
            "detailWatchdog",
            "psdExport",
        ] {
            assert_eq!(category_for_kind(kind), Some(CpuBound), "{kind}");
        }
        // Native compute splits: ONNX matte on the GPU, crop is CPU geometry.
        assert_eq!(category_for_kind("subjectMask"), Some(Gpu));
        assert_eq!(category_for_kind("crop"), Some(CpuBound));
        // Broker / hybrid calls are network-bound, not GPU.
        assert_eq!(category_for_kind("generate"), Some(Network));
        assert_eq!(category_for_kind("detailRepaint"), Some(Network));
        assert_eq!(category_for_kind("promptOptimize"), Some(Network));
        // Unknown kinds stay unclassified (single gate, like the executor map).
        assert_eq!(category_for_kind("nope"), None);
    }

    #[test]
    fn gpu_is_single_slot_regardless_of_pool() {
        assert_eq!(concurrency_limit(JobCategory::Gpu, 1), 1);
        assert_eq!(concurrency_limit(JobCategory::Gpu, 64), 1);
        assert_eq!(concurrency_limit(JobCategory::CpuBound, 8), 8);
        assert_eq!(concurrency_limit(JobCategory::CpuBound, 0), 1);
        assert_eq!(concurrency_limit(JobCategory::CpuLight, 8), usize::MAX);
        assert_eq!(concurrency_limit(JobCategory::Network, 8), usize::MAX);
    }

    #[test]
    fn scheduler_floors_cpu_pool_at_one() {
        let scheduler = StudioScheduler::with_cpu_pool(0);
        assert_eq!(scheduler.cpu_pool(), 1);
    }

    #[tokio::test]
    async fn gpu_permit_serialises_and_releases() {
        let scheduler = StudioScheduler::with_cpu_pool(4);
        // Light / network lanes are never gated.
        assert!(scheduler.acquire(JobCategory::CpuLight).await.is_none());
        assert!(scheduler.acquire(JobCategory::Network).await.is_none());

        // Only one GPU permit exists; while it's held the gate is exhausted.
        let permit = scheduler.acquire(JobCategory::Gpu).await;
        assert!(permit.is_some());
        assert!(
            scheduler.gpu.try_acquire().is_err(),
            "a second GPU permit must not be available while one is held"
        );
        // CPU-bound work still flows in parallel with the held GPU permit.
        assert!(scheduler.acquire(JobCategory::CpuBound).await.is_some());

        // Dropping the permit frees the single GPU slot again.
        drop(permit);
        assert!(scheduler.gpu.try_acquire().is_ok());
    }
}
