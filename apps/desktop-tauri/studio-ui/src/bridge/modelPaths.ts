import { tauriInvoke } from "./core";

// --- Local model manager --------------------------------------------------
// Persisted weight paths for the opt-in ML engines (the `weights_path`
// management surface over the capability probe). The desktop backend stores
// the mapping in a small JSON config and applies it as env vars on every
// bridge subprocess; a real process-level env var always wins.

/** The persisted mapping (mirrors Rust `ModelPathsConfig`). */
export interface ModelPathsConfig {
  /** Shared weight cache dir (`HGRIPE_MODEL_CACHE`); null/empty = default. */
  model_cache_dir?: string | null;
  /** Engine id -> explicit weight path (file, or dir for a HF snapshot). */
  weights: Record<string, string>;
}

/** One manager row (mirrors Rust `ModelPathEntry`). */
export interface ModelPathEntry {
  /** Engine id (matches the `probe_engines` report keys). */
  engine: string;
  /** The env var the Python backend reads. */
  env_var: string;
  /** The persisted override, if any. */
  configured_path?: string | null;
  /** Whether the persisted override exists on disk. */
  configured_exists: boolean;
  /** A process-level env var is set and wins over the config. */
  env_active: boolean;
  /** The active process-level value, when `env_active`. */
  env_value?: string | null;
}

/** The full manager report (mirrors Rust `ModelPathsReport`). */
export interface ModelPathsReport {
  config: ModelPathsConfig;
  entries: ModelPathEntry[];
  cache_env_active: boolean;
  cache_env_value?: string | null;
  /** Where the config file lives on disk. */
  config_file: string;
}

/** Engine -> env var rows the browser mock mirrors from the Rust backend. */
const MOCK_ENGINE_ENV_VARS: [string, string][] = [
  ["realesrgan", "HGRIPE_REALESRGAN_MODEL"],
  ["ccsr", "HGRIPE_CCSR_MODEL"],
  ["supir", "HGRIPE_SUPIR_MODEL"],
  ["sd_inpaint", "HGRIPE_INPAINT_MODEL"],
  ["sdxl_inpaint", "HGRIPE_SDXL_INPAINT_MODEL"],
  ["flux_fill", "HGRIPE_FLUX_FILL_MODEL"],
  ["onnx_defect", "HGRIPE_WATCHDOG_MODEL"],
  ["onnx_harmonize", "HGRIPE_COLOR_MODEL"],
  ["onnx_matting", "HGRIPE_MATTING_MODEL"],
];

const MOCK_STORAGE_KEY = "hgripe.mock.modelPaths";

function loadMockConfig(): ModelPathsConfig {
  try {
    const raw = window.localStorage.getItem(MOCK_STORAGE_KEY);
    if (raw) return JSON.parse(raw) as ModelPathsConfig;
  } catch {
    // Ignore parse/storage failures; start blank.
  }
  return { model_cache_dir: null, weights: {} };
}

/** Browser-dev mock of the manager report: config persisted in localStorage,
 * no real env vars or disk, so `configured_exists` is always false. */
function mockReport(config: ModelPathsConfig): ModelPathsReport {
  return {
    config,
    entries: MOCK_ENGINE_ENV_VARS.map(([engine, env_var]) => ({
      engine,
      env_var,
      configured_path: config.weights[engine] ?? null,
      configured_exists: false,
      env_active: false,
      env_value: null,
    })),
    cache_env_active: false,
    cache_env_value: null,
    config_file: "(browser dev mock: stored in localStorage)",
  };
}

/**
 * Read the persisted local-model weight paths. Outside the desktop shell
 * (browser preview) there is no config file, so the panel runs on a
 * localStorage-backed mock (paths are never checked against disk there).
 */
export async function getModelPaths(): Promise<ModelPathsReport> {
  const invoke = tauriInvoke();
  if (!invoke) return mockReport(loadMockConfig());
  return (await invoke("get_model_paths")) as ModelPathsReport;
}

/** Persist an updated config and return the resulting report. */
export async function setModelPaths(config: ModelPathsConfig): Promise<ModelPathsReport> {
  const invoke = tauriInvoke();
  if (!invoke) {
    try {
      window.localStorage.setItem(MOCK_STORAGE_KEY, JSON.stringify(config));
    } catch {
      // Storage may be unavailable (private mode); the in-memory copy still shows.
    }
    return mockReport(config);
  }
  return (await invoke("set_model_paths", { config })) as ModelPathsReport;
}
