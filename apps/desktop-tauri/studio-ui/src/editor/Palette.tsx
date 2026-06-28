import { useEffect, useMemo, useRef, useState } from "react";
import { paletteGroups, type NodeSpec } from "../graph/nodeSpecs";

interface PaletteProps {
  /** Click-to-add (node is placed at a default spot on the canvas). */
  onAdd: (kind: string) => void;
}

const CATEGORY_LABEL: Record<NodeSpec["category"], string> = {
  input: "Inputs",
  generate: "Generate",
  control: "Control",
  utility: "Utility",
  output: "Outputs",
};

// MIME-ish key carried on drag so the canvas knows which node kind to create.
export const DND_NODE_KIND = "application/hgripe-node-kind";

// The Group container is not in NODE_SPECS' palette groups; describe it here so
// it participates in search alongside the catalogue.
const GROUP_ITEM = {
  kind: "group",
  title: "Group",
  description: "A resizable frame. Drag nodes inside to group them; members move together.",
};

export function matches(spec: { title: string; kind: string; description: string }, q: string): boolean {
  if (!q) return true;
  const hay = `${spec.title} ${spec.kind} ${spec.description}`.toLowerCase();
  return q
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((term) => hay.includes(term));
}

// Left rail listing the available node kinds. A search box filters by title /
// kind / description; each item can be dragged onto the canvas (drop position
// is honoured) or clicked to add at a default location.
export function Palette({ onAdd }: PaletteProps) {
  const [query, setQuery] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  // "/" focuses search (unless already typing in a field), so you can add a
  // node without reaching for the mouse.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "/") return;
      const t = e.target as HTMLElement | null;
      const editable =
        !!t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable);
      if (editable) return;
      e.preventDefault();
      inputRef.current?.focus();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const groups = useMemo(
    () =>
      paletteGroups()
        .map(({ category, specs }) => ({ category, specs: specs.filter((s) => matches(s, query)) }))
        .filter((g) => g.specs.length > 0),
    [query],
  );
  const showGroupItem = matches(GROUP_ITEM, query);
  const empty = groups.length === 0 && !showGroupItem;

  return (
    <aside className="palette">
      <h2>Nodes</h2>
      <input
        ref={inputRef}
        className="palette-search"
        type="search"
        placeholder="Search nodes…  ( / )"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            setQuery("");
            inputRef.current?.blur();
          }
        }}
      />
      {groups.map(({ category, specs }) => (
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
      {showGroupItem && (
        <div className="palette-group">
          <h3>Containers</h3>
          <button
            className="palette-item"
            draggable
            onDragStart={(e) => {
              e.dataTransfer.setData(DND_NODE_KIND, "group");
              e.dataTransfer.effectAllowed = "move";
            }}
            onClick={() => onAdd("group")}
            title={GROUP_ITEM.description}
          >
            Group
          </button>
        </div>
      )}
      {empty ? (
        <p className="muted palette-hint">No nodes match “{query}”.</p>
      ) : (
        <p className="muted palette-hint">Drag onto the canvas, or click to add.</p>
      )}
    </aside>
  );
}
