import type { Node } from "@xyflow/react";
import { nodeSpec } from "../graph/nodeSpecs";
import { ParamField } from "./ParamField";
import type { HgripeNodeData } from "./HgripeNode";

interface InspectorProps {
  node: Node | null;
  onParamChange: (nodeId: string, key: string, value: unknown) => void;
}

// Right-side panel. Full-resolution media preview belongs here (not inside the
// node card), so the canvas stays light and previews never blow up node size.
export function Inspector({ node, onParamChange }: InspectorProps) {
  if (!node) {
    return (
      <aside className="inspector">
        <p className="muted">Select a node to edit its parameters.</p>
      </aside>
    );
  }

  const data = node.data as HgripeNodeData;
  const spec = nodeSpec(data.kind);

  return (
    <aside className="inspector">
      <h2>{spec.title}</h2>
      <p className="muted">{spec.description}</p>

      {spec.params.map((p) => {
        const raw = data.params[p.key];
        const onChange = (v: unknown) => onParamChange(node.id, p.key, v);
        return (
          <label key={p.key} className="field">
            <span>{p.label}</span>
            <ParamField spec={p} value={raw} onChange={onChange} />
            {p.hint && <small className="hint">{p.hint}</small>}
          </label>
        );
      })}

      {data.imagePath && (
        <div className="field">
          <span>Output</span>
          {data.thumbnail ? (
            <img className="inspector-img" src={data.thumbnail} alt="output" />
          ) : null}
          <code className="path">{data.imagePath}</code>
        </div>
      )}
    </aside>
  );
}
