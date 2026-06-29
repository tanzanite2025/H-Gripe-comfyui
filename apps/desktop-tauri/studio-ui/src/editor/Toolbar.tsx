import type { Dispatch, MutableRefObject, SetStateAction } from "react";
import type { Node } from "@xyflow/react";

import { NodeSearchBox } from "./NodeSearchBox";
import { baseName } from "./ProjectPanel";
import type { EdgeStyle } from "./FlowCanvas";
import type { ValidationIssue } from "../runtime/dag";
import { useT } from "../i18n";

export interface ToolbarProps {
  // Status
  issues: ValidationIssue[];
  isDesktop: boolean;
  currentFile: string | null;
  fileDirty: boolean;
  saved: boolean;
  message: string;

  // History
  canUndo: boolean;
  canRedo: boolean;
  onUndo: () => void;
  onRedo: () => void;

  // Language
  onToggleLang: () => void;

  // Panels
  showProject: boolean;
  setShowProject: Dispatch<SetStateAction<boolean>>;
  showSnapshots: boolean;
  setShowSnapshots: Dispatch<SetStateAction<boolean>>;
  showLog: boolean;
  setShowLog: Dispatch<SetStateAction<boolean>>;
  showHistory: boolean;
  setShowHistory: Dispatch<SetStateAction<boolean>>;
  snapshotCount: number;
  logCount: number;
  historyCount: number;

  // Node search
  nodes: Node[];
  onJumpToNode: (nodeId: string) => void;

  // File actions
  onNew: () => void;
  onOpen: () => void;
  onSave: () => void;
  onSaveAs: () => void;
  onReset: () => void;
  onClear: () => void;
  fileInputRef: MutableRefObject<HTMLInputElement | null>;
  onFilePicked: (file: File) => void;

  // Canvas options
  snapToGrid: boolean;
  setSnapToGrid: Dispatch<SetStateAction<boolean>>;
  onTidyLayout: () => void;
  edgeType: EdgeStyle;
  onChangeEdgeType: (style: EdgeStyle) => void;
  showMinimap: boolean;
  setShowMinimap: Dispatch<SetStateAction<boolean>>;

  // Run
  running: boolean;
  canCancel: boolean;
  onRun: () => void;
  onCancelRun: () => void;
  hasBatch: boolean;
  batchCount: number;
  onRunBatch: () => void;
}

/**
 * The editor's top toolbar: status indicators, undo/redo, language toggle,
 * panel toggles (project / snapshots / log / history), node search, file
 * actions, canvas options, and the run controls. Pure presentation — every
 * action and piece of state is supplied by `App`.
 */
