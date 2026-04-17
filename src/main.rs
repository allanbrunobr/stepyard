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
#[cfg(feature = "slack")]
mod slack;
mod steps;
mod workflow;

use cli::Cli;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .without_time()
        .init();

    // D4 per-process shutdown channel. Only `main()` owns the `Sender`; every
    // `Engine` calls `config.shutdown_tx.subscribe()` (Story 2.1). Stories 2.2
    // and 2.3 wire signal handlers + the `select!` arm that consumes it.
    let (tx, _) = broadcast::channel::<()>(16);
    let shutdown_tx = Arc::new(tx);

    let cli = Cli::parse();

    if let Err(e) = cli.run(shutdown_tx).await {
        eprintln!("\x1b[31merror:\x1b[0m {e:#}");
        std::process::exit(1);
    }
}
