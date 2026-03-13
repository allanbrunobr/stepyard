mod commands;
pub mod display;
pub mod init_templates;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "minion",
    about = "AI Workflow Engine — orchestrate Claude Code CLI with YAML workflows",
    version,
    after_help = "\x1b[1mQuick start:\x1b[0m
  cargo install --path .
  export ANTHROPIC_API_KEY=\"sk-ant-...\"
  gh auth login
  minion execute workflows/code-review.yaml --sandbox -- <PR_NUMBER>

\x1b[1mRequirements:\x1b[0m
  • ANTHROPIC_API_KEY   — required for AI steps (chat, map)
  • gh auth login       — required for GitHub-based workflows (GH_TOKEN auto-detected)
  • Docker Desktop      — required for --sandbox mode (creates isolated containers)

\x1b[1mExamples:\x1b[0m
  minion execute workflows/code-review.yaml -- 42        Review PR #42 (sandbox on by default)
  minion execute workflows/fix-issue.yaml -- 123         Fix issue #123
  minion execute my-workflow.yaml --no-sandbox -- main   Run without Docker sandbox
  minion list                                            List available workflows
  minion init my-workflow --template code-review         Create a new workflow"
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
    /// Show version
    Version,
}

impl Cli {
    pub async fn run(self) -> anyhow::Result<()> {
        match self.command {
            Command::Execute(args) => commands::execute(args).await,
            Command::Validate(args) => commands::validate(args).await,
            Command::List => commands::list().await,
            Command::Init(args) => commands::init(args).await,
            Command::Inspect(args) => commands::inspect(args).await,
            Command::Version => {
                println!("minion {}", env!("CARGO_PKG_VERSION"));
                Ok(())
            }
        }
    }
}
