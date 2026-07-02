//! Provider-profile summaries used by the desktop production surfaces.
//!
//! H-Gripe no longer ships an in-app account/config editor. Credentials and
//! provider profiles remain local API configuration files handled by the CLI and
//! broker, while the desktop UI only reads provider-profile summaries.

use hgripe_api::{list_provider_profile_summaries, ProviderProfileSummary};

#[tauri::command]
pub(crate) fn get_profiles() -> Result<Vec<ProviderProfileSummary>, String> {
    list_provider_profile_summaries(None).map_err(|err| err.to_string())
}
