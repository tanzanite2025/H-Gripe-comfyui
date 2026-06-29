// Credentials / Profiles tabs: summary cards, validation, and in-place editing
// of credentials.json / provider_profiles.json.

import {
  commands,
  type ConfigKind,
  type CredentialSummary,
  type ProfileSummary,
} from "./tauri";
import { $, $$, el, esc, pretty, runWithStatus, t, toast } from "./dom";

const summaryRenderers = {
  credentials: (items: CredentialSummary[]) =>
    items
      .map(
        (c) =>
          `<div class="card"><div class="label">${esc(c.credential_ref)}</div><div class="value">${esc(t("field.provider"))}: ${esc(c.provider ?? "-")}<br/>${esc(t("field.key"))}: ${c.api_key_configured ? esc(t("creds.keySet")) : c.api_key_env ? esc(t("creds.keyEnv")) + esc(c.api_key_env) : esc(t("creds.keyNone"))}<br/>${esc(t("field.headers"))}: ${esc(c.headers_count)}</div></div>`
      )
      .join("") || `<div class="card"><div class="value">${esc(t("common.noEntries"))}</div></div>`,
  profiles: (items: ProfileSummary[]) =>
    items
      .map(
        (p) =>
          `<div class="card"><div class="label">${esc(p.profile_ref)}</div><div class="value">${esc(t("field.provider"))}: ${esc(p.provider ?? "-")}<br/>${esc(t("field.model"))}: ${esc(p.model ?? "-")}<br/>${esc(t("field.creds"))}: ${esc(p.credentials_ref ?? "-")}<br/>${esc(t("field.params"))}: ${esc(p.params_count)}</div></div>`
      )
      .join("") || `<div class="card"><div class="value">${esc(t("common.noEntries"))}</div></div>`,
};

export async function loadConfig(kind: ConfigKind): Promise<void> {
  const summary = el(`#${kind}-summary`);
  try {
    const html =
      kind === "credentials"
        ? summaryRenderers.credentials(await commands.getCredentials())
        : summaryRenderers.profiles(await commands.getProfiles());
    summary.innerHTML = html;
  } catch (err) {
    summary.innerHTML = `<div class="card"><div class="value missing">${esc(err)}</div></div>`;
  }
  try {
    el<HTMLTextAreaElement>(`#${kind}-editor`).value = await commands.readConfigFile(kind);
  } catch (err) {
    toast(String(err), "err");
  }
}

async function saveConfig(kind: ConfigKind): Promise<void> {
  const content = el<HTMLTextAreaElement>(`#${kind}-editor`).value;
  const status = $(`[data-status="${kind}"]`);
  const res = await runWithStatus(
    status,
    { running: t("status.saving"), done: t("status.saved"), toastErr: true },
    () => commands.writeConfigFile(kind, content)
  );
  if (res.ok) {
    toast(t("toast.savedKind", { kind }), "ok");
    loadConfig(kind);
  }
}

async function validateConfig(kind: ConfigKind): Promise<void> {
  const target = el(`#${kind}-validation`);
  try {
    const result = kind === "credentials" ? await commands.checkCredentials() : await commands.checkProfiles();
    const issues = result.issues ?? [];
    const ok = result.ok ?? issues.length === 0;
    target.innerHTML =
      `<span class="badge ${ok ? "ok" : "err"}">${esc(ok ? t("validation.valid") : t("validation.issues", { count: issues.length }))}</span>` +
      (issues.length ? `<pre class="json">${esc(pretty(issues))}</pre>` : "");
  } catch (err) {
    target.innerHTML = `<span class="badge err">${esc(err)}</span>`;
  }
}

function asKind(value: string | undefined): ConfigKind | null {
  return value === "credentials" || value === "profiles" ? value : null;
}

export function initConfig(): void {
  $$<HTMLElement>("[data-load]").forEach((b) =>
    b.addEventListener("click", () => {
      const kind = asKind(b.dataset.load);
      if (kind) loadConfig(kind);
    })
  );
  $$<HTMLElement>("[data-save]").forEach((b) =>
    b.addEventListener("click", () => {
      const kind = asKind(b.dataset.save);
      if (kind) saveConfig(kind);
    })
  );
  $$<HTMLElement>("[data-validate]").forEach((b) =>
    b.addEventListener("click", () => {
      const kind = asKind(b.dataset.validate);
      if (kind) validateConfig(kind);
    })
  );
}
