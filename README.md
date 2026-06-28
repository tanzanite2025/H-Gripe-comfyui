<div align="center">

# H-Gripe ComfyUI
**A ComfyUI-based source branch moving toward an API-first Rust-backed desktop workflow.**

<img width="1590" height="795" alt="ComfyUI Screenshot" src="https://github.com/user-attachments/assets/36e065e0-bfae-4456-8c7f-8369d5ea48a2" />
<br>
</div>

H-Gripe ComfyUI is an independent source branch based on ComfyUI. The project keeps the existing ComfyUI user experience and node workflow behavior, while the long-term direction is to move API task execution, provider calls, caching, queueing, file handling, and desktop orchestration into Rust/Tauri. This repository is maintained independently and is not configured to automatically follow upstream update history.

## Project Direction

- Keep Python as the compatibility layer for ComfyUI nodes, UI/API integration, custom node loading, and PyTorch model execution.
- Introduce Rust for API broker infrastructure, provider adapters, retry/caching, task state, and later Tauri desktop integration.
- Start with small, reversible Rust broker modules called from Python, keeping the existing ComfyUI path available while the migration is in progress.
- Avoid changing user-facing workflow behavior while replacing internal execution pieces step by step.

## Rust Migration Targets

Initial Rust rewrite targets:

- Graph dependency analysis and topological scheduling in `comfy_execution/graph.py`.
- Execution queue coordination and ready-node selection in `execution.py`.
- Cache key generation, input signatures, and cache lifecycle helpers in `comfy_execution/caching.py`.
- Stable data boundaries between Python node objects and Rust execution primitives.

Not planned for the first stage:

- Rewriting PyTorch tensor/model inference.
- Replacing the ComfyUI frontend.
- Breaking existing custom node compatibility.

## API-First Broker Prototype

The current prototype adds a Rust API broker plus thin Python/ComfyUI bridge nodes:

- Rust workspace: `Cargo.toml`
- API broker crate: `crates/hgripe-api`
- Python bridge examples: `python/bridge`
- ComfyUI nodes: `custom_nodes/hgripe_api_nodes.py`
- Credential ref example: `docs/credentials.example.json`
- Provider profile example: `docs/provider_profiles.example.json`

Credential refs keep API keys out of workflow files. `openai_compatible` and `custom_http` tasks/nodes can use them. The default local credential file is ignored by git:

```text
user/hgripe/credentials.json
```

You can also point to another file with `HGRIPE_CREDENTIALS_FILE`.

Provider profiles keep non-secret provider defaults out of workflow files. The default local profile file is ignored by git:

```text
user/hgripe/provider_profiles.json
```

Profiles can define defaults such as `base_url`, `model`, `credentials_ref`, `no_auth`, headers, `params`, and `extra_body`. Use `profile_ref` on OpenAI-compatible tasks/nodes to load one. You can also point to another file with `HGRIPE_PROVIDER_PROFILES_FILE` or task param `profiles_file`.

Task history is recorded locally as JSONL and indexed into SQLite for UI/query use:

```text
user/hgripe/history/tasks.jsonl
user/hgripe/history/tasks.sqlite3
```

New history records also store a sanitized `task_snapshot` so a task can be rerun later without keeping inline API keys, tokens, passwords, or Authorization headers in history. Older records created before this field exists are still readable, but they are not rerunnable.

Generated/downloaded API outputs should use the local output root:

```text
user/hgripe/outputs
```

`openai_compatible image.generate` can save `b64_json` and downloaded `url` image outputs there and return those paths through `output_files`.
`openai_compatible audio.speech` saves generated audio bytes there by default and returns the local audio file through `output_files`.
`openai_compatible audio.transcriptions` and `audio.translations` upload local audio files with multipart requests and return extracted text through `output_json.text`.
`custom_http` can also save raw successful response bytes when `save_response=true`, which is useful for API endpoints that directly return images, audio, video, PDFs, or other files.
`custom_http` supports multipart form fields and local file uploads for APIs that accept images, audio, video, PDFs, or dataset files.
`custom_http async_job` can submit an async API job, poll a status endpoint, and download a final result URL into `output_files`.
`custom_http` can use `credentials_ref` for `base_url`, bearer API keys, env-based API keys, and secret/non-secret headers, keeping them out of workflow JSON.

