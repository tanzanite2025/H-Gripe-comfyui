# H-Gripe API-First Desktop Plan

## 项目定位

H-Gripe 的长期方向不是做本地模型推理加速版 ComfyUI，而是做一个更轻、更顺手、更适合个人使用的 API-first 创作工作流桌面端。

核心目标：

- 使用 Tauri 作为桌面端壳，避免 Electron 带来的体积和资源占用。
- 使用 Rust 管理任务执行、API 请求、缓存、队列、文件和运行状态。
- 保留 ComfyUI 的节点式工作流思想，但不被完整 ComfyUI 的本地推理生态绑定。
- 优先适配各种云端/API 模型，让用户通过节点自由组合 OpenAI、Gemini、Claude、Runway、Kling、Veo、Stability、Replicate 等服务。
- 面向个人高频使用体验，优先追求稳定、清爽、可复用、可扩展。

## 不做什么

第一阶段不重点做这些事情：

- 不重写 PyTorch / CUDA / 本地模型推理。
- 不追求兼容所有 ComfyUI 节点。
- 不把完整 ComfyUI 前端直接重写一遍。
- 不做单个超大 exe，把 Python、Torch、模型和所有依赖硬塞进去。
- 不为了通用性牺牲个人使用体验。

## 长期架构

推荐架构：

```text
Tauri Desktop
  |
  |-- 窗口、菜单、托盘、设置、日志、文件选择、更新
  |
Rust Core
  |
  |-- API 任务队列
  |-- Provider 适配器
  |-- 缓存和历史记录
  |-- 文件上传/下载
  |-- 凭据管理
  |-- 工作流执行状态
  |
Workflow Layer
  |
  |-- 节点图
  |-- 参数绑定
  |-- 批量任务
  |-- 结果复用
  |
Provider APIs
  |
  |-- OpenAI
  |-- Gemini
  |-- Claude
  |-- Runway
  |-- Kling
  |-- Veo
  |-- Stability
  |-- Replicate
  |-- 其他自定义 API
```

## 和 ComfyUI 的关系

短期可以继续借用 ComfyUI 的节点思想和部分后端结构，但项目方向应逐步从“完整 ComfyUI 魔改”转向“独立 API 工作流工具”。

建议演进：

1. 保留现有 ComfyUI 源码，先写 API 节点和 Rust broker。
2. 把 API 请求、重试、缓存、上传下载、任务队列放到 Rust。
3. Python 节点只做很薄的桥接层，把任务交给 Rust。
4. Tauri 桌面端先嵌入现有 Web UI 或本地页面。
5. 后续再做自己的轻量节点编辑器，只兼容真正需要的工作流格式。

## API 节点设计

不要每个 API 都完全从零硬写一套逻辑。应该建立统一的 Provider 层。

通用任务格式：

```text
ApiTask
  id
  provider
  operation
  inputs
  params
  credentials_ref
  output_type
  cache_policy
  retry_policy
```

通用输出格式：

```text
ApiResult
  id
  status
  output_files
  output_json
  metadata
  cost
  duration
  provider_request_id
  cache_hit
```

节点示例：

- OpenAI Image Generate
- OpenAI Image Edit
- Gemini Image
- Claude Text
- Kling Text to Video
- Runway Image to Video
- Veo Video
- Replicate Model Run
- HTTP Custom API
- File Upload
- File Download
- Result Cache
- Batch Prompt

## Rust 负责的部分

Rust 适合长期负责这些基础设施：

- HTTP / WebSocket / polling 请求。
- 并发限制和任务排队。
- 自动重试、超时、取消、恢复。
- API key 安全存储和读取。
- 结果缓存和去重。
- 文件上传、下载、校验、归档。
- 任务历史、费用统计、耗时统计。
- 工作流运行状态同步。
- 本地数据库，例如 SQLite。
- Tauri 命令接口。

## Python 保留的部分

如果继续兼容 ComfyUI，Python 主要保留：

- 节点定义。
- ComfyUI 插件加载。
- 工作流兼容层。
- 少量数据转换。
- 调用 Rust 扩展或本地 Rust 服务。

Python 节点应该尽量薄：

```text
ComfyUI Node
  -> 参数校验
  -> 构造 ApiTask
  -> 交给 Rust 执行
  -> 返回 ApiResult
```

## Tauri 桌面端职责

Tauri 不只是一个壳，应该承担桌面体验：

