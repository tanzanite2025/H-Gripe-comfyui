// Typed wrapper around the Tauri IPC bridge.
//
// The shell previously called `window.__TAURI__.core.invoke("cmd", args)` with
// bare string command names and untyped results, which drifts silently from the
// Rust command signatures in `src-tauri`. This module is the single place that
// touches `invoke`: every backend command has an explicit argument + return
// type derived from the Rust structs (see `src-tauri/src/*.rs` and the
// `hgripe-api` crate), so a rename or shape change surfaces as a `tsc` error.

interface TauriCore {
  invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
}

interface TauriGlobal {
  core: TauriCore;
}

declare global {
  interface Window {
    __TAURI__: TauriGlobal;
  }
}

// ---- backend payload shapes (mirror the Rust serde structs) ----

/** A filesystem path plus whether it currently exists (`main.rs::PathInfo`). */
export interface PathInfo {
  path: string;
  exists: boolean;
}

/** `main.rs::RuntimeInfo` — the dashboard's runtime paths + provider list. */
export interface RuntimeInfo {
  providers: string[];
  credentials_file: PathInfo;
  profiles_file: PathInfo;
  history_file: PathInfo;
  history_db: PathInfo;
  output_dir: PathInfo;
}

/** `hgripe-api::CredentialSummary` (Rust `Option<String>` → `string | null`). */
export interface CredentialSummary {
  credential_ref: string;
  provider: string | null;
  base_url: string | null;
  api_key_configured: boolean;
  api_key_env: string | null;
  headers_count: number;
}

/** `hgripe-api::ProviderProfileSummary`. */
export interface ProfileSummary {
  profile_ref: string;
  provider: string | null;
  credentials_ref: string | null;
  base_url: string | null;
  model: string | null;
  no_auth: boolean | null;
  has_headers: boolean;
  params_count: number;
  extra_body_count: number;
}

/** A single credential/profile validation issue. */
export interface ValidationIssue {
  severity: string;
  code: string;
  message: string;
  credential_ref?: string;
  profile_ref?: string;
}

/** `check_credentials` / `check_profiles` result. */
export interface ValidationResult {
  ok: boolean;
  issues: ValidationIssue[];
  credential_count?: number;
  profile_count?: number;
}

/** An output file produced by a task run. */
export interface OutputFile {
  path: string;
}

/**
 * `hgripe-api::ApiResult`. Only the fields the shell reads are named; the rest
 * is preserved (and pretty-printed) via the index signature.
 */
export interface ApiResult {
  status: string;
  output_files?: OutputFile[];
  [key: string]: unknown;
}

/** A recorded task history row (`list_history`). */
export interface HistoryRecord {
  task_id: string;
  timestamp_ms: number;
  provider: string;
  operation: string;
  status: string;
  output_file_count: number;
}

/** `psd.rs::PsdOutputFile` — one PSD export triplet. */
export interface PsdOutput {
  name: string;
  psd_path: string;
  preview_path: string | null;
  metadata_path: string | null;
  modified_ms: number | null;
  size_bytes: number;
  smart_object: boolean;
}

/** Which on-disk config file an editor operates on. */
export type ConfigKind = "credentials" | "profiles";

/** Filter for `list_history`. */
export interface HistoryQuery {
  limit: number;
  provider: string | null;
  operation: string | null;
  status: string | null;
  has_output_files: boolean | null;
}

/** Options for `history_cleanup_preview` / `history_cleanup_apply`. */
export interface CleanupOptions {
  keep_latest: number | null;
  older_than_timestamp_ms: number | null;
  provider: string | null;
  operation: string | null;
  status: string | null;
  has_output_files: boolean | null;
  delete_all_matched: boolean;
  delete_output_files: boolean;
}

function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  return window.__TAURI__.core.invoke<T>(command, args);
}

// ---- typed command surface ----
// One method per backend command. The names match the Rust `#[tauri::command]`
// functions; arguments are passed through with the same keys Tauri expects.
export const commands = {
  getRuntimeInfo: () => invoke<RuntimeInfo>("get_runtime_info"),
  doctor: () => invoke<unknown>("doctor"),

  getCredentials: () => invoke<CredentialSummary[]>("get_credentials"),
  getProfiles: () => invoke<ProfileSummary[]>("get_profiles"),
  readConfigFile: (kind: ConfigKind) => invoke<string>("read_config_file", { kind }),
  writeConfigFile: (kind: ConfigKind, content: string) =>
    invoke<void>("write_config_file", { kind, content }),
  checkCredentials: () => invoke<ValidationResult>("check_credentials"),
  checkProfiles: () => invoke<ValidationResult>("check_profiles"),

  runTaskJson: (taskJson: string) => invoke<ApiResult>("run_task_json", { taskJson }),
  listHistory: (query: HistoryQuery) => invoke<HistoryRecord[]>("list_history", { query }),
  historyDetail: (taskId: string) => invoke<unknown>("history_detail", { taskId }),
  rerunTask: (taskId: string, disableCache: boolean) =>
    invoke<ApiResult>("rerun_task", { taskId, disableCache }),
  historyCleanupPreview: (options: CleanupOptions) =>
    invoke<unknown>("history_cleanup_preview", { options }),
  historyCleanupApply: (options: CleanupOptions) =>
    invoke<unknown>("history_cleanup_apply", { options }),

  listPsdOutputs: (dir: string) => invoke<PsdOutput[]>("list_psd_outputs", { dir }),
  readImageDataUrl: (path: string) => invoke<string>("read_image_data_url", { path }),
  readTextFile: (path: string, maxBytes: number) =>
    invoke<string>("read_text_file", { path, maxBytes }),
  openPath: (path: string) => invoke<void>("open_path", { path }),
  openUrl: (url: string) => invoke<void>("open_url", { url }),
};
