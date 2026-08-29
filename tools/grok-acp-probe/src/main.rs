use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use grok_acp_probe::{
    ControlOptions, LifecycleOptions, ProbeError, ProbeKind, ProbeReport, ProbeRunError,
    ProbeTarget, run_probe, summarize_version_command,
};
#[cfg(windows)]
use grok_acp_probe::{run_managed_initialize_probe, run_managed_restart_probe};
use grok_runtime::{GROK_STDIO_ARGS, ResolveGrokExecutableError, resolve_grok_executable};
use serde_json::json;

#[derive(Debug, Parser)]
#[command(name = "grok-acp-probe")]
#[command(about = "Sanitized Phase 0 compatibility probes for Grok Build ACP")]
struct Cli {
    /// Override the Grok executable path.
    #[arg(long, global = true)]
    grok: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Resolve the executable and print non-sensitive version information.
    Detect,
    /// Initialize against an empty temporary GROK_HOME without authenticating.
    Initialize {
        #[arg(long, default_value_t = 30)]
        timeout_seconds: u64,
    },
    /// Initialize through the Windows Job Object-managed process boundary.
    #[cfg(windows)]
    ManagedInitialize {
        #[arg(long, default_value_t = 30)]
        timeout_seconds: u64,
    },
    /// Prove crash-safe managed process restart and zero-turn session recovery.
    #[cfg(windows)]
    ManagedRestart {
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        #[arg(long)]
        auth_method: Option<String>,
        #[arg(long, default_value_t = 60)]
        timeout_seconds: u64,
    },
    /// Run the authenticated lifecycle against the ambient Grok-owned credentials.
    Lifecycle {
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        #[arg(
            long,
            default_value = "Reply with exactly GROK_BUILD_GUI_COMPAT_OK. Do not use tools or modify files."
        )]
        prompt: String,
        #[arg(long)]
        auth_method: Option<String>,
        #[arg(long)]
        exercise_cancel: bool,
        #[arg(long, default_value_t = 120)]
        timeout_seconds: u64,
    },
    /// Safely exercise session-local model, reasoning, and mode controls without a model turn.
    Controls {
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        #[arg(long)]
        auth_method: Option<String>,
        #[arg(long, default_value_t = 60)]
        timeout_seconds: u64,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    match execute(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{}", safe_cli_error(error.as_ref()));
            ExitCode::FAILURE
        }
    }
}

