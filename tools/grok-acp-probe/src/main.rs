use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use grok_acp_probe::{LifecycleOptions, ProbeKind, ProbeTarget, run_probe};
use grok_runtime::{GROK_STDIO_ARGS, redact_diagnostic, resolve_grok_executable};
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
}

#[tokio::main]
async fn main() -> ExitCode {
    match execute(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{}", redact_diagnostic(&error.to_string()));
            ExitCode::FAILURE
        }
    }
}

async fn execute(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let resolved = resolve_grok_executable(cli.grok.as_deref())?;

    match cli.command {
        Command::Detect => {
            let output = std::process::Command::new(resolved.path())
                .args(["version", "--json"])
                .output()?;
            let version = serde_json::from_slice::<serde_json::Value>(&output.stdout)
                .unwrap_or_else(|_| json!({ "available": output.status.success() }));
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "path": redact_diagnostic(&resolved.path().display().to_string()),
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
            let report = run_probe(
                target,
                ProbeKind::InitializeOnly,
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
            let report = run_probe(
                target,
                ProbeKind::Lifecycle(LifecycleOptions {
                    workspace,
                    prompt,
                    auth_method,
                    exercise_cancel,
                }),
                Duration::from_secs(timeout_seconds),
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
    }

    Ok(())
}
