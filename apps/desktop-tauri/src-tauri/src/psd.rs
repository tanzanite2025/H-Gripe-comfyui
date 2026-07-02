//! PSD & Python-bridge tooling for the desktop app: the shared project-root /
//! interpreter resolution and subprocess helpers, plus the per-domain command
//! submodules split out of this file (PSD compose/inspect/analyze, the local
//! card processors, the engine capability probe, and the detail-repaint
//! pipeline). Every submodule command is re-exported here so
//! `crate::psd::<command>` and the Tauri `invoke_handler` registrations stay
//! unchanged.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::studio::studio_reject_unsafe_basename;

mod cards;
mod compose;
mod engines;
mod model_paths;
mod repaint;

pub(crate) use cards::*;
pub(crate) use compose::*;
pub(crate) use engines::*;
pub(crate) use model_paths::*;
pub(crate) use repaint::*;
/// The Tauri resource directory captured at startup (see `set_resource_dir`).
/// When the installer bundles the `h-gripe.project.json` marker together with
/// the `python/bridge`, `custom_nodes` and `third_party` subtree under
/// `bundle.resources`, this directory *is* a self-contained project root, so the
/// PSD nodes keep working in a packaged build without the user pointing at a
/// separate source checkout.
static RESOURCE_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Record the bundled resource directory. Called once from the Tauri `setup`
/// hook; ignored if the resolver could not determine a resource path.
pub(crate) fn set_resource_dir(dir: Option<PathBuf>) {
    let _ = RESOURCE_DIR.set(dir);
}

fn resource_dir() -> Option<PathBuf> {
    RESOURCE_DIR.get().cloned().flatten()
}

/// A directory is an H-Gripe project root when it holds the explicit
/// `h-gripe.project.json` marker or the `python/bridge` runtime the PSD nodes
/// invoke. This intentionally no longer depends on ComfyUI's `main.py`, so the
/// ComfyUI main body can be removed without breaking the PSD nodes.
fn is_project_root(base: &Path) -> bool {
    base.join("h-gripe.project.json").is_file() || base.join("python").join("bridge").is_dir()
}

/// Accept `base` as the project root only if it looks like an H-Gripe project,
/// otherwise fail fast with an actionable message.
fn require_project_root(base: PathBuf) -> Result<PathBuf, String> {
    if is_project_root(&base) {
        Ok(base)
    } else {
        Err(format!(
            "not an H-Gripe project folder: {} (expected h-gripe.project.json or python/bridge; \
             set the project folder or HGRIPE_PROJECT_DIR)",
            base.display()
        ))
    }
}

/// Resolve the project directory that hosts the vendored `python/bridge`
/// helpers. Resolution order, first match wins:
///   1. the caller-provided path (the folder picked in the UI),
///   2. the `HGRIPE_PROJECT_DIR` environment variable (a packaging launcher can
///      point at the extracted project root without any UI),
///   3. the process working directory when it is a project root (the repo root
///      in dev),
///   4. the bundled Tauri resource directory (a packaged install).
///
/// Every branch requires an H-Gripe project root (`is_project_root`) so a
/// misconfigured folder fails fast.
pub(crate) fn resolve_project_dir(dir: &Option<String>) -> Result<PathBuf, String> {
    if let Some(d) = dir.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        return require_project_root(PathBuf::from(d));
    }
    if let Some(env_dir) = std::env::var_os("HGRIPE_PROJECT_DIR")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
    {
        return require_project_root(env_dir);
    }
    if let Ok(cwd) = std::env::current_dir() {
        if is_project_root(&cwd) {
            return Ok(cwd);
        }
    }
    if let Some(res) = resource_dir() {
        if is_project_root(&res) {
            return Ok(res);
        }
    }
    Err(
        "no H-Gripe project root found in the working directory or bundled resources \
         (expected h-gripe.project.json or python/bridge; set the project folder or \
         HGRIPE_PROJECT_DIR)"
            .to_string(),
    )
}

/// Pick a Python interpreter: prefer the portable `python_embeded` shipped in
/// the project root (the Windows embeddable layout), otherwise fall back to
/// PATH `python` / `python3`.
pub(crate) fn project_python(dir: &Path) -> PathBuf {
    for candidate in [
        dir.join("python_embeded").join("python.exe"),
        dir.join("python_embeded").join("python"),
    ] {
        if candidate.is_file() {
            return candidate;
        }
    }
    PathBuf::from(if cfg!(windows) { "python" } else { "python3" })
}

