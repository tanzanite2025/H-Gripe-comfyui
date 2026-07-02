import { tauriInvoke } from "./core";

// --- Engine capability probe ------------------------------------------------
// The `doctor`-style cross-card probe behind the opt-in ML `engine` seams. The
// inspector uses it to grey out engines whose optional deps / weights are
// missing on this box (the CPU/`rules` baseline always stays available), and
// the Dashboard surfaces it as a capability report.

/** Cached-weight inventory for one engine (mirrors Rust `WeightInfo`). */
export interface WeightInfo {
  /** Path of the non-bundled weight this engine would load. */
  path: string;
  /** Whether that weight is already present on this box. */
  present: boolean;
  /** Size in MB for a file weight; `null` for a directory weight (HF snapshot). */
  size_mb?: number | null;
}

/** Availability of one `engine` option (mirrors Rust `EngineAvailability`). */
export interface EngineAvailability {
  available: boolean;
  reason: string;
  /**
   * GPU-capable (an ML backend). Paired with the report `runtime` device probe
   * to warn it falls back to CPU when no CUDA device is present; the
   * CPU/`rules`/`provider` baseline is `false`.
   */
  accelerated?: boolean;
  /**
   * Cached-weight inventory: which non-bundled weight this engine loads and
   * whether it is present. Absent for the CPU/`rules`/`provider` baseline.
   */
  weight?: WeightInfo | null;
}

/** Per-card engine probe (mirrors Rust `CardEngineProbe`). */
export interface CardEngineProbe {
  /** Node kind whose `engine` param these cover, e.g. `imageEnhance`. */
  node_kind: string;
  /** Bridge CLI that produced the probe. */
  cli: string;
  /** Engine id -> availability (e.g. `cpu`/`realesrgan`, `rules`/`onnx_defect`). */
  engines: Record<string, EngineAvailability>;
  /** Why the probe could not run, when `engines` is empty. */
  error?: string | null;
}

/** One CUDA device from the device probe (mirrors Rust `DeviceInfo`). */
export interface DeviceInfo {
  index: number;
  name: string;
  total_memory_mb: number;
}

/** `torch` presence + CUDA flag (mirrors Rust `TorchInfo`). */
export interface TorchInfo {
  installed: boolean;
  version?: string | null;
  cuda?: boolean | null;
  reason?: string | null;
}

/** `onnxruntime` presence + execution providers (mirrors Rust `OnnxRuntimeInfo`). */
export interface OnnxRuntimeInfo {
  installed: boolean;
  version?: string | null;
  providers: string[];
  reason?: string | null;
}

/**
 * Machine compute capability (mirrors Rust `DeviceProbe`): which accelerator
 * the opt-in GPU engines would actually run on. The per-card probes say *which*
 * engines could run; this says *where*, so the inspector can warn that a GPU
 * engine falls back to CPU on a box with no CUDA device.
 */
export interface DeviceProbe {
  cuda_available: boolean;
  devices: DeviceInfo[];
  torch: TorchInfo;
  onnxruntime: OnnxRuntimeInfo;
}

/** Cross-card engine capability report (mirrors Rust `EngineProbeReport`). */
export interface EngineProbeReport {
  cards: CardEngineProbe[];
  /** Shared weight cache (`HGRIPE_MODEL_CACHE` or the bundled dir). */
  model_cache_dir?: string | null;
  /** Machine compute capability, probed once; absent when it could not run. */
  runtime?: DeviceProbe | null;
}

/**
 * Probe the opt-in ML `engine` seams across local cards (`probe_engines`).
 *
 * Outside the desktop shell (browser preview) there is no Python bridge, so we
 * return an empty report; the inspector then leaves every engine enabled rather
 * than greying options out from a probe that never ran.
 */
export async function probeEngines(): Promise<EngineProbeReport> {
  const invoke = tauriInvoke();
  if (!invoke) {
    return { cards: [], model_cache_dir: null };
  }
  return (await invoke("probe_engines", { dir: null })) as EngineProbeReport;
}