- 启动和关闭后端服务。
- 自动选择本地端口。
- 管理 API key 设置页。
- 管理模型/API provider 配置。
- 展示任务队列和历史记录。
- 展示日志和错误详情。
- 管理输出目录。
- 提供拖拽文件、复制结果、打开文件夹等桌面能力。
- 后续接入自动更新。

## 性能预期

这个方向的性能提升主要来自工程结构，而不是模型推理加速。

会明显改善：

- 桌面端启动和资源占用。
- 多 API 并发任务管理。
- 大量请求时的稳定性。
- 上传/下载和缓存效率。
- 失败重试和断点恢复。
- 重复任务秒级复用缓存。
- 长时间运行的可靠性。

不会直接改善：

- 云 API 自身生成速度。
- 云服务排队时间。
- 本地 PyTorch / CUDA 推理速度。
- 第三方 API 的速率限制。

## 阶段计划

### Phase 1: API Broker

- 新建 Rust API broker。
- 支持 OpenAI / Gemini 等少数高频 provider。
- 支持任务队列、重试、缓存、文件下载。
- ComfyUI 节点通过 Python 调用 Rust broker。

当前已开始实现：

- 已新增 Rust workspace：`Cargo.toml`。
- 已新增 API broker crate：`crates/hgripe-api`。
- 已定义统一任务/结果契约：`ApiTask`、`ApiResult`、`CachePolicy`、`RetryPolicy`。
- 已定义 Provider 注册层和 `Provider` trait。
- 已实现内存缓存、基础重试框架和 `mock` provider。
- 已实现 `custom_http` provider，支持通用 HTTP GET/POST、headers、query、JSON body、timeout、重试和 2xx/4xx/5xx 状态分流。
- 已实现 `openai_compatible` provider，支持 `chat.completions`、`text.generate`、`vision.analyze`、`image.generate`，可配置 `base_url`、API key/env、额外请求体和本地/代理 OpenAI-compatible 服务。
- 已新增 provider profile 第一版：OpenAI-compatible 任务可用 `profile_ref` 引用本地 `user/hgripe/provider_profiles.json`，把 `base_url`、`model`、默认参数、headers、`extra_body`、`credentials_ref` 或 `no_auth` 从 workflow/node 参数里抽出来。
- 已新增 CLI 桥：`hgripe-api-broker`，支持 stdin 输入 `ApiTask` JSON，stdout 输出 `ApiResult` JSON。
- 已新增 Python 桥接示例：`python/bridge/hgripe_api_bridge.py`。
- 已新增本地 HTTP 验证示例：`python/bridge/custom_http_example.py`，不依赖外部网络服务。
- 已新增 OpenAI-compatible 本地验证示例：`python/bridge/openai_compatible_text_example.py`，用本地临时服务模拟 chat completions。
- 已新增 ComfyUI 薄节点：`custom_nodes/hgripe_api_nodes.py`，当前提供 `H-Gripe Custom HTTP API`、`H-Gripe OpenAI Compatible Text`、`H-Gripe OpenAI Compatible Image` 和 `H-Gripe OpenAI Compatible Vision`，把参数组装成 `ApiTask` 后交给 Rust broker。
- `H-Gripe OpenAI Compatible Image` 支持 `b64_json` 和 `url` 返回，并转换为 ComfyUI `IMAGE` tensor，同时保留完整 `result_json` 和 `status` 输出。
- `H-Gripe OpenAI Compatible Vision` 支持把 ComfyUI `IMAGE` tensor 编码为 data URL，通过 OpenAI-compatible chat/vision 接口返回文本分析。
- 已新增 credential ref 第一版：OpenAI-compatible 节点可用 `credentials_ref` 引用本地凭据，默认读取被 git 忽略的 `user/hgripe/credentials.json`，也支持 `HGRIPE_CREDENTIALS_FILE` 指向其他文件。
- 已新增本地任务历史第一版：CLI broker 每次执行后追加 JSONL 记录到 `user/hgripe/history/tasks.jsonl`，记录 provider、operation、status、duration、request id、输出文件列表和输出摘要。
- 已新增 SQLite 历史索引：CLI broker 同步写入 `user/hgripe/history/tasks.sqlite3`，支持按时间读取最近任务，并支持按 provider、operation、status、是否有输出文件筛选。
- 已新增历史重跑基础：新历史记录会保存脱敏后的 `task_snapshot`，去掉 inline API key、token、password、Authorization 等敏感字段，并可通过 `history_rerun_example.py` 按 `task_id` 重跑。
- 已新增 Rust 历史动作入口：`hgripe-api-history` 支持 `list`、`show`、`rerun-task` 和 `rerun`，用于模拟后续 Tauri 历史面板需要的查询详情、重跑任务构造和一键重跑。
- 已新增历史清理第一版：`hgripe-api-history cleanup` 支持按最新保留条数、时间、provider、operation、status、是否有输出文件筛选清理；默认 dry-run，只有 `--apply` 才会改 SQLite/JSONL，输出文件需要额外 `--delete-output-files` 才会删除。
- 已新增本地输出根目录约定：默认 `user/hgripe/outputs`，也支持 `HGRIPE_OUTPUT_DIR` 指定其他目录。
- `openai_compatible image.generate` 已支持输出落盘：`b64_json` 图片可直接保存，`url` 图片可通过 `download_url_outputs` 下载保存，并写入 `output_files` 和 `images[*].local_path`。

