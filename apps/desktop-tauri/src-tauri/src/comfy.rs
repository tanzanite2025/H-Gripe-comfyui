//! Launcher for a locally spawned ComfyUI server that backs the embedded
//! "Advanced Canvas" iframe. The desktop shell can start / stop it and report
//! status; the process is cleaned up on app exit so it never lingers as an
//! orphan.

use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::Mutex;

/// Holds the locally spawned ComfyUI server process, if any, so the desktop
/// shell can act as a launcher (start / stop) for the embedded UI.
#[derive(Default)]
pub(crate) struct ComfyServer(Mutex<Option<Child>>);

/// Resolve the ComfyUI project directory: the caller-provided path, else the
/// process working directory (the repo root in dev / the install dir packaged).
pub(crate) fn resolve_comfy_dir(dir: &Option<String>) -> Result<PathBuf, String> {
    let base = match dir {
        Some(d) if !d.trim().is_empty() => PathBuf::from(d.trim()),
        _ => std::env::current_dir().map_err(|err| err.to_string())?,
    };
    if !base.join("main.py").is_file() {
        return Err(format!(
            "ComfyUI main.py not found in {} (set the ComfyUI folder)",
            base.display()
        ));
    }
    Ok(base)
}

/// Pick a Python interpreter: prefer the bundled `python_embeded` shipped with
/// the ComfyUI Windows distribution, otherwise fall back to PATH `python`.
pub(crate) fn comfy_python(dir: &Path) -> PathBuf {
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

#[tauri::command]
pub(crate) fn comfyui_reachable(port: Option<u16>) -> bool {
    let port = port.unwrap_or(8188);
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_millis(400),
    )
    .is_ok()
}

#[tauri::command]
pub(crate) fn comfyui_status(state: tauri::State<'_, ComfyServer>) -> bool {
    let mut guard = state.0.lock().unwrap();
    match guard.as_mut() {
        Some(child) => match child.try_wait() {
            Ok(Some(_)) => {
                // Process has exited; clear the slot.
                *guard = None;
                false
            }
            Ok(None) => true,
            Err(_) => false,
        },
        None => false,
    }
}

#[tauri::command]
pub(crate) fn start_comfyui(
    state: tauri::State<'_, ComfyServer>,
    dir: Option<String>,
    port: Option<u16>,
    args: Option<String>,
) -> Result<String, String> {
    let mut guard = state.0.lock().unwrap();
    if let Some(child) = guard.as_mut() {
        if matches!(child.try_wait(), Ok(None)) {
            return Err("ComfyUI is already running".to_string());
        }
    }
    let dir = resolve_comfy_dir(&dir)?;
    let python = comfy_python(&dir);
    let port = port.unwrap_or(8188);

    // Bootstrap that injects the project dir onto sys.path at runtime before
    // running main.py as __main__. This works even with the restrictive
    // `._pth` of embeddable Python builds (which ignore PYTHONPATH and do not
    // auto-add the script directory), as well as normal/standalone Python.
    // Extra CLI args (e.g. `--cpu`, `--listen`, `--lowvram`) are passed through
    // HG_COMFY_ARGS and split on whitespace.
    let bootstrap = "import os, sys, runpy; d = os.environ['HG_COMFY_DIR']; \
sys.argv = ['main.py', '--port', os.environ['HG_COMFY_PORT']] + os.environ.get('HG_COMFY_ARGS', '').split(); \
sys.path.insert(0, d); \
runpy.run_path(os.path.join(d, 'main.py'), run_name='__main__')";
    let mut cmd = std::process::Command::new(&python);
    cmd.arg("-c")
        .arg(bootstrap)
        .current_dir(&dir)
        .env("HG_COMFY_DIR", &dir)
        .env("HG_COMFY_PORT", port.to_string())
        .env("HG_COMFY_ARGS", args.unwrap_or_default());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: don't pop a console window for the child.
        cmd.creation_flags(0x0800_0000);
    }

    let child = cmd
        .spawn()
        .map_err(|err| format!("failed to launch {}: {err}", python.display()))?;
    *guard = Some(child);
    Ok(format!("started ComfyUI on port {port}"))
}

/// Terminate the locally spawned ComfyUI process, if one is tracked. Shared by
/// the `stop_comfyui` command and the app-exit cleanup so the launched server
/// never outlives the desktop shell as an orphan.
pub(crate) fn kill_comfy_child(state: &ComfyServer) {
    let mut guard = state.0.lock().unwrap();
    if let Some(mut child) = guard.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[tauri::command]
pub(crate) fn stop_comfyui(state: tauri::State<'_, ComfyServer>) -> Result<(), String> {
    kill_comfy_child(&state);
    Ok(())
}
