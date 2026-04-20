mod commands;
pub mod display;
mod harness_adapter;
pub mod init_templates;
mod remote;
mod session_setup;
mod setup;

use std::sync::{Arc, OnceLock};

use clap::{Args, Parser, Subcommand};
use tokio::sync::broadcast;

#[derive(Parser)]
#[command(
    name = "stepyard",
    about = "AI Workflow Engine — orchestrate Claude Code CLI with YAML workflows",
    version,
    after_help = "\x1b[1mQuick start:\x1b[0m
  cargo install minion-engine
  stepyard setup
  stepyard execute workflows/code-review.yaml -- 42

\x1b[1mRequirements:\x1b[0m
  • ANTHROPIC_API_KEY   — required for AI steps (chat, map)
  • gh auth login       — required for GitHub-based workflows (GH_TOKEN auto-detected)
  • Docker Desktop      — required for --sandbox mode (creates isolated containers)

\x1b[1mExamples:\x1b[0m
  stepyard execute workflows/code-review.yaml -- 42        Review PR #42 (sandbox on by default)
  stepyard execute workflows/fix-issue.yaml -- 123         Fix issue #123
  stepyard execute my-workflow.yaml --no-sandbox -- main   Run without Docker sandbox
  stepyard list                                            List available workflows
  stepyard init my-workflow --template code-review         Create a new workflow
  stepyard setup                                           Interactive setup wizard
  stepyard slack start                                     Start Slack bot (requires --features slack)"
)]
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
    /// List available workflows (current dir, ./workflows/, ~/.minion/workflows/)
    List,
    /// Create a new workflow from a template
    Init(commands::InitArgs),
    /// Inspect a workflow: show config, scopes, dependency graph and dry-run summary
    Inspect(commands::InspectArgs),
    /// Interactive setup wizard — configure API keys, Docker, and Slack integration
    Setup,
    /// Manage default configuration (model, provider, timeouts)
    Config(ConfigArgs),
    /// Inspect sessions stored in PostgreSQL (Story 2.5, FR24)
    Session(SessionArgs),
    /// Slack bot integration (requires: cargo install minion-engine --features slack)
    #[cfg(feature = "slack")]
    Slack(SlackArgs),
    /// Dispatch workflows to a remote engine (Epic 5 Story 5.2)
    Remote(remote::RemoteArgs),
    /// Show version
    Version,
}

#[derive(Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    command: ConfigCommand,
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Show current effective configuration (embedded + user + project merged)
    Show,
    /// Create or edit user-level defaults (~/.minion/defaults.yaml)
    Init,
    /// Set a config value. Example: stepyard config set chat.model claude-opus-4-20250514
    Set {
        /// Config key in dot notation (e.g., chat.model, agent.model, global.timeout)
        key: String,
        /// Value to set
        value: String,
    },
    /// Show where config files are located and which ones exist
    Path,
}

#[derive(Args)]
pub struct SessionArgs {
    #[command(subcommand)]
    command: SessionCommand,
}

#[derive(Subcommand)]
enum SessionCommand {
    /// List sessions filtered by lifecycle status
    List(commands::SessionListArgs),
}

#[cfg(feature = "slack")]
#[derive(Args)]
struct SlackArgs {
    #[command(subcommand)]
    command: SlackCommand,
}

#[cfg(feature = "slack")]
#[derive(Subcommand)]
enum SlackCommand {
    /// Start the Slack bot server
    Start {
        /// Port to listen on (default: 9000)
        #[arg(long, short, default_value = "9000")]
        port: u16,
    },
}

impl Cli {
    /// Dispatch the parsed subcommand. `shutdown_tx` is the per-process
    /// broadcast channel owned by `main()`; command paths that construct a
    /// `HarnessConfig` clone it in so every `Engine` subscribes to the same
    /// sender (Story 2.1 — D1/D4). `shutdown_signal` is the shared
    /// signal-name slot populated by the signal handler before the
    /// broadcast fires (Story 2.3 — Option B).
    pub async fn run(
        self,
        shutdown_tx: Arc<broadcast::Sender<()>>,
        shutdown_signal: Arc<OnceLock<String>>,
    ) -> anyhow::Result<()> {
        match self.command {
            Command::Execute(args) => commands::execute(args, shutdown_tx, shutdown_signal).await,
            Command::Validate(args) => commands::validate(args).await,
            Command::List => commands::list().await,
            Command::Init(args) => commands::init(args).await,
            Command::Inspect(args) => commands::inspect(args).await,
            Command::Setup => setup::run_setup().await,
            Command::Config(args) => match args.command {
                ConfigCommand::Show => commands::config_show().await,
                ConfigCommand::Init => commands::config_init().await,
                ConfigCommand::Set { key, value } => commands::config_set(&key, &value).await,
                ConfigCommand::Path => commands::config_path().await,
            },
            Command::Session(args) => match args.command {
                SessionCommand::List(a) => commands::session_list(a).await,
            },
            #[cfg(feature = "slack")]
            Command::Slack(args) => match args.command {
                SlackCommand::Start { port } => crate::slack::start_server(port).await,
            },
            Command::Remote(args) => remote::run(args).await,
            Command::Version => {
                println!("stepyard {}", env!("CARGO_PKG_VERSION"));
                Ok(())
            }
        }
    }
}
