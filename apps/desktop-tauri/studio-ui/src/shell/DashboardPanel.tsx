// Dashboard tab: runtime paths + `doctor` diagnostics.

import { useCallback, useEffect, useState } from "react";

import { commands, pretty, type PathInfo, type RuntimeInfo } from "../bridge/desktop";
import { probeEngines, type EngineProbeReport } from "../bridge/engineProbe";
import {
  getModelPaths,
  setModelPaths,
  type ModelPathsReport,
} from "../bridge/modelPaths";
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

/** Editor for the persisted local-model weight paths (`weights_path`
 * management): one row per opt-in ML engine plus the shared cache dir. A row
 * whose env var is set on the process shows that value read-only instead,
 * since the env override always wins. Saving re-runs the capability probe so
 * the Engines section reflects the new paths. */
function ModelManagerSection({
  report,
  onSaved,
}: {
  report: ModelPathsReport;
  onSaved: () => void;
}) {
  const t = useT();
  const [cacheDir, setCacheDir] = useState(report.config.model_cache_dir ?? "");
  const [weights, setWeights] = useState<Record<string, string>>(() => {
    const map: Record<string, string> = {};
    for (const entry of report.entries) map[entry.engine] = entry.configured_path ?? "";
    return map;
  });
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const save = async () => {
    setSaving(true);
    setError(null);
    try {
      const cleaned: Record<string, string> = {};
      for (const [engine, path] of Object.entries(weights)) {
        if (path.trim()) cleaned[engine] = path.trim();
      }
      await setModelPaths({ model_cache_dir: cacheDir.trim() || null, weights: cleaned });
      onSaved();
    } catch (err) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  };

  return (
    <>
      <h2>{t("dashboard.models")}</h2>
      <p className="hint">{t("dashboard.modelsDesc")}</p>
      <div className="cards">
        <div className="card">
          <div className="label">{t("dashboard.modelsCacheDir")} (HGRIPE_MODEL_CACHE)</div>
          {report.cache_env_active ? (
            <div className="value">
              {report.cache_env_value}
              <small className="hint"> {t("dashboard.modelsEnvActive")}</small>
            </div>
          ) : (
            <input
              type="text"
              value={cacheDir}
              placeholder={t("dashboard.modelsCachePlaceholder")}
              onChange={(e) => setCacheDir(e.target.value)}
            />
          )}
        </div>
        {report.entries.map((entry) => (
          <div className="card" key={entry.engine}>
            <div className="label">
              {entry.engine} ({entry.env_var})
            </div>
            {entry.env_active ? (
              <div className="value">
                {entry.env_value}
                <small className="hint"> {t("dashboard.modelsEnvActive")}</small>
              </div>
            ) : (
              <>
                <input
                  type="text"
                  value={weights[entry.engine] ?? ""}
                  placeholder={t("dashboard.modelsPathPlaceholder")}
                  onChange={(e) =>
                    setWeights((prev) => ({ ...prev, [entry.engine]: e.target.value }))
                  }
                />
                {(entry.configured_path ?? "").trim() !== "" && (
                  <small className={entry.configured_exists ? "hint ok" : "hint missing"}>
                    {entry.configured_exists
                      ? t("dashboard.modelsPathFound")
                      : t("dashboard.modelsPathMissing")}
                  </small>
                )}
              </>
            )}
          </div>
        ))}
      </div>
      {error && <p className="hint missing">{error}</p>}
      <div className="row">
        <button onClick={() => void save()} disabled={saving}>
          {saving ? t("common.loadingShort") : t("dashboard.modelsSave")}
        </button>
        <small className="hint">
          {t("dashboard.modelsConfigFile")} {report.config_file}
        </small>
      </div>
    </>
  );
}

export function DashboardPanel() {
  const t = useT();
  const [info, setInfo] = useState<RuntimeInfo | null>(null);
  const [infoErr, setInfoErr] = useState<string | null>(null);
  const [doctor, setDoctor] = useState<string>(() => t("common.loading"));
  const [engines, setEngines] = useState<EngineProbeReport | null>(null);
  const [modelPaths, setModelPathsReport] = useState<ModelPathsReport | null>(null);

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
    try {
      setModelPathsReport(await getModelPaths());
    } catch {
      setModelPathsReport(null);
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
            <PathCard label="API keys file" info={info.credentials_file} />
            <PathCard label="provider profiles file" info={info.profiles_file} />
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
      {modelPaths && (
        <ModelManagerSection
          key={JSON.stringify(modelPaths.config)}
          report={modelPaths}
          onSaved={() => void load()}
        />
      )}
      <h2>{t("dashboard.doctor")}</h2>
      <pre className="json">{doctor}</pre>
    </>
  );
}
