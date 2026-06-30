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
  "brand.subtitle": { en: "API-first control shell", zh: "API 优先控制台" },
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

  // ---- desktop shell (Dashboard / PSD Studio / Credentials / Profiles / Run /
  // History / PSD tabs), merged in from the former dependency-free shell-ui. ----
  "brand.title": { en: "H-Gripe Desktop", zh: "H-Gripe 桌面端" },

  "tab.dashboard": { en: "Dashboard", zh: "仪表盘" },
  "tab.studio": { en: "PSD Studio", zh: "PSD 工作室" },
  "tab.credentials": { en: "Credentials", zh: "凭据" },
  "tab.profiles": { en: "Profiles", zh: "配置档案" },
  "tab.run": { en: "Run Task", zh: "运行任务" },
  "tab.history": { en: "History", zh: "历史" },
  "tab.psd": { en: "PSD", zh: "PSD" },
  "tab.nodeEditor": { en: "Node Editor", zh: "节点编辑器" },

  "lang.toggle": { en: "中文", zh: "EN" },
  "lang.toggleTitle": { en: "switch to Chinese", zh: "切换到英文" },

  "dashboard.runtime": { en: "Runtime", zh: "运行环境" },
  "dashboard.doctor": { en: "Doctor", zh: "诊断" },
  "dashboard.engines": { en: "Engines", zh: "引擎" },
  "dashboard.enginesDesc": {
    en: "Opt-in ML detectors/backends. The CPU baseline is always available; learned engines need their dependency + weight.",
    zh: "可选 ML 检测器/后端。CPU 基线始终可用；学习型引擎需要对应依赖与权重。",
  },
  "dashboard.engineAvailable": { en: "available", zh: "可用" },
  "dashboard.engineUnavailable": { en: "unavailable", zh: "不可用" },
  "dashboard.compute": { en: "Compute", zh: "计算设备" },
  "dashboard.computeDesc": {
    en: "What accelerator the opt-in GPU engines would run on. With no CUDA device they fall back to CPU.",
    zh: "可选 GPU 引擎将运行在哪种加速器上。无 CUDA 设备时回退到 CPU。",
  },
  "dashboard.computeCuda": { en: "CUDA", zh: "CUDA" },
  "dashboard.computeCpuOnly": { en: "CPU only", zh: "仅 CPU" },
  "dashboard.computeProviders": { en: "ONNX Runtime providers", zh: "ONNX Runtime 提供者" },
  "dashboard.computeNotInstalled": { en: "not installed", zh: "未安装" },
  "dashboard.weightCached": { en: "weight cached", zh: "权重已缓存" },
  "dashboard.weightMissing": { en: "weight not downloaded", zh: "权重未下载" },
  "btn.refresh": { en: "Refresh", zh: "刷新" },
  "btn.validate": { en: "Validate", zh: "校验" },
  "btn.reload": { en: "Reload", zh: "重新加载" },

  "common.loading": { en: "Loading…", zh: "加载中…" },
  "common.loadingShort": { en: "loading…", zh: "加载中…" },
  "common.noEntries": { en: "no entries", zh: "无条目" },
  "common.error": { en: "error", zh: "错误" },
  "common.found": { en: "found", zh: "存在" },
  "common.missing": { en: "missing", zh: "缺失" },

  "studio.heading": { en: "PSD Studio", zh: "PSD 工作室" },
  "studio.hint": {
    en: "Compose a production job — pick a provider profile, add a prompt / reference image / PSD template, and generate.",
    zh: "组装一个生产任务——选择一个提供方档案，填写提示词 / 参考图 / PSD 模板，然后生成。",
  },
  "studio.providerProfile": { en: "Provider profile", zh: "提供方档案" },
  "studio.optionNone": { en: "— none (use provider below) —", zh: "— 无（使用下方提供方）—" },
  "studio.provider": { en: "Provider", zh: "提供方" },
  "studio.providerPh": { en: "mock / openai_compatible / custom_http / replicate", zh: "mock / openai_compatible / custom_http / replicate" },
  "studio.operation": { en: "Operation", zh: "操作" },
  "studio.outputType": { en: "Output type", zh: "输出类型" },
  "studio.prompt": { en: "Prompt", zh: "提示词" },
  "studio.promptPh": { en: "Describe what to generate…", zh: "描述要生成的内容…" },
  "studio.psdTemplate": { en: "PSD template (path)", zh: "PSD 模板（路径）" },
  "studio.psdTemplatePh": { en: "path to .psd template (optional)", zh: ".psd 模板路径（可选）" },
  "studio.pickFromPsd": { en: "Pick from PSD outputs", zh: "从 PSD 输出中选取" },
  "studio.reference": { en: "Reference image (path)", zh: "参考图（路径）" },
  "studio.referencePh": { en: "path to a reference image (optional)", zh: "参考图路径（可选）" },
  "studio.referenceHint": {
    en: "sent as image_path (used by image.edit)",
    zh: "作为 image_path 发送（用于 image.edit）",
  },
  "studio.params": { en: "Params (JSON)", zh: "参数 (JSON)" },
  "studio.previewTask": { en: "Preview task JSON", zh: "预览任务 JSON" },
  "studio.generate": { en: "Generate", zh: "生成" },
  "studio.task": { en: "Task", zh: "任务" },
  "studio.result": { en: "Result", zh: "结果" },
  "studio.taskReady": { en: "task ready", zh: "任务就绪" },
  "studio.generating": { en: "generating…", zh: "生成中…" },
  "studio.outputN": { en: "output {n}", zh: "输出 {n}" },
  "studio.open": { en: "Open", zh: "打开" },
  "studio.preview": { en: "Preview", zh: "预览" },
  "studio.openedOutput": { en: "opened output", zh: "已打开输出" },
  "studio.noPsdInOutput": { en: "no PSD files in output dir", zh: "输出目录中没有 PSD 文件" },
  "studio.pickedPsd": { en: "picked {name}.psd ({count} found)", zh: "已选取 {name}.psd（共 {count} 个）" },

  "creds.heading": { en: "Credentials", zh: "凭据" },
  "creds.file": { en: "credentials.json", zh: "credentials.json" },
  "creds.keyEnv": { en: "env:", zh: "环境变量:" },
  "creds.keySet": { en: "set", zh: "已设置" },
  "creds.keyNone": { en: "none", zh: "无" },
  "profiles.heading": { en: "Provider Profiles", zh: "提供方档案" },
  "profiles.file": { en: "provider_profiles.json", zh: "provider_profiles.json" },

  "status.saved": { en: "saved", zh: "已保存" },
  "toast.savedKind": { en: "{kind} saved", zh: "{kind} 已保存" },
  "validation.valid": { en: "valid", zh: "有效" },
  "validation.issues": { en: "{count} issue(s)", zh: "{count} 个问题" },

  "field.provider": { en: "provider", zh: "提供方" },
  "field.model": { en: "model", zh: "模型" },
  "field.creds": { en: "creds", zh: "凭据" },
  "field.key": { en: "key", zh: "密钥" },
  "field.headers": { en: "headers", zh: "请求头" },
  "field.params": { en: "params", zh: "参数" },

  "run.heading": { en: "Run API Task", zh: "运行 API 任务" },
  "run.hint": {
    en: "Paste an ApiTask JSON payload and submit it to the broker.",
    zh: "粘贴一个 ApiTask JSON 负载并提交给 broker。",
  },
  "run.insertTemplate": { en: "Insert mock template", zh: "插入 mock 模板" },
  "run.runTask": { en: "Run task", zh: "运行任务" },
  "run.running": { en: "running…", zh: "运行中…" },

  "history.heading": { en: "Task History", zh: "任务历史" },
  "history.providerFilter": { en: "provider filter", zh: "按提供方过滤" },
  "history.anyStatus": { en: "any status", zh: "任意状态" },
  "history.statusSucceeded": { en: "succeeded", zh: "成功" },
  "history.statusFailed": { en: "failed", zh: "失败" },
  "history.statusCached": { en: "cached", zh: "已缓存" },
  "history.statusCancelled": { en: "cancelled", zh: "已取消" },
  "history.colTime": { en: "Time", zh: "时间" },
  "history.colProvider": { en: "Provider", zh: "提供方" },
  "history.colOperation": { en: "Operation", zh: "操作" },
  "history.colStatus": { en: "Status", zh: "状态" },
  "history.colFiles": { en: "Files", zh: "文件" },
  "history.detail": { en: "Detail", zh: "详情" },
  "history.cleanup": { en: "Cleanup", zh: "清理" },
  "history.view": { en: "view", zh: "查看" },
  "history.rerun": { en: "rerun", zh: "重跑" },
  "history.noRecords": { en: "no records", zh: "无记录" },
  "history.rerunning": { en: "rerunning {id}", zh: "正在重跑 {id}" },
  "history.rerunDone": { en: "rerun {status}", zh: "重跑 {status}" },
  "cleanup.keepLatest": { en: "keep latest", zh: "保留最新" },
  "cleanup.deleteFiles": { en: "delete output files", zh: "删除输出文件" },
  "cleanup.preview": { en: "Preview", zh: "预览" },
  "cleanup.apply": { en: "Apply", zh: "应用" },
  "cleanup.applied": { en: "cleanup applied", zh: "清理已应用" },

  "psd.heading": { en: "PSD Outputs", zh: "PSD 输出" },
  "psd.dirPh": { en: "output directory", zh: "输出目录" },
  "psd.useOutput": { en: "Use output dir", zh: "使用输出目录" },
  "psd.scanHint": {
    en: "Scans a folder for PSD exports (<name>.psd with matching _preview.png / _metadata.json) produced by the H-Gripe PSD Export node.",
    zh: "扫描文件夹中的 PSD 导出（<name>.psd 及配套的 _preview.png / _metadata.json），由 H-Gripe PSD Export 节点生成。",
  },
  "psd.openPsd": { en: "Open PSD", zh: "打开 PSD" },
  "psd.openFolder": { en: "Open folder", zh: "打开文件夹" },
  "psd.smartObject": { en: "smart object", zh: "智能对象" },
  "psd.smartObjectNote": {
    en: "Generated image was written inside the template's smart object (editable in Photoshop).",
    zh: "生成的图像被写入模板的智能对象内部（可在 Photoshop 中编辑）。",
  },
  "psd.metadata": { en: "metadata.json", zh: "metadata.json" },
  "psd.enterDir": { en: "enter an output directory", zh: "请输入输出目录" },
  "psd.noFiles": { en: "no PSD files found", zh: "未找到 PSD 文件" },
  "psd.loadingPreview": { en: "loading preview…", zh: "预览加载中…" },
  "psd.noMetadata": { en: "(no metadata.json)", zh: "(无 metadata.json)" },
  "psd.openedFolder": { en: "opened folder", zh: "已打开文件夹" },
  "psd.openedPsd": { en: "opened PSD", zh: "已打开 PSD" },
  "psd.preview": { en: "preview", zh: "原始预览" },
  "psd.tagPreview": { en: "preview", zh: "预览" },
  "psd.tagMetadata": { en: "metadata", zh: "元数据" },

  // ---- node cards (HgripeNode) — hardcoded card chrome around the localized
  // NODE_SPECS strings (titles / params / ports live in nodeSpecsI18n.ts). ----
  "node.noImage": { en: "no image", zh: "暂无图像" },
  "node.noExport": { en: "no export yet", zh: "尚未导出" },
  "node.copied": { en: "copied!", zh: "已复制！" },
  "node.copyHint": { en: "click to copy: {path}", zh: "点击复制：{path}" },
  "node.clickSelect": { en: "click-to-select", zh: "点选" },
  "node.connectImage": { en: "connect an image", zh: "请连接图像" },
  "node.clickSelectTitle": {
    en: "Click-to-select runs the magic wand on the connected image",
    zh: "点选会对连接的图像运行魔棒",
  },
  "node.auto": { en: "Auto", zh: "自动" },
  "node.autoTitle": {
    en: "Auto-detect the subject (Phase 2 models; Phase 1 seeds an empty mask)",
    zh: "自动检测主体（Phase 2 模型；Phase 1 生成空蒙版）",
  },
  "node.editMask": { en: "Edit Mask", zh: "编辑蒙版" },
  "node.editMaskTitle": {
    en: "Open the mask editor (brush / wand / morphology)",
    zh: "打开蒙版编辑器（画笔 / 魔棒 / 形态学）",
  },
  "node.preview": { en: "Preview", zh: "预览" },
  "node.previewTitle": {
    en: "Preview the current mask / cutout (review gate)",
    zh: "预览当前蒙版 / 抠像（审阅关卡）",
  },
  "canvas.dropImages": { en: "added {n} image card(s)", zh: "已添加 {n} 张图片卡" },
  "canvas.dropVideos": { en: "added {n} video card(s)", zh: "已添加 {n} 张视频卡" },
  "canvas.dropMedia": {
    en: "added {images} image + {videos} video card(s)",
    zh: "已添加 {images} 张图片卡、{videos} 张视频卡",
  },
  "canvas.dropUnsupported": {
    en: "unsupported file type — drop an image or video",
    zh: "不支持的文件类型 — 请拖入图片或视频",
  },
  "video.probeFailed": { en: "could not read video", zh: "无法读取视频" },
  "node.mediaEditMask": { en: "Edit (Mask)", zh: "编辑（蒙版）" },
  "node.mediaEditMaskTitle": {
    en: "Create a bound mask-edit node from this image and open its editor",
    zh: "基于此图新建一个绑定的蒙版编辑节点并打开编辑器",
  },
  "node.mediaCrop": { en: "Crop", zh: "裁剪" },
  "node.mediaCropSoon": { en: "Crop — coming soon", zh: "裁剪 — 即将推出" },
  "node.mediaCropTitle": {
    en: "Create a bound crop node from this image and draw the box manually",
    zh: "基于此图新建一个绑定的裁剪节点，手动框选裁剪框",
  },
  "node.cropAuto": { en: "Crop to subject (auto)", zh: "裁剪到主体（自动）" },
  "crop.title": { en: "Crop", zh: "裁剪" },
  "crop.modeManual": { en: "Manual box", zh: "手动框" },
  "crop.modeAuto": { en: "Auto (to subject)", zh: "自动（到主体）" },
  "crop.modeManualTitle": { en: "Drag a crop box on the image", zh: "在图像上拖出裁剪框" },
  "crop.modeAutoTitle": {
    en: "Crop to the detected subject on run",
    zh: "运行时裁剪到检测出的主体",
  },
  "crop.aspect": { en: "Aspect", zh: "宽高比" },
  "crop.aspectFree": { en: "Free", zh: "自由" },
  "crop.margin": { en: "Subject margin %", zh: "主体边距 %" },
  "crop.reset": { en: "Reset box", zh: "重置框" },
  "crop.autoHint": {
    en: "The subject box is computed by the backend when the node runs.",
    zh: "主体框在节点运行时由后端计算。",
  },
  "crop.boxLabel": { en: "Box", zh: "框" },
  "crop.apply": { en: "Apply", zh: "应用" },
  "crop.applyTitle": { en: "Apply the crop to the node and show the result", zh: "将裁剪应用到节点并显示结果" },
  "crop.closeTitle": { en: "Close without applying (Esc)", zh: "不应用直接关闭（Esc）" },
  "crop.drawHint": {
    en: "Drag to draw a crop box; drag inside to move, corners to resize.",
    zh: "拖动绘制裁剪框；框内拖动移动，拖角缩放。",
  },
  "node.connImage": { en: "image", zh: "图像" },
  "node.connTemplate": { en: "template", zh: "模板" },
  "node.metaPlaceholder": { en: "placeholder", zh: "占位符" },
  "node.metaSmart": { en: "smart", zh: "智能对象" },

  // ---- inspector (right-side panel) ----
  "inspector.selectNode": {
    en: "Select a node to edit its parameters.",
    zh: "选择一个节点以编辑其参数。",
  },
  "inspector.group": { en: "Group", zh: "分组" },
  "inspector.groupDesc": {
    en: "A container frame. Drag nodes in/out; members move with it.",
    zh: "容器框。将节点拖入/拖出；成员随之移动。",
  },
  "inspector.label": { en: "Label", zh: "标签" },
  "inspector.engineUnavailable": {
    en: "This engine can't run on this machine — falls back to the CPU baseline.",
    zh: "此引擎在本机无法运行——将回落到 CPU 基线。",
  },
  "inspector.engineGpu": {
    en: "Runs on GPU (CUDA device detected).",
    zh: "将在 GPU 上运行(检测到 CUDA 设备)。",
  },
  "inspector.engineCpuFallback": {
    en: "No CUDA device — this engine runs on CPU (slower).",
    zh: "无 CUDA 设备——此引擎将在 CPU 上运行(较慢)。",
  },
  "inspector.output": { en: "Output", zh: "输出" },
  "inspector.viewFull": { en: "View full size", zh: "查看原图" },

  // ---- palette (left node catalogue rail) ----
  "palette.heading": { en: "Nodes", zh: "节点" },
  "palette.searchPh": { en: "Search nodes…  ( / )", zh: "搜索节点…  ( / )" },
  "palette.catInput": { en: "Inputs", zh: "输入" },
  "palette.catGenerate": { en: "Generate", zh: "生成" },
  "palette.catControl": { en: "Control", zh: "控制" },
  "palette.catUtility": { en: "Utility", zh: "工具" },
  "palette.catOutput": { en: "Outputs", zh: "输出" },
  "palette.containers": { en: "Containers", zh: "容器" },
  "palette.group": { en: "Group", zh: "分组" },
  "palette.noMatch": { en: "No nodes match “{query}”.", zh: "没有匹配「{query}」的节点。" },
  "palette.hint": { en: "Drag onto the canvas, or click to add.", zh: "拖到画布上，或点击添加。" },

  // ---- Mask-Edit modal (MaskEditModal) — tool labels/hints live in maskToolsI18n.ts ----
  "mask.titleSubject": { en: "Subject Mask / Matte", zh: "主体蒙版 / 抠像" },
  "mask.titleDefault": { en: "Mask editor", zh: "蒙版编辑器" },
  "mask.editor": { en: "mask editor", zh: "蒙版编辑器" },
  "mask.undo": { en: "Undo", zh: "撤销" },
  "mask.undoTitle": { en: "Undo (Ctrl+Z)", zh: "撤销（Ctrl+Z）" },
  "mask.redo": { en: "Redo", zh: "重做" },
  "mask.redoTitle": { en: "Redo (Ctrl+Y)", zh: "重做（Ctrl+Y）" },
  "mask.clear": { en: "Clear", zh: "清除" },
  "mask.clearTitle": { en: "Discard all edits", zh: "丢弃全部编辑" },
  "mask.showImage": { en: "Show image", zh: "显示图像" },
  "mask.maskOnly": { en: "Mask only", zh: "仅蒙版" },
  "mask.togglePreviewTitle": { en: "Toggle transparency preview", zh: "切换透明度预览" },
  "mask.apply": { en: "Apply", zh: "应用" },
  "mask.applyTitle": { en: "Apply edits to the node", zh: "将编辑应用到节点" },
  "mask.closeTitle": { en: "Close without applying (Esc)", zh: "不应用直接关闭（Esc）" },
  "mask.comingSoon": { en: "coming soon", zh: "即将推出" },
  "mask.soon": { en: "soon", zh: "即将" },
  "mask.brushSize": { en: "Brush size", zh: "笔刷大小" },
  "mask.amount": { en: "Amount (px)", zh: "数量（px）" },
  "mask.wandTolerance": { en: "Wand tolerance", zh: "魔棒容差" },
  "mask.queuedOps": { en: "Queued ops ({count})", zh: "排队操作（{count}）" },
  "mask.opsEmpty": { en: "none — paint or pick a tool", zh: "无 —— 涂抹或选择工具" },
  "mask.mattingBand": { en: "Matting band ({count})", zh: "抠像带（{count}）" },
  "mask.matteEmpty": {
    en: "none — pick Matting and paint over hair / fur / glass",
    zh: "无 —— 选择 抠像 并在 头发 / 绒毛 / 玻璃 上涂抹",
  },
  "mask.bandRadius": { en: "band r{radius}", zh: "带 r{radius}" },
  "mask.samPoints": { en: "SAM 2 points ({count})", zh: "SAM 2 点（{count}）" },
  "mask.pointsEmpty": {
    en: "none — pick Point (SAM 2) and click the subject",
    zh: "无 —— 选择 点 (SAM 2) 并点击主体",
  },
  "mask.notePrefix": { en: "Edits ({count}) are recorded as ", zh: "编辑（{count}）记录为 " },
  "mask.noteSuffix": {
    en: " and applied by the backend on run. Point (SAM 2) prompts route auto modes to the SAM 2 segmenter — left-click includes, right-click excludes; Matting paints the trimap unknown band (resolved to soft alpha by ViTMatte / the builtin guided filter); pen/lasso are planned (greyed).",
    zh: "，在运行时由后端应用。点 (SAM 2) 提示会把 auto 模式路由到 SAM 2 分割器——左键包含、右键排除；抠像 在三分图未知带上涂抹（由 ViTMatte / 内置引导滤波解算为软 alpha）；钢笔/套索 为计划中（置灰）。",
  },
} satisfies Record<string, { en: string; zh: string }>;

export type MsgKey = keyof typeof messages;

/**
 * Translate a key into the given language, falling back to the key itself.
 * Interpolates `{name}` placeholders from `vars` (matches the former shell-ui
 * `t(key, vars)` behaviour).
 */
export function translate(
  lang: Lang,
  key: MsgKey,
  vars?: Record<string, string | number>,
): string {
  const m = messages[key];
  let text = m ? m[lang] : (key as string);
  if (vars) {
    text = text.replace(/\{(\w+)\}/g, (match, name: string) =>
      Object.prototype.hasOwnProperty.call(vars, name) ? String(vars[name]) : match,
    );
  }
  return text;
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

/** Hook returning a `t(key, vars?)` translator bound to the current language. */
export function useT(): (key: MsgKey, vars?: Record<string, string | number>) => string {
  const lang = useContext(LangContext);
  return (key: MsgKey, vars?: Record<string, string | number>) => translate(lang, key, vars);
}
