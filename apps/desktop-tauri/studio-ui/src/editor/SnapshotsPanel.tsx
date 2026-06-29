import type { Snapshot } from "./snapshots";
import { isEmptyDiff, type GraphDiff } from "./snapshotdiff";
import { useT } from "../i18n";

function formatTaken(ms: number): string {
  try {
    return new Date(ms).toLocaleString();
  } catch {
    return "";
  }
}

/** A computed comparison of one snapshot against the current graph. */
export interface SnapshotDiffView {
  id: string;
  name: string;
  diff: GraphDiff;
}

export interface SnapshotsPanelProps {
  /** Saved snapshots, newest first. */
  snapshots: Snapshot[];
  /** Whether a snapshot is captured automatically before each run. */
  autoSnapshot: boolean;
  onToggleAutoSnapshot: (on: boolean) => void;
  onCapture: () => void;
  onRestore: (id: string) => void;
  onRename: (id: string) => void;
  onDelete: (id: string) => void;
  /** Compare the given snapshot against the current graph. */
  onDiff: (id: string) => void;
  /** Active comparison result (vs. the current graph), if any. */
  diff: SnapshotDiffView | null;
  onClearDiff: () => void;
  onClose: () => void;
}

function DiffSummary({ view, onClear }: { view: SnapshotDiffView; onClear: () => void }) {
  const t = useT();
  const { diff } = view;
  const empty = isEmptyDiff(diff);
  return (
    <div className="snapshot-diff">
      <div className="snapshot-diff-head">
        <span>
          {t("snap.diffVs")} <strong>{view.name}</strong>
        </span>
        <button onClick={onClear} title={t("snap.diffCloseTitle")}>
          ✕
        </button>
      </div>
      {empty ? (
        <p className="snapshot-diff-same">{t("snap.diffSame")}</p>
      ) : (
        <ul className="snapshot-diff-list">
          {diff.addedNodes.map((n) => (
            <li key={`an-${n.id}`} className="diff-add">+ node {n.kind} ({n.id})</li>
          ))}
          {diff.removedNodes.map((n) => (
            <li key={`rn-${n.id}`} className="diff-del">− node {n.kind} ({n.id})</li>
          ))}
          {diff.changedNodes.map((n) => (
            <li key={`cn-${n.id}`} className="diff-chg">
              ~ {n.kind} ({n.id}){" "}
              {n.kindChanged ? "kind changed" : `params: ${n.params.join(", ")}`}
            </li>
          ))}
          {diff.addedEdges.map((e) => (
            <li key={`ae-${e}`} className="diff-add">+ edge {e}</li>
          ))}
          {diff.removedEdges.map((e) => (
            <li key={`re-${e}`} className="diff-del">− edge {e}</li>
          ))}
        </ul>
      )}
    </div>
  );
}

/**
 * Left-rail snapshot history: capture the current graph under a name and
 * restore / rename / delete saved snapshots. Snapshots are kept in browser
 * localStorage (see snapshots.ts), independent of the on-disk workflow file.
 */
export function SnapshotsPanel({
  snapshots,
  autoSnapshot,
  onToggleAutoSnapshot,
  onCapture,
  onRestore,
  onRename,
  onDelete,
  onDiff,
  diff,
  onClearDiff,
  onClose,
}: SnapshotsPanelProps) {
  const t = useT();
  return (
    <aside className="project-panel snapshots-panel">
      <div className="project-head">
        <h2>{t("snap.heading")}</h2>
        <button className="project-new" onClick={onClose} title={t("snap.hideTitle")}>
          {t("snap.hide")}
        </button>
      </div>

      <button className="project-newfile" onClick={onCapture} title={t("snap.takeTitle")}>
        {t("snap.take")}
      </button>

      <label className="snapshot-auto" title={t("snap.autoTitle")}>
        <input
          type="checkbox"
          checked={autoSnapshot}
          onChange={(e) => onToggleAutoSnapshot(e.target.checked)}
        />
        {t("snap.auto")}
      </label>

      {diff ? <DiffSummary view={diff} onClear={onClearDiff} /> : null}

      <div className="project-list">
        {snapshots.length === 0 ? (
          <p className="project-empty">{t("snap.empty")}</p>
        ) : (
          snapshots.map((s) => (
            <div key={s.id} className="project-row">
              <button
                className="project-item"
                onClick={() => onRestore(s.id)}
                title={`restore "${s.name}"\n${formatTaken(s.t)}`}
              >
                <span className="project-item-name">{s.name}</span>
                <span className="project-item-meta">
                  {formatTaken(s.t)} · {s.graph.nodes.length}{" "}
                  {s.graph.nodes.length === 1 ? t("snap.nodeSuffix") : t("snap.nodesSuffix")}
                </span>
              </button>
              <div className="project-actions">
                <button
                  onClick={() => onDiff(s.id)}
                  title={t("snap.diffTitle")}
                  aria-label={`compare ${s.name}`}
                >
                  ⇄
                </button>
                <button onClick={() => onRename(s.id)} title={t("snap.renameTitle")} aria-label={`rename ${s.name}`}>
                  ✎
                </button>
                <button onClick={() => onDelete(s.id)} title={t("snap.deleteTitle")} aria-label={`delete ${s.name}`}>
                  ✕
                </button>
              </div>
            </div>
          ))
        )}
      </div>

      <p className="project-hint muted">{t("snap.hint")}</p>
    </aside>
  );
}
