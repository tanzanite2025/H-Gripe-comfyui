import { paletteGroups, type NodeSpec } from "../graph/nodeSpecs";

interface PaletteProps {
  /** Click-to-add (node is placed at a default spot on the canvas). */
  onAdd: (kind: string) => void;
}

const CATEGORY_LABEL: Record<NodeSpec["category"], string> = {
  input: "Inputs",
  generate: "Generate",
  utility: "Utility",
  output: "Outputs",
};

// MIME-ish key carried on drag so the canvas knows which node kind to create.
export const DND_NODE_KIND = "application/hgripe-node-kind";

// Left rail listing the available node kinds. Each item can be dragged onto the
// canvas (drop position is honoured) or clicked to add at a default location.
export function Palette({ onAdd }: PaletteProps) {
  return (
    <aside className="palette">
      <h2>Nodes</h2>
      {paletteGroups().map(({ category, specs }) => (
        <div key={category} className="palette-group">
          <h3>{CATEGORY_LABEL[category]}</h3>
          {specs.map((spec) => (
            <button
              key={spec.kind}
              className="palette-item"
              draggable
              onDragStart={(e) => {
                e.dataTransfer.setData(DND_NODE_KIND, spec.kind);
                e.dataTransfer.effectAllowed = "move";
              }}
              onClick={() => onAdd(spec.kind)}
              title={spec.description}
            >
              {spec.title}
            </button>
          ))}
        </div>
      ))}
      <p className="muted palette-hint">Drag onto the canvas, or click to add.</p>
    </aside>
  );
}
