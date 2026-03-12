mod commands;
pub mod display;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "minion", about = "AI Workflow Engine", version)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run a workflow
    Execute(commands::ExecuteArgs),
    /// Validate a workflow YAML without running
    Validate(commands::ValidateArgs),
    /// List available workflows
    List,
    /// Show version
    Version,
}

impl Cli {
    pub async fn run(self) -> anyhow::Result<()> {
        match self.command {
            Command::Execute(args) => commands::execute(args).await,
            Command::Validate(args) => commands::validate(args).await,
            Command::List => commands::list().await,
            Command::Version => {
                println!("minion {}", env!("CARGO_PKG_VERSION"));
                Ok(())
            }
        }
    }
}
