// Explicit workflow save/open + project folder + recents.
// The desktop backend persists named workflow files anywhere on disk and
// browses a chosen project folder. Outside Tauri these resolve to no-ops /
// empty results so the editor keeps using the browser download/upload fallback.

import { tauriInvoke } from "./core";

/** Native save dialog for a workflow file; resolves to the path or null. */
export async function pickWorkflowSavePath(
  defaultName?: string,
  dir?: string | null,
): Promise<string | null> {
  const invoke = tauriInvoke();
  if (!invoke) return null;
  const path = await invoke("pick_workflow_save_path", {
    defaultName: defaultName ?? null,
    dir: dir ?? null,
  });
  return (path as string | null) ?? null;
}

/** Native open dialog for a workflow file; resolves to the path or null. */
export async function pickWorkflowOpenPath(dir?: string | null): Promise<string | null> {
  const invoke = tauriInvoke();
  if (!invoke) return null;
  const path = await invoke("pick_workflow_open_path", { dir: dir ?? null });
  return (path as string | null) ?? null;
}

/** Native folder picker for the project folder; resolves to the path or null. */
export async function pickProjectFolder(dir?: string | null): Promise<string | null> {
  const invoke = tauriInvoke();
  if (!invoke) return null;
  const path = await invoke("pick_project_folder", { dir: dir ?? null });
  return (path as string | null) ?? null;
}

/** Read (and validate) a workflow file from disk. */
export async function readStudioWorkflow(path: string): Promise<string> {
  const invoke = tauriInvoke();
  if (!invoke) throw new Error("workflow files require the desktop backend");
  return (await invoke("read_studio_workflow", { path })) as string;
}

/** Write a workflow file to disk (validated by the backend). */
export async function writeStudioWorkflow(path: string, graph: unknown): Promise<void> {
  const invoke = tauriInvoke();
  if (!invoke) throw new Error("workflow files require the desktop backend");
  await invoke("write_studio_workflow", { path, graphJson: JSON.stringify(graph) });
}

// Fields are snake_case to match the Rust `StudioWorkflowFile` serialization.
export interface StudioWorkflowFile {
  name: string;
  path: string;
  modified_ms?: number | null;
  size_bytes: number;
}

/** List workflow JSON files in a project folder (newest first). */
export async function listStudioWorkflows(dir: string): Promise<StudioWorkflowFile[]> {
  const invoke = tauriInvoke();
  if (!invoke) return [];
  return (await invoke("list_studio_workflows", { dir })) as StudioWorkflowFile[];
}

/** Rename a workflow file within its folder; resolves to the new path. */
export async function renameStudioWorkflow(path: string, newName: string): Promise<string> {
  const invoke = tauriInvoke();
  if (!invoke) throw new Error("workflow files require the desktop backend");
  return (await invoke("rename_studio_workflow", { path, newName })) as string;
}

/** Delete a workflow file from disk. */
export async function deleteStudioWorkflow(path: string): Promise<void> {
  const invoke = tauriInvoke();
  if (!invoke) throw new Error("workflow files require the desktop backend");
  await invoke("delete_studio_workflow", { path });
}

/** Duplicate a workflow file; resolves to the new copy's path. */
export async function duplicateStudioWorkflow(path: string): Promise<string> {
  const invoke = tauriInvoke();
  if (!invoke) throw new Error("workflow files require the desktop backend");
  return (await invoke("duplicate_studio_workflow", { path })) as string;
}

// Fields are snake_case to match the Rust `StudioRecents` serialization.
export interface StudioRecents {
  project_dir?: string | null;
  current_file?: string | null;
  files: string[];
}

/** Restore the persisted project folder + recent files, if any. */
export async function readStudioRecents(): Promise<StudioRecents> {
  const invoke = tauriInvoke();
  if (!invoke) return { project_dir: null, current_file: null, files: [] };
  return (await invoke("read_studio_recents")) as StudioRecents;
}

/** Persist the project folder + recent files. */
export async function writeStudioRecents(recents: StudioRecents): Promise<void> {
  const invoke = tauriInvoke();
  if (!invoke) return;
  await invoke("write_studio_recents", { recents });
}
