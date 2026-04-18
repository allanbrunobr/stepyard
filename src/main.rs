use std::process::ExitCode;
use std::sync::{Arc, OnceLock};

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
mod startup;
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

    // Register SIGINT/SIGTERM handlers SYNCHRONOUSLY before anything else
    // kicks off — in particular, before the `tokio::select!` below spawns
    // `cli.run(..)` and `signal::install_handlers(..)` as sibling futures.
    // Without this, cli.run's first poll can consume CPU (parser, env
    // validation, DB connect) for longer than the kernel delay between
    // startup and an early SIGTERM, during which `install_handlers` never
    // gets polled and the kernel's default disposition kills the process
    // with `unix_wait_status(15)` instead of the canonical 143 exit code.
    // Tokio's `signal(..)` installs a process-global handler on first call;
    // the named `_eager_*` bindings keep the streams alive (a bare `let _`
    // would drop immediately). `install_handlers` then calls `signal(..)`
    // again to obtain its own stream — tokio documents this as supported
    // and equivalent to the eagerly-installed one.
    let _eager_sigint = tokio::signal::unix::signal(
        tokio::signal::unix::SignalKind::interrupt(),
    )
    .expect("install SIGINT handler");
    let _eager_sigterm = tokio::signal::unix::signal(
        tokio::signal::unix::SignalKind::terminate(),
    )
    .expect("install SIGTERM handler");

    // D4 per-process shutdown channel. Only `main()` owns the `Sender`; every
    // `Engine` calls `config.shutdown_tx.subscribe()` (Story 2.1). The signal
    // handler below fires on SIGINT/SIGTERM and races `cli.run(..)` via
    // `tokio::select!` — whichever finishes first decides the exit code.
    let (tx, _) = broadcast::channel::<()>(16);
    let shutdown_tx = Arc::new(tx);
    // Story 2.3 — Option B: a separate `Arc<OnceLock<String>>` carries the
    // lowercase signal name (`"sigint"` / `"sigterm"`) from the signal
    // handler to every engine. Populating this before firing the broadcast
    // preserves Story 2.1's fixed `broadcast::channel::<()>` signature.
    let shutdown_signal: Arc<OnceLock<String>> = Arc::new(OnceLock::new());

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
        run_result = cli.run(shutdown_tx.clone(), shutdown_signal.clone()) => {
            match run_result {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("\x1b[31merror:\x1b[0m {e:#}");
                    ExitCode::from(1)
                }
            }
        }
        exit_code = signal::install_handlers(shutdown_tx.clone(), shutdown_signal.clone(), grace_s) => exit_code,
    }
}
