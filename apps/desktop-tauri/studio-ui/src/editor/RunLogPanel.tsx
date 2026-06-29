import { useEffect, useRef } from "react";

import { formatTime, type RunLogEntry } from "./runlog";

export interface RunLogProps {
  entries: RunLogEntry[];
  onClear: () => void;
  onClose: () => void;
}

/** Streaming run log shown beneath the canvas; auto-scrolls to the newest line. */
export function RunLog({ entries, onClear, onClose }: RunLogProps) {
  const bodyRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const el = bodyRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [entries]);

  return (
    <section className="run-log" aria-label="run log">
      <div className="run-log-head">
        <h2>Run log</h2>
        <span className="muted">{entries.length}</span>
        <div className="spacer" />
        <button onClick={onClear} disabled={entries.length === 0} title="clear the log">
          Clear
        </button>
        <button onClick={onClose} title="hide the run log">
          Hide
        </button>
      </div>
      <div className="run-log-body" ref={bodyRef}>
        {entries.length === 0 ? (
          <p className="run-log-empty">No runs yet — press Run to execute the graph.</p>
        ) : (
          entries.map((e) => (
            <div key={e.id} className={`run-log-line level-${e.level}`}>
              <span className="run-log-time">{formatTime(e.t)}</span>
              {e.node && <span className="run-log-node">{e.node}</span>}
              <span className="run-log-msg">{e.message}</span>
            </div>
          ))
        )}
      </div>
    </section>
  );
}
