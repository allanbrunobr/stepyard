use clap::Parser;
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

    let cli = Cli::parse();

    if let Err(e) = cli.run().await {
        eprintln!("\x1b[31merror:\x1b[0m {e:#}");
        std::process::exit(1);
    }
}
