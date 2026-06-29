// PSD tab: browse PSD exports (preview / metadata / smart-object markers).

import { useCallback, useState } from "react";

import { commands, type PsdOutput } from "../bridge/desktop";
import { useT } from "../i18n";
import { useToast } from "./common";

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

interface Detail {
  output: PsdOutput;
  previewSrc: string | null;
  previewAlt: string;
  metadata: string;
}

export function PsdPanel() {
  const t = useT();
  const toast = useToast();
  const [dir, setDir] = useState("");
  const [outputs, setOutputs] = useState<PsdOutput[]>([]);
  const [listMsg, setListMsg] = useState<{ text: string; missing: boolean } | null>(null);
  const [detail, setDetail] = useState<Detail | null>(null);

  const load = useCallback(
    async (overrideDir?: string) => {
      const target = (overrideDir ?? dir).trim();
      setDetail(null);
      if (!target) {
        setOutputs([]);
        setListMsg({ text: t("psd.enterDir"), missing: true });
        return;
      }
      setOutputs([]);
      setListMsg({ text: t("common.loadingShort"), missing: false });
      try {
        const found = await commands.listPsdOutputs(target);
        if (!found.length) {
          setListMsg({ text: t("psd.noFiles"), missing: false });
          return;
        }
        setOutputs(found);
        setListMsg(null);
      } catch (err) {
        setListMsg({ text: String(err), missing: true });
      }
    },
    [dir, t],
  );

  const showDetail = useCallback(
    async (o: PsdOutput) => {
      let previewSrc: string | null = null;
      let previewAlt = "preview";
      if (o.preview_path) {
        previewAlt = t("psd.loadingPreview");
        try {
          previewSrc = await commands.readImageDataUrl(o.preview_path);
        } catch (err) {
          previewAlt = String(err);
        }
      }
      let metadata: string;
      if (o.metadata_path) {
        try {
          metadata = await commands.readTextFile(o.metadata_path, 20000);
        } catch (err) {
          metadata = String(err);
        }
      } else {
        metadata = t("psd.noMetadata");
      }
      setDetail({ output: o, previewSrc, previewAlt, metadata });
    },
    [t],
  );

  const open = useCallback(
    async (which: "psd" | "folder") => {
      const o = detail?.output;
      if (!o) return;
      const path = which === "folder" ? o.psd_path.replace(/[/\\][^/\\]*$/, "") : o.psd_path;
      try {
        await commands.openPath(path);
        toast(which === "folder" ? t("psd.openedFolder") : t("psd.openedPsd"), "ok");
      } catch (err) {
        toast(String(err), "err");
      }
    },
    [detail, t, toast],
  );

  const useOutputDir = useCallback(async () => {
    try {
      const info = await commands.getRuntimeInfo();
      setDir(info.output_dir.path);
      void load(info.output_dir.path);
    } catch (err) {
      toast(String(err), "err");
    }
  }, [load, toast]);

  return (
    <>
      <div className="row wrap">
        <h2>{t("psd.heading")}</h2>
        <input
          placeholder={t("psd.dirPh")}
          style={{ flex: 1 }}
          value={dir}
          onChange={(e) => setDir(e.target.value)}
        />
        <button onClick={() => void useOutputDir()}>{t("psd.useOutput")}</button>
        <button className="primary" onClick={() => void load()}>
          {t("btn.load")}
        </button>
      </div>
      <p className="hint">{t("psd.scanHint")}</p>
      <div className="psd-grid">
        {listMsg && (
          <div className="card">
            <div className={"value" + (listMsg.missing ? " missing" : "")}>{listMsg.text}</div>
          </div>
        )}
        {outputs.map((o) => {
          const tags = [
            o.preview_path ? t("psd.tagPreview") : null,
            o.metadata_path ? t("psd.tagMetadata") : null,
          ].filter((tag): tag is string => Boolean(tag));
          return (
            <div className="card psd-card" key={o.psd_path} onClick={() => void showDetail(o)}>
              <div className="label">{o.name}.psd</div>
              <div className="value">
                {o.modified_ms ? new Date(Number(o.modified_ms)).toLocaleString() : "-"}
                <br />
                {fmtBytes(o.size_bytes)}{" "}
                {tags.map((tag) => (
                  <span className="badge ok" key={tag}>
                    {tag}
                  </span>
                ))}
                {o.smart_object && <span className="badge so"> {t("psd.smartObject")}</span>}
              </div>
            </div>
          );
        })}
      </div>
      {detail && (
        <div className="psd-detail">
          <div className="row wrap">
            <h3>{detail.output.name}.psd</h3>
            <button onClick={() => void open("psd")}>{t("psd.openPsd")}</button>
            <button onClick={() => void open("folder")}>{t("psd.openFolder")}</button>
          </div>
          {detail.output.smart_object && (
            <div className="so-note">
              <span className="badge so">{t("psd.smartObject")}</span>{" "}
              <span>{t("psd.smartObjectNote")}</span>
            </div>
          )}
          {detail.previewSrc ? (
            <img className="psd-preview" src={detail.previewSrc} alt={detail.previewAlt} />
          ) : (
            detail.output.preview_path && (
              <div className="hint">{detail.previewAlt}</div>
            )
          )}
          <h4>{t("psd.metadata")}</h4>
          <pre className="json">{detail.metadata}</pre>
        </div>
      )}
    </>
  );
}
