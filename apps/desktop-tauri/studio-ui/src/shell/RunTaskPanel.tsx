// Run Task tab: submit a raw ApiTask JSON payload to the broker.

import { useCallback, useState } from "react";

import { commands, pretty } from "../bridge/desktop";
import { useT } from "../i18n";
import { emptyStatus, Status, type StatusState } from "./common";

function mockTemplate(): string {
  return pretty({
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
}

export function RunTaskPanel() {
  const t = useT();
  const [editor, setEditor] = useState("");
  const [result, setResult] = useState("");
  const [status, setStatus] = useState<StatusState>(emptyStatus);

  const run = useCallback(async () => {
    setStatus({ text: t("run.running"), kind: "" });
    try {
      const res = await commands.runTaskJson(editor);
      setResult(pretty(res));
      setStatus({ text: res.status, kind: res.status === "failed" ? "err" : "ok" });
    } catch (err) {
      setResult(String(err));
      setStatus({ text: t("common.error"), kind: "err" });
    }
  }, [editor, t]);

  return (
    <>
      <h2>{t("run.heading")}</h2>
      <p className="hint">{t("run.hint")}</p>
      <textarea
        spellCheck={false}
        value={editor}
        onChange={(e) => setEditor(e.target.value)}
      />
      <div className="row">
        <button onClick={() => setEditor(mockTemplate())}>{t("run.insertTemplate")}</button>
        <button className="primary" onClick={() => void run()}>
          {t("run.runTask")}
        </button>
        <Status status={status} />
      </div>
      <h3>{t("studio.result")}</h3>
      <pre className="json">{result}</pre>
    </>
  );
}
