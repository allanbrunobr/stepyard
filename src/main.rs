use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use tokio::sync::broadcast;
use tracing_subscriber::EnvFilter;

mod claude;
mod cli;
mod config;
mod control_flow;
mod engine;
mod error;
mod events;
mod plugins;
mod prompts;
mod sandbox;
mod signal;
#[cfg(feature = "slack")]
mod slack;
mod steps;
mod workflow;

use cli::Cli;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .without_time()
        .init();

    // D4 per-process shutdown channel. Only `main()` owns the `Sender`; every
    // `Engine` calls `config.shutdown_tx.subscribe()` (Story 2.1). The signal
    // handler below fires on SIGINT/SIGTERM and races `cli.run(..)` via
    // `tokio::select!` — whichever finishes first decides the exit code.
    let (tx, _) = broadcast::channel::<()>(16);
    let shutdown_tx = Arc::new(tx);

    // D2 default is 10s; override via env var so integration tests can drive a
    // tight deadline without patching the binary. Documented as a Story 2.2
    // deviation in the Dev Agent Record (AC fixes the 10s default but is
    // silent on runtime overrides; NFR1 cleanup-within-1s is test-relevant).
    let grace_s: u64 = std::env::var("MINION_SHUTDOWN_GRACE_S")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);

    let cli = Cli::parse();

    tokio::select! {
        run_result = cli.run(shutdown_tx.clone()) => {
            match run_result {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("\x1b[31merror:\x1b[0m {e:#}");
                    ExitCode::from(1)
                }
            }
        }
        exit_code = signal::install_handlers(shutdown_tx.clone(), grace_s) => exit_code,
    }
}
