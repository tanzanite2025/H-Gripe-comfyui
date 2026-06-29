const invoke = window.__TAURI__.core.invoke;

const $ = (sel) => document.querySelector(sel);
const $$ = (sel) => Array.from(document.querySelectorAll(sel));

let toastTimer = null;
function toast(message, kind = "") {
  const el = $("#toast");
  el.textContent = message;
  el.className = `toast show ${kind}`;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (el.className = "toast"), 3200);
}

function pretty(value) {
  return JSON.stringify(value, null, 2);
}

// Escape a value for safe interpolation into an innerHTML string (text or a
// double-quoted attribute). Backend data (paths, provider/profile names, file
// names, error strings) is untrusted for HTML purposes, so every dynamic value
// spliced into innerHTML must go through this.
function esc(value) {
  return String(value)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

// ---- tabs ----
$$("#tabs button").forEach((btn) => {
  btn.addEventListener("click", () => {
    $$("#tabs button").forEach((b) => b.classList.remove("active"));
    $$(".panel").forEach((p) => p.classList.remove("active"));
    btn.classList.add("active");
    $(`#${btn.dataset.tab}`).classList.add("active");
    if (btn.dataset.tab === "comfyui") ensureComfyEmbedded();
    if (btn.dataset.tab === "studio") ensureStudioProfiles();
    if (btn.dataset.tab === "node-editor") ensureNodeEditorEmbedded();
  });
});

// ---- dashboard ----
function pathCard(label, info) {
  const cls = info.exists ? "ok" : "missing";
  const flag = info.exists ? "found" : "missing";
  return `<div class="card"><div class="label">${esc(label)} (${flag})</div><div class="value ${cls}">${esc(info.path)}</div></div>`;
}

async function loadDashboard() {
  try {
    const info = await invoke("get_runtime_info");
    $("#runtime-info").innerHTML = [
      `<div class="card"><div class="label">providers</div><div class="value">${esc(info.providers.join(", "))}</div></div>`,
      pathCard("credentials.json", info.credentials_file),
      pathCard("provider_profiles.json", info.profiles_file),
      pathCard("history file", info.history_file),
      pathCard("history db", info.history_db),
      pathCard("output dir", info.output_dir),
    ].join("");
  } catch (err) {
    $("#runtime-info").innerHTML = `<div class="card"><div class="value missing">${esc(err)}</div></div>`;
  }
  try {
    $("#doctor-output").textContent = pretty(await invoke("doctor"));
  } catch (err) {
    $("#doctor-output").textContent = String(err);
  }
}
$("#refresh-dashboard").addEventListener("click", loadDashboard);

// ---- config editors (credentials / profiles) ----
const summaryRenderers = {
  credentials: (items) =>
    items
      .map(
        (c) =>
          `<div class="card"><div class="label">${esc(c.credential_ref)}</div><div class="value">provider: ${esc(c.provider ?? "-")}<br/>key: ${c.api_key_configured ? "set" : c.api_key_env ? "env:" + esc(c.api_key_env) : "none"}<br/>headers: ${esc(c.headers_count)}</div></div>`
      )
      .join("") || `<div class="card"><div class="value">no entries</div></div>`,
  profiles: (items) =>
    items
      .map(
        (p) =>
          `<div class="card"><div class="label">${esc(p.profile_ref)}</div><div class="value">provider: ${esc(p.provider ?? "-")}<br/>model: ${esc(p.model ?? "-")}<br/>creds: ${esc(p.credentials_ref ?? "-")}<br/>params: ${esc(p.params_count)}</div></div>`
      )
      .join("") || `<div class="card"><div class="value">no entries</div></div>`,
};

async function loadConfig(kind) {
  const listCmd = kind === "credentials" ? "get_credentials" : "get_profiles";
  try {
    const items = await invoke(listCmd);
    $(`#${kind}-summary`).innerHTML = summaryRenderers[kind](items);
  } catch (err) {
    $(`#${kind}-summary`).innerHTML = `<div class="card"><div class="value missing">${esc(err)}</div></div>`;
  }
  try {
    $(`#${kind}-editor`).value = await invoke("read_config_file", { kind });
  } catch (err) {
    toast(String(err), "err");
  }
}

async function saveConfig(kind) {
  const content = $(`#${kind}-editor`).value;
  const status = $(`[data-status="${kind}"]`);
  try {
    await invoke("write_config_file", { kind, content });
    status.textContent = "saved";
    status.className = "status ok";
    toast(`${kind} saved`, "ok");
    loadConfig(kind);
  } catch (err) {
    status.textContent = String(err);
    status.className = "status err";
    toast(String(err), "err");
  }
}

async function validateConfig(kind) {
  const cmd = kind === "credentials" ? "check_credentials" : "check_profiles";
  const target = $(`#${kind}-validation`);
  try {
    const result = await invoke(cmd);
    const issues = result.issues ?? [];
    const ok = result.ok ?? issues.length === 0;
    target.innerHTML =
      `<span class="badge ${ok ? "ok" : "err"}">${ok ? "valid" : issues.length + " issue(s)"}</span>` +
      (issues.length ? `<pre class="json">${esc(pretty(issues))}</pre>` : "");
  } catch (err) {
    target.innerHTML = `<span class="badge err">${esc(err)}</span>`;
  }
}

$$("[data-load]").forEach((b) => b.addEventListener("click", () => loadConfig(b.dataset.load)));
$$("[data-save]").forEach((b) => b.addEventListener("click", () => saveConfig(b.dataset.save)));
$$("[data-validate]").forEach((b) => b.addEventListener("click", () => validateConfig(b.dataset.validate)));

// ---- run task ----
$("#task-template").addEventListener("click", () => {
  $("#task-editor").value = pretty({
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

$("#task-run").addEventListener("click", async () => {
  const status = $("#task-status");
  status.textContent = "running…";
  status.className = "status";
  try {
    const result = await invoke("run_task_json", { taskJson: $("#task-editor").value });
    $("#task-result").textContent = pretty(result);
    status.textContent = result.status;
    status.className = "status " + (result.status === "failed" ? "err" : "ok");
  } catch (err) {
    $("#task-result").textContent = String(err);
    status.textContent = "error";
    status.className = "status err";
  }
});

// ---- history ----
async function loadHistory() {
  const query = {
    limit: parseInt($("#history-limit").value || "50", 10),
    provider: $("#history-provider").value.trim() || null,
    operation: null,
    status: $("#history-status").value || null,
    has_output_files: null,
  };
  const tbody = $("#history-table tbody");
  try {
    const records = await invoke("list_history", { query });
    tbody.innerHTML = records
      .map((r) => {
        const time = new Date(Number(r.timestamp_ms)).toLocaleString();
        return `<tr>
          <td>${esc(time)}</td><td>${esc(r.provider)}</td><td>${esc(r.operation)}</td>
          <td>${esc(r.status)}</td><td>${esc(r.output_file_count)}</td>
          <td><button data-detail="${esc(r.task_id)}">view</button> <button data-rerun="${esc(r.task_id)}">rerun</button></td>
        </tr>`;
      })
      .join("");
    if (!records.length) tbody.innerHTML = `<tr><td colspan="6">no records</td></tr>`;
  } catch (err) {
    tbody.innerHTML = `<tr><td colspan="6" class="status err">${esc(err)}</td></tr>`;
  }
}

$("#history-table").addEventListener("click", async (e) => {
  const detailId = e.target.dataset.detail;
  const rerunId = e.target.dataset.rerun;
  if (detailId) {
    try {
      $("#history-detail").textContent = pretty(await invoke("history_detail", { taskId: detailId }));
    } catch (err) {
      $("#history-detail").textContent = String(err);
    }
  } else if (rerunId) {
    toast("rerunning " + rerunId);
    try {
      const result = await invoke("rerun_task", { taskId: rerunId, disableCache: true });
      $("#history-detail").textContent = pretty(result);
      toast("rerun " + result.status, result.status === "failed" ? "err" : "ok");
      loadHistory();
    } catch (err) {
      toast(String(err), "err");
    }
  }
});
$("#history-refresh").addEventListener("click", loadHistory);

function cleanupOptions() {
  const keep = $("#cleanup-keep").value.trim();
  return {
    keep_latest: keep === "" ? null : parseInt(keep, 10),
    older_than_timestamp_ms: null,
    provider: $("#history-provider").value.trim() || null,
    operation: null,
    status: $("#history-status").value || null,
    has_output_files: null,
    delete_all_matched: false,
    delete_output_files: $("#cleanup-delete-files").checked,
  };
}

$("#cleanup-preview").addEventListener("click", async () => {
  try {
    $("#cleanup-output").textContent = pretty(
      await invoke("history_cleanup_preview", { options: cleanupOptions() })
    );
  } catch (err) {
    $("#cleanup-output").textContent = String(err);
  }
});

$("#cleanup-apply").addEventListener("click", async () => {
  try {
    const result = await invoke("history_cleanup_apply", { options: cleanupOptions() });
    $("#cleanup-output").textContent = pretty(result);
    toast("cleanup applied", "ok");
    loadHistory();
  } catch (err) {
    $("#cleanup-output").textContent = String(err);
    toast(String(err), "err");
  }
});

// ---- psd ----
let psdOutputs = [];

function fmtBytes(n) {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

async function loadPsdOutputs() {
  const dir = $("#psd-dir").value.trim();
  const list = $("#psd-list");
  $("#psd-detail").classList.add("hidden");
  if (!dir) {
    list.innerHTML = `<div class="card"><div class="value missing">enter an output directory</div></div>`;
    return;
  }
  list.innerHTML = `<div class="card"><div class="value">loading…</div></div>`;
  try {
    psdOutputs = await invoke("list_psd_outputs", { dir });
    if (!psdOutputs.length) {
      list.innerHTML = `<div class="card"><div class="value">no PSD files found</div></div>`;
      return;
    }
    list.innerHTML = psdOutputs
      .map((o, i) => {
        const time = o.modified_ms ? new Date(Number(o.modified_ms)).toLocaleString() : "-";
        const tags = [
          o.preview_path ? "preview" : null,
          o.metadata_path ? "metadata" : null,
        ].filter(Boolean);
        const badges = tags.map((t) => `<span class="badge ok">${t}</span>`).join(" ");
        const soBadge = o.smart_object ? ` <span class="badge so">smart object</span>` : "";
        return `<div class="card psd-card" data-psd-index="${i}">
          <div class="label">${esc(o.name)}.psd</div>
          <div class="value">${esc(time)}<br/>${fmtBytes(o.size_bytes)} ${badges}${soBadge}</div>
        </div>`;
      })
      .join("");
  } catch (err) {
    list.innerHTML = `<div class="card"><div class="value missing">${esc(err)}</div></div>`;
  }
}

async function showPsdDetail(index) {
  const o = psdOutputs[index];
  if (!o) return;
  const detail = $("#psd-detail");
  detail.classList.remove("hidden");
  detail.dataset.index = String(index);
  $("#psd-detail-name").textContent = `${o.name}.psd`;

  const soNote = $("#psd-detail-so");
  if (soNote) soNote.classList.toggle("hidden", !o.smart_object);

  const img = $("#psd-detail-preview");
  if (o.preview_path) {
    img.classList.remove("hidden");
    img.alt = "loading preview…";
    try {
      img.src = await invoke("read_image_data_url", { path: o.preview_path });
    } catch (err) {
      img.removeAttribute("src");
      img.alt = String(err);
    }
  } else {
    img.classList.add("hidden");
    img.removeAttribute("src");
  }

  const meta = $("#psd-detail-metadata");
  if (o.metadata_path) {
    try {
      meta.textContent = await invoke("read_text_file", { path: o.metadata_path, maxBytes: 20000 });
    } catch (err) {
      meta.textContent = String(err);
    }
  } else {
    meta.textContent = "(no metadata.json)";
  }
}

$("#psd-list").addEventListener("click", (e) => {
  const card = e.target.closest("[data-psd-index]");
  if (card) showPsdDetail(parseInt(card.dataset.psdIndex, 10));
});

$("#psd-detail").addEventListener("click", async (e) => {
  const which = e.target.dataset.psdOpen;
  if (!which) return;
  const o = psdOutputs[parseInt($("#psd-detail").dataset.index, 10)];
  if (!o) return;
  const path = which === "folder" ? o.psd_path.replace(/[/\\][^/\\]*$/, "") : o.psd_path;
  try {
    await invoke("open_path", { path });
    toast(which === "folder" ? "opened folder" : "opened PSD", "ok");
  } catch (err) {
    toast(String(err), "err");
  }
});

$("#psd-refresh").addEventListener("click", loadPsdOutputs);
$("#psd-use-output").addEventListener("click", async () => {
  try {
    const info = await invoke("get_runtime_info");
    $("#psd-dir").value = info.output_dir.path;
    loadPsdOutputs();
  } catch (err) {
    toast(String(err), "err");
  }
});

// ---- node editor (studio-ui React Flow sub-app) ----
// Lazily mount the studio-ui build (served at studio/index.html under the same
// Tauri origin) on first open. Its Tauri bridge reaches IPC via the parent
// window, so the embedded editor can call run_task_json / generate_thumbnail.
let nodeEditorEmbedded = false;

async function ensureNodeEditorEmbedded() {
  if (nodeEditorEmbedded) return;
  const frame = $("#studio-frame");
  const placeholder = $("#studio-placeholder");
  // The editor build (studio/) is produced by `npm run build` in studio-ui.
  // A plain `cargo run` does not build it, so check first and show a hint
  // instead of a broken iframe when the build is missing.
  try {
    const res = await fetch("studio/index.html", { method: "GET" });
    if (!res.ok) throw new Error(String(res.status));
  } catch {
    placeholder.innerHTML =
      "<p>Node Editor build not found</p>" +
      '<p class="hint">Build it once with ' +
      "<code>npm --prefix apps/desktop-tauri/studio-ui ci &amp;&amp; " +
      "npm --prefix apps/desktop-tauri/studio-ui run build</code>, then reopen this tab. " +
      "(The Tauri CLI does this automatically; a plain <code>cargo run</code> does not.)</p>";
    return;
  }
  frame.addEventListener("load", () => {
    placeholder.classList.add("hidden");
    frame.classList.remove("hidden");
  });
  frame.src = "studio/index.html";
  nodeEditorEmbedded = true;
}

// ---- comfyui ----
let comfyEmbedded = false;

function embedComfy() {
  const url = $("#comfy-url").value.trim();
  const frame = $("#comfy-frame");
  const status = $("#comfy-status");
  const placeholder = $("#comfy-placeholder");
  if (!url) {
    status.textContent = "enter a ComfyUI URL";
    status.className = "status err";
    return;
  }
  status.textContent = "connecting…";
  status.className = "status";
  placeholder.classList.add("hidden");
  frame.classList.remove("hidden");
  // Cache-bust so Reload re-fetches even if the URL is unchanged.
  frame.src = url + (url.includes("?") ? "&" : "?") + "_hg=" + Date.now();
  comfyEmbedded = true;
}

function ensureComfyEmbedded() {
  if (!comfyEmbedded) embedComfy();
}

$("#comfy-frame").addEventListener("load", () => {
  const status = $("#comfy-status");
  status.textContent = "connected";
  status.className = "status ok";
});

function comfyPort() {
  try {
    return Number(new URL($("#comfy-url").value.trim()).port) || 8188;
  } catch {
    return 8188;
  }
}

$("#comfy-reload").addEventListener("click", embedComfy);

$("#comfy-start").addEventListener("click", async () => {
  const status = $("#comfy-status");
  const dir = $("#comfy-dir").value.trim();
  const args = $("#comfy-args").value.trim();
  const port = comfyPort();
  try {
    status.textContent = "starting ComfyUI…";
    status.className = "status";
    const msg = await invoke("start_comfyui", {
      dir: dir || null,
      port,
      args: args || null,
    });
    status.textContent = msg + " — waiting for server…";
    // Poll until the port accepts connections, then embed once (avoids
    // hammering the iframe while ComfyUI is still booting).
    let waited = 0;
    const poll = async () => {
      if (await invoke("comfyui_reachable", { port })) {
        embedComfy();
        return;
      }
      waited += 1500;
      if (waited < 90000) setTimeout(poll, 1500);
      else {
        status.textContent = "server did not come up — check args/folder";
        status.className = "status err";
      }
    };
    setTimeout(poll, 1500);
  } catch (err) {
    status.textContent = String(err);
    status.className = "status err";
  }
});

$("#comfy-stop").addEventListener("click", async () => {
  const status = $("#comfy-status");
  try {
    await invoke("stop_comfyui");
    const frame = $("#comfy-frame");
    frame.classList.add("hidden");
    frame.src = "about:blank";
    $("#comfy-placeholder").classList.remove("hidden");
    comfyEmbedded = false;
    status.textContent = "stopped";
    status.className = "status";
  } catch (err) {
    status.textContent = String(err);
    status.className = "status err";
  }
});

$("#comfy-open").addEventListener("click", async () => {
  const status = $("#comfy-status");
  try {
    await invoke("open_url", { url: $("#comfy-url").value.trim() });
    status.textContent = "opened in browser";
    status.className = "status ok";
  } catch (err) {
    status.textContent = String(err);
    status.className = "status err";
  }
});

// ---- psd studio ----
// First production-flow entry point. Composes an ApiTask from a provider
// profile + prompt + reference image + PSD template and runs it through the
// existing broker (`run_task_json`). The PSD template path is carried on the
// task so a future export step can write the result back into the template.
let studioProfiles = {};
let studioProfilesLoaded = false;

async function loadStudioProfiles() {
  const select = $("#studio-profile");
  const current = select.value;
  try {
    const items = await invoke("get_profiles");
    studioProfiles = {};
    const options = ['<option value="">— none (use provider below) —</option>'];
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

function ensureStudioProfiles() {
  if (!studioProfilesLoaded) loadStudioProfiles();
}

function applyStudioProfile() {
  const ref = $("#studio-profile").value;
  const summary = $("#studio-profile-summary");
  const profile = studioProfiles[ref];
  if (!profile) {
    summary.textContent = "";
    return;
  }
  if (profile.provider) $("#studio-provider").value = profile.provider;
  // Seed the model into params without clobbering anything the user typed.
  if (profile.model) {
    let params = {};
    const raw = $("#studio-params").value.trim();
    if (raw) {
      try {
        params = JSON.parse(raw);
      } catch {
        params = {};
      }
    }
    if (params.model === undefined) {
      params.model = profile.model;
      $("#studio-params").value = pretty(params);
    }
  }
  summary.textContent =
    `provider: ${profile.provider ?? "-"} · model: ${profile.model ?? "-"} · ` +
    `creds: ${profile.credentials_ref ?? "-"}`;
}

function studioBuildTask() {
  const profileRef = $("#studio-profile").value;
  const provider = $("#studio-provider").value.trim() || "mock";
  const operation = $("#studio-operation").value;
  const outputType = $("#studio-output").value;
  const prompt = $("#studio-prompt").value;
  const template = $("#studio-template").value.trim();
  const reference = $("#studio-reference").value.trim();

  let params = {};
  const rawParams = $("#studio-params").value.trim();
  if (rawParams) params = JSON.parse(rawParams); // surfaced as a JSON error

  const inputs = {};
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

function studioPreview() {
  const status = $("#studio-status");
  try {
    $("#studio-task").textContent = pretty(studioBuildTask());
    status.textContent = "task ready";
    status.className = "status ok";
  } catch (err) {
    status.textContent = "params: " + err;
    status.className = "status err";
  }
}

function renderStudioOutputs(result) {
  const target = $("#studio-outputs");
  const files = result.output_files ?? [];
  if (!files.length) {
    target.innerHTML = "";
    return;
  }
  target.innerHTML = files
    .map(
      (f, i) =>
        `<div class="card"><div class="label">output ${i + 1}</div><div class="value">${esc(f.path)}</div><div class="row"><button data-studio-open="${i}">Open</button></div></div>`
    )
    .join("");
  target.dataset.files = JSON.stringify(files.map((f) => f.path));
}

$("#studio-profile").addEventListener("change", applyStudioProfile);
$("#studio-preview").addEventListener("click", studioPreview);

$("#studio-run").addEventListener("click", async () => {
  const status = $("#studio-status");
  let task;
  try {
    task = studioBuildTask();
  } catch (err) {
    status.textContent = "params: " + err;
    status.className = "status err";
    return;
  }
  $("#studio-task").textContent = pretty(task);
  status.textContent = "generating…";
  status.className = "status";
  try {
    const result = await invoke("run_task_json", { taskJson: JSON.stringify(task) });
    $("#studio-result").textContent = pretty(result);
    renderStudioOutputs(result);
    status.textContent = result.status;
    status.className = "status " + (result.status === "failed" ? "err" : "ok");
  } catch (err) {
    $("#studio-result").textContent = String(err);
    status.textContent = "error";
    status.className = "status err";
  }
});

$("#studio-outputs").addEventListener("click", async (e) => {
  const idx = e.target.dataset.studioOpen;
  if (idx === undefined) return;
  const files = JSON.parse($("#studio-outputs").dataset.files || "[]");
  const path = files[parseInt(idx, 10)];
  if (!path) return;
  try {
    await invoke("open_path", { path });
    toast("opened output", "ok");
  } catch (err) {
    toast(String(err), "err");
  }
});

$("#studio-reference-preview").addEventListener("click", async () => {
  const path = $("#studio-reference").value.trim();
  const img = $("#studio-reference-img");
  if (!path) {
    img.classList.add("hidden");
    img.removeAttribute("src");
    return;
  }
  try {
    img.src = await invoke("read_image_data_url", { path });
    img.classList.remove("hidden");
  } catch (err) {
    img.classList.add("hidden");
    img.removeAttribute("src");
    toast(String(err), "err");
  }
});

$("#studio-template-from-psd").addEventListener("click", async () => {
  try {
    const info = await invoke("get_runtime_info");
    const outputs = await invoke("list_psd_outputs", { dir: info.output_dir.path });
    if (!outputs.length) {
      toast("no PSD files in output dir", "err");
      return;
    }
    $("#studio-template").value = outputs[0].psd_path;
    toast(`picked ${outputs[0].name}.psd (${outputs.length} found)`, "ok");
  } catch (err) {
    toast(String(err), "err");
  }
});

// ---- init ----
loadDashboard();
loadConfig("credentials");
loadConfig("profiles");
loadStudioProfiles();
