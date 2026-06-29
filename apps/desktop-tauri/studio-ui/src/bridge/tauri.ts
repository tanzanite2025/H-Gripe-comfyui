// Thin bridge to the Tauri backend. When running outside Tauri (e.g. `vite dev`
// in a plain browser) it falls back to mocks so the editor stays usable for UI
// development without the desktop backend.
//
// The implementation is split by domain into sibling modules; this file is the
// stable public surface and re-exports all of them so existing imports
// (`from ".../bridge/tauri"`) keep working unchanged.

export { isTauri } from "./core";
export type { Invoke, UnlistenFn, EventCallback, Listen } from "./core";

export * from "./run";
export * from "./persistence";
export * from "./workflows";
export * from "./files";
export * from "./psd";