export function Toolbar({
  issues,
  isDesktop,
  currentFile,
  fileDirty,
  saved,
  message,
  canUndo,
  canRedo,
  onUndo,
  onRedo,
  onToggleLang,
  showProject,
  setShowProject,
  showSnapshots,
  setShowSnapshots,
  showLog,
  setShowLog,
  showHistory,
  setShowHistory,
  snapshotCount,
  logCount,
  historyCount,
  nodes,
  onJumpToNode,
  onNew,
  onOpen,
  onSave,
  onSaveAs,
  onReset,
  onClear,
  fileInputRef,
  onFilePicked,
  snapToGrid,
  setSnapToGrid,
  onTidyLayout,
  edgeType,
  onChangeEdgeType,
  showMinimap,
  setShowMinimap,
  running,
  canCancel,
  onRun,
  onCancelRun,
  hasBatch,
  batchCount,
  onRunBatch,
}: ToolbarProps) {
  const t = useT();
  return (
    <header className="toolbar">
      <strong>H-Gripe Studio</strong>
      <span className="muted">{t("brand.subtitle")}</span>
      <div className="spacer" />
      {issues.length > 0 && (
        <span className="issues" title={issues.map((i) => i.message).join("\n")}>
          ⚠ {issues.length} {issues.length > 1 ? t("issues.many") : t("issues.one")}
        </span>
      )}
      {isDesktop && (
        <span className="muted current-file" title={currentFile ?? t("status.untitledTitle")}>
          {currentFile ? baseName(currentFile) : t("status.untitled")}
          {fileDirty ? " *" : ""}
        </span>
      )}
      <span className="muted autosave" title={t("status.autosaveTitle")}>
        {saved ? t("status.autosaved") : t("status.saving")}
      </span>
      <button onClick={onToggleLang} title={t("label.langTitle")} className="lang-toggle">
        {t("label.lang")}
      </button>
      <button onClick={onUndo} disabled={!canUndo} title={t("btn.undoTitle")}>
        {t("btn.undo")}
      </button>
      <button onClick={onRedo} disabled={!canRedo} title={t("btn.redoTitle")}>
        {t("btn.redo")}
      </button>
      {isDesktop && (
        <button onClick={() => setShowProject((s) => !s)} title={t("btn.projectTitle")}>
          {showProject ? t("btn.hideProject") : t("btn.project")}
        </button>
      )}
      <button onClick={() => setShowSnapshots((s) => !s)} title={t("btn.snapshotsTitle")}>
        {showSnapshots ? t("btn.hideSnapshots") : t("btn.snapshots")}
        {snapshotCount > 0 ? ` (${snapshotCount})` : ""}
      </button>
      <NodeSearchBox nodes={nodes} onJump={onJumpToNode} />
      {isDesktop && (
        <button onClick={onNew} title={t("btn.newTitle")}>
          {t("btn.new")}
        </button>
      )}
      <button onClick={onOpen} title={isDesktop ? t("btn.openTitle") : t("btn.loadTitle")}>
        {isDesktop ? t("btn.open") : t("btn.load")}
      </button>
      <button onClick={onSave} title={isDesktop ? t("btn.saveTitleDesktop") : t("btn.saveTitleWeb")}>
        {t("btn.save")}
      </button>
      {isDesktop && (
        <button onClick={onSaveAs} title={t("btn.saveAsTitle")}>
          {t("btn.saveAs")}
        </button>
      )}
      <button onClick={onReset}>{t("btn.reset")}</button>
      <button onClick={onClear}>{t("btn.clear")}</button>
      <label className="snap-toggle" title={t("label.snapTitle")}>
        <input type="checkbox" checked={snapToGrid} onChange={(e) => setSnapToGrid(e.target.checked)} />
        {t("label.snap")}
      </label>
      <button onClick={onTidyLayout} title={t("btn.tidyTitle")}>
        {t("btn.tidy")}
      </button>
      <label className="snap-toggle" title={t("label.edgesTitle")}>
        {t("label.edges")}
        <select value={edgeType} onChange={(e) => onChangeEdgeType(e.target.value as EdgeStyle)}>
          <option value="default">{t("label.edgesCurved")}</option>
          <option value="smoothstep">{t("label.edgesOrthogonal")}</option>
          <option value="smart">{t("label.edgesAvoid")}</option>
        </select>
      </label>
      <label className="snap-toggle" title={t("label.mapTitle")}>
        <input type="checkbox" checked={showMinimap} onChange={(e) => setShowMinimap(e.target.checked)} />
        {t("label.map")}
      </label>
      <button onClick={() => setShowLog((s) => !s)} title={t("btn.logTitle")}>
        {showLog ? t("btn.hideLog") : t("btn.log")}
        {logCount > 0 ? ` (${logCount})` : ""}
      </button>
      <button
        onClick={() => setShowHistory((s) => !s)}
        title="show past runs (persisted with the project)"
      >
        {showHistory ? "Hide history" : "History"}
        {historyCount > 0 ? ` (${historyCount})` : ""}
      </button>
      <button className="primary" onClick={onRun} disabled={running || issues.length > 0} title={t("btn.runTitle")}>
        {running ? t("btn.running") : t("btn.run")}
      </button>
      {canCancel && (
        <button onClick={onCancelRun} title={t("btn.cancelTitle")}>
          {t("btn.cancel")}
        </button>
      )}
      {hasBatch && (
        <button
          onClick={onRunBatch}
          disabled={running || issues.length > 0 || batchCount === 0}
          title={t("btn.runBatchTitle")}
        >
          {t("btn.run")} ×{batchCount}
        </button>
      )}
      <span className="muted">{message}</span>
      <input
        ref={fileInputRef}
        type="file"
        accept="application/json,.json"
        style={{ display: "none" }}
        onChange={(e) => {
          const f = e.target.files?.[0];
          if (f) onFilePicked(f);
          e.target.value = "";
        }}
      />
    </header>
  );
}