/// Validate a user-supplied `output_name` before handing it to a Python CLI
/// that joins it onto the output directory (`directory / f"{stem}.png"`). An
/// empty name is allowed (the CLI picks its own `<image>_<suffix>` default); a
/// non-empty name must be a plain basename so an untrusted workflow cannot use
/// `..` or a path separator to redirect the write outside the chosen folder.
pub(crate) fn reject_unsafe_output_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Ok(());
    }
    studio_reject_unsafe_basename(name)
}

#[cfg(windows)]
pub(crate) fn no_window(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    // CREATE_NO_WINDOW: don't pop a console window for the child.
    cmd.creation_flags(0x0800_0000);
}

#[cfg(not(windows))]
pub(crate) fn no_window(_cmd: &mut std::process::Command) {}

/// One-shot subprocess fallback for a bridge CLI: launch `script_name` with
/// `argv`, returning its trimmed stdout (JSON) or, on a non-zero exit, its
/// trimmed stderr. This mirrors the per-command `Command` launch the torch CLIs
/// used before the warm worker and is what [`run_torch_cli`] falls back to.
pub(crate) fn run_bridge_oneshot(
    python: &Path,
    dir: &Path,
    script_name: &str,
    argv: &[String],
) -> Result<String, String> {
    let script = dir.join("python").join("bridge").join(script_name);
    if !script.is_file() {
        return Err(format!("{script_name} not found at {}", script.display()));
    }
    let mut cmd = std::process::Command::new(python);
    cmd.arg(&script);
    for arg in argv {
        cmd.arg(arg);
    }
    cmd.current_dir(dir);
    model_paths::apply_model_env(&mut cmd);
    no_window(&mut cmd);

    let output = cmd
        .output()
        .map_err(|err| format!("failed to launch {}: {err}", python.display()))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Run a torch bridge CLI, preferring the long-lived warm worker (models stay
/// resident across calls) and transparently falling back to a one-shot
/// subprocess when the worker is unavailable. `cmd` is the worker command
/// (`"image_enhance"` / `"detail_repaint"`); `script_name` the CLI used for the
/// fallback; `argv` the argument vector both paths receive (for
/// `detail_repaint` it starts with the `prepare`/`repaint`/`composite`
/// subcommand). Returns the CLI's stdout JSON. Because the worker returns `Err`
/// both when its infrastructure is unavailable *and* when the hosted CLI exits
/// non-zero, the fallback re-runs authoritatively either way — so behaviour is
/// identical to the pre-worker path and a genuine CLI error still surfaces.
pub(crate) fn run_torch_cli(
    python: &Path,
    dir: &Path,
    script_name: &str,
    cmd: &str,
    argv: &[String],
) -> Result<String, String> {
    match crate::studio::torch_worker::run_cli(python, dir, cmd, argv) {
        Ok(stdout) => Ok(stdout),
        Err(_) => run_bridge_oneshot(python, dir, script_name, argv),
    }
}

/// Resolve the project's `python/bridge/detail_repaint_cli.py`, erroring if the
/// helper is missing from the checkout / bundle.
pub(crate) fn detail_repaint_script(dir: &Path) -> Result<PathBuf, String> {
    let script = dir
        .join("python")
        .join("bridge")
        .join("detail_repaint_cli.py");
    if !script.is_file() {
        return Err(format!(
            "detail_repaint_cli.py not found at {}",
            script.display()
        ));
    }
    Ok(script)
}

#[cfg(test)]
mod tests {
    use super::{
        is_project_root, reject_unsafe_output_name, require_project_root, resolve_project_dir,
        CliEngineProbe, DeviceProbe, EngineProbeReport,
    };
    use std::fs;
    use std::path::PathBuf;

    fn unique_tmp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("hgripe_psd_{tag}_{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn require_project_root_accepts_folder_with_marker() {
        let dir = unique_tmp_dir("root_marker");
        fs::write(dir.join("h-gripe.project.json"), b"{}\n").unwrap();
        assert_eq!(require_project_root(dir.clone()).unwrap(), dir);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn require_project_root_accepts_folder_with_python_bridge() {
        let dir = unique_tmp_dir("root_bridge");
        fs::create_dir_all(dir.join("python").join("bridge")).unwrap();
        assert!(is_project_root(&dir));
        assert_eq!(require_project_root(dir.clone()).unwrap(), dir);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn require_project_root_rejects_unmarked_folder() {
        let dir = unique_tmp_dir("root_missing");
        assert!(!is_project_root(&dir));
        let err = require_project_root(dir.clone()).unwrap_err();
        assert!(err.contains("not an H-Gripe project folder"));
        assert!(err.contains("HGRIPE_PROJECT_DIR"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_project_dir_uses_explicit_folder_when_valid() {
        let dir = unique_tmp_dir("explicit");
        fs::write(dir.join("h-gripe.project.json"), b"{}\n").unwrap();
        let resolved = resolve_project_dir(&Some(dir.to_string_lossy().to_string())).unwrap();
        assert_eq!(resolved, dir);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_project_dir_errors_on_unmarked_explicit_folder() {
        let dir = unique_tmp_dir("explicit_bad");
        let err = resolve_project_dir(&Some(dir.to_string_lossy().to_string())).unwrap_err();
        assert!(err.contains("not an H-Gripe project folder"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn output_name_allows_empty_so_cli_picks_default() {
        assert!(reject_unsafe_output_name("").is_ok());
        assert!(reject_unsafe_output_name("   ").is_ok());
    }

    #[test]
    fn output_name_allows_plain_basenames() {
        assert!(reject_unsafe_output_name("matched").is_ok());
        assert!(reject_unsafe_output_name("  result  ").is_ok());
        assert!(reject_unsafe_output_name("my.output").is_ok());
    }

    #[test]
    fn cli_engine_probe_parses_card_probe_json() {
        // Mirror a `detail_watchdog_cli.py --probe-engines` payload: the CPU/rule
        // baseline is available, the ML engine is not, and extra per-engine
        // fields (e.g. native_scale) are tolerated.
        let raw = r#"{
            "engines": {
                "rules": {"available": true, "reason": "built-in CPU rule layer", "accelerated": false},
                "onnx_defect": {"available": false, "reason": "missing optional dependency: onnxruntime", "native_scale": null, "accelerated": true,
                    "weight": {"path": "/cache/models/watchdog_defect.onnx", "present": false, "size_mb": null}}
            },
            "model_cache_dir": "/cache/models"
        }"#;
        let probe: CliEngineProbe = serde_json::from_str(raw).unwrap();
        assert_eq!(probe.model_cache_dir.as_deref(), Some("/cache/models"));
        assert!(probe.engines["rules"].available);
        assert!(!probe.engines["rules"].accelerated);
        // The CPU/rule baseline loads no downloadable weight.
        assert!(probe.engines["rules"].weight.is_none());
        assert!(!probe.engines["onnx_defect"].available);
        assert!(probe.engines["onnx_defect"].accelerated);
        assert!(probe.engines["onnx_defect"].reason.contains("onnxruntime"));
        // The cached-weight inventory parses: the ML weight is not present yet.
        let weight = probe.engines["onnx_defect"].weight.as_ref().unwrap();
        assert!(weight.path.ends_with("watchdog_defect.onnx"));
        assert!(!weight.present);
        assert!(weight.size_mb.is_none());
    }

    #[test]
    fn device_probe_parses_gpu_box() {
        // A CUDA box: torch sees one device, ORT exposes the CUDA provider.
        let raw = r#"{
            "cuda_available": true,
            "devices": [{"index": 0, "name": "NVIDIA RTX 4090", "total_memory_mb": 24564}],
            "torch": {"installed": true, "version": "2.3.0", "cuda": true},
            "onnxruntime": {"installed": true, "version": "1.18.0",
                "providers": ["CUDAExecutionProvider", "CPUExecutionProvider"]}
        }"#;
        let probe: DeviceProbe = serde_json::from_str(raw).unwrap();
        assert!(probe.cuda_available);
        assert_eq!(probe.devices.len(), 1);
        assert_eq!(probe.devices[0].name, "NVIDIA RTX 4090");
        assert_eq!(probe.devices[0].total_memory_mb, 24564);
        assert_eq!(probe.torch.cuda, Some(true));
        assert!(probe
            .onnxruntime
            .providers
            .iter()
            .any(|p| p == "CUDAExecutionProvider"));
    }

    #[test]
    fn device_probe_parses_cpu_only_box() {
        // The common case (CI / no GPU): no CUDA, no devices, deps may be absent;
        // missing optional fields default rather than failing to parse.
        let raw = r#"{
            "cuda_available": false,
            "devices": [],
            "torch": {"installed": false, "reason": "ModuleNotFoundError: torch"},
            "onnxruntime": {"installed": true, "providers": ["CPUExecutionProvider"]}
        }"#;
        let probe: DeviceProbe = serde_json::from_str(raw).unwrap();
        assert!(!probe.cuda_available);
        assert!(probe.devices.is_empty());
        assert!(!probe.torch.installed);
        assert_eq!(probe.torch.cuda, None);
        assert_eq!(probe.onnxruntime.version, None);
        assert_eq!(probe.onnxruntime.providers, ["CPUExecutionProvider"]);
    }

    #[test]
    fn engine_probe_report_carries_optional_runtime() {
        // `runtime` is absent on older payloads -> None; present -> parsed.
        let without = r#"{"cards": [], "model_cache_dir": null}"#;
        let report: EngineProbeReport = serde_json::from_str(without).unwrap();
        assert!(report.runtime.is_none());

        let with = r#"{"cards": [], "model_cache_dir": null,
            "runtime": {"cuda_available": false, "devices": [],
                "torch": {"installed": false},
                "onnxruntime": {"installed": false, "providers": []}}}"#;
        let report: EngineProbeReport = serde_json::from_str(with).unwrap();
        assert!(report.runtime.is_some());
        assert!(!report.runtime.unwrap().cuda_available);
    }

    #[test]
    fn engine_probe_report_round_trips() {
        // The cross-card report serialises to the shape the UI bridge expects.
        let raw = r#"{
            "cards": [
                {"node_kind": "imageEnhance", "cli": "image_enhance_cli.py",
                 "engines": {"cpu": {"available": true, "reason": "built-in CPU path"}}},
                {"node_kind": "detailWatchdog", "cli": "detail_watchdog_cli.py",
                 "engines": {}, "error": "probe failed"}
            ],
            "model_cache_dir": null
        }"#;
        let report: EngineProbeReport = serde_json::from_str(raw).unwrap();
        assert_eq!(report.cards.len(), 2);
        assert_eq!(report.cards[0].node_kind, "imageEnhance");
        assert!(report.cards[0].engines["cpu"].available);
        assert!(report.cards[0].error.is_none());
        assert_eq!(report.cards[1].error.as_deref(), Some("probe failed"));
        assert!(report.cards[1].engines.is_empty());
    }

    #[test]
    fn local_repaint_result_parses_backend_and_fallback() {
        // A successful local run: a backend ran and returned one repainted crop.
        let raw = r#"{
            "repainted": [{"index": 0, "path": "/out/hero_region0_repainted.png"}],
            "skipped": [],
            "engine": "sd_inpaint",
            "engine_requested": "sd_inpaint",
            "engine_fallback_reason": null,
            "backend_model": "sd-inpaint",
            "requested_count": 1,
            "repainted_count": 1
        }"#;
        let res: super::LocalRepaintResult = serde_json::from_str(raw).unwrap();
        assert_eq!(res.engine, "sd_inpaint");
        assert_eq!(res.repainted.len(), 1);
        assert_eq!(res.repainted[0].index, 0);
        assert!(res.engine_fallback_reason.is_none());
        assert_eq!(res.backend_model.as_deref(), Some("sd-inpaint"));

        // The provider-fallback shape: no local repaint, a recorded reason.
        let fallback = r#"{
            "repainted": [],
            "engine": "provider",
            "engine_requested": "sd_inpaint",
            "engine_fallback_reason": "missing optional dependency: torch",
            "requested_count": 2,
            "repainted_count": 0
        }"#;
        let res: super::LocalRepaintResult = serde_json::from_str(fallback).unwrap();
        assert_eq!(res.engine, "provider");
        assert!(res.repainted.is_empty());
        assert_eq!(
            res.engine_fallback_reason.as_deref(),
            Some("missing optional dependency: torch")
        );
        assert!(res.backend_model.is_none());
    }

    #[test]
    fn output_name_rejects_traversal_and_separators() {
        assert!(reject_unsafe_output_name(".").is_err());
        assert!(reject_unsafe_output_name("..").is_err());
        assert!(reject_unsafe_output_name("../evil").is_err());
        assert!(reject_unsafe_output_name("..\\evil").is_err());
        assert!(reject_unsafe_output_name("sub/dir").is_err());
        assert!(reject_unsafe_output_name("/etc/passwd").is_err());
    }
}
