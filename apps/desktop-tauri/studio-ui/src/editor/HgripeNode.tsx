import { memo } from "react";
import { Handle, Position, type NodeProps } from "@xyflow/react";
import { nodeSpec } from "../graph/nodeSpecs";
import type { NodeStatus } from "../runtime/dag";

export interface HgripeNodeData extends Record<string, unknown> {
  kind: string;
  params: Record<string, unknown>;
  status?: NodeStatus;
  /** Path of the most recent output image, if any (for the preview node). */
  imagePath?: string | null;
  /** Backend-generated thumbnail data URL / path for display. */
  thumbnail?: string | null;
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
        {spec.kind === "preview" &&
          (d.thumbnail ? (
            <img className="node-thumb" src={d.thumbnail} alt="preview" />
          ) : (
            <div className="node-thumb placeholder">no image</div>
          ))}
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
