//! OS-shell escape hatches: open a URL or local path with the platform default
//! handler, and the native file-open dialog.

use std::path::Path;

#[tauri::command]
pub(crate) fn open_url(url: String) -> Result<(), String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("only http(s) URLs are allowed".to_string());
    }
    open_external(&url)
}

/// Open a native file-open dialog and return the chosen path, or `None` if the
/// user cancelled. `filter_name` + `extensions` optionally scope the picker
/// (e.g. images, or `.psd` templates); extensions are bare (no leading dot).
#[tauri::command]
pub(crate) fn pick_file(
    app: tauri::AppHandle,
    title: Option<String>,
    filter_name: Option<String>,
    extensions: Option<Vec<String>>,
) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let mut builder = app.dialog().file();
    if let Some(title) = title {
        builder = builder.set_title(title);
    }
    if let Some(exts) = extensions.as_ref().filter(|e| !e.is_empty()) {
        let refs: Vec<&str> = exts.iter().map(String::as_str).collect();
        builder = builder.add_filter(filter_name.unwrap_or_else(|| "Files".to_string()), &refs);
    }
    builder.blocking_pick_file().map(|path| path.to_string())
}

/// Read a text file, truncating to `max_bytes` so large files cannot freeze
/// the UI. A truncation marker is appended when the file is clipped.
#[tauri::command]
pub(crate) fn read_text_file(path: String, max_bytes: usize) -> Result<String, String> {
    let path = Path::new(path.trim());
    let bytes =
        std::fs::read(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let limit = if max_bytes == 0 {
        bytes.len()
    } else {
        max_bytes
    };
    if bytes.len() > limit {
        let mut end = limit;
        // Avoid slicing in the middle of a UTF-8 sequence.
        while end > 0 && (bytes[end] & 0xC0) == 0x80 {
            end -= 1;
        }
        let mut text = String::from_utf8_lossy(&bytes[..end]).to_string();
        text.push_str("\n… (truncated)");
        Ok(text)
    } else {
        Ok(String::from_utf8_lossy(&bytes).to_string())
    }
}

/// Open a local file or folder with the OS default handler.
#[tauri::command]
pub(crate) fn open_path(path: String) -> Result<(), String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("path is empty".to_string());
    }
    if !Path::new(trimmed).exists() {
        return Err(format!("path does not exist: {trimmed}"));
    }
    open_external(trimmed)
}

// NOTE: Long term this should move to the official `tauri-plugin-opener`
// (Tauri 2) so opening files/URLs goes through a vetted, permissioned path
// rather than spawning a child process here. Until then we invoke the OS
// handler directly without going through `cmd /C start`, whose shell re-parses
// metacharacters (`&`, `^`, `%`, …) in the target. `rundll32 url.dll,
// FileProtocolHandler` opens http(s) URLs, files, and folders via the default
// handler and receives the target as a single, un-reparsed argv element.
#[cfg(target_os = "windows")]
fn open_external(url: &str) -> Result<(), String> {
    std::process::Command::new("rundll32.exe")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn()
        .map(|_| ())
        .map_err(|err| err.to_string())
}

#[cfg(target_os = "macos")]
fn open_external(url: &str) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|err| err.to_string())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_external(url: &str) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|err| err.to_string())
}
