// Simplified-Chinese overlay for the Mask-Edit tool registry (maskTools.ts).
//
// MASK_TOOLS stays the English source of truth (the contract table in
// docs/cards/subject-mask-matte.md). This map holds only the zh label/hint per
// tool id; `localizeTool` blends them at render time, falling back to English
// for any missing entry. A coverage test asserts every tool has an entry.

import type { Lang } from "../i18n";
import type { MaskTool } from "./maskTools";

export const MASK_TOOL_ZH: Record<string, { label: string; hint: string }> = {
  brush: { label: "画笔", hint: "把蒙版涂进来。" },
  eraser: { label: "橡皮", hint: "把蒙版擦掉。" },
  point: {
    label: "点 (SAM 2)",
    hint: "左键点击主体以包含，右键点击以排除——SAM 2 根据你的点进行分割（auto 模式）。",
  },
  wand: { label: "魔棒", hint: "按颜色相似度漫水填充一个区域（wand_tolerance）。" },
  rect: { label: "矩形", hint: "框选添加一个矩形。" },
  ellipse: { label: "椭圆", hint: "框选添加一个椭圆。" },
  invert: { label: "反相", hint: "反相整个蒙版。" },
  fill_holes: { label: "填洞", hint: "封闭内部孔洞。" },
  smooth: { label: "平滑", hint: "形态学开/闭运算。" },
  grow: { label: "扩张", hint: "将蒙版膨胀 N 像素。" },
  shrink: { label: "收缩", hint: "将蒙版腐蚀 N 像素。" },
  feather: { label: "羽化", hint: "对蒙版边缘做高斯羽化。" },
  matting: {
    label: "抠像",
    hint: "在 头发 / 绒毛 / 玻璃 上涂出三分图未知带——抠像器会将其解算为软 alpha。",
  },
  pen: { label: "钢笔", hint: "Phase 3 —— 贝塞尔路径，栅格化 + 布尔合并。" },
  lasso: { label: "套索", hint: "Phase 3 —— 自由手绘路径选择。" },
};

/** Return the tool's `label` / `hint` translated into `lang` (English fallback). */
export function localizeTool(tool: MaskTool, lang: Lang): { label: string; hint: string } {
  if (lang !== "zh") return { label: tool.label, hint: tool.hint };
  const tr = MASK_TOOL_ZH[tool.id];
  return { label: tr?.label ?? tool.label, hint: tr?.hint ?? tool.hint };
}
