// Application shell: top bar + tab navigation hosting the former vanilla
// shell-ui tabs (Dashboard / PSD Studio / Run / History / PSD) as React panels,
// plus the React Flow Node Editor. This is the single front-end entry point
// that replaced the shell-ui + iframe split.

import { useCallback, useState } from "react";

import NodeEditor from "../App";
import { LangContext, loadLang, saveLang, useT, type Lang, type MsgKey } from "../i18n";
import { ToastProvider } from "./common";
import { DashboardPanel } from "./DashboardPanel";
import { PsdStudioPanel } from "./PsdStudioPanel";
import { RunTaskPanel } from "./RunTaskPanel";
import { HistoryPanel } from "./HistoryPanel";
import { PsdPanel } from "./PsdPanel";

type TabId =
  | "dashboard"
  | "studio"
  | "run"
  | "history"
  | "psd"
  | "node-editor";

const TABS: { id: TabId; label: MsgKey }[] = [
  { id: "dashboard", label: "tab.dashboard" },
  { id: "studio", label: "tab.studio" },
  { id: "run", label: "tab.run" },
  { id: "history", label: "tab.history" },
  { id: "psd", label: "tab.psd" },
  { id: "node-editor", label: "tab.nodeEditor" },
];

function ShellBody({ onToggleLang }: { onToggleLang: () => void }) {
  const t = useT();
  const [tab, setTab] = useState<TabId>("dashboard");

  return (
    <div className="shell">
      <header className="topbar">
        <h1>{t("brand.title")}</h1>
        <span className="subtitle">{t("brand.subtitle")}</span>
        <button
          className="lang-toggle"
          title={t("lang.toggleTitle")}
          onClick={onToggleLang}
        >
          {t("lang.toggle")}
        </button>
      </header>

      <nav className="tabs">
        {TABS.map((tabDef) => (
          <button
            key={tabDef.id}
            className={tab === tabDef.id ? "active" : undefined}
            onClick={() => setTab(tabDef.id)}
          >
            {t(tabDef.label)}
          </button>
        ))}
      </nav>

      <div className="shell-content">
        {tab === "node-editor" ? (
          <NodeEditor onToggleLang={onToggleLang} />
        ) : (
          <div className="shell-scroll">
            {tab === "dashboard" && <DashboardPanel />}
            {tab === "studio" && <PsdStudioPanel />}
            {tab === "run" && <RunTaskPanel />}
            {tab === "history" && <HistoryPanel />}
            {tab === "psd" && <PsdPanel />}
          </div>
        )}
      </div>
    </div>
  );
}

export default function Shell() {
  const [lang, setLang] = useState<Lang>(() => loadLang());
  const toggleLang = useCallback(() => {
    setLang((prev) => {
      const next: Lang = prev === "en" ? "zh" : "en";
      saveLang(next);
      return next;
    });
  }, []);

  return (
    <LangContext.Provider value={lang}>
      <ToastProvider>
        <ShellBody onToggleLang={toggleLang} />
      </ToastProvider>
    </LangContext.Provider>
  );
}
