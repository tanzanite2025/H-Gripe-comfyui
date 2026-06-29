import { useEffect, useRef, useState } from "react";
import type { Node } from "@xyflow/react";

import { searchNodes } from "./nodesearch";

export interface NodeSearchBoxProps {
  nodes: Node[];
  /** Select + center the view on a node. */
  onJump: (nodeId: string) => void;
}

/**
 * Toolbar node finder: filters the graph's nodes by id / kind / title and jumps
 * to the chosen one. Enter jumps to the first match; Escape clears.
 */
export function NodeSearchBox({ nodes, onJump }: NodeSearchBoxProps) {
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState(false);
  const boxRef = useRef<HTMLDivElement | null>(null);

  const matches = searchNodes(nodes, query);

  // Close the results when clicking elsewhere.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (boxRef.current && !boxRef.current.contains(e.target as HTMLElement | null)) setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [open]);

  const jump = (id: string) => {
    onJump(id);
    setOpen(false);
  };

  return (
    <div className="node-search" ref={boxRef}>
      <input
        type="search"
        placeholder="Find node…"
        value={query}
        title="find a node by id, type or title"
        onChange={(e) => {
          setQuery(e.target.value);
          setOpen(true);
        }}
        onFocus={() => setOpen(true)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && matches.length > 0) jump(matches[0].id);
          else if (e.key === "Escape") {
            setQuery("");
            setOpen(false);
          }
        }}
      />
      {open && query.trim() !== "" && (
        <ul className="node-search-results">
          {matches.length === 0 ? (
            <li className="node-search-empty">no matches</li>
          ) : (
            matches.map((m) => (
              <li key={m.id}>
                <button onClick={() => jump(m.id)} title={`go to ${m.id}`}>
                  <span className="node-search-title">{m.title}</span>
                  <span className="node-search-id">{m.id}</span>
                </button>
              </li>
            ))
          )}
        </ul>
      )}
    </div>
  );
}
