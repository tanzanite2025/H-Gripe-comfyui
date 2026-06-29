import { memo, useEffect, useRef, useState } from "react";
import { Handle, Position, useStore, type NodeProps } from "@xyflow/react";
import { nodeSpec } from "../graph/nodeSpecs";
import { isLodActive } from "./lod";
import type { NodeStatus } from "../runtime/dag";
import { generateThumbnail } from "../bridge/tauri";
import { ParamField } from "./ParamField";
import { useNodeEditing } from "./editingContext";

export interface HgripeNodeData extends Record<string, unknown> {
  kind: string;
  params: Record<string, unknown>;
  status?: NodeStatus;
  /** Last run's wall-clock duration in ms (executed nodes only). */
  durationMs?: number;
  /** Last run's error message, when `status === "failed"`. */
  error?: string | null;
  /** Path of the most recent output image, if any (for the preview node). */
  imagePath?: string | null;
  /** Backend-generated thumbnail data URL / path for display. */
  thumbnail?: string | null;
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
          .then((t) => {
            if (!cancelled) setSrc(t.data_url || null);
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
        <div className="node-thumb placeholder">loading…</div>
      )}
    </div>
  );
}

// Custom node is memoized (React Flow perf guidance): node drags must not
// re-render every node. The node shows only a compact summary + a thumbnail;
// full params live in the Inspector and full-res media is opened there.
function HgripeNodeImpl({ id, data, selected }: NodeProps) {
  const d = data as HgripeNodeData;
  const spec = nodeSpec(d.kind);
  const status = d.status ?? "idle";
  const editing = useNodeEditing();
  // Collapse to a title-only card when zoomed far out. A boolean selector means
  // nodes only re-render when crossing the threshold, not on every zoom tick.
  const lod = useStore((s) => isLodActive(s.transform[2]));
  // Params flagged `inline` are edited directly on the card; the rest live in
  // the Inspector. `imageSource`/`psdTemplate` paths get a basename caption so
  // the card stays readable even with a long absolute path.
  const inlineParams = spec.params.filter((p) => p.inline);

  return (
    <div className={`node ${selected ? "selected" : ""} status-${status} ${lod ? "lod" : ""}`}>
      <div className="node-header">
        <span className="node-title">{spec.title}</span>
        <span className={`badge badge-${status}`} title={fmtDuration(d.durationMs)}>
          {status}
          {d.durationMs != null && (status === "succeeded" || status === "failed") ? (
            <em className="badge-time"> {fmtDuration(d.durationMs)}</em>
          ) : null}
        </span>
      </div>
      {!lod && status === "failed" && d.error ? (
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
            <div className="node-thumb placeholder">no image</div>
          ))}
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
