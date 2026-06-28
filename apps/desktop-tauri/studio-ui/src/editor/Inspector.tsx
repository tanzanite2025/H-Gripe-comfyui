import { useState } from "react";
import type { Node } from "@xyflow/react";
import { nodeSpec } from "../graph/nodeSpecs";
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

  if (!node) {
    return (
      <aside className="inspector">
        <p className="muted">Select a node to edit its parameters.</p>
      </aside>
    );
  }

  const data = node.data as HgripeNodeData;

  // Group container: no ports/params, just a rename field.
  if (data.kind === "group") {
    return (
      <aside className="inspector">
        <h2>Group</h2>
        <p className="muted">A container frame. Drag nodes in/out; members move with it.</p>
        <label className="field">
          <span>Label</span>
          <input
            value={String(data.params.label ?? "")}
            onChange={(e) => onParamChange(node.id, "label", e.target.value)}
          />
        </label>
      </aside>
    );
  }

  const spec = nodeSpec(data.kind);

  return (
    <aside className="inspector">
      <h2>{spec.title}</h2>
      <p className="muted">{spec.description}</p>

      {spec.kind === "generate" && (
        <ProfilePicker
          onApply={(profile) => {
            if (profile.provider) onParamChange(node.id, "provider", profile.provider);
            if (profile.model) onParamChange(node.id, "model", profile.model);
            onParamChange(node.id, "credentials_ref", profile.credentials_ref ?? "");
          }}
        />
      )}

      {spec.params.map((p) => {
        const raw = data.params[p.key];
        const onChange = (v: unknown) => onParamChange(node.id, p.key, v);
        return (
          <label key={p.key} className="field">
            <span>{p.label}</span>
            <ParamField spec={p} value={raw} onChange={onChange} />
            {p.control === "path" && (
              <OutputPicker
                kind={spec.kind === "psdTemplate" ? "template" : "image"}
                onPick={(path) => onChange(path)}
              />
            )}
            {p.hint && <small className="hint">{p.hint}</small>}
          </label>
        );
      })}

      {data.imagePath && (
        <div className="field">
          <span>Output</span>
          <button
            type="button"
            className="inspector-img-btn"
            onClick={() => setViewerPath(data.imagePath ?? null)}
            title="View full size"
          >
            {data.thumbnail ? (
              <img className="inspector-img" src={data.thumbnail} alt="output" />
            ) : (
              <div className="inspector-img placeholder">View full size</div>
            )}
          </button>
          <code className="path">{data.imagePath}</code>
        </div>
      )}

      {viewerPath && <MediaViewer path={viewerPath} onClose={() => setViewerPath(null)} />}
    </aside>
  );
}
