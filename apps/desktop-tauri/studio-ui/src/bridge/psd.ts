// PSD Studio integration.
// Reuses the same backend commands the static PSD Studio tab uses, so the node
// editor shares provider profiles and the output directory rather than
// re-implementing them.

import { tauriInvoke } from "./core";

// Fields are snake_case to match the Rust `ProviderProfileSummary`.
export interface ProviderProfile {
  profile_ref: string;
  provider?: string | null;
  model?: string | null;
  credentials_ref?: string | null;
  params_count?: number;
}

/** List H-Gripe provider profiles (`get_profiles`). */
export async function listProfiles(): Promise<ProviderProfile[]> {
  const invoke = tauriInvoke();
  if (!invoke) {
    return [
      { profile_ref: "mock-openai", provider: "openai", model: "gpt-image-1", credentials_ref: "openai-key" },
      { profile_ref: "mock-local", provider: "comfyui", model: "sdxl", credentials_ref: null },
    ];
  }
  return (await invoke("get_profiles")) as ProviderProfile[];
}

/** Resolve the configured output directory (`get_runtime_info().output_dir`). */
export async function getOutputDir(): Promise<string> {
  const invoke = tauriInvoke();
  if (!invoke) return "/mock/outputs";
  const info = (await invoke("get_runtime_info")) as { output_dir?: { path?: string } };
  return info.output_dir?.path ?? "";
}

// Fields are snake_case to match the Rust `PsdOutputFile`.
export interface PsdOutput {
  name: string;
  psd_path: string;
  preview_path?: string | null;
  metadata_path?: string | null;
  smart_object?: boolean;
}

/** List `.psd` outputs in a directory (`list_psd_outputs`). */
export async function listPsdOutputs(dir: string): Promise<PsdOutput[]> {
  const invoke = tauriInvoke();
  if (!invoke) {
    return [
      { name: "fox-poster", psd_path: "/mock/outputs/fox-poster.psd", preview_path: "/mock/outputs/fox-poster_preview.png", smart_object: true },
      { name: "banner", psd_path: "/mock/outputs/banner.psd", preview_path: null, smart_object: false },
    ];
  }
  return (await invoke("list_psd_outputs", { dir })) as PsdOutput[];
}

// --- PSD compose / export ---------------------------------------------------
// Wraps the Rust `compose_psd` command, which shells out to the torch-free
// `compose_psd_cli.py` helper to write the generated image into a PSD
// template's placeholder (true smart-object content replacement when possible)
// and export `<filename>.psd` + `_preview.png` + `_metadata.json`.

export interface ComposePsdRequest {
  /** Path to the `.psd` template. */
  template: string;
  /** Path to the generated image to place into the placeholder. */
  image: string;
  /** Directory the exported files are written to. */
  outputDir: string;
  /** Base name for the exported triplet (default `final`). */
  filename?: string;
  /** JSON: `{"name": "<layer>"}` or `{left,top,width,height}`. */
  placeholder?: string;
  fitMode?: "contain" | "cover" | "stretch";
  zOrder?: "above_background" | "placeholder" | "top";
  smartObjectMode?: "disable" | "replace_content";
  hidePlaceholder?: "enable" | "disable";
  /** JSON object merged into the exported metadata. */
  metadata?: string;
  savePreview?: boolean;
}

// Fields are snake_case to match the Rust `ComposePsdResult` serialization.
export interface ComposePsdResult {
  status: string;
  psd_path: string;
  /** Empty string when preview generation was disabled. */
  preview_path: string;
  metadata_path: string;
  placeholder_kind: string | null;
  smart_object_mode: string;
}

/**
 * Compose + export a PSD via the backend (`compose_psd`). Outside Tauri there is
 * no Python/psd-tools pipeline, so this returns a mocked succeeded result so the
 * editor stays runnable in browser dev.
 */
export async function composePsd(req: ComposePsdRequest): Promise<ComposePsdResult> {
  const invoke = tauriInvoke();
  if (!invoke) {
    const base = `${req.outputDir}/${req.filename ?? "final"}`;
    return {
      status: "succeeded",
      psd_path: `${base}.psd`,
      preview_path: req.savePreview === false ? "" : `${base}_preview.png`,
      metadata_path: `${base}_metadata.json`,
      placeholder_kind: req.smartObjectMode === "replace_content" ? "smartobject" : "pixel",
      smart_object_mode: req.smartObjectMode ?? "disable",
    };
  }
  return (await invoke("compose_psd", {
    template: req.template,
    image: req.image,
    outputDir: req.outputDir,
    filename: req.filename ?? null,
    placeholder: req.placeholder ?? null,
    fitMode: req.fitMode ?? null,
    zOrder: req.zOrder ?? null,
    smartObjectMode: req.smartObjectMode ?? null,
    hidePlaceholder: req.hidePlaceholder ?? null,
    metadata: req.metadata ?? null,
    savePreview: req.savePreview ?? null,
  })) as ComposePsdResult;
}

// --- PSD inspection ---------------------------------------------------------
// Wraps the Rust `inspect_psd` command, which shells out to the torch-free
// `inspect_psd_cli.py` helper to read a PSD template's layers via psd-tools.
// Used to validate a real PSD on disk before a run: that the template path
// points at a file, and that a configured placeholder layer name truly exists.

// Fields are snake_case to match the Rust `PsdLayerInfo` serialization.
export interface PsdLayer {
  name: string;
  /** "group" | "smartobject" | "pixel". */
  kind: string;
}

// Fields are snake_case to match the Rust `InspectPsdResult` serialization.
export interface InspectPsdResult {
  status: string;
  /** `false` when the template path does not point at a file on disk. */
  exists: boolean;
  width: number;
  height: number;
  layers: PsdLayer[];
  /** Subset of the requested `names` that were not found in the PSD. */
  missing: string[];
}

/**
 * Inspect a PSD template's layers via the backend (`inspect_psd`). Reading a
 * `.psd` from disk requires the Python/psd-tools pipeline, which only exists in
 * the desktop build, so outside Tauri this resolves to `null` and callers fall
 * back to the syntactic path check.
 */
export async function inspectPsd(
  template: string,
  names?: string[],
): Promise<InspectPsdResult | null> {
  const invoke = tauriInvoke();
  if (!invoke) return null;
  return (await invoke("inspect_psd", {
    template,
    names: names ?? null,
  })) as InspectPsdResult;
}
