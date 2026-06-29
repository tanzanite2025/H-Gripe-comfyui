// Node Editor tab: lazily embed the studio-ui React Flow build (served at
// studio/index.html under the same Tauri origin) on first open. Its Tauri
// bridge reaches IPC via the parent window, so the embedded editor can call
// run_task_json / generate_thumbnail.

import { el, esc, t } from "./dom";

let nodeEditorEmbedded = false;

export async function ensureNodeEditorEmbedded(): Promise<void> {
  if (nodeEditorEmbedded) return;
  const frame = el<HTMLIFrameElement>("#studio-frame");
  const placeholder = el("#studio-placeholder");
  // The editor build (studio/) is produced by `npm run build` in studio-ui.
  // A plain `cargo run` does not build it, so check first and show a hint
  // instead of a broken iframe when the build is missing.
  try {
    const res = await fetch("studio/index.html", { method: "GET" });
    if (!res.ok) throw new Error(String(res.status));
  } catch {
    placeholder.innerHTML =
      "<p>" + esc(t("node.buildMissing")) + "</p>" +
      '<p class="hint">' + t("node.buildHint") + "</p>";
    return;
  }
  frame.addEventListener("load", () => {
    placeholder.classList.add("hidden");
    frame.classList.remove("hidden");
  });
  frame.src = "studio/index.html";
  nodeEditorEmbedded = true;
}
