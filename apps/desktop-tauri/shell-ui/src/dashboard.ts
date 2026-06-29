// Dashboard tab: runtime paths + `doctor` diagnostics.

import { commands, type PathInfo } from "./tauri";
import { $, el, esc, pretty, t } from "./dom";

function pathCard(label: string, info: PathInfo): string {
  const cls = info.exists ? "ok" : "missing";
  const flag = info.exists ? t("common.found") : t("common.missing");
  return `<div class="card"><div class="label">${esc(label)} (${flag})</div><div class="value ${cls}">${esc(info.path)}</div></div>`;
}

export async function loadDashboard(): Promise<void> {
  const runtimeInfo = el("#runtime-info");
  try {
    const info = await commands.getRuntimeInfo();
    runtimeInfo.innerHTML = [
      `<div class="card"><div class="label">providers</div><div class="value">${esc(info.providers.join(", "))}</div></div>`,
      pathCard("credentials.json", info.credentials_file),
      pathCard("provider_profiles.json", info.profiles_file),
      pathCard("history file", info.history_file),
      pathCard("history db", info.history_db),
      pathCard("output dir", info.output_dir),
    ].join("");
  } catch (err) {
    runtimeInfo.innerHTML = `<div class="card"><div class="value missing">${esc(err)}</div></div>`;
  }
  const doctorOutput = el("#doctor-output");
  try {
    doctorOutput.textContent = pretty(await commands.doctor());
  } catch (err) {
    doctorOutput.textContent = String(err);
  }
}

export function initDashboard(): void {
  $("#refresh-dashboard")?.addEventListener("click", loadDashboard);
}
