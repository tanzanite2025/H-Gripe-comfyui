// Desktop-managed Studio persistence: the autosave file plus project-scoped
// JSON stores (snapshots, run history).

import { tauriInvoke } from "./core";

/** Restore the desktop-managed Studio autosave file, if one exists. */
export async function readStudioAutosave(): Promise<string | null> {
  const invoke = tauriInvoke();
  if (!invoke) return null;
  return (await invoke("read_studio_autosave")) as string | null;
}

/** Persist the current Studio workflow through the Rust backend. */
export async function writeStudioAutosave(graph: unknown): Promise<void> {
  const invoke = tauriInvoke();
  if (!invoke) return;
  await invoke("write_studio_autosave", { graphJson: JSON.stringify(graph) });
}

/** Clear the desktop-managed Studio autosave file. */
export async function clearStudioAutosave(): Promise<void> {
  const invoke = tauriInvoke();
  if (!invoke) return;
  await invoke("clear_studio_autosave");
}

// Project-scoped JSON stores (snapshots, run history) share the same on-disk
// contract: a single file in the project folder holding a serialized array.
// These helpers centralize the desktop guard + (de)serialization so each store
// is just a thin named wrapper.

/** Read a project-scoped store file (raw JSON text), or `null` off-desktop. */
async function readStudioStore(command: string, dir: string): Promise<string | null> {
  const invoke = tauriInvoke();
  if (!invoke) return null;
  return (await invoke(command, { dir })) as string;
}

/** Persist a project-scoped store's data (desktop only; no-op off-desktop). */
async function writeStudioStore(
  command: string,
  payloadKey: string,
  dir: string,
  data: unknown,
): Promise<void> {
  const invoke = tauriInvoke();
  if (!invoke) return;
  await invoke(command, { dir, [payloadKey]: JSON.stringify(data) });
}

/**
 * Read the active project folder's persisted snapshots file (raw JSON array
 * text), or `null` outside the desktop build. Returns `"[]"` when no file
 * exists yet.
 */
export function readStudioSnapshots(dir: string): Promise<string | null> {
  return readStudioStore("read_studio_snapshots", dir);
}

/** Persist the snapshot list into the active project folder (desktop only). */
export function writeStudioSnapshots(dir: string, snapshots: unknown): Promise<void> {
  return writeStudioStore("write_studio_snapshots", "snapshotsJson", dir, snapshots);
}

/**
 * Read the active project folder's run-history file (raw JSON array text), or
 * `null` outside the desktop build. Returns `"[]"` when no file exists yet.
 */
export function readStudioRunHistory(dir: string): Promise<string | null> {
  return readStudioStore("read_studio_run_history", dir);
}

/** Persist the run history into the active project folder (desktop only). */
export function writeStudioRunHistory(dir: string, history: unknown): Promise<void> {
  return writeStudioStore("write_studio_run_history", "historyJson", dir, history);
}
