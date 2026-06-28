use crate::credentials::{credentials_file_path, load_credentials, validate_credentials};
use crate::profiles::{load_provider_profiles, provider_profiles_path, validate_provider_profiles};
use crate::provider::BrokerResult;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DoctorOptions {
    pub credentials_file: Option<String>,
    pub profiles_file: Option<String>,
    pub history_file: Option<String>,
    pub history_db: Option<String>,
    pub output_dir: Option<String>,
    pub broker_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PathDiagnostic {
    pub path: String,
    pub exists: bool,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigFileDiagnostic {
    pub file: PathDiagnostic,
    pub configured_count: usize,
    pub ok: bool,
    pub error_count: usize,
    pub warning_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimePathDiagnostics {
    pub broker: PathDiagnostic,
    pub history_file: PathDiagnostic,
    pub history_db: PathDiagnostic,
    pub output_dir: PathDiagnostic,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnvironmentDiagnostics {
    pub hgripe_credentials_file: bool,
    pub hgripe_provider_profiles_file: bool,
    pub hgripe_history_file: bool,
    pub hgripe_history_db: bool,
    pub hgripe_output_dir: bool,
    pub hgripe_api_broker: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiagnosticIssue {
    pub scope: String,
    pub severity: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DoctorReport {
    pub ok: bool,
    pub working_dir: String,
    pub executable: Option<String>,
    pub credentials: ConfigFileDiagnostic,
    pub provider_profiles: ConfigFileDiagnostic,
    pub runtime: RuntimePathDiagnostics,
    pub environment: EnvironmentDiagnostics,
    pub issues: Vec<DiagnosticIssue>,
}

pub fn build_doctor_report(options: DoctorOptions) -> BrokerResult<DoctorReport> {
    let working_dir = env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .to_string_lossy()
        .to_string();
    let executable = env::current_exe()
        .ok()
        .map(|path| path.to_string_lossy().to_string());

    let mut issues = Vec::new();
    let mut credentials = credentials_diagnostic(&options, &mut issues);
    let mut provider_profiles = profiles_diagnostic(&options, &mut issues);
    cross_validate_profiles_credentials(&options, &mut issues);
    refresh_config_issue_counts(&mut credentials, &issues, "credentials:", "credentials");
    refresh_config_issue_counts(
        &mut provider_profiles,
        &issues,
        "provider_profile:",
        "provider_profiles",
    );
    let runtime = runtime_diagnostics(&options, &mut issues)?;
    let environment = environment_diagnostics();
    let ok = !issues.iter().any(|issue| issue.severity == "error");

    Ok(DoctorReport {
        ok,
        working_dir,
        executable,
        credentials,
        provider_profiles,
        runtime,
        environment,
        issues,
    })
}

fn credentials_diagnostic(
    options: &DoctorOptions,
    issues: &mut Vec<DiagnosticIssue>,
) -> ConfigFileDiagnostic {
    let file = path_diagnostic(credentials_file_path(options.credentials_file.as_deref()));
    match validate_credentials(options.credentials_file.as_deref()) {
        Ok(validation) => {
            for issue in validation.issues {
                issues.push(DiagnosticIssue {
                    scope: format!("credentials:{}", issue.credential_ref),
                    severity: issue.severity,
                    code: issue.code,
                    message: issue.message,
                });
            }

            ConfigFileDiagnostic {
                file,
                configured_count: validation.credential_count,
                ok: validation.ok,
                error_count: issues
                    .iter()
                    .filter(|issue| {
                        issue.scope.starts_with("credentials:")
                            && issue.severity.as_str() == "error"
                    })
                    .count(),
                warning_count: issues
                    .iter()
                    .filter(|issue| {
                        issue.scope.starts_with("credentials:")
                            && issue.severity.as_str() == "warning"
                    })
                    .count(),
            }
        }
        Err(err) => {
            issues.push(DiagnosticIssue {
                scope: "credentials".to_string(),
                severity: "error".to_string(),
                code: "credentials_unreadable".to_string(),
                message: err.to_string(),
            });
            ConfigFileDiagnostic {
                file,
                configured_count: 0,
                ok: false,
                error_count: 1,
                warning_count: 0,
            }
        }
    }
}

fn profiles_diagnostic(
    options: &DoctorOptions,
    issues: &mut Vec<DiagnosticIssue>,
) -> ConfigFileDiagnostic {
    let file = path_diagnostic(provider_profiles_path(options.profiles_file.as_deref()));
    match validate_provider_profiles(options.profiles_file.as_deref()) {
        Ok(validation) => {
            for issue in validation.issues {
                issues.push(DiagnosticIssue {
                    scope: format!("provider_profile:{}", issue.profile_ref),
                    severity: issue.severity,
                    code: issue.code,
                    message: issue.message,
                });
            }

            ConfigFileDiagnostic {
                file,
                configured_count: validation.profile_count,
                ok: validation.ok,
                error_count: issues
                    .iter()
                    .filter(|issue| {
                        issue.scope.starts_with("provider_profile:")
                            && issue.severity.as_str() == "error"
                    })
                    .count(),
                warning_count: issues
                    .iter()
                    .filter(|issue| {
                        issue.scope.starts_with("provider_profile:")
                            && issue.severity.as_str() == "warning"
                    })
                    .count(),
            }
        }
        Err(err) => {
            issues.push(DiagnosticIssue {
                scope: "provider_profiles".to_string(),
                severity: "error".to_string(),
                code: "provider_profiles_unreadable".to_string(),
                message: err.to_string(),
            });
            ConfigFileDiagnostic {
                file,
                configured_count: 0,
                ok: false,
                error_count: 1,
                warning_count: 0,
            }
        }
    }
}

fn runtime_diagnostics(
    options: &DoctorOptions,
    issues: &mut Vec<DiagnosticIssue>,
) -> BrokerResult<RuntimePathDiagnostics> {
    let broker = path_diagnostic(resolve_broker_path(options));
    if !broker.exists {
        issues.push(DiagnosticIssue {
            scope: "runtime:broker".to_string(),
            severity: "warning".to_string(),
            code: "broker_not_found".to_string(),
            message: "hgripe-api-broker binary was not found at the expected path".to_string(),
        });
    }

    let history_file = path_diagnostic(resolve_history_file_path(options));
    let history_db = path_diagnostic(resolve_history_db_path(options));
    let output_dir = path_diagnostic(resolve_output_dir_path(options));

    if output_dir.exists && output_dir.kind != "directory" {
        issues.push(DiagnosticIssue {
            scope: "runtime:output_dir".to_string(),
            severity: "error".to_string(),
            code: "output_dir_not_directory".to_string(),
            message: "output_dir exists but is not a directory".to_string(),
        });
    }

    Ok(RuntimePathDiagnostics {
        broker,
        history_file,
        history_db,
        output_dir,
    })
}

fn cross_validate_profiles_credentials(options: &DoctorOptions, issues: &mut Vec<DiagnosticIssue>) {
    let credentials = match load_credentials(options.credentials_file.as_deref()) {
        Ok(credentials) => credentials,
        Err(_) => return,
    };
    let profiles = match load_provider_profiles(options.profiles_file.as_deref()) {
        Ok(profiles) => profiles,
        Err(_) => return,
    };

    for (profile_ref, profile) in profiles {
        if profile.no_auth == Some(true) {
            continue;
        }

        let Some(credentials_ref) = trimmed_string(profile.credentials_ref.as_deref()) else {
            continue;
        };

        if !credentials.contains_key(&credentials_ref) {
            issues.push(DiagnosticIssue {
                scope: format!("provider_profile:{profile_ref}"),
                severity: "error".to_string(),
                code: "missing_credentials_ref".to_string(),
                message: format!(
                    "provider profile references credentials_ref '{credentials_ref}' but it was not found"
                ),
            });
        }
    }
}

fn refresh_config_issue_counts(
    diagnostic: &mut ConfigFileDiagnostic,
    issues: &[DiagnosticIssue],
    item_scope_prefix: &str,
    file_scope: &str,
) {
    diagnostic.error_count = count_issues(issues, item_scope_prefix, file_scope, "error");
    diagnostic.warning_count = count_issues(issues, item_scope_prefix, file_scope, "warning");
    diagnostic.ok = diagnostic.error_count == 0;
}

fn count_issues(
    issues: &[DiagnosticIssue],
    item_scope_prefix: &str,
    file_scope: &str,
    severity: &str,
) -> usize {
    issues
        .iter()
        .filter(|issue| {
            issue.severity == severity
                && (issue.scope.starts_with(item_scope_prefix) || issue.scope == file_scope)
        })
        .count()
}

fn environment_diagnostics() -> EnvironmentDiagnostics {
    EnvironmentDiagnostics {
        hgripe_credentials_file: env_var_is_set("HGRIPE_CREDENTIALS_FILE"),
        hgripe_provider_profiles_file: env_var_is_set("HGRIPE_PROVIDER_PROFILES_FILE"),
        hgripe_history_file: env_var_is_set("HGRIPE_HISTORY_FILE"),
        hgripe_history_db: env_var_is_set("HGRIPE_HISTORY_DB"),
        hgripe_output_dir: env_var_is_set("HGRIPE_OUTPUT_DIR"),
        hgripe_api_broker: env_var_is_set("HGRIPE_API_BROKER"),
    }
}

fn path_diagnostic(path: PathBuf) -> PathDiagnostic {
    let kind = match fs::metadata(&path) {
        Ok(metadata) if metadata.is_file() => "file",
        Ok(metadata) if metadata.is_dir() => "directory",
        Ok(_) => "other",
        Err(_) => "missing",
    }
    .to_string();

    PathDiagnostic {
        path: path.to_string_lossy().to_string(),
        exists: kind != "missing",
        kind,
    }
}

fn resolve_broker_path(options: &DoctorOptions) -> PathBuf {
    explicit_path(options.broker_path.as_deref())
        .or_else(|| env_path("HGRIPE_API_BROKER"))
        .unwrap_or_else(default_broker_path)
}

fn resolve_history_file_path(options: &DoctorOptions) -> PathBuf {
    explicit_path(options.history_file.as_deref())
        .or_else(|| env_path("HGRIPE_HISTORY_FILE"))
        .unwrap_or_else(|| {
            local_root()
                .join("user")
                .join("hgripe")
                .join("history")
                .join("tasks.jsonl")
        })
}

fn resolve_history_db_path(options: &DoctorOptions) -> PathBuf {
    explicit_path(options.history_db.as_deref())
        .or_else(|| env_path("HGRIPE_HISTORY_DB"))
        .unwrap_or_else(|| {
            local_root()
                .join("user")
                .join("hgripe")
                .join("history")
                .join("tasks.sqlite3")
        })
}

fn resolve_output_dir_path(options: &DoctorOptions) -> PathBuf {
    explicit_path(options.output_dir.as_deref())
        .or_else(|| env_path("HGRIPE_OUTPUT_DIR"))
        .unwrap_or_else(|| local_root().join("user").join("hgripe").join("outputs"))
}

fn default_broker_path() -> PathBuf {
    if let Ok(exe) = env::current_exe() {
        if let Some(parent) = exe.parent() {
            let exe_name = if cfg!(windows) {
                "hgripe-api-broker.exe"
            } else {
                "hgripe-api-broker"
            };
            return parent.join(exe_name);
        }
    }

    let exe_name = if cfg!(windows) {
        "hgripe-api-broker.exe"
    } else {
        "hgripe-api-broker"
    };
    local_root().join("target").join("debug").join(exe_name)
}

fn explicit_path(value: Option<&str>) -> Option<PathBuf> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn trimmed_string(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn env_var_is_set(name: &str) -> bool {
    env::var(name)
        .ok()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

fn local_root() -> PathBuf {
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}
