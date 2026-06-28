use hgripe_api::{
    build_doctor_report, credentials_file_path, get_provider_profile, get_redacted_credential_ref,
    initialize_local_config, list_credential_summaries, list_provider_profile_summaries,
    provider_profiles_path, resolve_provider_profile, validate_credentials,
    validate_provider_profiles, DoctorOptions, InitOptions,
};
use serde::Serialize;
use serde_json::json;
use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), String> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || matches!(args[0].as_str(), "-h" | "--help") {
        print_help();
        return Ok(());
    }

    let group = args.remove(0);
    match group.as_str() {
        "init" => run_init(args),
        "doctor" => run_doctor(args),
        "profiles" => run_profiles(args),
        "credentials" => run_credentials(args),
        _ => Err(format!(
            "unknown config group '{group}'. Run `hgripe-api-config --help`."
        )),
    }
}

fn run_init(args: Vec<String>) -> Result<(), String> {
    let options = parse_init_options(args)?;
    let report = initialize_local_config(options).map_err(|err| err.to_string())?;

    print_json(&report)
}

fn run_doctor(args: Vec<String>) -> Result<(), String> {
    let options = parse_doctor_options(args)?;
    let report = build_doctor_report(options).map_err(|err| err.to_string())?;

    print_json(&report)
}

fn run_profiles(mut args: Vec<String>) -> Result<(), String> {
    if args.is_empty() || matches!(args[0].as_str(), "-h" | "--help") {
        print_help();
        return Ok(());
    }

    let command = args.remove(0);
    match command.as_str() {
        "list" => run_profiles_list(args),
        "show" => run_profiles_show(args),
        "resolve" => run_profiles_resolve(args),
        "validate" => run_profiles_validate(args),
        _ => Err(format!(
            "unknown profiles command '{command}'. Run `hgripe-api-config --help`."
        )),
    }
}

fn run_profiles_list(args: Vec<String>) -> Result<(), String> {
    let profiles_file = parse_profiles_file_only(args)?;
    let path = provider_profiles_path(profiles_file.as_deref());
    let profiles =
        list_provider_profile_summaries(profiles_file.as_deref()).map_err(|err| err.to_string())?;

    print_json(&json!({
        "profiles_file": path,
        "profiles": profiles,
    }))
}

fn run_profiles_show(args: Vec<String>) -> Result<(), String> {
    let ParsedProfileCommand {
        profiles_file,
        profile_ref,
    } = parse_profile_command(args)?;
    let path = provider_profiles_path(profiles_file.as_deref());
    let profile = get_provider_profile(&profile_ref, profiles_file.as_deref())
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("provider profile '{profile_ref}' was not found"))?;

    print_json(&json!({
        "profiles_file": path,
        "profile_ref": profile_ref,
        "profile": profile,
    }))
}

fn run_profiles_resolve(args: Vec<String>) -> Result<(), String> {
    let ParsedProfileResolveCommand {
        profiles_file,
        credentials_file,
        profile_ref,
    } = parse_profile_resolve_command(args)?;
    let profiles_path = provider_profiles_path(profiles_file.as_deref());
    let credentials_path = credentials_file_path(credentials_file.as_deref());
    let resolved = resolve_provider_profile(
        &profile_ref,
        profiles_file.as_deref(),
        credentials_file.as_deref(),
    )
    .map_err(|err| err.to_string())?;

    print_json(&json!({
        "profiles_file": profiles_path,
        "credentials_file": credentials_path,
        "profile_ref": profile_ref,
        "resolved": resolved,
    }))
}

fn run_profiles_validate(args: Vec<String>) -> Result<(), String> {
    let profiles_file = parse_profiles_file_only(args)?;
    let path = provider_profiles_path(profiles_file.as_deref());
    let validation =
        validate_provider_profiles(profiles_file.as_deref()).map_err(|err| err.to_string())?;

    print_json(&json!({
        "profiles_file": path,
        "validation": validation,
    }))
}

fn run_credentials(mut args: Vec<String>) -> Result<(), String> {
    if args.is_empty() || matches!(args[0].as_str(), "-h" | "--help") {
        print_help();
        return Ok(());
    }

    let command = args.remove(0);
    match command.as_str() {
        "list" => run_credentials_list(args),
        "show" => run_credentials_show(args),
        "validate" => run_credentials_validate(args),
        _ => Err(format!(
            "unknown credentials command '{command}'. Run `hgripe-api-config --help`."
        )),
    }
}

