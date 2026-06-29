import { useEffect, useRef } from "react";

// Global keyboard shortcuts for the editor. Split into two listeners matching
// the original behavior:
//   - edit shortcuts (undo/redo/select-all/copy/paste) are suppressed while a
//     form field is focused so native text editing keeps working there;
//   - file/run shortcuts (save / save as / open / new / run) fire even inside a
//     field so a quick Ctrl+S always saves.
// Handlers are read through a ref so the listeners mount once and always call
// the latest closures, instead of re-subscribing on every render.

export interface KeyboardShortcutHandlers {
  undo: () => void;
  redo: () => void;
  selectAll: () => void;
  copySelection: () => void;
  pasteClipboard: () => void;
  save: () => void;
  saveAs: () => void;
  open: () => void;
  newWorkflow: () => void;
  run: () => void;
  /** Whether a run may be started (not already running and no blocking issues). */
  canRun: boolean;
}

function isEditableTarget(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  return (
    !!el &&
    (el.tagName === "INPUT" ||
      el.tagName === "TEXTAREA" ||
      el.tagName === "SELECT" ||
      el.isContentEditable)
  );
}

export function useKeyboardShortcuts(handlers: KeyboardShortcutHandlers): void {
  const ref = useRef(handlers);
  ref.current = handlers;

  // Undo/redo + copy/paste + select-all (skipped while editing a field).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey)) return;
      if (isEditableTarget(e.target)) return;
      const h = ref.current;
      switch (e.key.toLowerCase()) {
        case "z":
          e.preventDefault();
          if (e.shiftKey) h.redo();
          else h.undo();
          break;
        case "y":
          e.preventDefault();
          h.redo();
          break;
        case "a":
          e.preventDefault();
          h.selectAll();
          break;
        case "c":
          h.copySelection();
          break;
        case "v":
          e.preventDefault();
          h.pasteClipboard();
          break;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // File + run shortcuts (fire even while editing a field).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.altKey) return;
      const h = ref.current;
      switch (e.key.toLowerCase()) {
        case "s":
          e.preventDefault();
          if (e.shiftKey) h.saveAs();
          else h.save();
          break;
        case "o":
          if (e.shiftKey) return;
          e.preventDefault();
          h.open();
          break;
        case "n":
          if (e.shiftKey) return;
          e.preventDefault();
          h.newWorkflow();
          break;
        case "enter":
          e.preventDefault();
          if (h.canRun) h.run();
          break;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}
