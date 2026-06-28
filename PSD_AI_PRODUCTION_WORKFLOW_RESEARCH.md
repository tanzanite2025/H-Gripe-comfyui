# PSD + AI Production Workflow Research

## 核心判断

这个方向是成立的，而且更接近生产环境需要的 AI 图片工作流。

单纯的生图链路通常是：

```text
prompt -> image
```

生产级链路更应该是：

```text
PSD template / fixed design assets
+ reference image
+ generation / edit / enhance / upscale nodes
+ deterministic layer composition
+ PSD export for manual finishing
```

模型输出本质是概率图，不适合直接承担所有版式、边框、装饰、文字、安全区和最终商业交付细节。更合理的分工是：

- PSD 模板负责固定版式、边框、装饰、文字层、占位区、蒙版、安全区。
- AI 模型负责主体生成、参考图重绘、局部修复、风格统一、细节增强。
- 单独节点负责 upscale、denoise、sharpen、色彩匹配、背景移除、mask 生成。
- PSD compose/export 节点负责把结果放回可编辑图层结构。
- 人最后在 Photoshop 中继续修细节。

目标输出不应该只有 PNG/JPG，而应该至少包含：

```text
final.psd
preview.png
metadata.json
```

其中 `final.psd` 是生产源文件，`preview.png` 是快速预览，`metadata.json` 记录模型、prompt、seed、provider、节点参数、输入资源路径和生成时间。

## 推荐工作流

```text
PSD Template Node
  -> load PSD
  -> read layer groups
  -> read named placeholders
  -> read masks / bounds / blend modes

Reference Image Node
  -> user image / product image / style image
  -> optional crop / normalize / background remove

Generate Image Node
  -> OpenAI-compatible / Gemini / Flux / local GPU service
  -> text-to-image / image-to-image / inpaint

Enhance Nodes
  -> upscale
  -> denoise
  -> sharpen
  -> face/detail restore
  -> color match

PSD Compose Node
  -> put generated result into named placeholder
  -> preserve border/decorative/text layers
  -> create candidate layers
  -> attach masks
  -> write hidden reference/prompt layers

PSD Export Node
  -> final.psd
  -> preview.png
  -> metadata.json
```

## 为什么必须导出 PSD

AI 生成图经常会出现小概率问题，例如：

- 局部细节不可靠。
- 文字不稳定。
- 装饰边缘可能乱。
- 商业图需要手工修瑕疵。
- 客户或自己后期需要快速替换内容。

如果只导出 PNG/JPG，后期修图成本会变高。导出 PSD 可以保留：

- 原始模板层。
- 生成结果层。
- 多个候选结果隐藏层。
- 参考图隐藏层。
- mask / alpha。
- 文案层。
- 调整层。
- 生成参数说明层或 metadata。

所以 PSD 不是附加格式，而应该是生产主格式。

## 市场和现有方案调研

结论：已经有类似方向，但多数只覆盖其中一段，没有看到一个完全等同于“PSD 模板生产流水线 + 节点式 API 编排 + 可继续 Photoshop 精修”的开源/个人工具闭环。

### Adobe Photoshop / Firefly / Photoshop API

Adobe 已经证明这个方向是对的。Photoshop 的 Generative Fill 会创建新的生成图层，用户需要保存为 PSD 或其他分层格式来保留编辑结果。Adobe 官方 FAQ 明确提到生成后会创建 Generative Layer，并保存 PSD 可保留图层结构。

参考：

- https://helpx.adobe.com/photoshop/desktop/generative-ai/frequently-asked-questions-about-generative-ai-features.html
- https://www.adobe.com/products/photoshop/generative-fill.html

Adobe Firefly Services / Photoshop API 也在往生产自动化方向走。Photoshop API v2 已经强调生产级、高容量工作流、linked smart objects、UXP scripting、5GB 文件等能力。

参考：

- https://developer.adobe.com/firefly-services/docs/photoshop/getting-started/v2-ga/
- https://developer.adobe.com/firefly-services/docs/photoshop/
- https://developer.adobe.com/firefly-services/docs/guides/

差异：Adobe 是强大的官方生产平台，但不是一个自由节点式、个人 API-first、可混接各种模型和本地小服务的工作流工具。

### Layer AI

Layer AI 明确主打 AI 生成后导出 PSD，并回到 Photoshop 精修。它也强调 batch generation、PSD export、Photoshop round-trip editing、最高 8K 输出等生产概念。

