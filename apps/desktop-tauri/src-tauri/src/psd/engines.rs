//! Cross-card engine capability probe (the `doctor`-style report): which opt-in
//! ML `engine` values each card CLI can run on this box, plus the machine
//! device probe. Split out of `psd.rs`; command names and result shapes are
//! unchanged.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{apply_model_env, project_python, resolve_project_dir};
/// Cached-weight inventory for one engine: the non-bundled weight it would load
/// and whether it is already present on this box. Lets the UI show what is
/// downloaded vs still missing instead of only "engine unavailable". A directory
/// weight (e.g. a diffusers snapshot) reports `present` with `size_mb` null.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct WeightInfo {
    #[serde(default)]
    pub(crate) path: String,
    #[serde(default)]
    pub(crate) present: bool,
    #[serde(default)]
    pub(crate) size_mb: Option<u64>,
}

/// Availability of one `engine` option for a card, as reported by a CLI
/// `--probe-engines` call. `available=false` carries a human `reason` the UI
/// shows when greying the option out.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct EngineAvailability {
    #[serde(default)]
    pub(crate) available: bool,
    #[serde(default)]
    pub(crate) reason: String,
    /// Whether this engine is GPU-capable (an ML backend). The UI pairs it with
    /// the machine [`DeviceProbe`] to warn it would fall back to CPU when no
    /// CUDA device is present; the CPU/`rules`/`provider` baseline is `false`.
    #[serde(default)]
    pub(crate) accelerated: bool,
    /// Cached-weight inventory for this engine (`None` for the CPU/`rules`/
    /// `provider` baseline, which loads no downloadable weight).
    #[serde(default)]
    pub(crate) weight: Option<WeightInfo>,
}

/// Engine capability probe for one card (node kind): which `engine` values its
/// CLI can actually run right now. `error` is set (engines empty) when the probe
/// itself could not run, so the UI degrades to "all enabled" rather than hiding
/// the always-available CPU path.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct CardEngineProbe {
    /// The node kind whose `engine` param these cover (e.g. `imageEnhance`).
    pub(crate) node_kind: String,
    /// The bridge CLI that produced the probe.
    pub(crate) cli: String,
    /// Engine id -> availability (e.g. `cpu`/`realesrgan`, `rules`/`onnx_defect`).
    pub(crate) engines: BTreeMap<String, EngineAvailability>,
    /// Why the probe could not run, when `engines` is empty.
    #[serde(default)]
    pub(crate) error: Option<String>,
}

/// One CUDA device reported by the machine device probe.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct DeviceInfo {
    #[serde(default)]
    pub(crate) index: u32,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) total_memory_mb: u64,
}

/// `torch` presence + CUDA flag from the device probe (filled when importable).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct TorchInfo {
    #[serde(default)]
    pub(crate) installed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cuda: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
}

/// `onnxruntime` presence + the execution providers available on this box.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct OnnxRuntimeInfo {
    #[serde(default)]
    pub(crate) installed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) version: Option<String>,
    #[serde(default)]
    pub(crate) providers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
}

/// Machine compute capability (which accelerator the opt-in GPU engines would
/// actually run on): CUDA device names / VRAM via `torch` and the ONNX Runtime
/// execution providers. The per-card probes only say *which* engines could run;
/// this says *where*, so the UI can warn that a GPU engine falls back to CPU on
/// a box with no CUDA device. Machine-global, so it is probed once.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct DeviceProbe {
    #[serde(default)]
    pub(crate) cuda_available: bool,
    #[serde(default)]
    pub(crate) devices: Vec<DeviceInfo>,
    #[serde(default)]
    pub(crate) torch: TorchInfo,
    #[serde(default)]
    pub(crate) onnxruntime: OnnxRuntimeInfo,
}

/// Cross-card engine capability report (the `doctor`-style probe). Aggregates
/// every local card that exposes an opt-in ML `engine` seam so the UI can grey
/// out engines whose deps/weights are missing on this box.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct EngineProbeReport {
    pub(crate) cards: Vec<CardEngineProbe>,
    /// The shared weight cache (`HGRIPE_MODEL_CACHE` or the bundled dir).
    #[serde(default)]
    pub(crate) model_cache_dir: Option<String>,
    /// Machine compute capability (CUDA devices / ONNX Runtime providers),
    /// probed once; `None` when the device probe itself could not run.
    #[serde(default)]
    pub(crate) runtime: Option<DeviceProbe>,
}

