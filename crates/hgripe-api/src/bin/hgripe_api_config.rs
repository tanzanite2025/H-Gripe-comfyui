use hgripe_api::{
    get_provider_profile, list_provider_profile_summaries, provider_profiles_path,
    validate_provider_profiles,
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
        "profiles" => run_profiles(args),
        _ => Err(format!(
            "unknown config group '{group}'. Run `hgripe-api-config --help`."
        )),
    }
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

#[derive(Debug, Clone)]
struct ParsedProfileCommand {
    profiles_file: Option<String>,
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
  hgripe-api-config profiles list [--profiles-file PATH]
  hgripe-api-config profiles show <profile_ref> [--profiles-file PATH]
  hgripe-api-config profiles validate [--profiles-file PATH]
"#
    );
}