Useful environment overrides:

```powershell
$env:HGRIPE_HISTORY_FILE="C:\path\to\tasks.jsonl"
$env:HGRIPE_HISTORY_DB="C:\path\to\tasks.sqlite3"
$env:HGRIPE_OUTPUT_DIR="C:\path\to\outputs"
$env:HGRIPE_HISTORY_DISABLED="1"
$env:HGRIPE_PROVIDER_PROFILES_FILE="C:\path\to\provider_profiles.json"
$env:HGRIPE_CUSTOM_HTTP_BASE_URL="https://api.example.com"
$env:HGRIPE_CUSTOM_HTTP_API_KEY="..."
```

Local verification:

```powershell
cargo test -p hgripe-api
cargo build -p hgripe-api --bins
.\.venv\Scripts\python.exe python\bridge\mock_task_example.py
.\.venv\Scripts\python.exe python\bridge\custom_http_example.py
.\.venv\Scripts\python.exe python\bridge\custom_http_binary_output_example.py
.\.venv\Scripts\python.exe python\bridge\custom_http_credentials_ref_example.py
.\.venv\Scripts\python.exe python\bridge\custom_http_multipart_example.py
.\.venv\Scripts\python.exe python\bridge\custom_http_async_job_example.py
.\.venv\Scripts\python.exe python\bridge\openai_compatible_text_example.py
.\.venv\Scripts\python.exe python\bridge\openai_compatible_image_node_example.py
.\.venv\Scripts\python.exe python\bridge\openai_compatible_image_edit_node_example.py
.\.venv\Scripts\python.exe python\bridge\openai_compatible_audio_speech_node_example.py
.\.venv\Scripts\python.exe python\bridge\openai_compatible_audio_text_node_example.py
.\.venv\Scripts\python.exe python\bridge\openai_compatible_vision_node_example.py
.\.venv\Scripts\python.exe python\bridge\openai_compatible_credentials_ref_example.py
.\.venv\Scripts\python.exe python\bridge\openai_compatible_profile_example.py
.\.venv\Scripts\python.exe python\bridge\history_tail_example.py
.\.venv\Scripts\python.exe python\bridge\history_tail_example.py --provider openai_compatible --limit 10
.\.venv\Scripts\python.exe python\bridge\history_tail_example.py --operation image.generate --has-output-files yes
.\.venv\Scripts\python.exe python\bridge\history_rerun_example.py <task_id>
.\target\debug\hgripe-api-config.exe init --dry-run
.\target\debug\hgripe-api-config.exe init
.\target\debug\hgripe-api-config.exe doctor
.\target\debug\hgripe-api-config.exe profiles list
.\target\debug\hgripe-api-config.exe profiles show <profile_ref>
.\target\debug\hgripe-api-config.exe profiles resolve <profile_ref>
.\target\debug\hgripe-api-config.exe profiles validate
.\target\debug\hgripe-api-config.exe credentials list
.\target\debug\hgripe-api-config.exe credentials show <credential_ref>
.\target\debug\hgripe-api-config.exe credentials validate
.\target\debug\hgripe-api-history.exe list --limit 10
.\target\debug\hgripe-api-history.exe show <task_id>
.\target\debug\hgripe-api-history.exe rerun-task <task_id>
.\target\debug\hgripe-api-history.exe rerun <task_id>
.\target\debug\hgripe-api-history.exe cleanup --keep-latest 100
.\target\debug\hgripe-api-history.exe cleanup --keep-latest 100 --apply
```

`hgripe-api-history cleanup` defaults to dry-run. It only changes SQLite/JSONL history when `--apply` is provided. Output files are preserved unless `--delete-output-files` is also provided.

`hgripe-api-config credentials show` redacts inline API keys and secret-like headers before printing JSON.
`hgripe-api-config profiles resolve` previews a profile's effective provider settings without printing API keys or header values.
`hgripe-api-config doctor` summarizes config validation, profile-to-credential references, runtime paths, broker location, and H-Gripe env overrides without printing secret values.
`hgripe-api-config init` creates local config/history/output directories and starter credentials/profile templates. Existing files are preserved unless `--force` is provided.