fn run_credentials_list(args: Vec<String>) -> Result<(), String> {
    let credentials_file = parse_credentials_file_only(args)?;
    let path = credentials_file_path(credentials_file.as_deref());
    let credentials =
        list_credential_summaries(credentials_file.as_deref()).map_err(|err| err.to_string())?;

    print_json(&json!({
        "credentials_file": path,
        "credentials": credentials,
    }))
}

fn run_credentials_show(args: Vec<String>) -> Result<(), String> {
    let ParsedCredentialCommand {
        credentials_file,
        credential_ref,
    } = parse_credential_command(args)?;
    let path = credentials_file_path(credentials_file.as_deref());
    let credential = get_redacted_credential_ref(&credential_ref, credentials_file.as_deref())
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("credential ref '{credential_ref}' was not found"))?;

    print_json(&json!({
        "credentials_file": path,
        "credential_ref": credential_ref,
        "credential": credential,
    }))
}

fn run_credentials_validate(args: Vec<String>) -> Result<(), String> {
    let credentials_file = parse_credentials_file_only(args)?;
    let path = credentials_file_path(credentials_file.as_deref());
    let validation =
        validate_credentials(credentials_file.as_deref()).map_err(|err| err.to_string())?;

    print_json(&json!({
        "credentials_file": path,
        "validation": validation,
    }))
}

#[derive(Debug, Clone)]
struct ParsedProfileCommand {
    profiles_file: Option<String>,
    profile_ref: String,
}

#[derive(Debug, Clone)]
struct ParsedProfileResolveCommand {
    profiles_file: Option<String>,
    credentials_file: Option<String>,
    profile_ref: String,
}

fn parse_profile_command(args: Vec<String>) -> Result<ParsedProfileCommand, String> {
    let mut profiles_file = None;
    let mut profile_ref = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--profiles-file" => {
                profiles_file = Some(option_value(&args, index)?);
                index += 2;
            }
            value if value.starts_with('-') => return Err(format!("unknown option '{value}'")),
            value => {
                if profile_ref.is_some() {
                    return Err(format!("unexpected extra argument '{value}'"));
                }
                profile_ref = Some(value.to_string());
                index += 1;
            }
        }
    }

    Ok(ParsedProfileCommand {
        profiles_file,
        profile_ref: profile_ref.ok_or_else(|| "missing profile_ref".to_string())?,
    })
}

fn parse_profile_resolve_command(args: Vec<String>) -> Result<ParsedProfileResolveCommand, String> {
    let mut profiles_file = None;
    let mut credentials_file = None;
    let mut profile_ref = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--profiles-file" => {
                profiles_file = Some(option_value(&args, index)?);
                index += 2;
            }
            "--credentials-file" => {
                credentials_file = Some(option_value(&args, index)?);
                index += 2;
            }
            value if value.starts_with('-') => return Err(format!("unknown option '{value}'")),
            value => {
                if profile_ref.is_some() {
                    return Err(format!("unexpected extra argument '{value}'"));
                }
                profile_ref = Some(value.to_string());
                index += 1;
            }
        }
    }

    Ok(ParsedProfileResolveCommand {
        profiles_file,
        credentials_file,
        profile_ref: profile_ref.ok_or_else(|| "missing profile_ref".to_string())?,
    })
}

fn parse_profiles_file_only(args: Vec<String>) -> Result<Option<String>, String> {
    let mut profiles_file = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--profiles-file" => {
                profiles_file = Some(option_value(&args, index)?);
                index += 2;
            }
            value => return Err(format!("unknown option or argument '{value}'")),
        }
    }

    Ok(profiles_file)
}

#[derive(Debug, Clone)]
struct ParsedCredentialCommand {
    credentials_file: Option<String>,
    credential_ref: String,
}