/// Shape of a single CLI's `--probe-engines` JSON. `engines` carries extra
/// per-engine fields (e.g. `native_scale`) that the UI does not need, so we
/// only pull `available` + `reason`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CliEngineProbe {
    #[serde(default)]
    pub(crate) engines: BTreeMap<String, EngineAvailability>,
    #[serde(default)]
    pub(crate) model_cache_dir: Option<String>,
}

/// Run the one-shot device probe CLI and parse its machine-capability JSON.
/// Unlike the per-card probes this is the same for every card, so it is run
/// once; a failure leaves `runtime` `None` rather than failing the report.
fn run_device_probe(python: &Path, dir: &Path) -> Result<DeviceProbe, String> {
    let script = dir
        .join("python")
        .join("bridge")
        .join("device_probe_cli.py");
    if !script.is_file() {
        return Err(format!(
            "device_probe_cli.py not found at {}",
            script.display()
        ));
    }
    let mut cmd = std::process::Command::new(python);
    cmd.arg(&script).current_dir(dir);
    apply_model_env(&mut cmd);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: don't pop a console window for the child.
        cmd.creation_flags(0x0800_0000);
    }
    let output = cmd
        .output()
        .map_err(|err| format!("failed to launch {}: {err}", python.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("device probe failed: {}", stderr.trim()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str::<DeviceProbe>(stdout.trim()).map_err(|err| {
        format!(
            "could not parse device probe: {err} (raw: {})",
            stdout.trim()
        )
    })
}

/// Run one card CLI's `--probe-engines` and parse its JSON.
fn run_engine_probe(python: &Path, dir: &Path, cli_name: &str) -> Result<CliEngineProbe, String> {
    let script = dir.join("python").join("bridge").join(cli_name);
    if !script.is_file() {
        return Err(format!("{cli_name} not found at {}", script.display()));
    }
    let mut cmd = std::process::Command::new(python);
    // Every card CLI requires `--image`; the probe short-circuits before the
    // image is read, so a placeholder satisfies argparse without touching disk.
    cmd.arg(&script)
        .arg("--image")
        .arg("__engine_probe__")
        .arg("--probe-engines")
        .current_dir(dir);
    apply_model_env(&mut cmd);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: don't pop a console window for the child.
        cmd.creation_flags(0x0800_0000);
    }
    let output = cmd
        .output()
        .map_err(|err| format!("failed to launch {}: {err}", python.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "{cli_name} --probe-engines failed: {}",
            stderr.trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str::<CliEngineProbe>(stdout.trim()).map_err(|err| {
        format!(
            "could not parse {cli_name} probe: {err} (raw: {})",
            stdout.trim()
        )
    })
}

/// Probe the opt-in ML `engine` seams across the local cards (the `doctor`
/// cross-card capability report). The CPU/rule baseline is always available; a
/// learned engine reports `available=false` with a reason when its optional
/// dependency or weight is missing, which the inspector uses to grey out the
/// option (and fall back to the baseline). A card whose probe fails to run
/// returns an `error` and no engines, so the UI leaves its select untouched.
#[tauri::command]
pub(crate) fn probe_engines(dir: Option<String>) -> Result<EngineProbeReport, String> {
    let dir = resolve_project_dir(&dir)?;
    let python = project_python(&dir);

    // (node kind, CLI) for every card that exposes an `engine` param.
    const CARDS: [(&str, &str); 5] = [
        ("matchLightColor", "color_match_cli.py"),
        ("imageEnhance", "image_enhance_cli.py"),
        ("detailWatchdog", "detail_watchdog_cli.py"),
        ("detailRepaint", "detail_repaint_cli.py"),
        ("refineMaskEdge", "edge_refine_cli.py"),
    ];

    let mut cards = Vec::with_capacity(CARDS.len());
    let mut model_cache_dir = None;
    for (node_kind, cli_name) in CARDS {
        let probe = run_engine_probe(&python, &dir, cli_name);
        let card = match probe {
            Ok(parsed) => {
                if model_cache_dir.is_none() {
                    model_cache_dir = parsed.model_cache_dir.clone();
                }
                CardEngineProbe {
                    node_kind: node_kind.to_string(),
                    cli: cli_name.to_string(),
                    engines: parsed.engines,
                    error: None,
                }
            }
            Err(err) => CardEngineProbe {
                node_kind: node_kind.to_string(),
                cli: cli_name.to_string(),
                engines: BTreeMap::new(),
                error: Some(err),
            },
        };
        cards.push(card);
    }

    let runtime = run_device_probe(&python, &dir).ok();

    Ok(EngineProbeReport {
        cards,
        model_cache_dir,
        runtime,
    })
}
