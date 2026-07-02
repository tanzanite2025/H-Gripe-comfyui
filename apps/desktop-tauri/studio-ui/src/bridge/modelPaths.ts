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

/**
 * Read the persisted local-model weight paths. Outside the desktop shell
 * (browser preview) there is no config file, so we return `null` and the
 * Dashboard hides the manager panel rather than showing an empty editor.
 */
export async function getModelPaths(): Promise<ModelPathsReport | null> {
  const invoke = tauriInvoke();
  if (!invoke) return null;
  return (await invoke("get_model_paths")) as ModelPathsReport;
}

/** Persist an updated config and return the resulting report. */
export async function setModelPaths(config: ModelPathsConfig): Promise<ModelPathsReport> {
  const invoke = tauriInvoke();
  if (!invoke) throw new Error("model path management requires the desktop shell");
  return (await invoke("set_model_paths", { config })) as ModelPathsReport;
}
