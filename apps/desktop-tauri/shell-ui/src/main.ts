// Entry point for the desktop shell. Wires the language toggle + tab navigation,
// registers each domain module's event listeners, and kicks off the initial
// data loads. The HTML markup lives in ../index.html; styles are imported here
// so Vite bundles and fingerprints them.

import "./styles.css";

import { $$, el } from "./dom";
import { applyI18n, getLang, setLang } from "./i18n";
import { initDashboard, loadDashboard } from "./dashboard";
import { initConfig, loadConfig } from "./config";
import { initRun } from "./run";
import { initHistory } from "./history";
import { initPsd } from "./psd";
import { initComfy, ensureComfyEmbedded } from "./comfy";
import { ensureNodeEditorEmbedded } from "./nodeEditor";
import { initStudio, ensureStudioProfiles, loadStudioProfiles } from "./studio";

// ---- language toggle ----
applyI18n();
el("#lang-toggle").addEventListener("click", () => {
  setLang(getLang() === "zh" ? "en" : "zh");
});

// ---- tabs ----
$$<HTMLElement>("#tabs button").forEach((btn) => {
  btn.addEventListener("click", () => {
    $$<HTMLElement>("#tabs button").forEach((b) => b.classList.remove("active"));
    $$<HTMLElement>(".panel").forEach((p) => p.classList.remove("active"));
    btn.classList.add("active");
    const tab = btn.dataset.tab;
    if (tab) el(`#${tab}`).classList.add("active");
    if (tab === "comfyui") ensureComfyEmbedded();
    if (tab === "studio") ensureStudioProfiles();
    if (tab === "node-editor") ensureNodeEditorEmbedded();
  });
});

// ---- register domain handlers ----
initDashboard();
initConfig();
initRun();
initHistory();
initPsd();
initComfy();
initStudio();

// ---- init ----
loadDashboard();
loadConfig("credentials");
loadConfig("profiles");
loadStudioProfiles();
