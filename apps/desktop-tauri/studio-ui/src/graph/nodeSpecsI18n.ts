// Simplified-Chinese overlay for the node catalogue (NODE_SPECS).
//
// NODE_SPECS stays the English source of truth (runtime/validation/tests read
// it directly, and English is the fallback). This file holds *only* the zh
// translations of the human-readable card strings — node title/description,
// each param's label/hint, and each port's label — keyed by node kind. Option
// values are intentionally not translated: they are technical enum tokens used
// as the stored param value (e.g. "image.generate", "hybrid").
//
// `localizeSpec` clones a spec with the zh strings applied; any string missing
// a translation falls back to its English original. A coverage test
// (nodeSpecsI18n.test.ts) walks NODE_SPECS and asserts every translatable
// string has a zh entry, so new nodes/params cannot silently ship English-only.

import type { Lang } from "../i18n";
import { type NodeSpec } from "./nodeSpecs";

export interface NodeSpecZh {
  title: string;
  description: string;
  /** Param key -> translated label / hint (hint only where the English param has one). */
  params?: Record<string, { label?: string; hint?: string }>;
  /** Port id -> translated label (inputs and outputs share the id namespace). */
  ports?: Record<string, string>;
}

const OUTPUT_DIR_HINT = "留空则使用已配置的输出目录";

/** The Group container is described in Palette.tsx, not NODE_SPECS. */
export const GROUP_ZH = {
  title: "分组",
  description: "可调整大小的框。将节点拖入即可分组；成员一起移动。",
};

