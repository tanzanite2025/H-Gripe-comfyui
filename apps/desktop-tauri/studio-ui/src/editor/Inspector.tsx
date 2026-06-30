import { useContext, useEffect, useState } from "react";
import type { Node } from "@xyflow/react";
import { nodeSpec } from "../graph/nodeSpecs";
import { localizeSpec } from "../graph/nodeSpecsI18n";
import { LangContext, useT } from "../i18n";
import { probeEngines, type EngineProbeReport } from "../bridge/psd";
import { ParamField } from "./ParamField";
import { ProfilePicker } from "./ProfilePicker";
import { OutputPicker } from "./OutputPicker";
import { MediaViewer } from "./MediaViewer";
import type { HgripeNodeData } from "./HgripeNode";

interface InspectorProps {
  node: Node | null;
  onParamChange: (nodeId: string, key: string, value: unknown) => void;
}

// Right-side panel. Full-resolution media preview belongs here (not inside the
// node card), so the canvas stays light and previews never blow up node size.
export function Inspector({ node, onParamChange }: InspectorProps) {
  const [viewerPath, setViewerPath] = useState<string | null>(null);
  const [engineProbe, setEngineProbe] = useState<EngineProbeReport | null>(null);
  const lang = useContext(LangContext);
  const t = useT();

  // Probe the opt-in ML `engine` seams once so the inspector can grey out
  // engines whose deps/weights are missing on this box. Failures are non-fatal:
  // we leave every option enabled rather than blocking selection on a probe.
  useEffect(() => {
    let alive = true;
    probeEngines()
      .then((report) => alive && setEngineProbe(report))
      .catch(() => alive && setEngineProbe(null));
    return () => {
      alive = false;
    };
  }, []);

  if (!node) {
    return (
      <aside className="inspector">
        <p className="muted">{t("inspector.selectNode")}</p>
      </aside>
    );
  }

  const data = node.data as HgripeNodeData;

  // Group container: no ports/params, just a rename field.
  if (data.kind === "group") {
    return (
      <aside className="inspector">
        <h2>{t("inspector.group")}</h2>
        <p className="muted">{t("inspector.groupDesc")}</p>
        <label className="field">
          <span>{t("inspector.label")}</span>
          <input
            value={String(data.params.label ?? "")}
            onChange={(e) => onParamChange(node.id, "label", e.target.value)}
          />
        </label>
      </aside>
    );
  }

  const spec = localizeSpec(nodeSpec(data.kind), lang);

  // A param can declare `visibleWhen` to hide itself unless a sibling param has
  // one of the listed values (e.g. show API fields only when mode === "api").
  const isVisible = (p: (typeof spec.params)[number]) =>
    !p.visibleWhen || p.visibleWhen.in.includes(String(data.params[p.visibleWhen.param] ?? ""));

  // The profile picker only makes sense where API credentials are used: always
  // for `generate`, and for `promptOptimize` only in its `api` mode.
  const showProfilePicker =
    spec.kind === "generate" ||
    (spec.kind === "promptOptimize" && String(data.params.mode ?? "") === "api");

  return (
    <aside className="inspector">
      <h2>{spec.title}</h2>
      <p className="muted">{spec.description}</p>

      {showProfilePicker && (
        <ProfilePicker
          onApply={(profile) => {
            if (profile.provider) onParamChange(node.id, "provider", profile.provider);
            if (profile.model) onParamChange(node.id, "model", profile.model);
            onParamChange(node.id, "credentials_ref", profile.credentials_ref ?? "");
          }}
        />
      )}

      {spec.params.filter(isVisible).map((p) => {
        const raw = data.params[p.key];
        const onChange = (v: unknown) => onParamChange(node.id, p.key, v);
        // For the opt-in `engine` select, grey out engines the probe reports as
        // unavailable on this box (the CPU/`rules` baseline stays enabled). A
        // probe that did not run (browser preview, error) leaves all enabled.
        const card = engineProbe?.cards.find((c) => c.node_kind === data.kind);
        const optionStates =
          p.key === "engine" && card && !card.error && Object.keys(card.engines).length > 0
            ? card.engines
            : undefined;
        const selectedState = optionStates?.[String(raw ?? "")];
        const selectedUnavailable = selectedState && !selectedState.available;
        // For an available GPU-capable engine, note whether it would actually
        // run on a CUDA device or fall back to CPU on this box (machine probe).
        const runtime = engineProbe?.runtime;
        const deviceNote =
          selectedState?.available && selectedState.accelerated && runtime
            ? runtime.cuda_available
              ? t("inspector.engineGpu")
              : t("inspector.engineCpuFallback")
            : undefined;
        return (
          <label key={p.key} className="field">
            <span>{p.label}</span>
            <ParamField spec={p} value={raw} onChange={onChange} optionStates={optionStates} />
            {p.control === "path" && (
              <OutputPicker
                kind={spec.kind === "psdTemplate" ? "template" : "image"}
                onPick={(path) => onChange(path)}
              />
            )}
            {selectedUnavailable && (
              <small className="hint warn">{t("inspector.engineUnavailable")}</small>
            )}
            {deviceNote && <small className="hint">{deviceNote}</small>}
            {p.hint && <small className="hint">{p.hint}</small>}
          </label>
        );
      })}

      {data.imagePath && (
        <div className="field">
          <span>{t("inspector.output")}</span>
          <button
            type="button"
            className="inspector-img-btn"
            onClick={() => setViewerPath(data.imagePath ?? null)}
            title={t("inspector.viewFull")}
          >
            {data.thumbnail ? (
              <img className="inspector-img" src={data.thumbnail} alt="output" />
            ) : (
              <div className="inspector-img placeholder">{t("inspector.viewFull")}</div>
            )}
          </button>
          <code className="path">{data.imagePath}</code>
        </div>
      )}

      {viewerPath && <MediaViewer path={viewerPath} onClose={() => setViewerPath(null)} />}
    </aside>
  );
}