参考：

- https://www.layer.ai/integrations/photoshop

差异：方向很接近，但更像一个专用商业平台；它不是围绕用户自有 PSD 模板、ComfyUI 式节点、任意 API provider、本地 GPU 服务组合来设计的。

### RunDiffusion Image-to-Layers

RunDiffusion 有 Image-to-Layers workflow，把平面图通过自动分割拆成可编辑 PSD 图层。它的重点是从 flat image 反推 layered PSD，降低手工抠图和分割成本。

参考：

- https://www.rundiffusion.com/introducing-image-to-layers-a-rundiffusion-exclusive-workflow-with-psd-export

差异：它偏“生成/已有图 -> 自动分层 PSD”，而不是“PSD 模板 -> 节点生成/修图/合成 -> 生产 PSD”。

### Copainter Layering AI

Copainter 已经提出从 AI 生成图自动拆出线稿、底色、阴影、光照，并导出 PSD，面向漫画和动画生产。

参考：

- https://blog.copainter.ai/en/layer-separation-en/

差异：它偏二次元/漫画/动画的 layer separation，不是通用 PSD 模板合成工作流。

### ComfyUI 相关节点

ComfyUI 已经有一些 PSD/layer 方向的节点和扩展：

- ComfyUI-Layers / LayersSaver：可把图片、mask 或 batch images 保存为 PSD 图层。
- ComfyUI-LayerStyle：提供类似 Photoshop 的合成、mask、layer style 工作流。
- LayerUtility LoadPSD：可加载 PSD 并提取图层用于工作流。
- D2 SavePSD ComfyUI：专门保存 Photoshop `.psd` 文件，支持 alpha 通道相关导出，但 layer mask 能力仍有限。
- Qwen Layer Nodes / See-through / LayerForge 等节点也在尝试“图像拆层、分层编辑、PSD 输出、类 Photoshop canvas”这些方向。

参考：

- https://github.com/alessandrozonta/ComfyUI-Layers
- https://www.alessandrozonta.net/post/comfyui-layerssaver/
- https://www.runcomfy.com/comfyui-nodes/ComfyUI_LayerStyle
- https://www.runcomfy.com/comfyui-nodes/ComfyUI_LayerStyle/LayerUtility--LoadPSD
- https://github.com/da2el-ai/D2-SavePSD-ComfyUI
- https://github.com/EricRollei/Qwen_Layers_Diffuser_Pipeline_Comfyui
- https://github.com/jtydhr88/ComfyUI-See-through
- https://github.com/Azornes/Comfyui-LayerForge

差异：这些组件证明技术可行，但多数是节点级能力，还没有形成“PSD 模板作为生产源文件 + API-first provider + 历史/配置/输出管理 + Tauri 桌面体验”的完整产品方向。

### Image-to-Layers 类服务

另外还有一些服务主打把一张平面图拆成可编辑图层，并导出 ZIP 或 PSD。这类产品说明“AI 输出后继续精修”的需求很真实。

参考：

- https://www.imagetolayers.com/

差异：这类服务通常从一张已经生成好的 flat image 出发，重点是自动分割和拆层；H-Gripe 更应该从 PSD 模板、占位层、参考图、API 生成、增强、合成、导出这一整条生产链路出发。

### Krita AI Diffusion

Krita AI Diffusion 是在图像编辑器内接入生成式 AI 的典型案例。它适合绘画/修图场景，支持局部生成、扩图、修复、参考等工作流。

参考：

- https://github.com/Acly/krita-ai-diffusion
- https://kritaaidiffusion.com/

差异：Krita 是绘画软件内 AI 插件路线，不是 PSD 模板自动化生产流水线。PSD 兼容和 Photoshop 生产交付并不是它的核心目标。

### Photopea

Photopea 支持脚本和 API，可以自动化 PSD/模板处理，也能把保存结果发回服务器。

参考：

- https://www.photopea.com/learn/scripts
- https://www.photopea.com/api/

差异：Photopea 是非常有价值的 PSD 自动化/浏览器编辑引擎候选，但它本身不是 AI 生成工作流编排系统。

## 机会点

已经存在的方案说明这个需求是真实的，但 H-Gripe 可以切入一个更适合个人生产和长期扩展的位置：

```text
PSD-first AI production workflow
```

核心差异：