export const NODE_ZH: Record<string, NodeSpecZh> = {
  prompt: {
    title: "提示词",
    description: "送入生成节点的文本提示词。",
    params: { text: { label: "提示词" } },
    ports: { text: "文本" },
  },
  promptOptimize: {
    title: "提示词优化",
    description:
      "初始文本节点。输入提示词后可选择优化——`local` 应用无模型的清理/增强预设，`api` 通过 LLM 提供方档案（本地服务器或云端）重写。输出（优化后的）提示词文本。",
    params: {
      text: { label: "提示词", hint: "初始提示词（连接的 `text` 输入会覆盖它）" },
      mode: { label: "优化", hint: "off = 直通 · local = 规则化 · api = 经档案走 LLM" },
      preset: { label: "本地预设", hint: "`local` 模式使用：去重 + 追加增强标签" },
      provider: { label: "提供方", hint: "`api` 模式使用（选择档案时自动设置）" },
      model: { label: "模型", hint: "`api` 模式使用" },
      instruction: { label: "指令", hint: "`api` 模式使用（作为系统提示发送）" },
      credentials_ref: { label: "凭据", hint: "`api` 模式使用（选择档案时自动设置）" },
      temperature: { label: "温度", hint: "`api` 模式使用（可选）：采样随机性，留空则用提供方默认值" },
      max_tokens: { label: "最大 token 数", hint: "`api` 模式使用（可选）：限制优化后提示词长度" },
      seed: { label: "种子", hint: "`api` 模式使用（可选）：固定以获得可复现输出" },
    },
    ports: { text: "文本" },
  },
  batch: {
    title: "批处理",
    description:
      "遍历一组文本项（每行一个）。普通运行只发出第一项；用「Run ×N」可为每一项各跑一次。",
    params: { items: { label: "项（每行一个）", hint: "每行一个 提示词 / 值" } },
    ports: { item: "项" },
  },
  imageSource: {
    title: "图像源",
    description: "磁盘上的图像文件，用作参考图 / 输入图。",
    params: { path: { label: "图像路径", hint: "图像文件的绝对路径" } },
    ports: { image: "图像" },
  },
  videoSource: {
    title: "视频源",
    description: "磁盘上的视频文件；显示海报帧并将路径透传给下游。",
    params: {
      path: { label: "视频路径", hint: "视频文件的绝对路径" },
      poster_timestamp: { label: "海报时间（秒）", hint: "海报帧的时间点（秒）" },
    },
    ports: { video: "视频" },
  },
  psdTemplate: {
    title: "PSD 模板",
    description: "贯穿到导出的 .psd 模板路径。",
    params: { path: { label: "模板路径", hint: ".psd 模板的绝对路径" } },
    ports: { template: "模板" },
  },
  number: {
    title: "数值",
    description: "送入其它节点的数值（种子、数量……）。",
    params: { value: { label: "值" } },
    ports: { value: "值" },
  },
  generate: {
    title: "生成",
    description: "通过 H-Gripe broker 运行一次图像生成操作。",
    params: {
      provider: { label: "提供方" },
      operation: { label: "操作" },
      model: { label: "模型" },
      size: { label: "尺寸" },
      steps: { label: "步数" },
      seed: { label: "种子", hint: "被连接的 seed 输入覆盖" },
      credentials_ref: { label: "凭据", hint: "选择档案时自动设置" },
    },
    ports: { prompt: "提示词", reference: "参考图", seed: "种子", image: "图像" },
  },
  compare: {
    title: "比较",
    description:
      "比较两个值并输出 1（真）或 0（假）。两侧都能解析为数字时按数值比较，否则按字符串比较。将 `result` 接入 If 的 `cond`。",
    params: { op: { label: "运算符" } },
    ports: { a: "a", b: "b", result: "结果" },
  },
  logic: {
    title: "逻辑",
    description:
      "对输入的真值做布尔运算，输出 1（真）或 0（假）。`not` 只使用 `a`。将 `result` 接入 If 的 `cond`。",
    params: { op: { label: "运算符" } },
    ports: { a: "a", b: "b", result: "结果" },
  },
  if: {
    title: "If 条件",
    description:
      "条件门：根据条件将 `value` 转发到 `true` 或 `false` 输出。未选中的分支会被剪除（其下游节点被跳过）。",
    params: { cond: { label: "条件（未接入输入时）", hint: "若连接了 `cond` 输入，以其真值为准。" } },
    ports: { value: "值", cond: "条件", true: "真", false: "假" },
  },
  switch: {
    title: "Switch 分支",
    description:
      "多路路由：将 `value` 转发到与 `index`（0/1/2）匹配的输出，否则到 `default`。未选中的分支会被剪除（跳过）。",
    params: { index: { label: "索引（未接入输入时）" } },
    ports: { value: "值", index: "索引", "0": "0", "1": "1", "2": "2", default: "默认" },
  },
  reroute: {
    title: "中继",
    description: "直通中继：原样转发输入。用它整理过长的连线、在画布上绕线。",
    ports: { in: "输入", out: "输出" },
  },
  preview: {
    title: "预览",
    description: "显示图像缩略图。原始路径会保留以供导出。",
    ports: { image: "图像" },
  },
  save: {
    title: "导出",
    description: "汇聚节点：收集结果图像路径（及可选的 PSD 模板）以供导出。",
    params: { filename: { label: "文件名" } },
    ports: { image: "图像", template: "模板" },
  },
  psdContextAnalyze: {
    title: "PSD 上下文分析",
    description:
      "将 PSD 模板读取为结构化的视觉上下文：背景色与光照启发、占位符几何与安全区、占位符蒙版与背景预览，以及供下游生成使用的提示词后缀。",
    params: {
      psd_path: { label: "PSD 路径", hint: "未连接 PSD Template 节点时使用" },
      background_layer: { label: "背景图层", hint: "要采样的图层（空 = 合成整个 PSD）" },
      target_placeholder: { label: "占位符图层", hint: "要测量的占位符（空 = 整张画布）" },
      reference_layers: { label: "参考图层", hint: "每行一个图层名（Phase 1 中仅供参考）" },
      output_dir: { label: "输出目录", hint: OUTPUT_DIR_HINT },
    },
    ports: {
      template: "模板",
      visual_context: "视觉上下文",
      prompt_suffix: "提示词后缀",
      background_image: "背景",
      placeholder_mask: "占位符蒙版",
      placeholder_bounds: "占位符边界",
    },
  },
  matchLightColor: {
    title: "光照与色彩匹配",
    description:
      "将生成主体的光照与色彩向 PSD 背景靠拢，让合成不再显得「贴上去」：Reinhard Lab 迁移 / 直方图匹配，并向阴影与高光加权，同时保护品牌色。输出匹配后图像、匹配报告与提示词后缀。",
    params: {
      mode: { label: "模式" },
      engine: {
        label: "引擎",
        hint: "cpu = 内置 Lab 迁移 / 直方图匹配（始终可用）；onnx_harmonize = 可选学习型协调器，权重/依赖缺失时回落 cpu",
      },
      device: {
        label: "设备",
        hint: "onnx_harmonize 协调器的计算设备：auto（有 CUDA 用 CUDA，否则 CPU）| cpu | cuda（无加速器时回落 CPU）；cpu 启发式忽略此项",
      },
      strength: { label: "强度" },
      shadow_strength: { label: "阴影强度", hint: "阴影区的额外校正权重" },
      highlight_strength: { label: "高光强度", hint: "高光区的额外校正权重" },
      protect_brand_color: {
        label: "保护品牌色",
        hint: "抑制高彩度（品牌）像素的偏移，让 logo/包装保持原色",
      },
      protect_saturation: { label: "保护饱和度", hint: "只匹配亮度，保留主体自身的彩度" },
      output_dir: { label: "输出目录", hint: OUTPUT_DIR_HINT },
      output_name: { label: "输出名", hint: "匹配后 PNG 的基础名（空 = <image>_matched）" },
    },
    ports: {
      image: "图像",
      visual_context: "视觉上下文",
      background: "背景",
      mask: "蒙版",
      matched_image: "匹配后图像",
      match_report: "匹配报告",
      prompt_suffix: "提示词后缀",
    },
  },
  crop: {
    title: "裁剪",
    description:
      "裁剪图像——首个非蒙版编辑，用于端到端验证统一的自动/手动 + 绑定模型。在原生 Rust 内进程的 Compute 通路运行。manual（手动）模式裁剪到编辑器中绘制的裁剪框（记录为图像像素坐标的 crop_box，属人为空间意图通路）；auto_subject（自动到主体）模式裁剪到主体——它用与 Subject Mask 相同的 Compute 通路分割器算出基底抠像，取其包围盒并按主体边距外扩（属算法推导通路）。两条通路之后都可选按宽高比调整裁剪框（居中、裁剪到图像内）。输出裁剪后的图像与裁剪报告。",
    params: {
      mode: {
        label: "模式",
        hint: "manual 裁剪到编辑器中绘制的框；auto_subject 裁剪到检测出的主体",
      },
      aspect: {
        label: "宽高比",
        hint: "把裁剪锁定到某个宽高比（居中、裁剪到图像内）；free 保持绘制的框",
      },
      margin_pct: {
        label: "主体边距 %",
        hint: "在检测出的主体周围保留的内边距（仅 auto_subject 模式）",
      },
      format: {
        label: "输出格式",
        hint: "png（默认）或 16-bit tiff；宽色域源两者都保留 16-bit + ICC",
      },
      output_dir: { label: "输出目录", hint: OUTPUT_DIR_HINT },
      output_name: { label: "输出名", hint: "裁剪后文件的基础名（空 = <image>_crop）" },
    },
    ports: {
      image: "图像",
      crop_report: "裁剪报告",
    },
  },
  subjectMask: {
    title: "主体蒙版 / 抠像",
    description:
      "选取主体并生成 蒙版 / 抠像 / alpha 三件套。Phase 1 在原生 Rust 内进程运行（无 python 桥）：魔棒漫水选择 + 画笔/橡皮笔触（记录在 edit_paths 中）、形态学（扩张/收缩、填洞）以及最后的羽化。输出蒙版、alpha 图、抠像图与增强版抠像报告。自动主体模型模式（SAM/RMBG/BiRefNet）属于 Phase 2。",
    params: {
      mode: { label: "模式", hint: "Phase 1 运行 manual_* / hybrid 模式；auto_* 模型模式属于 Phase 2" },
      wand_tolerance: { label: "魔棒容差", hint: "魔棒漫水选择的颜色距离" },
      grow_px: { label: "扩张 / 收缩 px", hint: "正值膨胀蒙版，负值腐蚀蒙版" },
      fill_holes: { label: "填洞", hint: "羽化前封闭内部封闭空隙" },
      feather_px: { label: "羽化 px", hint: "柔化蒙版边缘（最后应用）" },
      alpha_matting: {
        label: "Alpha 抠像",
        hint: "通过三分图把二值边缘解算为连续 alpha（头发 / 玻璃）——有 ViTMatte 权重时用 ViTMatte，否则退到确定性羽化兜底",
      },
      matting_band_px: {
        label: "抠像带宽 px",
        hint: "抠像器解算的三分图未知带宽度（仅在开启 Alpha 抠像时）",
      },
      output_dir: { label: "输出目录", hint: OUTPUT_DIR_HINT },
      output_name: { label: "输出名", hint: "三件套 PNG 的基础名（空 = <image>_mask）" },
    },
    ports: {
      image: "图像",
      reference: "参考图",
      visual_context: "视觉上下文",
      placeholder_mask: "占位符蒙版",
      previous_mask: "上一蒙版",
      edit_paths: "编辑路径",
      mask: "蒙版",
      alpha_image: "Alpha 图",
      cutout_image: "抠像图",
      trimap: "三分图",
      matte_report: "抠像报告",
    },
  },
  refineMaskEdge: {
    title: "蒙版边缘精修",
    description:
      "清理抠出主体的边缘，使其放入 PSD 占位符时不带白边或杂边：腐蚀/膨胀形态学、引导滤波边缘吸附、羽化与边缘颜色去污。把 Subject Mask 的 `trimap` 输出接进来，可保护其未知带（头发 / 绒毛 / 玻璃的连续 alpha）不被腐蚀/羽化清理破坏，从而保留细节。输出精修图像、精修蒙版与边缘报告。预设会隐藏细节；选 `custom` 可展开全部参数。",
    params: {
      preset: {
        label: "预设",
        hint: "clean = 紧致 1px 收边，natural = 柔和 6px 羽化，soft = 不收边，custom = 展开全部",
      },
      engine: {
        label: "引擎",
        hint: "cpu = 内置启发式精修（始终可用）；onnx_matting = 可选学习抠像，需要连接 trimap，权重/依赖缺失时回落 cpu",
      },
      device: {
        label: "设备",
        hint: "onnx_matting 抠像的计算设备：auto（有 CUDA 用 CUDA，否则 CPU）| cpu | cuda（无加速器时回落 CPU）；cpu 启发式忽略此项",
      },
      erode_px: { label: "腐蚀 px", hint: "向内收边以去除白边" },
      dilate_px: { label: "膨胀 px", hint: "向外扩张蒙版" },
      feather_px: { label: "羽化 px", hint: "柔化边缘过渡" },
      guided_radius: { label: "引导半径", hint: "将蒙版吸附到亮度边缘（0 关闭）" },
      edge_decontaminate: { label: "边缘去污", hint: "把不透明主体颜色渗入边缘带以消除残余杂边" },
      background_blend_strength: { label: "背景混合", hint: "将边缘带向所连背景色混合" },
      output_dir: { label: "输出目录", hint: OUTPUT_DIR_HINT },
      output_name: { label: "输出名", hint: "精修 PNG 的基础名（空 = <image>_refined）" },
    },
    ports: {
      image: "图像",
      mask: "蒙版",
      background: "背景",
      placeholder_mask: "占位符蒙版",
      trimap: "三分图",
      refined_image: "精修图像",
      refined_mask: "精修蒙版",
      edge_report: "边缘报告",
    },
  },
  imageEnhance: {
    title: "图像增强",
    description:
      "对低分辨率主体放大（Lanczos）并锐化（USM），使其以印刷 DPI 清晰填满 PSD 占位符。接入占位符边界可自动定尺，或显式设定目标像素。Phase 1 仅 CPU（无 GPU 超分）。输出增强图像、所用缩放系数与增强报告。预设会隐藏细节；选 `custom` 可展开 降噪/纹理/缩放。",
    params: {
      mode: {
        label: "模式",
        hint: "conservative = 温和，texture_rebuild = 强细节，print_ready = 均衡，custom = 展开滑块",
      },
      engine: {
        label: "引擎",
        hint: "cpu = 内置 Lanczos+锐化（始终可用）；realesrgan = 可选 GPU/CPU 模型，权重/依赖缺失时回落 cpu",
      },
      device: {
        label: "设备",
        hint: "realesrgan 放大器的计算设备：auto（有 CUDA 用 CUDA，否则 CPU）| cpu | cuda（无加速器时回落 CPU）；cpu 路径忽略此项",
      },
      precision: {
        label: "精度",
        hint: "realesrgan 放大器的计算精度：auto（CUDA 上 fp16，否则 fp32）| fp32 | fp16（CPU 运行时回落 fp32）；cpu 路径忽略此项",
      },
      target_width: { label: "目标宽度", hint: "显式目标像素（0 = 由所连边界或预设缩放自动推算）" },
      target_height: { label: "目标高度", hint: "显式目标像素（0 = 由所连边界或预设缩放自动推算）" },
      target_dpi: { label: "目标 DPI", hint: "写入输出 PNG 元数据的 DPI" },
      scale: { label: "缩放", hint: "未给定目标尺寸时的放大倍数" },
      denoise_strength: { label: "降噪", hint: "放大前的高斯模糊降噪混合" },
      texture_strength: { label: "纹理", hint: "放大后 USM 细节强度" },
      max_pixels: { label: "最大像素", hint: "限制输出像素；缩放会相应降低以适配" },
      preserve_text_logo: { label: "保护文字/logo", hint: "限制锐化，避免 logo / 包装文字被破坏" },
      output_dir: { label: "输出目录", hint: OUTPUT_DIR_HINT },
      output_name: { label: "输出名", hint: "增强 PNG 的基础名（空 = <image>_enhanced）" },
    },
    ports: {
      image: "图像",
      target_bounds: "目标边界",
      enhanced_image: "增强图像",
      scale_factor: "缩放系数",
      enhance_report: "增强报告",
    },
  },
  detailWatchdog: {
    title: "细节看护",
    description:
      "扫描候选图像中的局部劣化（全局/区域模糊、alpha 边缘光晕、与所连背景的颜色不匹配、低于目标的分辨率）并输出 QualityReport，让工作流决定是重跑还是手工修。Phase 1 仅检测（不自动重绘）：`fixed_image` 即未改动的输入。CPU 规则层始终运行；手/文字/logo 等语义目标默认标记为跳过，除非通过 `engine` 选用可选的 ML 检测器来覆盖。接入 VisualContext 和/或占位符边界以进行分辨率与颜色检查。",
    params: {
      mode: { label: "模式", hint: "检测灵敏度：strict = 标记更多，lenient = 标记更少" },
      watch_targets: {
        label: "看护目标",
        hint: "face,hands,text,logo,product_edges 的逗号列表（空 = 全部）；未启用 ML 引擎时跳过 hands/text/logo",
      },
      engine: {
        label: "引擎",
        hint: "rules = 内置 CPU 规则层（始终可用）；onnx_defect = 可选 ML 检测器，覆盖 手/文字/logo，权重/依赖缺失时回落 rules",
      },
      device: {
        label: "设备",
        hint: "onnx_defect 检测器的计算设备：auto（有 CUDA 用 CUDA，否则 CPU）| cpu | cuda（无加速器时回落 CPU）；rules 层忽略此项",
      },
      output_dir: { label: "输出目录", hint: OUTPUT_DIR_HINT },
      output_name: { label: "输出名", hint: "问题叠加 PNG 的基础名（空 = <image>_issues）" },
    },
    ports: {
      image: "图像",
      visual_context: "视觉上下文",
      target_bounds: "目标边界",
      fixed_image: "修复图像",
      quality_report: "质量报告",
      issue_masks: "问题蒙版",
      watchdog_report: "看护报告",
    },
  },
  detailRepaint: {
    title: "细节重绘",
    description:
      "对 Detail Watchdog 标记的问题区域做局部重绘。为每个可重绘问题（其 suggested_action 在 `Repaint actions` 列表中）带边距裁剪，写出 inpaint 蒙版，将每块裁剪通过 broker 的 image.edit 操作（与 Generate 相同的提供方/凭据路径）发送，再以羽化接缝贴回。输出修复图像与 RepaintReport。若未配置具备编辑能力的提供方（空 / `mock`），则所有区域都不重绘，图像原样通过。",
    params: {
      provider: {
        label: "提供方",
        hint: "具备 image.edit 能力的提供方（选择档案时自动设置）；空/mock 则直通",
      },
      operation: { label: "操作" },
      engine: {
        label: "引擎",
        hint: "provider = 远程 image.edit（默认）；sd_inpaint = 可选本地 GPU 重绘，权重/依赖缺失时回落 provider",
      },
      precision: {
        label: "精度",
        hint: "sd_inpaint 后端的计算精度：auto（CUDA 上 fp16，否则 fp32）| fp32 | fp16（CPU 运行时回落 fp32）；provider 路径忽略此项",
      },
      credentials_ref: { label: "凭据", hint: "选择档案时自动设置" },
      repaint_prompt_base: {
        label: "重绘提示词",
        hint: "每个区域的基础提示词（空 = 通用修复提示；问题类型会被追加）",
      },
      repaint_actions: { label: "重绘动作", hint: "要重绘的 suggested_action 值的逗号列表" },
      min_confidence: { label: "最小置信度", hint: "仅重绘置信度达到/高于此值的问题（0..1）" },
      region_padding: { label: "区域边距", hint: "在每个问题框周围添加的上下文边距（px）" },
      max_regions: { label: "最大区域数", hint: "限制重绘的区域数量（优先置信度最高的）" },
      feather_px: { label: "羽化 px", hint: "接缝羽化半径（0 = 按问题尺寸自动）；poisson 混合忽略此项" },
      blend: {
        label: "接缝混合",
        hint: "feather = 补丁接缝处的软 alpha 过渡（默认）；poisson = 梯度域无缝克隆，适合更难的接缝（区域过小时回落 feather）",
      },
      output_dir: { label: "输出目录", hint: OUTPUT_DIR_HINT },
      output_name: { label: "输出名", hint: "修复图像的基础名（空 = <image>_repainted）" },
    },
    ports: {
      image: "图像",
      quality_report: "质量报告",
      fixed_image: "修复图像",
      repaint_report: "重绘报告",
    },
  },
  videoAssemble: {
    title: "视频合成",
    description:
      "通过媒体引擎的 FFmpeg 后端（PyAV）将有序的帧图像序列编码为视频文件。连接帧列表（或在 frames 参数中每行填一个路径），选择帧率与编码器，即可在磁盘上得到 .mp4 及编码报告。",
    params: {
      frames: { label: "帧列表", hint: "帧图像路径，每行一个（连接的 frames 输入优先）" },
      fps: { label: "帧率", hint: "输出帧率" },
      codec: { label: "编码器", hint: "ffmpeg 编码器；libx264 兼容性最好" },
      output_dir: { label: "输出目录", hint: OUTPUT_DIR_HINT },
      output_name: {
        label: "输出名",
        hint: "输出文件名（空 = assembled-<时间戳>.mp4；缺省扩展名为 .mp4）",
      },
    },
    ports: {
      frames: "帧列表",
      video: "视频",
      frame_count: "帧数",
      duration_sec: "时长（秒）",
      assemble_report: "合成报告",
    },
  },
  videoTrim: {
    title: "视频剪辑",
    description:
      "通过媒体引擎的 FFmpeg 后端（PyAV）从视频文件中剪出一个时间区间。连接视频（或在 video 参数中填路径），设置起止秒数，即可得到帧精确的重编码片段及剪辑报告。音频不会保留。",
    params: {
      video: { label: "视频", hint: "源视频路径（连接的 video 输入优先）" },
      start_sec: { label: "起始秒", hint: "剪辑起点（自开头起的秒数）" },
      end_sec: { label: "结束秒", hint: "剪辑终点秒数（0 = 到片尾）" },
      codec: { label: "编码器", hint: "重编码使用的 ffmpeg 编码器；libx264 兼容性最好" },
      output_dir: { label: "输出目录", hint: OUTPUT_DIR_HINT },
      output_name: {
        label: "输出名",
        hint: "输出文件名（空 = trimmed-<时间戳>.mp4；缺省扩展名为 .mp4）",
      },
    },
    ports: {
      video: "视频",
      frame_count: "帧数",
      duration_sec: "时长（秒）",
      trim_report: "剪辑报告",
    },
  },
  psdExport: {
    title: "PSD 导出",
    description:
      "将生成图像写入 PSD 模板的占位符（尽可能做真正的智能对象替换），并导出 final.psd + preview.png + metadata.json。可接收可选的精修蒙版（作为图像 alpha 应用）以及一个并入导出元数据的生产元数据对象。",
    params: {
      filename: { label: "文件名" },
      output_dir: { label: "输出目录", hint: OUTPUT_DIR_HINT },
      placeholder: { label: "占位符图层", hint: "要替换的模板图层名（空 = 整张画布）" },
      fit_mode: { label: "适配" },
      smart_object_mode: {
        label: "智能对象",
        hint: "replace_content 重写智能对象（在 Photoshop 中保持可编辑）",
      },
    },
    ports: { image: "图像", template: "模板", mask: "蒙版", metadata: "元数据" },
  },
};

/**
 * Return a copy of `spec` with its human-readable strings translated into
 * `lang`. For `en` (or a kind without an entry) the original spec is returned
 * unchanged; any individual missing string falls back to its English original.
 */
export function localizeSpec(spec: NodeSpec, lang: Lang): NodeSpec {
  if (lang !== "zh") return spec;
  const tr = NODE_ZH[spec.kind];
  if (!tr) return spec;
  return {
    ...spec,
    title: tr.title || spec.title,
    description: tr.description || spec.description,
    inputs: spec.inputs.map((p) => ({ ...p, label: tr.ports?.[p.id] ?? p.label })),
    outputs: spec.outputs.map((p) => ({ ...p, label: tr.ports?.[p.id] ?? p.label })),
    params: spec.params.map((p) => ({
      ...p,
      label: tr.params?.[p.key]?.label ?? p.label,
      hint: tr.params?.[p.key]?.hint ?? p.hint,
    })),
  };
}
