# Subject Mask / Matte Editor Card Plan

## 核心判断

H-Gripe Studio 需要一个生产级的 **Subject Mask / Matte Editor** 卡片。

它不是普通“抠图”节点，而是 PSD-first 工作流里的主体选区与蒙版生产卡片。它负责回答：

```text
主体在哪里？
哪些区域应该保留？
哪些边缘需要半透明？
哪些地方需要人工修正？
```

它和现有卡片的边界应该清楚：

```text
Subject Mask / Matte Editor  -> 负责识别主体、生成/编辑 mask
Refine Mask Edge             -> 负责清理白边、毛刺、羽化、边缘融合
PSD Compose / PSD Export     -> 负责把结果写回 PSD
```

不要把它和 `Refine Mask Edge` 合并。一个负责“选区”，一个负责“边缘质量”。

## 推荐工作流

```text
Image Source / Generated Image
  -> Subject Mask / Matte Editor
  -> Refine Mask Edge
  -> Match Light & Color
  -> PSD Compose
  -> PSD Export
```

PSD 生产链路里：

```text
PSD Context Analyze
  + Image Generate / Image Edit
  -> Subject Mask / Matte Editor
  -> Refine Mask Edge
  -> PSD Export
```

## 卡片定位

这个卡片应该是一个“节点 + 小编辑器”的组合，而不是只有几个参数的普通节点。

节点负责：

- 接收图片。
- 调用自动主体识别或本地/远端 segmentation provider。
- 保存 mask / matte / cutout。
- 把结果交给后续节点。

编辑器负责：

- 预览原图和 mask。
- 允许人工修正主体范围。
- 支持画笔、橡皮、钢笔、套索、羽化、反选、填洞、平滑。
- 保存人工编辑路径和 mask 到 workflow。

## 输入

```text
image
optional_reference
optional_visual_context
optional_psd_placeholder_mask
optional_prompt
optional_previous_mask
```

说明：

- `image`：要抠主体的图。
- `optional_reference`：参考图或目标主体图。
- `optional_visual_context`：来自 `PSD Context Analyze` 的上下文。
- `optional_psd_placeholder_mask`：PSD 占位区域 mask，可用于限制主体范围。
- `optional_prompt`：例如 “perfume bottle”, “main product”, “person”.
- `optional_previous_mask`：用于继续编辑上一次的 mask。

## 输出

```text
mask
alpha_image
cutout_image
matte_report
edit_paths
```

说明：

- `mask`：灰度 PNG，黑白或半透明 mask。
- `alpha_image`：带 alpha 的完整图。
- `cutout_image`：主体裁切图，可直接传给 `Refine Mask Edge`。
- `matte_report`：记录自动识别、人工修正、羽化、扩张/收缩等参数。
- `edit_paths`：钢笔路径、套索路径、人工操作记录，方便以后继续编辑。

## 运行模式

```text
auto_subject
auto_product
auto_person
auto_transparent_object
manual_brush
manual_pen
hybrid
```

推荐默认模式是 `hybrid`：

```text
自动识别主体
  -> 用户预览
  -> 用户必要时手动修正
  -> 输出 mask / cutout
```

## UI 设计

卡片本体保持简单：

```text
Subject Mask
[Auto Detect]
[Edit Mask]
[Apply]
```

点击 `Edit Mask` 打开局部编辑器。

编辑器布局：

```text
左侧：原图 / 生成图
中间：mask 半透明叠加预览
右侧：工具和参数
底部：缩放、撤销、重做、应用、取消
```

必备工具：

```text
画笔添加
橡皮扣除
钢笔路径
套索
矩形/椭圆选区
反选
填洞
平滑
收缩/扩张
羽化
边缘预览
透明度预览
```

钢笔非常重要。产品图、香水瓶、玻璃瓶、金属边、透明材质，自动识别经常不稳定。钢笔路径应该能作为硬约束：

```text
pen_path -> rasterize mask -> boolean combine with AI mask
```

## 数据结构建议

### SubjectMaskResult