- 不把模型输出当最终结果，而是当 PSD 图层素材。
- 不让模型生成边框、装饰和版式，而是从 PSD 模板复用。
- 不只支持单一 AI 平台，而是通过 API provider 接 OpenAI-compatible、Gemini、Flux、本地 GPU 服务、Kling、Runway、Veo、Replicate 等。
- 不强依赖完整 ComfyUI 本地模型生态，但保留节点工作流优势。
- 最终输出可继续在 Photoshop 精修。

## 建议的 H-Gripe 节点

### PSD Template Load

输入：

- PSD 文件路径。
- 图层组过滤规则。
- 占位层命名规则。

输出：

- PSD document info。
- placeholder list。
- layer bounds。
- preview image。

### PSD Placeholder Select

输入：

- PSD document info。
- placeholder name。

输出：

- bounds。
- mask。
- target size。
- target aspect ratio。

### Reference Image Prepare

输入：

- 参考图。
- crop mode。
- background remove 开关。
- color normalize 开关。

输出：

- normalized reference。
- optional mask。

### API Image Generate / Edit

输入：

- prompt。
- reference image。
- target bounds / aspect。
- provider profile。

输出：

- generated image。
- candidates。
- raw result json。

### Upscale / Enhance

输入：

- image。
- scale。
- target size。
- provider。

输出：

- enhanced image。

### PSD Compose

输入：

- PSD template。
- placeholder target。
- generated/enhanced image。
- optional candidates。
- optional reference image。
- metadata。

输出：

- composed PSD object。
- preview image。

### PSD Export

输入：

- composed PSD object。
- output directory。
- filename template。

输出：

- final.psd。
- preview.png。
- metadata.json。

## PSD 内推荐图层结构

```text
project.psd
  00_META
    prompt.txt
    generation_info.json
  01_TEMPLATE
    border
    decoration
    logo
    text
  02_REFERENCE
    reference_image hidden
  03_GENERATED
    candidate_01
    candidate_02 hidden
    candidate_03 hidden
  04_FINAL
    final_composite
  05_MASKS
    placeholder_mask
    subject_mask
```

## MVP 建议

第一阶段不要追求完整 Photoshop 兼容。建议先做最小闭环：

1. 读取一个 PSD 模板或 PNG 模板。
2. 允许用 JSON 描述占位区域。
3. 用现有 API Image 节点生成/编辑图片。
4. 用 Rust/Python PSD 库写入一个简单 PSD：
   - template background。
   - generated layer。
   - border/decor layer。
   - hidden reference layer。
   - metadata text layer 或 sidecar JSON。
5. 同时导出 preview.png。

第二阶段再做：

- 真实 PSD layer bounds 读取。
- mask 读取/写入。
- smart object 替换。
- 多候选隐藏层。
- Photoshop/Photopea round-trip。
- Tauri 里打开 PSD、打开输出目录、预览历史。

## 技术路线选择

可选路线：

### Python PSD 库

适合快速验证，例如 `psd-tools`、`pytoshop` 或已有 ComfyUI 节点依赖。优点是快，缺点是 PSD 写入能力可能受限。

### Photoshop API / Firefly Services

适合真正企业级 PSD 自动化，支持更接近 Photoshop 的语义，但成本、权限和云依赖更重。

### Photopea API / Scripting

适合做浏览器/服务端 PSD 自动化桥接，可以考虑作为 Tauri 内部预览/编辑方案之一。

### Photoshop 本地 UXP / 脚本桥

适合未来和本机 Photoshop 联动：H-Gripe 生成 PSD/metadata，Photoshop 插件负责打开、替换 smart object、执行动作。

## 当前结论

这个方向已经被多个产品和插件从不同角度验证：

- Adobe 验证了 AI + PSD/layers 是主流生产方向。
- Layer AI 验证了 AI 生成后 PSD round-trip 有商业需求。
- RunDiffusion/Copainter 验证了 AI 输出 layered PSD 有需求。
- ComfyUI 社区验证了 PSD save/load/layer compose 在节点系统里可行。
- Photopea 验证了 PSD 自动化和脚本桥接可行。

但 H-Gripe 的机会不是复制它们，而是做一个更个人化、更开放、更 API-first 的生产流水线：

```text
PSD template in
reference / prompt / API generation
enhance / upscale / compose
PSD + preview + metadata out
Photoshop manual finish
```

这个方向值得作为 H-Gripe 的核心产品方向之一。
