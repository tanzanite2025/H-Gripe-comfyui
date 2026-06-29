import type { Snapshot } from "./snapshots";

function formatTaken(ms: number): string {
  try {
    return new Date(ms).toLocaleString();
  } catch {
    return "";
  }
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
  onClose: () => void;
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
  onClose,
}: SnapshotsPanelProps) {
  return (
    <aside className="project-panel snapshots-panel">
      <div className="project-head">
        <h2>Snapshots</h2>
        <button className="project-new" onClick={onClose} title="hide the snapshots panel">
          Hide
        </button>
      </div>

      <button className="project-newfile" onClick={onCapture} title="save the current workflow as a named snapshot">
        + Take snapshot
      </button>

      <label className="snapshot-auto" title="capture a snapshot automatically before each run">
        <input
          type="checkbox"
          checked={autoSnapshot}
          onChange={(e) => onToggleAutoSnapshot(e.target.checked)}
        />
        Auto-snapshot before run
      </label>

      <div className="project-list">
        {snapshots.length === 0 ? (
          <p className="project-empty">no snapshots yet</p>
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
                  {formatTaken(s.t)} · {s.graph.nodes.length} node{s.graph.nodes.length === 1 ? "" : "s"}
                </span>
              </button>
              <div className="project-actions">
                <button onClick={() => onRename(s.id)} title="rename" aria-label={`rename ${s.name}`}>
                  ✎
                </button>
                <button onClick={() => onDelete(s.id)} title="delete" aria-label={`delete ${s.name}`}>
                  ✕
                </button>
              </div>
            </div>
          ))
        )}
      </div>

      <p className="project-hint muted">
        Snapshots are stored in this browser and capture the whole graph. Restoring replaces the
        current workflow.
      </p>
    </aside>
  );
}
