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

// ---- tabs ----
$$("#tabs button").forEach((btn) => {
  btn.addEventListener("click", () => {
    $$("#tabs button").forEach((b) => b.classList.remove("active"));
    $$(".panel").forEach((p) => p.classList.remove("active"));
    btn.classList.add("active");
    $(`#${btn.dataset.tab}`).classList.add("active");
  });
});

// ---- dashboard ----
function pathCard(label, info) {
  const cls = info.exists ? "ok" : "missing";
  const flag = info.exists ? "found" : "missing";
  return `<div class="card"><div class="label">${label} (${flag})</div><div class="value ${cls}">${info.path}</div></div>`;
}

async function loadDashboard() {
  try {
    const info = await invoke("get_runtime_info");
    $("#runtime-info").innerHTML = [
      `<div class="card"><div class="label">providers</div><div class="value">${info.providers.join(", ")}</div></div>`,
      pathCard("credentials.json", info.credentials_file),
      pathCard("provider_profiles.json", info.profiles_file),
      pathCard("history file", info.history_file),
      pathCard("history db", info.history_db),
      pathCard("output dir", info.output_dir),
    ].join("");
  } catch (err) {
    $("#runtime-info").innerHTML = `<div class="card"><div class="value missing">${err}</div></div>`;
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
          `<div class="card"><div class="label">${c.credential_ref}</div><div class="value">provider: ${c.provider ?? "-"}<br/>key: ${c.api_key_configured ? "set" : c.api_key_env ? "env:" + c.api_key_env : "none"}<br/>headers: ${c.headers_count}</div></div>`
      )
      .join("") || `<div class="card"><div class="value">no entries</div></div>`,
  profiles: (items) =>
    items
      .map(
        (p) =>
          `<div class="card"><div class="label">${p.profile_ref}</div><div class="value">provider: ${p.provider ?? "-"}<br/>model: ${p.model ?? "-"}<br/>creds: ${p.credentials_ref ?? "-"}<br/>params: ${p.params_count}</div></div>`
      )
      .join("") || `<div class="card"><div class="value">no entries</div></div>`,
};

async function loadConfig(kind) {
  const listCmd = kind === "credentials" ? "get_credentials" : "get_profiles";
  try {
    const items = await invoke(listCmd);
    $(`#${kind}-summary`).innerHTML = summaryRenderers[kind](items);
  } catch (err) {
    $(`#${kind}-summary`).innerHTML = `<div class="card"><div class="value missing">${err}</div></div>`;
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
      (issues.length ? `<pre class="json">${pretty(issues)}</pre>` : "");
  } catch (err) {
    target.innerHTML = `<span class="badge err">${err}</span>`;
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
          <td>${time}</td><td>${r.provider}</td><td>${r.operation}</td>
          <td>${r.status}</td><td>${r.output_file_count}</td>
          <td><button data-detail="${r.task_id}">view</button> <button data-rerun="${r.task_id}">rerun</button></td>
        </tr>`;
      })
      .join("");
    if (!records.length) tbody.innerHTML = `<tr><td colspan="6">no records</td></tr>`;
  } catch (err) {
    tbody.innerHTML = `<tr><td colspan="6" class="status err">${err}</td></tr>`;
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

// ---- comfyui ----
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

// ---- init ----
loadDashboard();
loadConfig("credentials");
loadConfig("profiles");
