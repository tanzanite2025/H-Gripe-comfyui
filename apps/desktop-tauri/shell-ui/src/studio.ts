// PSD Studio tab: the first production-flow entry point. Composes an ApiTask
// from a provider profile + prompt + reference image + PSD template and runs it
// through the existing broker (`run_task_json`). The PSD template path is
// carried on the task so a future export step can write the result back into the
// template.

import { commands, type ApiResult, type ProfileSummary } from "./tauri";
import { el, esc, pretty, setStatus, t, toast } from "./dom";

let studioProfiles: Record<string, ProfileSummary> = {};
let studioProfilesLoaded = false;

interface StudioTask {
  id: string;
  provider: string;
  operation: string;
  inputs: Record<string, string>;
  params: Record<string, unknown>;
  credentials_ref: string | null;
  output_type: string;
  cache_policy: { enabled: boolean; ttl_seconds: number | null; key: string | null };
  retry_policy: { max_attempts: number; backoff_ms: number; timeout_ms: number };
}

export async function loadStudioProfiles(): Promise<void> {
  const select = el<HTMLSelectElement>("#studio-profile");
  const current = select.value;
  try {
    const items = await commands.getProfiles();
    studioProfiles = {};
    const options = [`<option value="">${esc(t("studio.optionNone"))}</option>`];
    items.forEach((p) => {
      studioProfiles[p.profile_ref] = p;
      options.push(`<option value="${esc(p.profile_ref)}">${esc(p.profile_ref)}</option>`);
    });
    select.innerHTML = options.join("");
    if (current && studioProfiles[current]) select.value = current;
    studioProfilesLoaded = true;
  } catch (err) {
    toast(String(err), "err");
  }
}

export function ensureStudioProfiles(): void {
  if (!studioProfilesLoaded) loadStudioProfiles();
}

function applyStudioProfile(): void {
  const ref = el<HTMLSelectElement>("#studio-profile").value;
  const summary = el("#studio-profile-summary");
  const profile = studioProfiles[ref];
  if (!profile) {
    summary.textContent = "";
    return;
  }
  if (profile.provider) el<HTMLInputElement>("#studio-provider").value = profile.provider;
  // Seed the model into params without clobbering anything the user typed.
  if (profile.model) {
    let params: Record<string, unknown> = {};
    const raw = el<HTMLTextAreaElement>("#studio-params").value.trim();
    if (raw) {
      try {
        params = JSON.parse(raw);
      } catch {
        params = {};
      }
    }
    if (params.model === undefined) {
      params.model = profile.model;
      el<HTMLTextAreaElement>("#studio-params").value = pretty(params);
    }
  }
  summary.textContent =
    `${t("field.provider")}: ${profile.provider ?? "-"} · ${t("field.model")}: ${profile.model ?? "-"} · ` +
    `${t("field.creds")}: ${profile.credentials_ref ?? "-"}`;
}

function studioBuildTask(): StudioTask {
  const profileRef = el<HTMLSelectElement>("#studio-profile").value;
  const provider = el<HTMLInputElement>("#studio-provider").value.trim() || "mock";
  const operation = el<HTMLSelectElement>("#studio-operation").value;
  const outputType = el<HTMLSelectElement>("#studio-output").value;
  const prompt = el<HTMLTextAreaElement>("#studio-prompt").value;
  const template = el<HTMLInputElement>("#studio-template").value.trim();
  const reference = el<HTMLInputElement>("#studio-reference").value.trim();

  let params: Record<string, unknown> = {};
  const rawParams = el<HTMLTextAreaElement>("#studio-params").value.trim();
  if (rawParams) params = JSON.parse(rawParams); // surfaced as a JSON error

  const inputs: Record<string, string> = {};
  if (prompt.trim()) inputs.prompt = prompt;
  if (reference) inputs.image_path = reference;
  if (template) inputs.template_path = template;

  return {
    id: "studio-" + Date.now(),
    provider,
    operation,
    inputs,
    params,
    credentials_ref: studioProfiles[profileRef]?.credentials_ref ?? null,
    output_type: outputType,
    cache_policy: { enabled: false, ttl_seconds: null, key: null },
    retry_policy: { max_attempts: 1, backoff_ms: 200, timeout_ms: 60000 },
  };
}

function studioPreview(): void {
  const status = el("#studio-status");
  try {
    el("#studio-task").textContent = pretty(studioBuildTask());
    setStatus(status, t("studio.taskReady"), "ok");
  } catch (err) {
    setStatus(status, t("field.params") + ": " + err, "err");
  }
}

function renderStudioOutputs(result: ApiResult): void {
  const target = el("#studio-outputs");
  const files = result.output_files ?? [];
  if (!files.length) {
    target.innerHTML = "";
    return;
  }
  target.innerHTML = files
    .map(
      (f, i) =>
        `<div class="card"><div class="label">${esc(t("studio.outputN", { n: i + 1 }))}</div><div class="value">${esc(f.path)}</div><div class="row"><button data-studio-open="${i}">${esc(t("studio.open"))}</button></div></div>`
    )
    .join("");
  target.dataset.files = JSON.stringify(files.map((f) => f.path));
}

export function initStudio(): void {
  el("#studio-profile").addEventListener("change", applyStudioProfile);
  el("#studio-preview").addEventListener("click", studioPreview);

  el("#studio-run").addEventListener("click", async () => {
    const status = el("#studio-status");
    let task: StudioTask;
    try {
      task = studioBuildTask();
    } catch (err) {
      setStatus(status, t("field.params") + ": " + err, "err");
      return;
    }
    el("#studio-task").textContent = pretty(task);
    setStatus(status, t("studio.generating"), "");
    try {
      const result = await commands.runTaskJson(JSON.stringify(task));
      el("#studio-result").textContent = pretty(result);
      renderStudioOutputs(result);
      setStatus(status, result.status, result.status === "failed" ? "err" : "ok");
    } catch (err) {
      el("#studio-result").textContent = String(err);
      setStatus(status, t("common.error"), "err");
    }
  });

  el("#studio-outputs").addEventListener("click", async (e) => {
    const idx = (e.target as HTMLElement).dataset.studioOpen;
    if (idx === undefined) return;
    const files: string[] = JSON.parse(el("#studio-outputs").dataset.files || "[]");
    const path = files[parseInt(idx, 10)];
    if (!path) return;
    try {
      await commands.openPath(path);
      toast(t("studio.openedOutput"), "ok");
    } catch (err) {
      toast(String(err), "err");
    }
  });

  el("#studio-reference-preview").addEventListener("click", async () => {
    const path = el<HTMLInputElement>("#studio-reference").value.trim();
    const img = el<HTMLImageElement>("#studio-reference-img");
    if (!path) {
      img.classList.add("hidden");
      img.removeAttribute("src");
      return;
    }
    try {
      img.src = await commands.readImageDataUrl(path);
      img.classList.remove("hidden");
    } catch (err) {
      img.classList.add("hidden");
      img.removeAttribute("src");
      toast(String(err), "err");
    }
  });

  el("#studio-template-from-psd").addEventListener("click", async () => {
    try {
      const info = await commands.getRuntimeInfo();
      const outputs = await commands.listPsdOutputs(info.output_dir.path);
      if (!outputs.length) {
        toast(t("studio.noPsdInOutput"), "err");
        return;
      }
      el<HTMLInputElement>("#studio-template").value = outputs[0].psd_path;
      toast(t("studio.pickedPsd", { name: outputs[0].name, count: outputs.length }), "ok");
    } catch (err) {
      toast(String(err), "err");
    }
  });
}
