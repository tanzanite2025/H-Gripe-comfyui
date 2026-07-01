//! Runtime introspection commands: environment paths, registered providers, and
//! the doctor self-check.

use std::path::PathBuf;

use hgripe_api::{
    build_doctor_report, credentials_file_path, provider_profiles_path, DoctorOptions, DoctorReport,
};
use serde::Serialize;

use crate::{broker, runtime_paths};

#[derive(Serialize)]
struct PathInfo {
    path: String,
    exists: bool,
}

impl PathInfo {
    fn new(path: PathBuf) -> Self {
        Self {
            exists: path.exists(),
            path: path.to_string_lossy().to_string(),
        }
    }
}

#[derive(Serialize)]
struct RuntimeInfo {
    providers: Vec<String>,
    credentials_file: PathInfo,
    profiles_file: PathInfo,
    history_file: PathInfo,
    history_db: PathInfo,
    output_dir: PathInfo,
}

#[tauri::command]
pub(crate) fn get_runtime_info() -> Result<RuntimeInfo, String> {
    let paths = runtime_paths()?;
    Ok(RuntimeInfo {
        providers: broker().providers(),
        credentials_file: PathInfo::new(credentials_file_path(None)),
        profiles_file: PathInfo::new(provider_profiles_path(None)),
        history_file: PathInfo::new(paths.history_file),
        history_db: PathInfo::new(paths.history_db),
        output_dir: PathInfo::new(paths.output_dir),
    })
}

#[tauri::command]
pub(crate) fn doctor() -> Result<DoctorReport, String> {
    build_doctor_report(DoctorOptions::default()).map_err(|err| err.to_string())
}
