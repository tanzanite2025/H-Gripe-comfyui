// PSD Studio tab: the first production-flow entry point. Composes an ApiTask
// from a provider profile + prompt + reference image + PSD template and runs it
// through the existing broker (`run_task_json`). The PSD template path is
// carried on the task so a future export step can write the result back into the
// template.

import { useCallback, useEffect, useState } from "react";

import { commands, pretty, type ApiResult, type ProfileSummary } from "../bridge/desktop";
import { useT } from "../i18n";
import { emptyStatus, Status, useToast, type StatusState } from "./common";

interface StudioTask {
  id: string;
  provider: string;
  operation: string;
  inputs: Record<string, string>;
  params: Record<string, unknown>;
  credentials_ref: string | null;
  output_type: string;
  cache_policy: { enabled: boolean; ttl_seconds: number | null; key: string | null };
  retry_policy: { max_attempts: number; backoff_ms: number; timeout_ms: number };
}

export function PsdStudioPanel() {
  const t = useT();
  const toast = useToast();
  const [profiles, setProfiles] = useState<Record<string, ProfileSummary>>({});
  const [profileRef, setProfileRef] = useState("");
  const [profileSummary, setProfileSummary] = useState("");
  const [provider, setProvider] = useState("");
  const [operation, setOperation] = useState("image.generate");
  const [outputType, setOutputType] = useState("image");
  const [prompt, setPrompt] = useState("");
  const [template, setTemplate] = useState("");
  const [reference, setReference] = useState("");
  const [params, setParams] = useState("");
  const [referenceImg, setReferenceImg] = useState<string | null>(null);
  const [taskJson, setTaskJson] = useState("");
  const [result, setResult] = useState("");
  const [outputs, setOutputs] = useState<string[]>([]);
  const [status, setStatus] = useState<StatusState>(emptyStatus);

  const loadProfiles = useCallback(async () => {
    try {
      const items = await commands.getProfiles();
      const map: Record<string, ProfileSummary> = {};
      items.forEach((p) => {
        map[p.profile_ref] = p;
      });
      setProfiles(map);
    } catch (err) {
      toast(String(err), "err");
    }
  }, [toast]);

  useEffect(() => {
    void loadProfiles();
  }, [loadProfiles]);

  const applyProfile = useCallback(
    (ref: string) => {
      setProfileRef(ref);
      const profile = profiles[ref];
      if (!profile) {
        setProfileSummary("");
        return;
      }
      if (profile.provider) setProvider(profile.provider);
      // Seed the model into params without clobbering anything the user typed.
      if (profile.model) {
        let parsed: Record<string, unknown> = {};
        const raw = params.trim();
        if (raw) {
          try {
            parsed = JSON.parse(raw);
          } catch {
            parsed = {};
          }
        }
        if (parsed.model === undefined) {
          parsed.model = profile.model;
          setParams(pretty(parsed));
        }
      }
      setProfileSummary(
        `${t("field.provider")}: ${profile.provider ?? "-"} · ${t("field.model")}: ${profile.model ?? "-"} · ` +
          `${t("field.creds")}: ${profile.credentials_ref ?? "-"}`,
      );
    },
    [profiles, params, t],
  );

  const buildTask = useCallback((): StudioTask => {
    let parsedParams: Record<string, unknown> = {};
    const rawParams = params.trim();
    if (rawParams) parsedParams = JSON.parse(rawParams); // surfaced as a JSON error

    const inputs: Record<string, string> = {};
    if (prompt.trim()) inputs.prompt = prompt;
    if (reference.trim()) inputs.image_path = reference.trim();
    if (template.trim()) inputs.template_path = template.trim();

    return {
      id: "studio-" + Date.now(),
      provider: provider.trim() || "mock",
      operation,
      inputs,
      params: parsedParams,
      credentials_ref: profiles[profileRef]?.credentials_ref ?? null,
      output_type: outputType,
      cache_policy: { enabled: false, ttl_seconds: null, key: null },
      retry_policy: { max_attempts: 1, backoff_ms: 200, timeout_ms: 60000 },
    };
  }, [params, prompt, reference, template, provider, operation, profiles, profileRef, outputType]);

  const preview = useCallback(() => {
    try {
      setTaskJson(pretty(buildTask()));
      setStatus({ text: t("studio.taskReady"), kind: "ok" });
    } catch (err) {
      setStatus({ text: t("field.params") + ": " + err, kind: "err" });
    }
  }, [buildTask, t]);

  const run = useCallback(async () => {
    let task: StudioTask;
    try {
      task = buildTask();
    } catch (err) {
      setStatus({ text: t("field.params") + ": " + err, kind: "err" });
      return;
    }
    setTaskJson(pretty(task));
    setStatus({ text: t("studio.generating"), kind: "" });
    try {
      const res: ApiResult = await commands.runTaskJson(JSON.stringify(task));
      setResult(pretty(res));
      setOutputs((res.output_files ?? []).map((f) => f.path));
      setStatus({ text: res.status, kind: res.status === "failed" ? "err" : "ok" });
    } catch (err) {
      setResult(String(err));
      setOutputs([]);
      setStatus({ text: t("common.error"), kind: "err" });
    }
  }, [buildTask, t]);

  const openOutput = useCallback(
    async (path: string) => {
      try {
        await commands.openPath(path);
        toast(t("studio.openedOutput"), "ok");
      } catch (err) {
        toast(String(err), "err");
      }
    },
    [t, toast],
  );

  const previewReference = useCallback(async () => {
    const path = reference.trim();
    if (!path) {
      setReferenceImg(null);
      return;
    }
    try {
      setReferenceImg(await commands.readImageDataUrl(path));
    } catch (err) {
      setReferenceImg(null);
      toast(String(err), "err");
    }
  }, [reference, toast]);

  const templateFromPsd = useCallback(async () => {
    try {
      const info = await commands.getRuntimeInfo();
      const found = await commands.listPsdOutputs(info.output_dir.path);
      if (!found.length) {
        toast(t("studio.noPsdInOutput"), "err");
        return;
      }
      setTemplate(found[0].psd_path);
      toast(t("studio.pickedPsd", { name: found[0].name, count: found.length }), "ok");
    } catch (err) {
      toast(String(err), "err");
    }
  }, [t, toast]);

  return (
    <>
      <div className="row wrap">
        <h2>{t("studio.heading")}</h2>
        <span className="hint">{t("studio.hint")}</span>
      </div>

      <div className="studio-grid">
        <div className="studio-field">
          <label>{t("studio.providerProfile")}</label>
          <select value={profileRef} onChange={(e) => applyProfile(e.target.value)}>
            <option value="">{t("studio.optionNone")}</option>
            {Object.keys(profiles).map((ref) => (
              <option value={ref} key={ref}>
                {ref}
              </option>
            ))}
          </select>
          <div className="hint">{profileSummary}</div>
        </div>
        <div className="studio-field">
          <label>{t("studio.provider")}</label>
          <input
            placeholder={t("studio.providerPh")}
            value={provider}
            onChange={(e) => setProvider(e.target.value)}
          />
        </div>
        <div className="studio-field">
          <label>{t("studio.operation")}</label>
          <select value={operation} onChange={(e) => setOperation(e.target.value)}>
            <option value="image.generate">image.generate</option>
            <option value="image.edit">image.edit</option>
            <option value="text.generate">text.generate</option>
            <option value="echo">echo (mock)</option>
          </select>
        </div>
        <div className="studio-field">
          <label>{t("studio.outputType")}</label>
          <select value={outputType} onChange={(e) => setOutputType(e.target.value)}>
            <option value="image">image</option>
            <option value="json">json</option>
            <option value="text">text</option>
            <option value="files">files</option>
            <option value="any">any</option>
          </select>
        </div>
      </div>

      <label className="studio-block-label">{t("studio.prompt")}</label>
      <textarea
        id="studio-prompt"
        spellCheck={false}
        placeholder={t("studio.promptPh")}
        value={prompt}
        onChange={(e) => setPrompt(e.target.value)}
      />

      <div className="studio-grid">
        <div className="studio-field">
          <label>{t("studio.psdTemplate")}</label>
          <input
            placeholder={t("studio.psdTemplatePh")}
            value={template}
            onChange={(e) => setTemplate(e.target.value)}
          />
          <div className="row">
            <button onClick={() => void templateFromPsd()}>{t("studio.pickFromPsd")}</button>
          </div>
        </div>
        <div className="studio-field">
          <label>{t("studio.reference")}</label>
          <input
            placeholder={t("studio.referencePh")}
            value={reference}
            onChange={(e) => setReference(e.target.value)}
          />
          <div className="row">
            <button onClick={() => void previewReference()}>{t("studio.preview")}</button>
            <span className="hint">{t("studio.referenceHint")}</span>
          </div>
        </div>
      </div>
      {referenceImg && <img className="psd-preview" src={referenceImg} alt="reference preview" />}

      <label className="studio-block-label">{t("studio.params")}</label>
      <textarea
        id="studio-params"
        spellCheck={false}
        placeholder='{ "model": "…", "size": "1024x1024" }'
        value={params}
        onChange={(e) => setParams(e.target.value)}
      />

      <div className="row wrap">
        <button onClick={preview}>{t("studio.previewTask")}</button>
        <button className="primary" onClick={() => void run()}>
          {t("studio.generate")}
        </button>
        <Status status={status} />
      </div>

      <h3>{t("studio.task")}</h3>
      <pre className="json">{taskJson}</pre>
      <h3>{t("studio.result")}</h3>
      <pre className="json">{result}</pre>
      <div className="cards">
        {outputs.map((path, i) => (
          <div className="card" key={path}>
            <div className="label">{t("studio.outputN", { n: i + 1 })}</div>
            <div className="value">{path}</div>
            <div className="row">
              <button onClick={() => void openOutput(path)}>{t("studio.open")}</button>
            </div>
          </div>
        ))}
      </div>
    </>
  );
}
