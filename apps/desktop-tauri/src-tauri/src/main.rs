#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use std::fs;
use std::path::PathBuf;

use hgripe_api::providers::custom_http::CustomHttpProvider;
use hgripe_api::providers::mock::MockProvider;
use hgripe_api::providers::openai_compatible::OpenAiCompatibleProvider;
use hgripe_api::providers::replicate::ReplicateProvider;
use hgripe_api::{ApiBroker, RuntimePaths};

mod commands;
mod contracts;
mod psd;
mod resource;
mod studio;
mod thumb_cache;

use studio::{StudioRunCancels, StudioScheduler};

/// The API broker with every built-in provider registered. Shared by the
/// `runtime`/`tasks` command modules and the studio API lane (`crate::broker`).
pub(crate) fn broker() -> ApiBroker {
    let mut broker = ApiBroker::new();
    broker.register_provider(CustomHttpProvider::default());
    broker.register_provider(MockProvider);
    broker.register_provider(OpenAiCompatibleProvider::default());
    broker.register_provider(ReplicateProvider::default());
    broker
}

/// Resolve the runtime paths (history DB, output dir, …) from the environment.
/// Reached as `crate::runtime_paths` by several command modules and studio
/// nodes that need the output directory.
pub(crate) fn runtime_paths() -> Result<RuntimePaths, String> {
    RuntimePaths::from_env().map_err(|err| err.to_string())
}

/// Resolve (creating on demand) a named cache subdirectory under the runtime
/// output dir, e.g. `.thumbnails` / `.posters`.
pub(crate) fn cache_subdir(name: &str) -> Result<PathBuf, String> {
    let dir = runtime_paths()?.output_dir.join(name);
    fs::create_dir_all(&dir).map_err(|err| format!("failed to create {}: {err}", dir.display()))?;
    Ok(dir)
}

/// File modification time in milliseconds since the Unix epoch, if available.
pub(crate) fn modified_ms(metadata: &fs::Metadata) -> Option<u64> {
    metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(StudioRunCancels::default())
        .manage(StudioScheduler::default())
        .setup(|app| {
            // Capture the bundled resource directory so the PSD nodes can fall
            // back to the `h-gripe.project.json` + `python/bridge` subtree
            // shipped via `bundle.resources` when running from a packaged
            // install.
            use tauri::Manager;
            let resource_dir = app.path().resource_dir().ok();
            psd::set_resource_dir(resource_dir.clone());
            // The auto-subject model is bundled under the same resource dir; the
            // handle-free `Compute` segmenter needs it captured here to resolve
            // the weight in a packaged install.
            studio::set_subject_model_resource_dir(resource_dir);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::runtime::get_runtime_info,
            commands::runtime::doctor,
            commands::config::get_credentials,
            commands::config::check_credentials,
            commands::config::get_profiles,
            commands::config::check_profiles,
            commands::config::read_config_file,
            commands::config::write_config_file,
            commands::history::list_history,
            commands::history::history_detail,
            commands::history::history_cleanup_preview,
            commands::history::history_cleanup_apply,
            commands::tasks::run_task,
            commands::tasks::run_task_json,
            studio::run_studio_graph,
            studio::read_studio_autosave,
            studio::write_studio_autosave,
            studio::clear_studio_autosave,
            studio::pick_workflow_save_path,
            studio::pick_workflow_open_path,
            studio::pick_project_folder,
            studio::read_studio_workflow,
            studio::write_studio_workflow,
            studio::list_studio_workflows,
            studio::rename_studio_workflow,
            studio::delete_studio_workflow,
            studio::duplicate_studio_workflow,
            studio::read_studio_snapshots,
            studio::write_studio_snapshots,
            studio::read_studio_run_history,
            studio::write_studio_run_history,
            studio::read_studio_recents,
            studio::write_studio_recents,
            studio::cancel_studio_run,
            commands::tasks::rerun_task,
            commands::shell::open_url,
            commands::shell::pick_file,
            psd::list_psd_outputs,
            commands::media::read_image_data_url,
            commands::media::generate_thumbnail,
            commands::media::probe_image_dims,
            commands::media::prime_ingest,
            commands::media::register_resource,
            commands::media::resource_info,
            commands::media::resource_thumbnail,
            commands::shell::read_text_file,
            commands::shell::open_path,
            psd::compose_psd,
            psd::inspect_psd,
            psd::analyze_psd_context,
            psd::match_light_color,
            psd::refine_mask_edge,
            psd::enhance_image,
            psd::detect_quality_issues,
            psd::probe_engines,
            commands::video::video_probe,
            commands::video::video_scrub,
            psd::prepare_repaint_regions,
            psd::local_repaint_regions,
            psd::composite_repaint
        ])
        .run(tauri::generate_context!())
        .expect("error while running H-Gripe Desktop");
}