fn parse_credential_command(args: Vec<String>) -> Result<ParsedCredentialCommand, String> {
    let mut credentials_file = None;
    let mut credential_ref = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--credentials-file" => {
                credentials_file = Some(option_value(&args, index)?);
                index += 2;
            }
            value if value.starts_with('-') => return Err(format!("unknown option '{value}'")),
            value => {
                if credential_ref.is_some() {
                    return Err(format!("unexpected extra argument '{value}'"));
                }
                credential_ref = Some(value.to_string());
                index += 1;
            }
        }
    }

    Ok(ParsedCredentialCommand {
        credentials_file,
        credential_ref: credential_ref.ok_or_else(|| "missing credential_ref".to_string())?,
    })
}

fn parse_credentials_file_only(args: Vec<String>) -> Result<Option<String>, String> {
    let mut credentials_file = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--credentials-file" => {
                credentials_file = Some(option_value(&args, index)?);
                index += 2;
            }
            value => return Err(format!("unknown option or argument '{value}'")),
        }
    }

    Ok(credentials_file)
}

fn parse_doctor_options(args: Vec<String>) -> Result<DoctorOptions, String> {
    let mut options = DoctorOptions::default();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--credentials-file" => {
                options.credentials_file = Some(option_value(&args, index)?);
                index += 2;
            }
            "--profiles-file" => {
                options.profiles_file = Some(option_value(&args, index)?);
                index += 2;
            }
            "--history-file" => {
                options.history_file = Some(option_value(&args, index)?);
                index += 2;
            }
            "--history-db" => {
                options.history_db = Some(option_value(&args, index)?);
                index += 2;
            }
            "--output-dir" => {
                options.output_dir = Some(option_value(&args, index)?);
                index += 2;
            }
            "--broker" => {
                options.broker_path = Some(option_value(&args, index)?);
                index += 2;
            }
            value => return Err(format!("unknown doctor option '{value}'")),
        }
    }

    Ok(options)
}

fn parse_init_options(args: Vec<String>) -> Result<InitOptions, String> {
    let mut options = InitOptions::default();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--root" => {
                options.root_dir = Some(option_value(&args, index)?);
                index += 2;
            }
            "--credentials-file" => {
                options.credentials_file = Some(option_value(&args, index)?);
                index += 2;
            }
            "--profiles-file" => {
                options.profiles_file = Some(option_value(&args, index)?);
                index += 2;
            }
            "--history-file" => {
                options.history_file = Some(option_value(&args, index)?);
                index += 2;
            }
            "--history-db" => {
                options.history_db = Some(option_value(&args, index)?);
                index += 2;
            }
            "--output-dir" => {
                options.output_dir = Some(option_value(&args, index)?);
                index += 2;
            }
            "--force" => {
                options.force = true;
                index += 1;
            }
            "--dry-run" => {
                options.dry_run = true;
                index += 1;
            }
            value => return Err(format!("unknown init option '{value}'")),
        }
    }

    Ok(options)
}

fn option_value(args: &[String], index: usize) -> Result<String, String> {
    args.get(index + 1)
        .filter(|value| !value.starts_with('-'))
        .cloned()
        .ok_or_else(|| format!("missing value for {}", args[index]))
}

fn print_json<T: Serialize>(value: &T) -> Result<(), String> {
    let encoded = serde_json::to_string_pretty(value)
        .map_err(|err| format!("failed to encode JSON output: {err}"))?;
    println!("{encoded}");
    Ok(())
}

fn print_help() {
    println!(
        r#"Usage:
  hgripe-api-config init [--root PATH] [--credentials-file PATH] [--profiles-file PATH] [--history-file PATH] [--history-db PATH] [--output-dir PATH] [--dry-run] [--force]
  hgripe-api-config doctor [--credentials-file PATH] [--profiles-file PATH] [--history-file PATH] [--history-db PATH] [--output-dir PATH] [--broker PATH]
  hgripe-api-config profiles list [--profiles-file PATH]
  hgripe-api-config profiles show <profile_ref> [--profiles-file PATH]
  hgripe-api-config profiles resolve <profile_ref> [--profiles-file PATH] [--credentials-file PATH]
  hgripe-api-config profiles validate [--profiles-file PATH]
  hgripe-api-config credentials list [--credentials-file PATH]
  hgripe-api-config credentials show <credential_ref> [--credentials-file PATH]
  hgripe-api-config credentials validate [--credentials-file PATH]
"#
    );
}
