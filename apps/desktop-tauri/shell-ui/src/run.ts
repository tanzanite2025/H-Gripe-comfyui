// Run Task tab: submit a raw ApiTask JSON payload to the broker.

import { commands } from "./tauri";
import { el, pretty, setStatus, t } from "./dom";

export function initRun(): void {
  el("#task-template").addEventListener("click", () => {
    el<HTMLTextAreaElement>("#task-editor").value = pretty({
      id: "desktop-" + Date.now(),
      provider: "mock",
      operation: "echo",
      inputs: { prompt: "hello from H-Gripe Desktop" },
      params: {},
      credentials_ref: null,
      output_type: "json",
      cache_policy: { enabled: false, ttl_seconds: null, key: null },
      retry_policy: { max_attempts: 1, backoff_ms: 200, timeout_ms: 30000 },
    });
  });

  el("#task-run").addEventListener("click", async () => {
    const status = el("#task-status");
    setStatus(status, t("run.running"), "");
    try {
      const result = await commands.runTaskJson(el<HTMLTextAreaElement>("#task-editor").value);
      el("#task-result").textContent = pretty(result);
      setStatus(status, result.status, result.status === "failed" ? "err" : "ok");
    } catch (err) {
      el("#task-result").textContent = String(err);
      setStatus(status, t("common.error"), "err");
    }
  });
}
