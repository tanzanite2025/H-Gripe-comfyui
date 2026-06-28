import { memo, useEffect, useRef, useState } from "react";
import { Handle, Position, type NodeProps } from "@xyflow/react";
import { nodeSpec } from "../graph/nodeSpecs";
import type { NodeStatus } from "../runtime/dag";
import { generateThumbnail } from "../bridge/tauri";

export interface HgripeNodeData extends Record<string, unknown> {
  kind: string;
  params: Record<string, unknown>;
  status?: NodeStatus;
  /** Path of the most recent output image, if any (for the preview node). */
  imagePath?: string | null;
  /** Backend-generated thumbnail data URL / path for display. */
  thumbnail?: string | null;
}

function basename(p: string): string {
  const parts = p.split(/[/\\]/);
  return parts[parts.length - 1] || p;
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
function HgripeNodeImpl({ data, selected }: NodeProps) {
  const d = data as HgripeNodeData;
  const spec = nodeSpec(d.kind);
  const status = d.status ?? "idle";

  return (
    <div className={`node ${selected ? "selected" : ""} status-${status}`}>
      <div className="node-header">
        <span className="node-title">{spec.title}</span>
        <span className={`badge badge-${status}`}>{status}</span>
      </div>

      <div className="node-body">
        {spec.kind === "prompt" && (
          <div className="node-preview-text">
            {String(d.params.text ?? "") || <em>empty prompt</em>}
          </div>
        )}
        {(spec.kind === "imageSource" || spec.kind === "psdTemplate") && (
          <div className="node-meta">
            {d.params.path ? basename(String(d.params.path)) : <em>no path set</em>}
          </div>
        )}
        {spec.kind === "number" && (
          <div className="node-meta">{String(d.params.value ?? 0)}</div>
        )}
        {spec.kind === "preview" &&
          (d.imagePath ? (
            <LazyThumb path={d.imagePath} />
          ) : (
            <div className="node-thumb placeholder">no image</div>
          ))}
        {spec.kind === "save" && (
          <div className="node-meta">{String(d.params.filename ?? "output.png")}</div>
        )}
        {spec.kind === "generate" && (
          <div className="node-meta">
            {String(d.params.operation ?? "")} · {String(d.params.provider ?? "")}
          </div>
        )}
      </div>

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
