use std::{
    env,
    ffi::OsString,
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Command, ExitCode},
};

use keysend::settings::{Credentials, Scope};
use loco_rs::environment::Environment;

const USAGE: &str = "Usage:
  keysend-credentials status --environment NAME [--directory DIRECTORY]
  keysend-credentials exec --scope deploy|backup --environment NAME [--directory DIRECTORY] -- COMMAND [ARGUMENT...]

The directory defaults to config/credentials. status prints JSON with
payments_ready, postdeploy_ready, deploy_ready and backup_ready. exec runs
COMMAND without a shell, with the variables of the scope and no other
credential variable.";

struct Arguments {
    action: String,
    scope: Option<Scope>,
    environment: Option<Environment>,
    directory: PathBuf,
    command: Vec<OsString>,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("keysend-credentials: {message}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<(), String> {
    let arguments = parse(env::args_os().skip(1))?;
    if arguments.action == "--help" {
        println!("{USAGE}");
        return Ok(());
    }
    let environment = arguments.environment.ok_or("--environment is required")?;
    let open =
        || Credentials::open(&environment, &arguments.directory).map_err(|error| error.to_string());
    match (arguments.action.as_str(), arguments.scope) {
        ("status", None) if arguments.command.is_empty() => {
            let status = open()?.status();
            println!(
                "{}",
                serde_json::to_string(&status).map_err(|error| error.to_string())?
            );
            Ok(())
        }
        ("exec", Some(scope)) => {
            let (program, rest) = arguments
                .command
                .split_first()
                .ok_or("exec needs a COMMAND after --")?;
            let mut command = Command::new(program);
            command.args(rest);
            open()?
                .prepare(
                    scope,
                    &Credentials::path(&environment, &arguments.directory),
                    &mut command,
                )
                .map_err(|error| error.to_string())?;
            let error = command.exec();
            Err(format!("cannot run {}: {error}", program.to_string_lossy()))
        }
        _ => Err(USAGE.into()),
    }
}

fn parse(mut arguments: impl Iterator<Item = OsString>) -> Result<Arguments, String> {
    let text = |value: Option<OsString>, option: &str| {
        value
            .and_then(|value| value.into_string().ok())
            .ok_or(format!("{option} needs a value"))
    };
    let mut parsed = Arguments {
        action: arguments
            .next()
            .and_then(|action| action.into_string().ok())
            .ok_or(USAGE)?,
        scope: None,
        environment: None,
        directory: PathBuf::from("config/credentials"),
        command: Vec::new(),
    };
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--scope") => {
                parsed.scope = Some(match text(arguments.next(), "--scope")?.as_str() {
                    "deploy" => Scope::Deploy,
                    "backup" => Scope::Backup,
                    _ => return Err("--scope is deploy or backup".into()),
                })
            }
            Some("--environment") => {
                parsed.environment =
                    Some(Environment::from(text(arguments.next(), "--environment")?))
            }
            Some("--directory") => {
                parsed.directory =
                    PathBuf::from(arguments.next().ok_or("--directory needs a value")?)
            }
            Some("--") => {
                parsed.command = arguments.collect();
                break;
            }
            _ => return Err(USAGE.into()),
        }
    }
    Ok(parsed)
}
