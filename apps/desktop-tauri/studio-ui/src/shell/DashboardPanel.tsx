// Dashboard tab: runtime paths + `doctor` diagnostics.

import { useCallback, useEffect, useState } from "react";

import { commands, pretty, type PathInfo, type RuntimeInfo } from "../bridge/desktop";
import { probeEngines, type EngineProbeReport } from "../bridge/psd";
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
  const [engines, setEngines] = useState<EngineProbeReport | null>(null);

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
    try {
      setEngines(await probeEngines());
    } catch {
      setEngines(null);
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
      {engines?.runtime && (
        <>
          <h2>{t("dashboard.compute")}</h2>
          <p className="hint">{t("dashboard.computeDesc")}</p>
          <div className="cards">
            <div className="card">
              <div className="label">{t("dashboard.computeCuda")}</div>
              <div
                className={"value " + (engines.runtime.cuda_available ? "ok" : "missing")}
              >
                {engines.runtime.cuda_available
                  ? engines.runtime.devices
                      .map((d) => `${d.name} (${d.total_memory_mb} MB)`)
                      .join(", ") || t("dashboard.computeCuda")
                  : t("dashboard.computeCpuOnly")}
              </div>
            </div>
            <div className="card">
              <div className="label">{t("dashboard.computeProviders")}</div>
              <div className="value">
                {engines.runtime.onnxruntime.installed
                  ? engines.runtime.onnxruntime.providers.join(", ")
                  : t("dashboard.computeNotInstalled")}
              </div>
            </div>
          </div>
        </>
      )}
      {engines && engines.cards.length > 0 && (
        <>
          <h2>{t("dashboard.engines")}</h2>
          <p className="hint">{t("dashboard.enginesDesc")}</p>
          <div className="cards">
            {engines.cards.map((card) => (
              <div className="card" key={card.node_kind}>
                <div className="label">
                  {card.node_kind} ({card.cli})
                </div>
                {card.error ? (
                  <div className="value missing">{card.error}</div>
                ) : (
                  Object.entries(card.engines).map(([id, state]) => (
                    <div className="value" key={id}>
                      <span className={state.available ? "ok" : "missing"}>
                        {id} —{" "}
                        {state.available
                          ? t("dashboard.engineAvailable")
                          : t("dashboard.engineUnavailable")}
                      </span>
                      {state.reason && <small className="hint"> {state.reason}</small>}
                      {state.weight && (
                        <small className={state.weight.present ? "hint ok" : "hint missing"}>
                          {" "}
                          {state.weight.present
                            ? t("dashboard.weightCached") +
                              (state.weight.size_mb != null
                                ? ` (${state.weight.size_mb} MB)`
                                : "")
                            : t("dashboard.weightMissing")}
                        </small>
                      )}
                    </div>
                  ))
                )}
              </div>
            ))}
          </div>
        </>
      )}
      <h2>{t("dashboard.doctor")}</h2>
      <pre className="json">{doctor}</pre>
    </>
  );
}