当前验证命令：

```powershell
cargo test -p hgripe-api
cargo build -p hgripe-api --bins
.\.venv\Scripts\python.exe python\bridge\mock_task_example.py
.\.venv\Scripts\python.exe python\bridge\custom_http_example.py
.\.venv\Scripts\python.exe python\bridge\openai_compatible_text_example.py
.\.venv\Scripts\python.exe python\bridge\openai_compatible_image_node_example.py
.\.venv\Scripts\python.exe python\bridge\openai_compatible_vision_node_example.py
.\.venv\Scripts\python.exe python\bridge\openai_compatible_credentials_ref_example.py
.\.venv\Scripts\python.exe python\bridge\openai_compatible_profile_example.py
.\.venv\Scripts\python.exe python\bridge\history_tail_example.py
.\.venv\Scripts\python.exe python\bridge\history_tail_example.py --provider openai_compatible --limit 10
.\.venv\Scripts\python.exe python\bridge\history_tail_example.py --operation image.generate --has-output-files yes
.\.venv\Scripts\python.exe python\bridge\history_rerun_example.py <task_id>
.\target\debug\hgripe-api-history.exe list --limit 10
.\target\debug\hgripe-api-history.exe show <task_id>
.\target\debug\hgripe-api-history.exe rerun-task <task_id>
.\target\debug\hgripe-api-history.exe rerun <task_id>
.\target\debug\hgripe-api-history.exe cleanup --keep-latest 100
.\target\debug\hgripe-api-history.exe cleanup --keep-latest 100 --apply
```

下一步：

- 后续把 credential ref 从本地 JSON 文件升级到 Tauri/系统 keychain，并把 provider profile 管理接入桌面设置页。
- 把历史列表、详情、重跑和清理接入 Tauri 桌面 UI。
- 把输出落盘能力扩展到通用下载、视频、音频和 Image Edit 节点。
- 把 ComfyUI 节点继续扩展为更多常用 API 专用节点，例如 OpenAI-compatible Image Edit、Video、Audio。

### Phase 2: Tauri Shell

- 新建 Tauri 桌面端。
- 管理本地服务启动、日志、设置和输出目录。
- WebView 打开现有 UI 或简化版本地页面。
- API key 改由桌面端统一管理。

### Phase 3: Workflow Runtime

- Rust 接管 API 工作流执行。
- 统一节点输入输出类型。
- 引入 SQLite 历史记录和缓存索引。
- 支持批量运行、暂停、取消、恢复。

### Phase 4: Lightweight Node Editor

- 做自己的轻量节点编辑器。
- 只支持 API-first 工作流需要的节点和数据类型。
- 保留导入部分 ComfyUI workflow 的能力。
- 逐步减少对完整 ComfyUI 的依赖。

## 推荐目录结构

```text
apps/
  desktop-tauri/

crates/
  hgripe-api/
  hgripe-runtime/
  hgripe-workflow/
  hgripe-storage/

python/
  nodes/
  bridge/

docs/
  providers/
  workflow-format/
```

## 判断标准

每个新功能都按这些问题判断是否值得做：

- 是否让个人使用更舒服？
- 是否减少重复操作？
- 是否能让 API 接入更统一？
- 是否能提高任务失败后的恢复能力？
- 是否能减少对完整 ComfyUI 的无关依赖？
- 是否能长期维护，而不是只解决一次性问题？

## 总结

H-Gripe 最有价值的方向是成为一个个人 API 工作流桌面端：Tauri 负责体验，Rust 负责稳定的任务执行和基础设施，节点负责组合能力。

这条路线不追求和完整 ComfyUI 做同一件事，而是保留节点式创作的优点，把重点放在云端/API 模型的统一调度、缓存、自动化和个人高频工作流上。