```json
{
  "mask_path": "",
  "alpha_image_path": "",
  "cutout_image_path": "",
  "edit_paths_path": "",
  "matte_report": {
    "mode": "hybrid",
    "provider": "local",
    "detected_subjects": [
      {
        "label": "product",
        "confidence": 0.92,
        "bbox": [120, 80, 900, 1300]
      }
    ],
    "operations": [
      { "type": "auto_detect", "provider": "sam" },
      { "type": "pen_add", "path_id": "path_1" },
      { "type": "brush_subtract", "radius": 18 },
      { "type": "feather", "px": 2.5 }
    ]
  }
}
```

### EditPaths

```json
{
  "version": 1,
  "paths": [
    {
      "id": "path_1",
      "mode": "add",
      "tool": "pen",
      "closed": true,
      "points": [
        { "x": 100, "y": 120, "in": [90, 110], "out": [110, 130] }
      ]
    }
  ],
  "brush_strokes": [
    {
      "id": "stroke_1",
      "mode": "subtract",
      "radius": 18,
      "points": [[100, 120], [105, 124]]
    }
  ]
}
```

## 技术路线

### Phase 1：手动可用

先不接大模型，先把编辑器和数据链路跑通。

- 支持加载图片。
- 支持显示 mask 叠加。
- 支持画笔添加/扣除。
- 支持橡皮。
- 支持羽化、反选、填洞、扩张/收缩。
- 输出 `mask.png`、`cutout.png`、`matte_report.json`。

这一阶段就可以用于真实 PSD 工作流。

### Phase 2：自动主体识别

接入自动 segmentation：

```text
SAM
RMBG
BiRefNet
U2Net / rembg
远端 segmentation API
```

不要把模型塞进前端。推荐走：

```text
Tauri / Rust
  -> provider/profile
  -> Python local worker or remote API
  -> mask result
```

### Phase 3：钢笔路径

加入钢笔路径和 Bezier rasterize。

关键能力：

- 保存路径到 workflow。
- 支持路径 add / subtract / intersect。
- 路径可再次编辑。
- 路径 rasterize 成 mask。

这个阶段会显著提升产品图、包装图、玻璃瓶、复杂边缘的生产可控性。

### Phase 4：高级 Alpha Matting

用于头发、毛绒、玻璃、半透明材质。

能力：

- 输出连续 alpha matte，而不是纯二值 mask。
- 支持 trimap。
- 支持透明物体保留折射/反光。
- 和 `Refine Mask Edge` 联动。

## 后端边界

推荐边界：

```text
React UI
  -> mask 编辑器、钢笔路径、预览、撤销重做

Rust / Tauri
  -> 文件读写、任务调度、缓存、历史、路径校验、provider 调用

Python bridge / local worker
  -> segmentation 模型、matting 模型、复杂图像处理

Refine Mask Edge
  -> 接收 mask/cutout，负责边缘融合
```

## 不建议的做法

不要拆出一堆底层卡片：

```text
SAM Detect Node
RMBG Node
Brush Mask Node
Pen Mask Node
Feather Mask Node
Invert Mask Node
Fill Hole Node
```

这些能力应该收敛在 `Subject Mask / Matte Editor` 这一个生产语义卡片里。

用户在工作流里想表达的是：

```text
我要得到可用于 PSD 合成的主体蒙版
```

而不是：

```text
我要手动拼十个底层 mask 算法节点
```

## 与现有卡片的关系

```text
PSD Context Analyze
  -> 提供 placeholder mask / bounds

Subject Mask / Matte Editor
  -> 生成主体 mask / cutout / alpha image

Refine Mask Edge
  -> 清理白边、毛刺、半透明边缘

Match Light & Color
  -> 让主体和背景光影色彩一致

PSD Export
  -> 写入 final.psd / preview.png / metadata.json
```

## 结论

这个卡片应该做，而且应该是 H-Gripe Studio 的核心生产卡片之一。

PSD 生产里真正难的不是调用更多 API，而是让生成结果可控、可修、可进入 Photoshop 工作流。主体蒙版、钢笔修正、边缘 alpha、路径保存这些能力做好，会比新增很多 provider-specific 节点更有长期价值。
