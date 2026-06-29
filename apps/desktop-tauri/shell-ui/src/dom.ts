// Small DOM + status helpers shared across the shell's domain modules.

import { t } from "./i18n";

export const $ = <T extends Element = HTMLElement>(sel: string): T | null =>
  document.querySelector<T>(sel);

export const $$ = <T extends Element = HTMLElement>(sel: string): T[] =>
  Array.from(document.querySelectorAll<T>(sel));

// Same as `$` but asserts the element exists; used for the static markup the
// shell ships with (where a missing node is a programming error, not runtime
// state). Keeps call sites free of non-null juggling.
export function el<T extends Element = HTMLElement>(sel: string): T {
  const found = document.querySelector<T>(sel);
  if (!found) throw new Error(`element not found: ${sel}`);
  return found;
}

export function pretty(value: unknown): string {
  return JSON.stringify(value, null, 2);
}

// Escape a value for safe interpolation into an innerHTML string (text or a
// double-quoted attribute). Backend data (paths, provider/profile names, file
// names, error strings) is untrusted for HTML purposes, so every dynamic value
// spliced into innerHTML must go through this.
export function esc(value: unknown): string {
  return String(value)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

let toastTimer: ReturnType<typeof setTimeout> | null = null;
export function toast(message: string, kind = ""): void {
  const elt = el("#toast");
  elt.textContent = message;
  elt.className = `toast show ${kind}`;
  if (toastTimer) clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (elt.className = "toast"), 3200);
}

// Set a `.status` element's text + state in one call (target may be a selector
// or an element). Centralizes the repeated `textContent` + `className` pair.
export function setStatus(target: string | Element | null, text: string, kind = ""): void {
  const elt = typeof target === "string" ? $(target) : target;
  if (!elt) return;
  elt.textContent = text;
  elt.className = "status " + kind;
}

interface DoneState {
  text: string;
  kind?: string;
}

interface RunWithStatusOpts<R> {
  running: string;
  done?: string | DoneState | ((result: R) => string | DoneState | undefined);
  toastErr?: boolean;
}

type RunWithStatusResult<R> = { ok: true; result: R } | { ok: false; err: unknown };

// Run an async command with the shared status lifecycle: show `running`, then
// on success show `done` and on failure show the error (optionally toasting it).
// `done` is a string (shown with the "ok" state) or a function of the result
// returning `{ text, kind }`. Returns `{ ok, result | err }` for follow-ups.
export async function runWithStatus<R>(
  target: string | Element | null,
  opts: RunWithStatusOpts<R>,
  command: () => Promise<R>
): Promise<RunWithStatusResult<R>> {
  setStatus(target, opts.running, "");
  try {
    const result = await command();
    const done = typeof opts.done === "function" ? opts.done(result) : opts.done;
    if (done) {
      const text = typeof done === "string" ? done : done.text;
      const kind = typeof done === "string" ? "ok" : done.kind ?? "ok";
      setStatus(target, text, kind);
    }
    return { ok: true, result };
  } catch (err) {
    setStatus(target, String(err), "err");
    if (opts.toastErr) toast(String(err), "err");
    return { ok: false, err };
  }
}

export { t };
