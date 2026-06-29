// Dashboard tab: runtime paths + `doctor` diagnostics.

import { useCallback, useEffect, useState } from "react";

import { commands, pretty, type PathInfo, type RuntimeInfo } from "../bridge/desktop";
import { useT } from "../i18n";

function PathCard({ label, info }: { label: string; info: PathInfo }) {
  const t = useT();
  const flag = info.exists ? t("common.found") : t("common.missing");
  return (
    <div className="card">
      <div className="label">
        {label} ({flag})
      </div>
      <div className={"value " + (info.exists ? "ok" : "missing")}>{info.path}</div>
    </div>
  );
}

export function DashboardPanel() {
  const t = useT();
  const [info, setInfo] = useState<RuntimeInfo | null>(null);
  const [infoErr, setInfoErr] = useState<string | null>(null);
  const [doctor, setDoctor] = useState<string>(() => t("common.loading"));

  const load = useCallback(async () => {
    try {
      setInfoErr(null);
      setInfo(await commands.getRuntimeInfo());
    } catch (err) {
      setInfo(null);
      setInfoErr(String(err));
    }
    try {
      setDoctor(pretty(await commands.doctor()));
    } catch (err) {
      setDoctor(String(err));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <>
      <div className="row">
        <h2>{t("dashboard.runtime")}</h2>
        <button onClick={() => void load()}>{t("btn.refresh")}</button>
      </div>
      <div className="cards">
        {infoErr && (
          <div className="card">
            <div className="value missing">{infoErr}</div>
          </div>
        )}
        {info && (
          <>
            <div className="card">
              <div className="label">providers</div>
              <div className="value">{info.providers.join(", ")}</div>
            </div>
            <PathCard label="credentials.json" info={info.credentials_file} />
            <PathCard label="provider_profiles.json" info={info.profiles_file} />
            <PathCard label="history file" info={info.history_file} />
            <PathCard label="history db" info={info.history_db} />
            <PathCard label="output dir" info={info.output_dir} />
          </>
        )}
      </div>
      <h2>{t("dashboard.doctor")}</h2>
      <pre className="json">{doctor}</pre>
    </>
  );
}
