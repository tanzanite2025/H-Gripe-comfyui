pub mod broker;
pub mod credentials;
pub mod diagnostics;
pub mod history;
pub mod model;
pub mod outputs;
pub mod profiles;
pub mod provider;
pub mod providers;

pub use broker::ApiBroker;
pub use credentials::{
    credentials_file_path, get_redacted_credential_ref, list_credential_summaries,
    load_credential_ref, load_credentials, validate_credentials, CredentialEntry,
    CredentialSummary, CredentialValidationIssue, CredentialsValidation, RedactedCredentialEntry,
};
pub use diagnostics::{
    build_doctor_report, ConfigFileDiagnostic, DiagnosticIssue, DoctorOptions, DoctorReport,
    EnvironmentDiagnostics, PathDiagnostic, RuntimePathDiagnostics,
};
pub use history::{
    append_history_record, apply_history_cleanup, build_history_cleanup_plan, build_history_record,
    build_rerun_task_from_record, get_history_detail, get_history_record,
    history_detail_from_record, list_recent_history_records, plan_history_cleanup,
    query_history_records, record_task_failure, record_task_result, upsert_sqlite_history_record,
    HistoryCleanupOptions, HistoryCleanupPlan, HistoryCleanupResult, HistoryDetail, HistoryQuery,
    HistoryRecord, HistoryRerunOptions, RuntimePaths,
};
pub use model::{
    ApiCost, ApiErrorInfo, ApiResult, ApiStatus, ApiTask, CachePolicy, OutputFile, OutputType,
    RetryPolicy,
};
pub use outputs::{output_dir_from_env, write_task_output_bytes};
pub use profiles::{
    get_provider_profile, list_provider_profile_summaries, load_provider_profile,
    load_provider_profiles, provider_profiles_path, validate_provider_profiles, ProviderProfile,
    ProviderProfileSummary, ProviderProfileValidationIssue, ProviderProfilesValidation,
};
pub use provider::{Provider, ProviderRegistry};
