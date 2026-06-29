import { useEffect, useRef } from "react";

// Shared persistence wiring for a project-scoped list store (snapshots, run
// history, ...). Each such store is a serialized array that lives in the active
// project folder on desktop (so it travels with the project) and falls back to
// browser localStorage otherwise. This hook owns the two effects that pattern
// requires, deduplicating what would otherwise be copied per store:
//   - load the selected folder's file when the folder changes
//   - persist the list whenever it changes, into the folder (else localStorage)
// A folder switch must not write the previous scope's data into the new folder,
// so the persist effect reads the target through a ref; and a `skip` flag
// prevents writing straight back right after a load populates state.

export interface ProjectScopedStore<T> {
  /** Sink folder (desktop + selected project), or null to use localStorage. */
  dir: string | null;
  /** Current list state and its setter. */
  state: T[];
  setState: (value: T[]) => void;
  /** Validate raw JSON (from disk or localStorage) into a list. */
  parse: (raw: string) => T[];
  /** Read the folder's store file (raw JSON text), or null off-desktop. */
  read: (dir: string) => Promise<string | null>;
  /** Persist the list into the folder (desktop only). */
  write: (dir: string, data: T[]) => Promise<void>;
  /** Persist the list to localStorage (no-folder fallback). */
  saveLocal: (data: T[]) => void;
  /** Short label used in error messages, e.g. "snapshots". */
  label: string;
  /** Surface load/save errors (e.g. a status message setter). */
  onError?: (message: string) => void;
}

export function useProjectScopedStore<T>({
  dir,
  state,
  setState,
  parse,
  read,
  write,
  saveLocal,
  label,
  onError,
}: ProjectScopedStore<T>): void {
  const dirRef = useRef(dir);
  dirRef.current = dir;
  const loadedDir = useRef<string | null>(null);
  const skipPersist = useRef(false);

  // Load the selected folder's store when it changes.
  useEffect(() => {
    if (!dir) {
      loadedDir.current = null;
      return;
    }
    if (loadedDir.current === dir) return;
    loadedDir.current = dir;
    let cancelled = false;
    void read(dir)
      .then((raw) => {
        if (cancelled || raw === null) return;
        skipPersist.current = true;
        setState(parse(raw));
      })
      .catch((err) => onError?.(`load ${label} failed: ${String(err)}`));
    return () => {
      cancelled = true;
    };
    // Only re-run when the sink folder changes; the other inputs are stable.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dir]);

  // Persist on every list change: into the folder on desktop, else localStorage.
  useEffect(() => {
    if (skipPersist.current) {
      skipPersist.current = false;
      return;
    }
    const target = dirRef.current;
    if (target) {
      void write(target, state).catch((err) => onError?.(`save ${label} failed: ${String(err)}`));
    } else {
      saveLocal(state);
    }
    // Only re-run when the list changes; the sink is read through a ref.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state]);
}
