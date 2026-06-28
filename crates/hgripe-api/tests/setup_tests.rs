use hgripe_api::{initialize_local_config, InitOptions};
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn init_dry_run_reports_actions_without_writing_files() {
    let root = temp_setup_dir();

    let report = initialize_local_config(InitOptions {
        root_dir: Some(root.to_string_lossy().to_string()),
        dry_run: true,
        ..InitOptions::default()
    })
    .expect("dry-run init should build report");

    assert_eq!(report.dry_run, true);
    assert!(report.would_create_count >= 5);
    assert!(!root.exists());
}

#[test]
fn init_creates_local_config_templates_and_preserves_existing_files() {
    let root = temp_setup_dir();

    let first = initialize_local_config(InitOptions {
        root_dir: Some(root.to_string_lossy().to_string()),
        ..InitOptions::default()
    })
    .expect("init should create files");

    let credentials_file = root.join("user").join("hgripe").join("credentials.json");
    let profiles_file = root
        .join("user")
        .join("hgripe")
        .join("provider_profiles.json");
    let history_dir = root.join("user").join("hgripe").join("history");
    let output_dir = root.join("user").join("hgripe").join("outputs");

    assert!(first.created_count >= 5);
    assert!(credentials_file.exists());
    assert!(profiles_file.exists());
    assert!(history_dir.is_dir());
    assert!(output_dir.is_dir());
    assert!(fs::read_to_string(&credentials_file)
        .expect("credentials should read")
        .contains("OPENAI_API_KEY"));

    fs::write(&credentials_file, "keep me").expect("credentials should overwrite manually");
    let second = initialize_local_config(InitOptions {
        root_dir: Some(root.to_string_lossy().to_string()),
        ..InitOptions::default()
    })
    .expect("second init should skip existing files");

    assert!(second.skipped_count >= 2);
    assert_eq!(
        fs::read_to_string(&credentials_file).expect("credentials should read"),
        "keep me"
    );

    let forced = initialize_local_config(InitOptions {
        root_dir: Some(root.to_string_lossy().to_string()),
        force: true,
        ..InitOptions::default()
    })
    .expect("forced init should overwrite files");

    assert_eq!(forced.overwritten_count, 2);
    assert!(fs::read_to_string(&credentials_file)
        .expect("credentials should read")
        .contains("OPENAI_API_KEY"));

    let _ = fs::remove_dir_all(root);
}

fn temp_setup_dir() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be valid")
        .as_nanos();
    std::env::temp_dir().join(format!("hgripe-setup-test-{nonce}"))
}
