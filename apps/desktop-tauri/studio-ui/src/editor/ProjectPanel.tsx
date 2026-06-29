import type { StudioWorkflowFile } from "../bridge/tauri";

/** Last path segment of a `/`- or `\`-separated path. */
export function baseName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

function formatModified(ms?: number | null): string {
  if (!ms) return "";
  try {
    return new Date(ms).toLocaleString();
  } catch {
    return "";
  }
}

export interface ProjectPanelProps {
  /** Active project folder, or null when none is chosen. */
  projectDir: string | null;
  /** Workflow files discovered in the project folder (newest first). */
  files: StudioWorkflowFile[];
  /** Recently opened workflow paths (newest first), across folders. */
  recentFiles: string[];
  /** Path of the workflow currently loaded in the editor, if any. */
  currentFile: string | null;
  /** True while a folder scan / open is in flight. */
  busy?: boolean;
  onPickFolder: () => void;
  onRefresh: () => void;
  onOpenFile: (path: string) => void;
  onNew: () => void;
}

/**
 * Left-rail project browser: choose a project folder, list its workflow files,
 * and reopen recent workflows. Pairs with the toolbar's explicit Open / Save /
 * Save As actions; this panel is the "project folder" surface.
 */
export function ProjectPanel({
  projectDir,
  files,
  recentFiles,
  currentFile,
  busy,
  onPickFolder,
  onRefresh,
  onOpenFile,
  onNew,
}: ProjectPanelProps) {
  return (
    <aside className="project-panel">
      <div className="project-head">
        <h2>Project</h2>
        <button className="project-new" onClick={onNew} title="start a new, empty workflow">
          New
        </button>
      </div>

      <div className="project-folder">
        <button onClick={onPickFolder} title="choose the folder your workflows live in">
          {projectDir ? "Change Folder…" : "Open Folder…"}
        </button>
        {projectDir && (
          <button
            className="project-refresh"
            onClick={onRefresh}
            disabled={busy}
            title="rescan the project folder"
          >
            ⟳
          </button>
        )}
      </div>

      {projectDir ? (
        <>
          <div className="project-path" title={projectDir}>
            {projectDir}
          </div>
          <div className="project-list">
            {files.length === 0 ? (
              <p className="project-empty">{busy ? "scanning…" : "no workflows here yet"}</p>
            ) : (
              files.map((f) => (
                <button
                  key={f.path}
                  className={`project-item${f.path === currentFile ? " active" : ""}`}
                  onClick={() => onOpenFile(f.path)}
                  title={`${f.path}\n${formatModified(f.modified_ms)}`}
                >
                  <span className="project-item-name">{f.name}</span>
                  <span className="project-item-meta">{formatModified(f.modified_ms)}</span>
                </button>
              ))
            )}
          </div>
        </>
      ) : (
        <p className="project-hint muted">
          Choose a project folder to browse and open its workflow files.
        </p>
      )}

      {recentFiles.length > 0 && (
        <div className="project-recent">
          <h3>Recent</h3>
          {recentFiles.map((path) => (
            <button
              key={path}
              className={`project-item${path === currentFile ? " active" : ""}`}
              onClick={() => onOpenFile(path)}
              title={path}
            >
              <span className="project-item-name">{baseName(path)}</span>
            </button>
          ))}
        </div>
      )}
    </aside>
  );
}
