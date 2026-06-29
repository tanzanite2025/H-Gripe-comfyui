// PSD tab: browse PSD exports (preview / metadata / smart-object markers).

import { commands, type PsdOutput } from "./tauri";
import { $, el, esc, t, toast } from "./dom";

let psdOutputs: PsdOutput[] = [];

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

export async function loadPsdOutputs(): Promise<void> {
  const dir = el<HTMLInputElement>("#psd-dir").value.trim();
  const list = el("#psd-list");
  el("#psd-detail").classList.add("hidden");
  if (!dir) {
    list.innerHTML = `<div class="card"><div class="value missing">${esc(t("psd.enterDir"))}</div></div>`;
    return;
  }
  list.innerHTML = `<div class="card"><div class="value">${esc(t("common.loadingShort"))}</div></div>`;
  try {
    psdOutputs = await commands.listPsdOutputs(dir);
    if (!psdOutputs.length) {
      list.innerHTML = `<div class="card"><div class="value">${esc(t("psd.noFiles"))}</div></div>`;
      return;
    }
    list.innerHTML = psdOutputs
      .map((o, i) => {
        const time = o.modified_ms ? new Date(Number(o.modified_ms)).toLocaleString() : "-";
        const tags = [
          o.preview_path ? t("psd.tagPreview") : null,
          o.metadata_path ? t("psd.tagMetadata") : null,
        ].filter((tag): tag is string => Boolean(tag));
        const badges = tags.map((tag) => `<span class="badge ok">${esc(tag)}</span>`).join(" ");
        const soBadge = o.smart_object ? ` <span class="badge so">${esc(t("psd.smartObject"))}</span>` : "";
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

async function showPsdDetail(index: number): Promise<void> {
  const o = psdOutputs[index];
  if (!o) return;
  const detail = el("#psd-detail");
  detail.classList.remove("hidden");
  detail.dataset.index = String(index);
  el("#psd-detail-name").textContent = `${o.name}.psd`;

  const soNote = $("#psd-detail-so");
  if (soNote) soNote.classList.toggle("hidden", !o.smart_object);

  const img = el<HTMLImageElement>("#psd-detail-preview");
  if (o.preview_path) {
    img.classList.remove("hidden");
    img.alt = t("psd.loadingPreview");
    try {
      img.src = await commands.readImageDataUrl(o.preview_path);
    } catch (err) {
      img.removeAttribute("src");
      img.alt = String(err);
    }
  } else {
    img.classList.add("hidden");
    img.removeAttribute("src");
  }

  const meta = el("#psd-detail-metadata");
  if (o.metadata_path) {
    try {
      meta.textContent = await commands.readTextFile(o.metadata_path, 20000);
    } catch (err) {
      meta.textContent = String(err);
    }
  } else {
    meta.textContent = t("psd.noMetadata");
  }
}

export function initPsd(): void {
  el("#psd-list").addEventListener("click", (e) => {
    const card = (e.target as HTMLElement).closest<HTMLElement>("[data-psd-index]");
    if (card?.dataset.psdIndex) showPsdDetail(parseInt(card.dataset.psdIndex, 10));
  });

  el("#psd-detail").addEventListener("click", async (e) => {
    const which = (e.target as HTMLElement).dataset.psdOpen;
    if (!which) return;
    const index = el("#psd-detail").dataset.index;
    const o = index !== undefined ? psdOutputs[parseInt(index, 10)] : undefined;
    if (!o) return;
    const path = which === "folder" ? o.psd_path.replace(/[/\\][^/\\]*$/, "") : o.psd_path;
    try {
      await commands.openPath(path);
      toast(which === "folder" ? t("psd.openedFolder") : t("psd.openedPsd"), "ok");
    } catch (err) {
      toast(String(err), "err");
    }
  });

  $("#psd-refresh")?.addEventListener("click", loadPsdOutputs);
  $("#psd-use-output")?.addEventListener("click", async () => {
    try {
      const info = await commands.getRuntimeInfo();
      el<HTMLInputElement>("#psd-dir").value = info.output_dir.path;
      loadPsdOutputs();
    } catch (err) {
      toast(String(err), "err");
    }
  });
}
