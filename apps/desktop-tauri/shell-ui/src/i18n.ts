// Lightweight, dependency-free internationalisation for the shell (English /
// Simplified Chinese). Mirrors studio-ui/src/i18n.ts: a flat dictionary keyed by
// stable ids, a `t(key, vars)` lookup with `{name}` interpolation, localStorage
// persistence (default: browser language), and `applyI18n()` which translates
// static markup tagged with `data-i18n` (textContent), `data-i18n-html`
// (innerHTML, trusted dictionary strings only), `data-i18n-placeholder`, and
// `data-i18n-title`.

export type Lang = "en" | "zh";

const LANG_KEY = "hgripe.shell.lang.v1";

interface Message {
  en: string;
  zh: string;
}

// Translatable UI strings. Keys are stable ids; values hold both languages.
const messages: Record<string, Message> = {
  "brand.title": { en: "H-Gripe Desktop", zh: "H-Gripe 桌面端" },
  "brand.subtitle": { en: "API-first control shell", zh: "API 优先控制台" },

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
  "btn.refresh": { en: "Refresh", zh: "刷新" },
  "common.loading": { en: "Loading…", zh: "加载中…" },
  "common.loadingShort": { en: "loading…", zh: "加载中…" },
  "common.noEntries": { en: "no entries", zh: "无条目" },
  "common.error": { en: "error", zh: "错误" },
  "common.found": { en: "found", zh: "存在" },
  "common.missing": { en: "missing", zh: "缺失" },
  "creds.keyEnv": { en: "env:", zh: "环境变量:" },

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
    en: 'sent as <code>image_path</code> (used by <code>image.edit</code>)',
    zh: '作为 <code>image_path</code> 发送（用于 <code>image.edit</code>）',
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
  "profiles.heading": { en: "Provider Profiles", zh: "提供方档案" },
  "profiles.file": { en: "provider_profiles.json", zh: "provider_profiles.json" },
  "btn.validate": { en: "Validate", zh: "校验" },
  "btn.reload": { en: "Reload", zh: "重新加载" },
  "btn.save": { en: "Save", zh: "保存" },
  "status.saved": { en: "saved", zh: "已保存" },
  "toast.savedKind": { en: "{kind} saved", zh: "{kind} 已保存" },
  "validation.valid": { en: "valid", zh: "有效" },
  "validation.issues": { en: "{count} issue(s)", zh: "{count} 个问题" },
  "creds.keySet": { en: "set", zh: "已设置" },
  "creds.keyNone": { en: "none", zh: "无" },
  "field.provider": { en: "provider", zh: "提供方" },
  "field.model": { en: "model", zh: "模型" },
  "field.creds": { en: "creds", zh: "凭据" },
  "field.key": { en: "key", zh: "密钥" },
  "field.headers": { en: "headers", zh: "请求头" },
  "field.params": { en: "params", zh: "参数" },

  "run.heading": { en: "Run API Task", zh: "运行 API 任务" },
  "run.hint": {
    en: "Paste an <code>ApiTask</code> JSON payload and submit it to the broker.",
    zh: "粘贴一个 <code>ApiTask</code> JSON 负载并提交给 broker。",
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
  "btn.load": { en: "Load", zh: "加载" },
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
    en: "Scans a folder for PSD exports (<code>&lt;name&gt;.psd</code> with matching <code>_preview.png</code> / <code>_metadata.json</code>) produced by the <code>H-Gripe PSD Export</code> node.",
    zh: "扫描文件夹中的 PSD 导出（<code>&lt;name&gt;.psd</code> 及配套的 <code>_preview.png</code> / <code>_metadata.json</code>），由 <code>H-Gripe PSD Export</code> 节点生成。",
  },
  "psd.openPsd": { en: "Open PSD", zh: "打开 PSD" },
  "psd.openFolder": { en: "Open folder", zh: "打开文件夹" },
  "psd.smartObject": { en: "smart object", zh: "智能对象" },
  "psd.smartObjectNote": {
    en: "Generated image was written <em>inside</em> the template's smart object (editable in Photoshop).",
    zh: "生成的图像被写入模板的智能对象<em>内部</em>（可在 Photoshop 中编辑）。",
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

  "node.title": { en: "H-Gripe Node Editor", zh: "H-Gripe 节点编辑器" },
  "node.loading": { en: "Loading the visual workflow canvas…", zh: "正在加载可视化工作流画布…" },
  "node.buildMissing": { en: "Node Editor build not found", zh: "未找到节点编辑器的构建产物" },
  "node.buildHint": {
    en: 'Build it once with <code>npm --prefix apps/desktop-tauri/studio-ui ci &amp;&amp; npm --prefix apps/desktop-tauri/studio-ui run build</code>, then reopen this tab. (The Tauri CLI does this automatically; a plain <code>cargo run</code> does not.)',
    zh: '先用 <code>npm --prefix apps/desktop-tauri/studio-ui ci &amp;&amp; npm --prefix apps/desktop-tauri/studio-ui run build</code> 构建一次，再重新打开此标签页。（Tauri CLI 会自动构建；单独的 <code>cargo run</code> 不会。）',
  },

};

let lang: Lang = loadLang();

function loadLang(): Lang {
  try {
    const v = localStorage.getItem(LANG_KEY);
    if (v === "zh" || v === "en") return v;
  } catch {
    /* storage disabled */
  }
  if (typeof navigator !== "undefined" && navigator.language && navigator.language.toLowerCase().indexOf("zh") === 0) {
    return "zh";
  }
  return "en";
}

function saveLang(next: Lang): void {
  try {
    localStorage.setItem(LANG_KEY, next);
  } catch {
    /* best-effort */
  }
}

// Translate a key, interpolating `{name}` placeholders from `vars`. Falls back
// to the key itself when missing (matches studio-ui behaviour).
export function t(key: string, vars?: Record<string, string | number>): string {
  const entry = messages[key];
  let text = entry ? entry[lang] : key;
  if (vars) {
    text = text.replace(/\{(\w+)\}/g, (m, name: string) =>
      Object.prototype.hasOwnProperty.call(vars, name) ? String(vars[name]) : m
    );
  }
  return text;
}

// Translate the static markup under `root` (default: document). Elements opt in
// via data attributes; `data-i18n-html` values are trusted dictionary strings,
// so assigning innerHTML here is safe under the app's CSP.
export function applyI18n(root?: ParentNode): void {
  const scope = root ?? document;
  scope.querySelectorAll<HTMLElement>("[data-i18n]").forEach((node) => {
    node.textContent = t(node.getAttribute("data-i18n") as string);
  });
  scope.querySelectorAll<HTMLElement>("[data-i18n-html]").forEach((node) => {
    node.innerHTML = t(node.getAttribute("data-i18n-html") as string);
  });
  scope.querySelectorAll<HTMLElement>("[data-i18n-placeholder]").forEach((node) => {
    node.setAttribute("placeholder", t(node.getAttribute("data-i18n-placeholder") as string));
  });
  scope.querySelectorAll<HTMLElement>("[data-i18n-title]").forEach((node) => {
    node.setAttribute("title", t(node.getAttribute("data-i18n-title") as string));
  });
  document.documentElement.lang = lang === "zh" ? "zh-CN" : "en";
}

export function setLang(next: Lang): void {
  lang = next === "zh" ? "zh" : "en";
  saveLang(lang);
  applyI18n();
}

export function getLang(): Lang {
  return lang;
}
