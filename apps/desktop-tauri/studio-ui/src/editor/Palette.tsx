import { useContext, useEffect, useMemo, useRef, useState } from "react";
import { paletteGroups, type NodeSpec } from "../graph/nodeSpecs";
import { GROUP_ZH, localizeSpec } from "../graph/nodeSpecsI18n";
import { LangContext, useT, type MsgKey } from "../i18n";

interface PaletteProps {
  /** Click-to-add (node is placed at a default spot on the canvas). */
  onAdd: (kind: string) => void;
}

const CATEGORY_LABEL: Record<NodeSpec["category"], MsgKey> = {
  input: "palette.catInput",
  generate: "palette.catGenerate",
  control: "palette.catControl",
  utility: "palette.catUtility",
  output: "palette.catOutput",
};

// Local vs API badge shown on palette items so the two kinds of card are
// visually separated. Pure `graph` nodes carry no badge.
const EXECUTOR_BADGE: Partial<Record<NodeSpec["executor"], string>> = {
  local: "Local",
  api: "API",
  hybrid: "Local/API",
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
const GROUP_ITEM_ZH = { kind: "group", title: GROUP_ZH.title, description: GROUP_ZH.description };

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
  const lang = useContext(LangContext);
  const t = useT();

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
        .map(({ category, specs }) => ({
          category,
          specs: specs
            .map((s) => localizeSpec(s, lang))
            .filter((s) => matches(s, query)),
        }))
        .filter((g) => g.specs.length > 0),
    [query, lang],
  );
  const showGroupItem = matches(lang === "zh" ? GROUP_ITEM_ZH : GROUP_ITEM, query);
  const empty = groups.length === 0 && !showGroupItem;

  return (
    <aside className="palette">
      <h2>{t("palette.heading")}</h2>
      <input
        ref={inputRef}
        className="palette-search"
        type="search"
        placeholder={t("palette.searchPh")}
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
          <h3>{t(CATEGORY_LABEL[category])}</h3>
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
              <span className="palette-item-title">{spec.title}</span>
              {EXECUTOR_BADGE[spec.executor] && (
                <span className={`palette-badge palette-badge-${spec.executor}`}>
                  {EXECUTOR_BADGE[spec.executor]}
                </span>
              )}
            </button>
          ))}
        </div>
      ))}
      {showGroupItem && (
        <div className="palette-group">
          <h3>{t("palette.containers")}</h3>
          <button
            className="palette-item"
            draggable
            onDragStart={(e) => {
              e.dataTransfer.setData(DND_NODE_KIND, "group");
              e.dataTransfer.effectAllowed = "move";
            }}
            onClick={() => onAdd("group")}
            title={lang === "zh" ? GROUP_ZH.description : GROUP_ITEM.description}
          >
            {t("palette.group")}
          </button>
        </div>
      )}
      {empty ? (
        <p className="muted palette-hint">{t("palette.noMatch", { query })}</p>
      ) : (
        <p className="muted palette-hint">{t("palette.hint")}</p>
      )}
    </aside>
  );
}
