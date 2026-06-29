// History tab: list / view / rerun / clean up recorded tasks.

import { commands, type ApiResult, type CleanupOptions, type HistoryQuery } from "./tauri";
import { $, el, esc, pretty, t, toast } from "./dom";

function inputValue(sel: string): string {
  return el<HTMLInputElement>(sel).value;
}

export async function loadHistory(): Promise<void> {
  const query: HistoryQuery = {
    limit: parseInt(inputValue("#history-limit") || "50", 10),
    provider: inputValue("#history-provider").trim() || null,
    operation: null,
    status: el<HTMLSelectElement>("#history-status").value || null,
    has_output_files: null,
  };
  const tbody = el("#history-table tbody");
  try {
    const records = await commands.listHistory(query);
    tbody.innerHTML = records
      .map((r) => {
        const time = new Date(Number(r.timestamp_ms)).toLocaleString();
        return `<tr>
          <td>${esc(time)}</td><td>${esc(r.provider)}</td><td>${esc(r.operation)}</td>
          <td>${esc(r.status)}</td><td>${esc(r.output_file_count)}</td>
          <td><button data-detail="${esc(r.task_id)}">${esc(t("history.view"))}</button> <button data-rerun="${esc(r.task_id)}">${esc(t("history.rerun"))}</button></td>
        </tr>`;
      })
      .join("");
    if (!records.length) tbody.innerHTML = `<tr><td colspan="6">${esc(t("history.noRecords"))}</td></tr>`;
  } catch (err) {
    tbody.innerHTML = `<tr><td colspan="6" class="status err">${esc(err)}</td></tr>`;
  }
}

function cleanupOptions(): CleanupOptions {
  const keep = inputValue("#cleanup-keep").trim();
  return {
    keep_latest: keep === "" ? null : parseInt(keep, 10),
    older_than_timestamp_ms: null,
    provider: inputValue("#history-provider").trim() || null,
    operation: null,
    status: el<HTMLSelectElement>("#history-status").value || null,
    has_output_files: null,
    delete_all_matched: false,
    delete_output_files: el<HTMLInputElement>("#cleanup-delete-files").checked,
  };
}

export function initHistory(): void {
  el("#history-table").addEventListener("click", async (e) => {
    const target = e.target as HTMLElement;
    const detailId = target.dataset.detail;
    const rerunId = target.dataset.rerun;
    if (detailId) {
      try {
        el("#history-detail").textContent = pretty(await commands.historyDetail(detailId));
      } catch (err) {
        el("#history-detail").textContent = String(err);
      }
    } else if (rerunId) {
      toast(t("history.rerunning", { id: rerunId }));
      try {
        const result: ApiResult = await commands.rerunTask(rerunId, true);
        el("#history-detail").textContent = pretty(result);
        toast(t("history.rerunDone", { status: result.status }), result.status === "failed" ? "err" : "ok");
        loadHistory();
      } catch (err) {
        toast(String(err), "err");
      }
    }
  });
  $("#history-refresh")?.addEventListener("click", loadHistory);

  $("#cleanup-preview")?.addEventListener("click", async () => {
    try {
      el("#cleanup-output").textContent = pretty(await commands.historyCleanupPreview(cleanupOptions()));
    } catch (err) {
      el("#cleanup-output").textContent = String(err);
    }
  });

  $("#cleanup-apply")?.addEventListener("click", async () => {
    try {
      const result = await commands.historyCleanupApply(cleanupOptions());
      el("#cleanup-output").textContent = pretty(result);
      toast(t("cleanup.applied"), "ok");
      loadHistory();
    } catch (err) {
      el("#cleanup-output").textContent = String(err);
      toast(String(err), "err");
    }
  });
}
