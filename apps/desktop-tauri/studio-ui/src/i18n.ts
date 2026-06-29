// Lightweight UI internationalisation (English / Simplified Chinese).
//
// A flat string dictionary keyed by a stable id, plus a tiny React context so
// child components can translate without prop-drilling. The active language is
// persisted to localStorage and defaults to the browser language on first run.
// Kept dependency-free and renderer-agnostic so the lookup helpers are unit
// testable.

import { createContext, useContext } from "react";

export type Lang = "en" | "zh";

const LANG_KEY = "hgripe.studio.lang.v1";

/** Translatable UI strings. Keys are stable ids; values hold both languages. */
export const messages = {
  "brand.subtitle": { en: "node-graph (React Flow)", zh: "节点图 (React Flow)" },
  "status.autosaved": { en: "● autosaved", zh: "● 已自动保存" },
  "status.saving": { en: "○ saving…", zh: "○ 保存中…" },
  "status.untitled": { en: "untitled", zh: "未命名" },
  "status.untitledTitle": { en: "untitled (not yet saved to a file)", zh: "未命名（尚未保存到文件）" },
  "status.autosaveTitle": {
    en: "this workflow is autosaved to the workspace and restored on next open",
    zh: "该工作流会自动保存到工作区，下次打开时恢复",
  },

  "btn.undo": { en: "Undo", zh: "撤销" },
  "btn.undoTitle": { en: "Undo (Ctrl+Z)", zh: "撤销 (Ctrl+Z)" },
  "btn.redo": { en: "Redo", zh: "重做" },
  "btn.redoTitle": { en: "Redo (Ctrl+Shift+Z)", zh: "重做 (Ctrl+Shift+Z)" },
  "btn.project": { en: "Project", zh: "项目" },
  "btn.hideProject": { en: "Hide Project", zh: "隐藏项目" },
  "btn.projectTitle": { en: "toggle the project folder browser", zh: "切换项目文件夹浏览器" },
  "btn.snapshots": { en: "Snapshots", zh: "快照" },
  "btn.hideSnapshots": { en: "Hide Snapshots", zh: "隐藏快照" },
  "btn.snapshotsTitle": {
    en: "toggle the snapshots panel (named versions of the workflow)",
    zh: "切换快照面板（工作流的命名版本）",
  },
  "btn.new": { en: "New", zh: "新建" },
  "btn.newTitle": { en: "start a new, empty workflow (Ctrl/Cmd+N)", zh: "新建空工作流 (Ctrl/Cmd+N)" },
  "btn.open": { en: "Open…", zh: "打开…" },
  "btn.load": { en: "Load", zh: "载入" },
  "btn.openTitle": { en: "open a workflow file (Ctrl/Cmd+O)", zh: "打开工作流文件 (Ctrl/Cmd+O)" },
  "btn.loadTitle": { en: "load workflow.json (Ctrl/Cmd+O)", zh: "载入 workflow.json (Ctrl/Cmd+O)" },
  "btn.save": { en: "Save", zh: "保存" },
  "btn.saveTitleDesktop": {
    en: "save to the current file (Save As… if none) — Ctrl/Cmd+S",
    zh: "保存到当前文件（无则另存为）— Ctrl/Cmd+S",
  },
  "btn.saveTitleWeb": { en: "download workflow.json (Ctrl/Cmd+S)", zh: "下载 workflow.json (Ctrl/Cmd+S)" },
  "btn.saveAs": { en: "Save As…", zh: "另存为…" },
  "btn.saveAsTitle": {
    en: "save to a new file via the native dialog (Ctrl/Cmd+Shift+S)",
    zh: "通过原生对话框另存为新文件 (Ctrl/Cmd+Shift+S)",
  },
  "btn.reset": { en: "Reset", zh: "重置" },
  "btn.clear": { en: "Clear", zh: "清空" },
  "btn.tidy": { en: "Tidy", zh: "整理" },
  "btn.tidyTitle": { en: "arrange nodes on a grid by DAG depth", zh: "按 DAG 深度在网格上排列节点" },
  "btn.log": { en: "Log", zh: "日志" },
  "btn.hideLog": { en: "Hide Log", zh: "隐藏日志" },
  "btn.logTitle": {
    en: "toggle the run log (per-node status, timing and errors)",
    zh: "切换运行日志（各节点状态、耗时与错误）",
  },
  "btn.run": { en: "Run", zh: "运行" },
  "btn.running": { en: "Running…", zh: "运行中…" },
  "btn.runTitle": { en: "execute the graph (Ctrl/Cmd+Enter)", zh: "执行图 (Ctrl/Cmd+Enter)" },
  "btn.cancel": { en: "Cancel", zh: "取消" },
  "btn.cancelTitle": {
    en: "request cancellation before the next node starts",
    zh: "在下一个节点开始前请求取消",
  },
  "btn.runBatchTitle": { en: "run the graph once per batch item", zh: "对每个批处理项各运行一次" },

  "label.snap": { en: "Snap", zh: "吸附" },
  "label.snapTitle": {
    en: "snap node positions to a 16px grid while dragging",
    zh: "拖动时将节点位置吸附到 16px 网格",
  },
  "label.edges": { en: "Edges", zh: "连线" },
  "label.edgesTitle": { en: "edge rendering style", zh: "连线渲染样式" },
  "label.edgesCurved": { en: "curved", zh: "曲线" },
  "label.edgesOrthogonal": { en: "orthogonal", zh: "直角" },
  "label.edgesAvoid": { en: "avoid", zh: "避让" },
  "label.map": { en: "Map", zh: "缩略图" },
  "label.mapTitle": { en: "toggle the minimap", zh: "切换小地图" },
  "label.lang": { en: "中文", zh: "EN" },
  "label.langTitle": { en: "switch to Chinese", zh: "Switch to English" },

  "issues.one": { en: "issue", zh: "个问题" },
  "issues.many": { en: "issues", zh: "个问题" },

  "search.placeholder": { en: "Find node…", zh: "查找节点…" },
  "search.title": { en: "find a node by id, type or title", zh: "按 id、类型或标题查找节点" },
  "search.noMatch": { en: "no matching nodes", zh: "没有匹配的节点" },

  "snap.heading": { en: "Snapshots", zh: "快照" },
  "snap.hide": { en: "Hide", zh: "隐藏" },
  "snap.hideTitle": { en: "hide the snapshots panel", zh: "隐藏快照面板" },
  "snap.take": { en: "+ Take snapshot", zh: "+ 拍摄快照" },
  "snap.takeTitle": { en: "save the current workflow as a named snapshot", zh: "将当前工作流保存为命名快照" },
  "snap.auto": { en: "Auto-snapshot before run", zh: "运行前自动快照" },
  "snap.autoTitle": { en: "capture a snapshot automatically before each run", zh: "每次运行前自动拍摄快照" },
  "snap.empty": { en: "no snapshots yet", zh: "暂无快照" },
  "snap.diffTitle": { en: "compare with the current graph", zh: "与当前图对比" },
  "snap.renameTitle": { en: "rename", zh: "重命名" },
  "snap.deleteTitle": { en: "delete", zh: "删除" },
  "snap.diffVs": { en: "vs", zh: "对比" },
  "snap.diffSame": { en: "identical to the current graph", zh: "与当前图相同" },
  "snap.diffCloseTitle": { en: "close comparison", zh: "关闭对比" },
  "snap.nodesSuffix": { en: "nodes", zh: "个节点" },
  "snap.nodeSuffix": { en: "node", zh: "个节点" },
  "snap.hint": {
    en: "Snapshots are stored in this browser and capture the whole graph. Restoring replaces the current workflow.",
    zh: "快照存储在本浏览器中并捕获整个图。恢复将替换当前工作流。",
  },
} satisfies Record<string, { en: string; zh: string }>;

export type MsgKey = keyof typeof messages;

/** Translate a key into the given language, falling back to the key itself. */
export function translate(lang: Lang, key: MsgKey): string {
  const m = messages[key];
  return m ? m[lang] : key;
}

/** Read the persisted language, defaulting to the browser language (zh / en). */
export function loadLang(): Lang {
  try {
    const v = localStorage.getItem(LANG_KEY);
    if (v === "zh" || v === "en") return v;
  } catch {
    /* storage disabled */
  }
  if (typeof navigator !== "undefined" && navigator.language?.toLowerCase().startsWith("zh")) {
    return "zh";
  }
  return "en";
}

/** Persist the active language (best-effort). */
export function saveLang(lang: Lang): void {
  try {
    localStorage.setItem(LANG_KEY, lang);
  } catch {
    /* best-effort */
  }
}

/** Active language, provided at the app root and read by child panels. */
export const LangContext = createContext<Lang>("en");

/** Hook returning a `t(key)` translator bound to the current language. */
export function useT(): (key: MsgKey) => string {
  const lang = useContext(LangContext);
  return (key: MsgKey) => translate(lang, key);
}
