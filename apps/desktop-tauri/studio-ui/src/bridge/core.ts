// Shared Tauri IPC plumbing for the bridge modules. When running outside Tauri
// (e.g. `vite dev` in a plain browser) `tauriInvoke()`/`tauriListen()` return
// `null`, and each domain helper falls back to a mock so the editor stays usable
// for UI development without the desktop backend.

export type Invoke = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
export type UnlistenFn = () => void;
export type EventCallback<T> = (event: { event: string; payload: T; id?: number }) => void;
export type Listen = <T>(event: string, callback: EventCallback<T>) => Promise<UnlistenFn>;

interface TauriWindow {
  __TAURI__?: { core?: { invoke?: Invoke }; event?: { listen?: Listen } };
}

function tauriFrames(): (Window | null)[] | null {
  if (typeof window === "undefined") return null;
  const candidates: (Window | null)[] = [window];
  try {
    if (window.parent && window.parent !== window) candidates.push(window.parent);
    if (window.top && window.top !== window) candidates.push(window.top);
  } catch {
    // Cross-origin access can throw; ignore and use what we have.
  }
  return candidates;
}

// When this app runs embedded as an iframe inside the desktop shell, the Tauri
// IPC may live on the parent/top window rather than this frame. Both are
// same-origin (tauri://localhost), so reaching across is allowed. We try this
// frame first, then parent, then top.
export function tauriInvoke(): Invoke | null {
  for (const frame of tauriFrames() ?? []) {
    const invoke = (frame as unknown as TauriWindow | null)?.__TAURI__?.core?.invoke;
    if (invoke) return invoke;
  }
  return null;
}

export function tauriListen(): Listen | null {
  for (const frame of tauriFrames() ?? []) {
    const listen = (frame as unknown as TauriWindow | null)?.__TAURI__?.event?.listen;
    if (listen) return listen;
  }
  return null;
}

export const isTauri = (): boolean => tauriInvoke() !== null;
