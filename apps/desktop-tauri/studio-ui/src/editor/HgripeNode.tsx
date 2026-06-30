import { memo, useContext, useEffect, useRef, useState } from "react";
import { Handle, Position, useStore, type NodeProps } from "@xyflow/react";
import { nodeSpec } from "../graph/nodeSpecs";
import { localizeSpec } from "../graph/nodeSpecsI18n";
import { LangContext, useT } from "../i18n";
import { isLodActive } from "./lod";
import type { NodeStatus } from "../runtime/dag";
import { generateThumbnail } from "../bridge/tauri";
import { ParamField } from "./ParamField";
import { useNodeEditing } from "./editingContext";
import { psdTemplatePathWarning } from "./psdcheck";

export interface HgripeNodeData extends Record<string, unknown> {
  kind: string;
  params: Record<string, unknown>;
  status?: NodeStatus;
  /** Last run's wall-clock duration in ms (executed nodes only). */
  durationMs?: number;
  /** Last run's error message, when `status === "failed"` / `cancelled`. */
  error?: string | null;
  /** Path of the most recent output image, if any (for the preview node). */
  imagePath?: string | null;
  /** Backend-generated thumbnail data URL / path for display. */
  thumbnail?: string | null;
  /** PSD Export results from the last run (psdExport node only). */
  psdPath?: string | null;
  psdPreviewPath?: string | null;
  psdMetadataPath?: string | null;
  /** Resolved placeholder kind / smart-object mode reported by the backend. */
  placeholderKind?: string | null;
  smartObjectMode?: string | null;
  /** Subject Mask outputs from the last run (subjectMask node only). */
  maskPath?: string | null;
  alphaImagePath?: string | null;
  cutoutImagePath?: string | null;
}

function basename(p: string): string {
  const parts = p.split(/[/\\]/);
  return parts[parts.length - 1] || p;
}

// Compact human-readable run time, e.g. "12ms" / "1.4s".
export function fmtDuration(ms?: number): string {
  if (ms == null) return "";
  if (ms < 1000) return `${Math.round(ms)}ms`;
  return `${(ms / 1000).toFixed(ms < 10000 ? 1 : 0)}s`;
}

// Thumbnail tile that only asks the backend for a thumbnail once the node
// actually scrolls into view (IntersectionObserver). This keeps the graph data
// light (it stores only the original path) and avoids decoding images for nodes
// parked off-screen — the real perf/quality discipline for large media.
function LazyThumb({ path }: { path: string }) {
  const t = useT();
  const ref = useRef<HTMLDivElement | null>(null);
  const [src, setSrc] = useState<string | null>(null);

  useEffect(() => {
    setSrc(null);
    const el = ref.current;
    if (!el) return;
    let cancelled = false;
    const io = new IntersectionObserver(
      (entries) => {
        if (!entries.some((e) => e.isIntersecting)) return;
        io.disconnect();
        generateThumbnail({ path, size: 256 })
          .then((thumb) => {
            if (!cancelled) setSrc(thumb.data_url || null);
          })
          .catch(() => {
            /* leave placeholder on failure */
          });
      },
      { threshold: 0.1 },
    );
    io.observe(el);
    return () => {
      cancelled = true;
      io.disconnect();
    };
  }, [path]);

  return (
    <div ref={ref} className="node-thumb-wrap">
      {src ? (
        <img className="node-thumb" src={src} alt="preview" />
      ) : (
        <div className="node-thumb placeholder">{t("common.loadingShort")}</div>
      )}
    </div>
  );
}