ComfyUI is the AI creation engine for visual professionals who demand control over every model, every parameter, and every output. Its powerful and modular node graph interface empowers creatives to generate images, videos, 3D models, audio, and more...
- ComfyUI natively supports the latest open-source state of the art models.
- API nodes provide access to the best closed source models such as Nano Banana, Seedance, Hunyuan3D, etc.
- The most sophisticated workflows can be exposed through a simple UI thanks to App Mode.
- It integrates seamlessly into production pipelines with our API endpoints.

## Features
- Nodes/graph/flowchart interface to experiment and create complex Stable Diffusion workflows without needing to code anything.
- NOTE: There are many more models supported than the list below, if you want to see what is supported see our templates list inside ComfyUI.
- Image Models
   - SD1.x, SD2.x ([unCLIP](https://comfyanonymous.github.io/ComfyUI_examples/unclip/))
   - [SDXL](https://comfyanonymous.github.io/ComfyUI_examples/sdxl/), [SDXL Turbo](https://comfyanonymous.github.io/ComfyUI_examples/sdturbo/)
   - [Stable Cascade](https://comfyanonymous.github.io/ComfyUI_examples/stable_cascade/)
   - [SD3 and SD3.5](https://comfyanonymous.github.io/ComfyUI_examples/sd3/)
   - Pixart Alpha and Sigma
   - [AuraFlow](https://comfyanonymous.github.io/ComfyUI_examples/aura_flow/)
   - [HunyuanDiT](https://comfyanonymous.github.io/ComfyUI_examples/hunyuan_dit/)
   - [Flux](https://comfyanonymous.github.io/ComfyUI_examples/flux/)
   - [Lumina Image 2.0](https://comfyanonymous.github.io/ComfyUI_examples/lumina2/)
   - [HiDream](https://comfyanonymous.github.io/ComfyUI_examples/hidream/)
   - [Qwen Image](https://comfyanonymous.github.io/ComfyUI_examples/qwen_image/)
   - [Hunyuan Image 2.1](https://comfyanonymous.github.io/ComfyUI_examples/hunyuan_image/)
   - [Flux 2](https://comfyanonymous.github.io/ComfyUI_examples/flux2/)
   - [Z Image](https://comfyanonymous.github.io/ComfyUI_examples/z_image/)
   - Ernie Image
- Image Editing Models
   - [Omnigen 2](https://comfyanonymous.github.io/ComfyUI_examples/omnigen/)
   - [Flux Kontext](https://comfyanonymous.github.io/ComfyUI_examples/flux/#flux-kontext-image-editing-model)
   - [HiDream E1.1](https://comfyanonymous.github.io/ComfyUI_examples/hidream/#hidream-e11)
   - [Qwen Image Edit](https://comfyanonymous.github.io/ComfyUI_examples/qwen_image/#edit-model)
- Video Models
   - [Stable Video Diffusion](https://comfyanonymous.github.io/ComfyUI_examples/video/)
   - [Mochi](https://comfyanonymous.github.io/ComfyUI_examples/mochi/)
   - [LTX-Video](https://comfyanonymous.github.io/ComfyUI_examples/ltxv/)
   - [Hunyuan Video](https://comfyanonymous.github.io/ComfyUI_examples/hunyuan_video/)
   - [Wan 2.1](https://comfyanonymous.github.io/ComfyUI_examples/wan/)
   - [Wan 2.2](https://comfyanonymous.github.io/ComfyUI_examples/wan22/)
   - [Hunyuan Video 1.5](https://docs.comfy.org/tutorials/video/hunyuan/hunyuan-video-1-5)
- Audio Models
   - [Stable Audio](https://comfyanonymous.github.io/ComfyUI_examples/audio/)
   - [ACE Step](https://comfyanonymous.github.io/ComfyUI_examples/audio/)
- 3D Models
   - [Hunyuan3D 2.0](https://docs.comfy.org/tutorials/3d/hunyuan3D-2)
- Asynchronous Queue system
- Many optimizations: Only re-executes the parts of the workflow that changes between executions.
- Smart memory management: can automatically run large models on GPUs with as low as 1GB vram with smart offloading.
- Works even if you don't have a GPU with: ```--cpu``` (slow)
- Can load ckpt and safetensors: All in one checkpoints or standalone diffusion models, VAEs and CLIP models.
- Safe loading of ckpt, pt, pth, etc.. files.
- Embeddings/Textual inversion
- [Loras (regular, locon and loha)](https://comfyanonymous.github.io/ComfyUI_examples/lora/)
- [Hypernetworks](https://comfyanonymous.github.io/ComfyUI_examples/hypernetworks/)
- Loading full workflows (with seeds) from generated PNG, WebP and FLAC files.
- Saving/Loading workflows as Json files.
- Nodes interface can be used to create complex workflows like one for [Hires fix](https://comfyanonymous.github.io/ComfyUI_examples/2_pass_txt2img/) or much more advanced ones.
- [Area Composition](https://comfyanonymous.github.io/ComfyUI_examples/area_composition/)
- [Inpainting](https://comfyanonymous.github.io/ComfyUI_examples/inpaint/) with both regular and inpainting models.
- [ControlNet and T2I-Adapter](https://comfyanonymous.github.io/ComfyUI_examples/controlnet/)
- [Upscale Models (ESRGAN, ESRGAN variants, SwinIR, Swin2SR, etc...)](https://comfyanonymous.github.io/ComfyUI_examples/upscale_models/)
- [GLIGEN](https://comfyanonymous.github.io/ComfyUI_examples/gligen/)
- [Model Merging](https://comfyanonymous.github.io/ComfyUI_examples/model_merging/)
- [LCM models and Loras](https://comfyanonymous.github.io/ComfyUI_examples/lcm/)
- Latent previews with [TAESD](#how-to-show-high-quality-previews)
- Works fully offline: core will never download anything unless you want to.
- Optional API nodes to use paid models from external providers through the online [Comfy API](https://docs.comfy.org/tutorials/api-nodes/overview) disable with: `--disable-api-nodes`
- [Config file](extra_model_paths.yaml.example) to set the search paths for models.

Workflow examples can be found on the [Examples page](https://comfyanonymous.github.io/ComfyUI_examples/)

## Shortcuts

| Keybind                            | Explanation                                                                                                        |
|------------------------------------|--------------------------------------------------------------------------------------------------------------------|
| `Ctrl` + `Enter`                      | Queue up current graph for generation                                                                              |
| `Ctrl` + `Shift` + `Enter`              | Queue up current graph as first for generation                                                                     |
| `Ctrl` + `Alt` + `Enter`                | Cancel current generation                                                                                          |
| `Ctrl` + `Z`/`Ctrl` + `Y`                 | Undo/Redo                                                                                                          |
| `Ctrl` + `S`                          | Save workflow                                                                                                      |
| `Ctrl` + `O`                          | Load workflow                                                                                                      |
| `Ctrl` + `A`                          | Select all nodes                                                                                                   |
| `Alt `+ `C`                           | Collapse/uncollapse selected nodes                                                                                 |
| `Ctrl` + `M`                          | Mute/unmute selected nodes                                                                                         |
| `Ctrl` + `B`                           | Bypass selected nodes (acts like the node was removed from the graph and the wires reconnected through)            |
| `Delete`/`Backspace`                   | Delete selected nodes                                                                                              |
| `Ctrl` + `Backspace`                   | Delete the current graph                                                                                           |
| `Space`                              | Move the canvas around when held and moving the cursor                                                             |
| `Ctrl`/`Shift` + `Click`                 | Add clicked node to selection                                                                                      |
| `Ctrl` + `C`/`Ctrl` + `V`                  | Copy and paste selected nodes (without maintaining connections to outputs of unselected nodes)                     |
| `Ctrl` + `C`/`Ctrl` + `Shift` + `V`          | Copy and paste selected nodes (maintaining connections from outputs of unselected nodes to inputs of pasted nodes) |
| `Shift` + `Drag`                       | Move multiple selected nodes at the same time                                                                      |
| `Ctrl` + `D`                           | Load default graph                                                                                                 |
| `Alt` + `+`                          | Canvas Zoom in                                                                                                     |
| `Alt` + `-`                          | Canvas Zoom out                                                                                                    |
| `Ctrl` + `Shift` + LMB + Vertical drag | Canvas Zoom in/out                                                                                                 |
| `P`                                  | Pin/Unpin selected nodes                                                                                           |
| `Ctrl` + `G`                           | Group selected nodes                                                                                               |
| `Q`                                 | Toggle visibility of the queue                                                                                     |
| `H`                                  | Toggle visibility of history                                                                                       |
| `R`                                  | Refresh graph                                                                                                      |
| `F`                                  | Show/Hide menu                                                                                                      |
| `.`                                  | Fit view to selection (Whole graph when nothing is selected)                                                        |
| Double-Click LMB                   | Open node quick search palette                                                                                     |
| `Shift` + Drag                       | Move multiple wires at once                                                                                        |
| `Ctrl` + `Alt` + LMB                   | Disconnect all wires from clicked slot                                                                             |

`Ctrl` can also be replaced with `Cmd` instead for macOS users

# Installing

## Windows Local Development

Use the project virtual environment directly so packages do not accidentally install into the global Python interpreter.

```powershell
cd C:\Users\P16V\Desktop\Github\H-Gripe-comfyui

py -3.10 -m venv .venv
.\.venv\Scripts\python.exe -m pip install --upgrade pip
```

For the local NVIDIA RTX 2000 Ada 8GB setup, install the CUDA PyTorch build:

```powershell
.\.venv\Scripts\python.exe -m pip install torch torchvision torchaudio --index-url https://download.pytorch.org/whl/cu128
```

This repository may have an empty `requirements.txt` while H-Gripe is being split from upstream. Until a curated dependency file is committed, install the upstream ComfyUI dependencies into the virtual environment:

```powershell
Invoke-WebRequest https://raw.githubusercontent.com/comfyanonymous/ComfyUI/master/requirements.txt -OutFile requirements.upstream.txt
.\.venv\Scripts\python.exe -m pip install -r requirements.upstream.txt
```

Verify CUDA:

```powershell
.\.venv\Scripts\python.exe -c "import torch; print(torch.__version__); print(torch.cuda.is_available()); print(torch.cuda.get_device_name(0) if torch.cuda.is_available() else 'NO CUDA')"
```

Port `8188` can be occupied by local proxy tools such as `verge-mihomo`. Use `8199` for local development:

```powershell
.\.venv\Scripts\python.exe main.py --listen 127.0.0.1 --port 8199 --preview-method auto --verbose INFO
```

Open:

```text
http://127.0.0.1:8199
```

For small local GPU models on an 8GB card, start with low VRAM mode:

```powershell
.\.venv\Scripts\python.exe main.py --listen 127.0.0.1 --port 8199 --lowvram --preview-method auto
```

For a quick startup check:

```powershell
.\.venv\Scripts\python.exe main.py --quick-test-for-ci --cpu --disable-all-custom-nodes --disable-api-nodes
```

## Manual Install (Windows, Linux)

Python 3.14 works but some custom nodes may have issues. The free threaded variant works but some dependencies will enable the GIL so it's not fully supported.

Python 3.13 is very well supported. If you have trouble with some custom node dependencies on 3.13 you can try 3.12

torch 2.4 and above is supported but some features and optimizations might only work on newer versions. We generally recommend using the latest major version of pytorch with the latest cuda version unless it is less than 2 weeks old.

### Instructions:

Git clone this repo.

Put your SD checkpoints (the huge ckpt/safetensors files) in: models/checkpoints

Put your VAE in: models/vae


### AMD GPUs (Linux)

AMD users can install rocm and pytorch with pip if you don't have it already installed, this is the command to install the stable version:

```pip install torch torchvision torchaudio --index-url https://download.pytorch.org/whl/rocm7.2```

This is the command to install the nightly with ROCm 7.2 which might have some performance improvements:

```pip install --pre torch torchvision torchaudio --index-url https://download.pytorch.org/whl/nightly/rocm7.2```


### AMD GPUs (Experimental: Windows and Linux), RDNA 3, 3.5 and 4 only.

These have less hardware support than the builds above but they work on windows. You also need to install the pytorch version specific to your hardware.

RDNA 3 (RX 7000 series):

```pip install --pre torch torchvision torchaudio --index-url https://rocm.nightlies.amd.com/v2/gfx110X-all/```

RDNA 3.5 (Strix halo/Ryzen AI Max+ 365):

```pip install --pre torch torchvision torchaudio --index-url https://rocm.nightlies.amd.com/v2/gfx1151/```

RDNA 4 (RX 9000 series):

```pip install --pre torch torchvision torchaudio --index-url https://rocm.nightlies.amd.com/v2/gfx120X-all/```

### Intel GPUs (Windows and Linux)

Intel Arc GPU users can install native PyTorch with torch.xpu support using pip. More information can be found [here](https://pytorch.org/docs/main/notes/get_start_xpu.html)

1. To install PyTorch xpu, use the following command:

```pip install torch torchvision torchaudio --index-url https://download.pytorch.org/whl/xpu```

This is the command to install the Pytorch xpu nightly which might have some performance improvements:

```pip install --pre torch torchvision torchaudio --index-url https://download.pytorch.org/whl/nightly/xpu```

### NVIDIA

Nvidia users should install stable pytorch using this command:

```pip install torch torchvision torchaudio --extra-index-url https://download.pytorch.org/whl/cu130```

This is the command to install pytorch nightly instead which might have performance improvements.

```pip install --pre torch torchvision torchaudio --index-url https://download.pytorch.org/whl/nightly/cu132```

#### Troubleshooting

If you get the "Torch not compiled with CUDA enabled" error, uninstall torch with:

```pip uninstall torch```

And install it again with the command above.

### Dependencies

Install the dependencies by opening your terminal inside the ComfyUI folder and:

```pip install -r requirements.txt```

After this you should have everything installed and can proceed to running ComfyUI.

### Others:

#### Apple Mac silicon

You can install ComfyUI in Apple Mac silicon (M1, M2, M3 or M4) with any recent macOS version.

1. Install pytorch nightly. For instructions, read the [Accelerated PyTorch training on Mac](https://developer.apple.com/metal/pytorch/) Apple Developer guide (make sure to install the latest pytorch nightly).
1. Follow the [ComfyUI manual installation](#manual-install-windows-linux) instructions for Windows and Linux.
1. Install the ComfyUI [dependencies](#dependencies). If you have another Stable Diffusion UI [you might be able to reuse the dependencies](#i-already-have-another-ui-for-stable-diffusion-installed-do-i-really-have-to-install-all-of-these-dependencies).
1. Launch ComfyUI by running `python main.py`

> **Note**: Remember to add your models, VAE, LoRAs etc. to the corresponding Comfy folders, as discussed in [ComfyUI manual installation](#manual-install-windows-linux).

#### Ascend NPUs

For models compatible with Ascend Extension for PyTorch (torch_npu). To get started, ensure your environment meets the prerequisites outlined on the [installation](https://ascend.github.io/docs/sources/ascend/quick_install.html) page. Here's a step-by-step guide tailored to your platform and installation method:

1. Begin by installing the recommended or newer kernel version for Linux as specified in the Installation page of torch-npu, if necessary.
2. Proceed with the installation of Ascend Basekit, which includes the driver, firmware, and CANN, following the instructions provided for your specific platform.
3. Next, install the necessary packages for torch-npu by adhering to the platform-specific instructions on the [Installation](https://ascend.github.io/docs/sources/pytorch/install.html#pytorch) page.
4. Finally, adhere to the [ComfyUI manual installation](#manual-install-windows-linux) guide for Linux. Once all components are installed, you can run ComfyUI as described earlier.

#### Cambricon MLUs

For models compatible with Cambricon Extension for PyTorch (torch_mlu). Here's a step-by-step guide tailored to your platform and installation method:

1. Install the Cambricon CNToolkit by adhering to the platform-specific instructions on the [Installation](https://www.cambricon.com/docs/sdk_1.15.0/cntoolkit_3.7.2/cntoolkit_install_3.7.2/index.html)
2. Next, install the PyTorch(torch_mlu) following the instructions on the [Installation](https://www.cambricon.com/docs/sdk_1.15.0/cambricon_pytorch_1.17.0/user_guide_1.9/index.html)
3. Launch ComfyUI by running `python main.py`

#### Iluvatar Corex

For models compatible with Iluvatar Extension for PyTorch. Here's a step-by-step guide tailored to your platform and installation method:

1. Install the Iluvatar Corex Toolkit by adhering to the platform-specific instructions on the [Installation](https://support.iluvatar.com/#/DocumentCentre?id=1&nameCenter=2&productId=520117912052801536)
2. Launch ComfyUI by running `python main.py`


## [ComfyUI-Manager](https://github.com/Comfy-Org/ComfyUI-Manager/tree/manager-v4)

**ComfyUI-Manager** is an extension that allows you to easily install, update, and manage custom nodes for ComfyUI.

### Setup

1. Install the manager dependencies:
   ```bash
   pip install -r manager_requirements.txt
   ```

2. Enable the manager with the `--enable-manager` flag when running ComfyUI:
   ```bash
   python main.py --enable-manager
   ```

### Command Line Options

| Flag | Description |
|------|-------------|
| `--enable-manager` | Enable ComfyUI-Manager |
| `--enable-manager-legacy-ui` | Use the legacy manager UI instead of the new UI (implies `--enable-manager`) |
| `--disable-manager-ui` | Disable the manager UI and endpoints while keeping background features like security checks and scheduled installation completion (requires `--enable-manager`) |


# Running

Recommended local development command:

```powershell
.\.venv\Scripts\python.exe main.py --listen 127.0.0.1 --port 8199 --preview-method auto --verbose INFO
```

Then open `http://127.0.0.1:8199`.

### For AMD cards not officially supported by ROCm

Try running it with this command if you have issues:

For 6700, 6600 and maybe other RDNA2 or older: ```HSA_OVERRIDE_GFX_VERSION=10.3.0 python main.py```

For AMD 7600 and maybe other RDNA3 cards: ```HSA_OVERRIDE_GFX_VERSION=11.0.0 python main.py```

### AMD ROCm Tips

You can try setting this env variable `PYTORCH_TUNABLEOP_ENABLED=1` which might speed things up at the cost of a very slow initial run.

# Notes

Only parts of the graph that have an output with all the correct inputs will be executed.

Only parts of the graph that change from each execution to the next will be executed, if you submit the same graph twice only the first will be executed. If you change the last part of the graph only the part you changed and the part that depends on it will be executed.

Dragging a generated png on the webpage or loading one will give you the full workflow including seeds that were used to create it.

You can use () to change emphasis of a word or phrase like: (good code:1.2) or (bad code:0.8). The default emphasis for () is 1.1. To use () characters in your actual prompt escape them like \\( or \\).

You can use {day|night}, for wildcard/dynamic prompts. With this syntax "{wild|card|test}" will be randomly replaced by either "wild", "card" or "test" by the frontend every time you queue the prompt. To use {} characters in your actual prompt escape them like: \\{ or \\}.

Dynamic prompts also support C-style comments, like `// comment` or `/* comment */`.

To use a textual inversion concepts/embeddings in a text prompt put them in the models/embeddings directory and use them in the CLIPTextEncode node like this (you can omit the .pt extension):

```embedding:embedding_filename.pt```


## How to show high-quality previews?

Use ```--preview-method auto``` to enable previews.

The default installation includes a fast latent preview method that's low-resolution. To enable higher-quality previews with [TAESD](https://github.com/madebyollin/taesd), download the [taesd_decoder.pth, taesdxl_decoder.pth, taesd3_decoder.pth and taef1_decoder.pth](https://github.com/madebyollin/taesd/) and place them in the `models/vae_approx` folder. Once they're installed, restart ComfyUI and launch it with `--preview-method taesd` to enable high-quality previews.

## How to use TLS/SSL?
Generate a self-signed certificate (not appropriate for shared/production use) and key by running the command: `openssl req -x509 -newkey rsa:4096 -keyout key.pem -out cert.pem -sha256 -days 3650 -nodes -subj "/C=XX/ST=StateName/L=CityName/O=CompanyName/OU=CompanySectionName/CN=CommonNameOrHostname"`

Use `--tls-keyfile key.pem --tls-certfile cert.pem` to enable TLS/SSL, the app will now be accessible with `https://...` instead of `http://...`.

> Note: Windows users can use [alexisrolland/docker-openssl](https://github.com/alexisrolland/docker-openssl) or one of the [3rd party binary distributions](https://wiki.openssl.org/index.php/Binaries) to run the command example above.
<br/><br/>If you use a container, note that the volume mount `-v` can be a relative path so `... -v ".\:/openssl-certs" ...` would create the key & cert files in the current directory of your command prompt or powershell terminal.