async fn execute(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let resolved = resolve_grok_executable(cli.grok.as_deref())?;

    match cli.command {
        Command::Detect => {
            let output = std::process::Command::new(resolved.path())
                .args(["--no-auto-update", "version", "--json"])
                .output()?;
            let version = summarize_version_command(&output.stdout, output.status.success());
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "executable": executable_product(resolved.path()),
                    "source": resolved.source().to_string(),
                    "launchArgs": resolved.launch_args(),
                    "version": version,
                }))?
            );
        }
        Command::Initialize { timeout_seconds } => {
            let isolated_home = tempfile::tempdir()?;
            let target = ProbeTarget::new(resolved.path(), GROK_STDIO_ARGS)
                .env("GROK_HOME", isolated_home.path().to_string_lossy())
                .env("XAI_API_KEY", "")
                .env("GROK_DEPLOYMENT_KEY", "")
                .env("GROK_DISABLE_AUTOUPDATER", "1")
                .env("GROK_CRASH_HANDLER", "0");
            print_probe_result(
                run_probe(
                    target,
                    ProbeKind::InitializeOnly,
                    Duration::from_secs(timeout_seconds),
                )
                .await,
            )?;
        }
        #[cfg(windows)]
        Command::ManagedInitialize { timeout_seconds } => {
            let isolated_home = tempfile::tempdir()?;
            let target = ProbeTarget::new(resolved.path(), GROK_STDIO_ARGS)
                .env("GROK_HOME", isolated_home.path().to_string_lossy())
                .env("XAI_API_KEY", "")
                .env("GROK_DEPLOYMENT_KEY", "")
                .env("GROK_DISABLE_AUTOUPDATER", "1")
                .env("GROK_CRASH_HANDLER", "0");
            let report =
                run_managed_initialize_probe(target, Duration::from_secs(timeout_seconds)).await?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        #[cfg(windows)]
        Command::ManagedRestart {
            workspace,
            auth_method,
            timeout_seconds,
        } => {
            let target = ProbeTarget::new(resolved.path(), GROK_STDIO_ARGS)
                .env("GROK_DISABLE_AUTOUPDATER", "1")
                .env("GROK_CRASH_HANDLER", "0");
            let report = run_managed_restart_probe(
                target,
                workspace,
                auth_method,
                Duration::from_secs(timeout_seconds),
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Command::Lifecycle {
            workspace,
            prompt,
            auth_method,
            exercise_cancel,
            timeout_seconds,
        } => {
            let target = ProbeTarget::new(resolved.path(), GROK_STDIO_ARGS)
                .env("GROK_DISABLE_AUTOUPDATER", "1")
                .env("GROK_CRASH_HANDLER", "0");
            print_probe_result(
                run_probe(
                    target,
                    ProbeKind::Lifecycle(LifecycleOptions {
                        workspace,
                        prompt,
                        auth_method,
                        exercise_cancel,
                    }),
                    Duration::from_secs(timeout_seconds),
                )
                .await,
            )?;
        }
        Command::Controls {
            workspace,
            auth_method,
            timeout_seconds,
        } => {
            let target = ProbeTarget::new(resolved.path(), GROK_STDIO_ARGS)
                .env("GROK_DISABLE_AUTOUPDATER", "1")
                .env("GROK_CRASH_HANDLER", "0");
            print_probe_result(
                run_probe(
                    target,
                    ProbeKind::Controls(ControlOptions {
                        workspace,
                        auth_method,
                    }),
                    Duration::from_secs(timeout_seconds),
                )
                .await,
            )?;
        }
    }

    Ok(())
}

fn executable_product(path: &std::path::Path) -> &'static str {
    match path.file_name().and_then(|name| name.to_str()) {
        Some(name)
            if name.eq_ignore_ascii_case("grok.exe") || name.eq_ignore_ascii_case("grok") =>
        {
            "grok_build"
        }
        _ => "other",
    }
}

fn safe_cli_error(error: &(dyn std::error::Error + 'static)) -> String {
    if let Some(error) = error.downcast_ref::<ProbeRunError>() {
        return error.to_string();
    }
    if let Some(error) = error.downcast_ref::<ProbeError>() {
        return error.to_string();
    }
    if error.downcast_ref::<ResolveGrokExecutableError>().is_some() {
        return "Grok executable resolution failed; verify the installation or --grok override"
            .to_owned();
    }
    if error.downcast_ref::<std::io::Error>().is_some() {
        return "the local probe process or temporary workspace operation failed".to_owned();
    }
    if error.downcast_ref::<serde_json::Error>().is_some() {
        return "the sanitized compatibility report could not be encoded".to_owned();
    }

    "the compatibility probe failed without a safe diagnostic".to_owned()
}

fn print_probe_result(
    result: Result<ProbeReport, ProbeRunError>,
) -> Result<(), Box<dyn std::error::Error>> {
    match result {
        Ok(report) => {
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        Err(error) => {
            if let Some(report) = error.partial_report() {
                println!("{}", serde_json::to_string_pretty(report)?);
            }
            Err(Box::new(error))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_classifies_executables_without_retaining_their_paths() {
        assert_eq!(
            executable_product(std::path::Path::new("grok.exe")),
            "grok_build"
        );
        let private = std::path::PathBuf::from("secret-workspace").join("private-agent.exe");
        let product = executable_product(&private);
        assert_eq!(product, "other");
        assert!(!product.contains("secret-workspace"));
        assert!(!product.contains("private-agent"));
    }

    #[test]
    fn generic_cli_errors_never_echo_arbitrary_error_text() {
        let error =
            std::io::Error::other(r"Bearer private-token at E:\secret-workspace\private-file.txt");

        let safe = safe_cli_error(&error);

        assert_eq!(
            safe,
            "the local probe process or temporary workspace operation failed"
        );
        assert!(!safe.contains("private-token"));
        assert!(!safe.contains("secret-workspace"));
    }
}