// Generic image media card body: a thumbnail + `name · W×H` info row + an
// action row whose buttons spawn a *bound* edit node (the source card is never
// mutated). Fetches the thumbnail once and reuses its reported dimensions for
// the info row. See docs/cards/generic-media-card.md.
function ImageSourceCard({ id, path }: { id: string; path: string }) {
  const t = useT();
  const editing = useNodeEditing();
  const ref = useRef<HTMLDivElement | null>(null);
  const [src, setSrc] = useState<string | null>(null);
  const [dims, setDims] = useState<{ w: number; h: number } | null>(null);

  useEffect(() => {
    setSrc(null);
    setDims(null);
    const el = ref.current;
    if (!el) return;
    let cancelled = false;
    const io = new IntersectionObserver(
      (entries) => {
        if (!entries.some((e) => e.isIntersecting)) return;
        io.disconnect();
        generateThumbnail({ path, size: 256 })
          .then((thumb) => {
            if (cancelled) return;
            setSrc(thumb.data_url || null);
            if (thumb.data_url && thumb.width && thumb.height) {
              setDims({ w: thumb.width, h: thumb.height });
            }
          })
          .catch(() => {
            /* leave placeholder on failure */
          });
      },
      { threshold: 0.1 },
    );
    io.observe(el);
    return () => {
      cancelled = true;
      io.disconnect();
    };
  }, [path]);

  return (
    <div ref={ref} className="media-card">
      {src ? (
        <img className="node-thumb" src={src} alt="preview" />
      ) : (
        <div className="node-thumb placeholder">{t("common.loadingShort")}</div>
      )}
      <div className="media-info">
        <span className="media-name" title={path}>
          {basename(path)}
        </span>
        {dims ? (
          <span className="media-dims">
            {dims.w}×{dims.h}
          </span>
        ) : null}
      </div>
      <div className="media-card-actions nodrag">
        <button
          type="button"
          className="primary"
          title={t("node.mediaEditMaskTitle")}
          onClick={() => editing?.addBoundEdit?.(id, "subjectMask")}
        >
          {t("node.mediaEditMask")}
        </button>
        <button
          type="button"
          title={t("node.mediaCropTitle")}
          onClick={() => editing?.addBoundEdit?.(id, "crop")}
        >
          {t("node.mediaCrop")}
        </button>
      </div>
    </div>
  );
}

// A single export-artifact row: label + basename, click to copy the full path.
function PathRow({ label, path }: { label: string; path: string }) {
  const t = useT();
  const [copied, setCopied] = useState(false);
  const copy = () => {
    void navigator.clipboard
      ?.writeText(path)
      .then(() => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1200);
      })
      .catch(() => {
        /* clipboard may be unavailable */
      });
  };
  return (
    <button className="psd-path-row nodrag" onClick={copy} title={t("node.copyHint", { path })}>
      <span className="psd-path-label">{label}</span>
      <span className="psd-path-name">{copied ? t("node.copied") : basename(path)}</span>
    </button>
  );
}

