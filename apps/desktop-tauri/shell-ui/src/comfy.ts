// Advanced Canvas tab: start/stop a local ComfyUI server and embed its web UI.

import { commands } from "./tauri";
import { el, runWithStatus, setStatus, t } from "./dom";

let comfyEmbedded = false;

// The Advanced Canvas embeds a *local* ComfyUI. The CSP `frame-src` only allows
// loopback origins, so a non-loopback URL would be silently blocked by the
// webview; reject it here with a clear message instead. Parsing is lenient: a
// bare `host:port` (no scheme) is treated as http.
function isLoopbackComfyUrl(raw: string): boolean {
  try {
    const u = new URL(/^\w+:\/\//.test(raw) ? raw : `http://${raw}`);
    const host = u.hostname.replace(/^\[|\]$/g, "").toLowerCase();
    return host === "127.0.0.1" || host === "localhost" || host === "::1";
  } catch {
    return false;
  }
}

function embedComfy(): void {
  const url = el<HTMLInputElement>("#comfy-url").value.trim();
  const frame = el<HTMLIFrameElement>("#comfy-frame");
  const status = el("#comfy-status");
  const placeholder = el("#comfy-placeholder");
  if (!url) {
    setStatus(status, t("comfy.enterUrl"), "err");
    return;
  }
  if (!isLoopbackComfyUrl(url)) {
    setStatus(status, t("comfy.onlyLocal"), "err");
    return;
  }
  setStatus(status, t("comfy.connecting"), "");
  placeholder.classList.add("hidden");
  frame.classList.remove("hidden");
  // Cache-bust so Reload re-fetches even if the URL is unchanged.
  frame.src = url + (url.includes("?") ? "&" : "?") + "_hg=" + Date.now();
  comfyEmbedded = true;
}

export function ensureComfyEmbedded(): void {
  if (!comfyEmbedded) embedComfy();
}

function comfyPort(): number {
  try {
    return Number(new URL(el<HTMLInputElement>("#comfy-url").value.trim()).port) || 8188;
  } catch {
    return 8188;
  }
}

export function initComfy(): void {
  el("#comfy-frame").addEventListener("load", () => {
    setStatus("#comfy-status", t("comfy.connected"), "ok");
  });

  el("#comfy-reload").addEventListener("click", embedComfy);

  el("#comfy-start").addEventListener("click", async () => {
    const status = el("#comfy-status");
    const dir = el<HTMLInputElement>("#comfy-dir").value.trim();
    const args = el<HTMLInputElement>("#comfy-args").value.trim();
    const port = comfyPort();
    try {
      setStatus(status, t("comfy.starting"), "");
      const msg = await commands.startComfyui(dir || null, port, args || null);
      setStatus(status, t("comfy.waiting", { msg }), "");
      // Poll until the port accepts connections, then embed once (avoids
      // hammering the iframe while ComfyUI is still booting).
      let waited = 0;
      const poll = async () => {
        if (await commands.comfyuiReachable(port)) {
          embedComfy();
          return;
        }
        waited += 1500;
        if (waited < 90000) setTimeout(poll, 1500);
        else {
          setStatus(status, t("comfy.noServer"), "err");
        }
      };
      setTimeout(poll, 1500);
    } catch (err) {
      setStatus(status, String(err), "err");
    }
  });

  el("#comfy-stop").addEventListener("click", async () => {
    const status = el("#comfy-status");
    try {
      await commands.stopComfyui();
      const frame = el<HTMLIFrameElement>("#comfy-frame");
      frame.classList.add("hidden");
      frame.src = "about:blank";
      el("#comfy-placeholder").classList.remove("hidden");
      comfyEmbedded = false;
      setStatus(status, t("comfy.stopped"), "");
    } catch (err) {
      setStatus(status, String(err), "err");
    }
  });

  el("#comfy-open").addEventListener("click", () =>
    runWithStatus(
      "#comfy-status",
      { running: t("comfy.connecting"), done: { text: t("comfy.openedBrowser"), kind: "ok" } },
      () => commands.openUrl(el<HTMLInputElement>("#comfy-url").value.trim())
    )
  );
}
