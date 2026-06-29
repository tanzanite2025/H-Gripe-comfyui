import { useState } from "react";

import { formatTime } from "./runlog";
import { runDurationMs, summarizeRun, type RunRecord } from "./runhistory";

function formatStarted(ms: number): string {
  try {
    return new Date(ms).toLocaleString();
  } catch {
    return "";
  }
}

export interface RunHistoryPanelProps {
  /** Past runs, newest first. */
  history: RunRecord[];
  onClear: () => void;
  onClose: () => void;
  /** Select/focus a node in the editor when its log line is clicked. */
  onSelectNode: (nodeId: string) => void;
}

/**
 * Left-rail run history: a list of past Run/Batch executions with their
 * outcome and timing; expanding a run shows the log lines it produced. Records
 * are persisted into the project folder on desktop (else localStorage), so they
 * survive a refresh, unlike the live in-memory run log.
 */
export function RunHistoryPanel({ history, onClear, onClose, onSelectNode }: RunHistoryPanelProps) {
  const [openId, setOpenId] = useState<string | null>(null);

  return (
    <aside className="project-panel runhistory-panel">
      <div className="project-head">
        <h2>Run history</h2>
        <div className="spacer" />
        <button onClick={onClear} disabled={history.length === 0} title="clear run history">
          Clear
        </button>
        <button className="project-new" onClick={onClose} title="hide run history">
          Hide
        </button>
      </div>

      <div className="project-list">
        {history.length === 0 ? (
          <p className="project-empty">No runs yet — past runs are recorded here.</p>
        ) : (
          history.map((r) => {
            const open = openId === r.id;
            return (
              <div key={r.id} className="project-row runhistory-row">
                <button
                  className="project-item"
                  onClick={() => setOpenId(open ? null : r.id)}
                  title={`${formatStarted(r.startedAt)} · ${(runDurationMs(r) / 1000).toFixed(1)}s`}
                >
                  <span className={`project-item-name runhistory-outcome-${r.outcome}`}>
                    {open ? "▾ " : "▸ "}
                    {summarizeRun(r)}
                  </span>
                  <span className="project-item-meta">
                    {formatStarted(r.startedAt)} · {r.backend}
                  </span>
                </button>
                {open ? (
                  <div className="runhistory-log">
                    {r.entries.length === 0 ? (
                      <p className="run-log-empty">No log lines for this run.</p>
                    ) : (
                      r.entries.map((e) => (
                        <div key={e.id} className={`run-log-line level-${e.level}`}>
                          <span className="run-log-time">{formatTime(e.t)}</span>
                          {e.node ? (
                            <button
                              className="run-log-node run-log-node-link"
                              onClick={() => onSelectNode(e.node as string)}
                              title={`select node ${e.node}`}
                            >
                              {e.node}
                            </button>
                          ) : null}
                          <span className="run-log-msg">{e.message}</span>
                        </div>
                      ))
                    )}
                  </div>
                ) : null}
              </div>
            );
          })
        )}
      </div>
    </aside>
  );
}