// Custom node is memoized (React Flow perf guidance): node drags must not
// re-render every node. The node shows only a compact summary + a thumbnail;
// full params live in the Inspector and full-res media is opened there.
function HgripeNodeImpl({ id, data, selected }: NodeProps) {
  const d = data as HgripeNodeData;
  const lang = useContext(LangContext);
  const t = useT();
  const spec = localizeSpec(nodeSpec(d.kind), lang);
  const status = d.status ?? "idle";
  const editing = useNodeEditing();
  // Collapse to a title-only card when zoomed far out. A boolean selector means
  // nodes only re-render when crossing the threshold, not on every zoom tick.
  const lod = useStore((s) => isLodActive(s.transform[2]));
  // Which input ports of this node currently have an incoming edge — used to
  // surface "image/template connected" hints on the PSD sink cards.
  const connectedPorts = useStore((s) =>
    s.edges
      .filter((e) => e.target === id)
      .map((e) => e.targetHandle ?? "")
      .sort()
      .join(","),
  );
  const isConnected = (port: string) => connectedPorts.split(",").includes(port);
  // Params flagged `inline` are edited directly on the card; the rest live in
  // the Inspector. `imageSource`/`psdTemplate` paths get a basename caption so
  // the card stays readable even with a long absolute path.
  const inlineParams = spec.params.filter((p) => p.inline);
  const templateWarn =
    spec.kind === "psdTemplate" ? psdTemplatePathWarning(String(d.params.path ?? "")) : null;

  return (
    <div className={`node ${selected ? "selected" : ""} status-${status} ${lod ? "lod" : ""}`}>
      <div className="node-header">
        <span className="node-title">{spec.title}</span>
        {spec.kind === "psdTemplate" ? <span className="node-tag">PSD</span> : null}
        <span className={`badge badge-${status}`} title={fmtDuration(d.durationMs)}>
          {status}
          {d.durationMs != null && (status === "succeeded" || status === "failed" || status === "cancelled") ? (
            <em className="badge-time"> {fmtDuration(d.durationMs)}</em>
          ) : null}
        </span>
      </div>
      {!lod && (status === "failed" || status === "cancelled") && d.error ? (
        <div className="node-error nodrag" title={d.error}>
          {d.error}
        </div>
      ) : null}

      {!lod && <div className="node-body">
        {inlineParams.map((p) => (
          <label key={p.key} className="inline-field">
            <span>{p.label}</span>
            <ParamField
              spec={p}
              value={d.params[p.key]}
              onChange={(v) => editing?.onParamChange(id, p.key, v)}
              compact
            />
            {p.control === "path" && d.params[p.key] ? (
              <small className="path">{basename(String(d.params[p.key]))}</small>
            ) : null}
          </label>
        ))}

        {spec.kind === "preview" &&
          (d.imagePath ? (
            <LazyThumb path={d.imagePath} />
          ) : (
            <div className="node-thumb placeholder">{t("node.noImage")}</div>
          ))}

        {spec.kind === "subjectMask" ? (
          <div className="subject-mask">
            {d.maskPath ? (
              <LazyThumb path={d.maskPath} />
            ) : (
              <div
                className="node-thumb placeholder click-select"
                title={t("node.clickSelectTitle")}
              >
                {isConnected("image") ? t("node.clickSelect") : t("node.connectImage")}
              </div>
            )}
            <div className="subject-mask-actions nodrag">
              <button
                type="button"
                title={t("node.autoTitle")}
                onClick={() => editing?.openPreview?.(id)}
              >
                {t("node.auto")}
              </button>
              <button
                type="button"
                className="primary"
                title={t("node.editMaskTitle")}
                onClick={() => editing?.openMaskEdit?.(id)}
              >
                {t("node.editMask")}
              </button>
              <button
                type="button"
                title={t("node.previewTitle")}
                onClick={() => editing?.openPreview?.(id)}
              >
                {t("node.preview")}
              </button>
            </div>
          </div>
        ) : null}

        {spec.kind === "crop" ? (
          <div className="subject-mask">
            {d.imagePath ? (
              <LazyThumb path={d.imagePath} />
            ) : (
              <div className="node-thumb placeholder" title={t("node.mediaCropTitle")}>
                {isConnected("image") ? t("crop.drawHint") : t("node.connectImage")}
              </div>
            )}
            <div className="subject-mask-actions nodrag">
              <button
                type="button"
                className="primary"
                title={t("crop.applyTitle")}
                onClick={() => editing?.openCropEdit?.(id)}
              >
                {t("crop.title")}
              </button>
            </div>
          </div>
        ) : null}

        {spec.kind === "imageSource" && d.params.path ? (
          <ImageSourceCard id={id} path={String(d.params.path)} />
        ) : null}

        {spec.kind === "psdTemplate" && templateWarn ? (
          <div className="node-warn nodrag" title={templateWarn}>
            ⚠ {templateWarn}
          </div>
        ) : null}

        {spec.kind === "save" ? (
          <div className="psd-conn">
            <span className={isConnected("image") ? "ok" : "warn"}>
              {t("node.connImage")} {isConnected("image") ? "✓" : "✕"}
            </span>
            <span className={isConnected("template") ? "ok" : "muted"}>
              {t("node.connTemplate")} {isConnected("template") ? "✓" : "—"}
            </span>
          </div>
        ) : null}

        {spec.kind === "psdExport" ? (
          <div className="psd-export">
            <div className="psd-conn">
              <span className={isConnected("image") ? "ok" : "warn"}>
                {t("node.connImage")} {isConnected("image") ? "✓" : "✕"}
              </span>
              <span className={isConnected("template") ? "ok" : "warn"}>
                {t("node.connTemplate")} {isConnected("template") ? "✓" : "✕"}
              </span>
            </div>
            {d.psdPreviewPath ? (
              <LazyThumb path={d.psdPreviewPath} />
            ) : (
              <div className="node-thumb placeholder">{t("node.noExport")}</div>
            )}
            {d.psdPath ? <PathRow label="psd" path={d.psdPath} /> : null}
            {d.psdPreviewPath ? <PathRow label="preview" path={d.psdPreviewPath} /> : null}
            {d.psdMetadataPath ? <PathRow label="meta" path={d.psdMetadataPath} /> : null}
            {d.placeholderKind || d.smartObjectMode ? (
              <small className="psd-meta">
                {d.placeholderKind ? `${t("node.metaPlaceholder")}: ${d.placeholderKind}` : ""}
                {d.placeholderKind && d.smartObjectMode ? " · " : ""}
                {d.smartObjectMode ? `${t("node.metaSmart")}: ${d.smartObjectMode}` : ""}
              </small>
            ) : null}
          </div>
        ) : null}
      </div>}

      {spec.inputs.map((p, i) => (
        <Handle
          key={`in-${p.id}`}
          id={p.id}
          type="target"
          position={Position.Left}
          className={`port port-${p.type}`}
          style={{ top: 44 + i * 22 }}
          title={`${p.label}: ${p.type}`}
        />
      ))}
      {spec.outputs.map((p, i) => (
        <Handle
          key={`out-${p.id}`}
          id={p.id}
          type="source"
          position={Position.Right}
          className={`port port-${p.type}`}
          style={{ top: 44 + i * 22 }}
          title={`${p.label}: ${p.type}`}
        />
      ))}
    </div>
  );
}

export const HgripeNode = memo(HgripeNodeImpl);
