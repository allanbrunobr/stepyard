use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use anyhow::{bail, Context};
use chrono::{DateTime, Utc};
use clap::{Args, ValueEnum};
use tokio::sync::broadcast;

use crate::engine::{Engine, EngineOptions};
use crate::sandbox::{self, SandboxMode};
use crate::workflow::parser;
use crate::workflow::validator;

use super::branch_strategy::{apply_cli_override, parse_cli_branch_strategy, CliBranchStrategy};
use super::display;
use super::harness_adapter;
use super::init_templates;
use super::session_setup::{connect_pg, open_session, open_session_with_pool};

/// Resolve a workflow path with fallback chain:
/// 1. As-is (if the file exists — developer running from repo or absolute path)
/// 2. `~/.stepyard/workflows/<filename>` (cargo install users)
fn resolve_workflow_path(path: &Path) -> anyhow::Result<PathBuf> {
    // If the file exists as specified, use it
    if path.exists() {
        return Ok(path.to_path_buf());
    }

    // Try ~/.stepyard/workflows/<filename>
    if let Some(filename) = path.file_name() {
        if let Some(home) = dirs::home_dir() {
            let home_path = home.join(".stepyard").join("workflows").join(filename);
            if home_path.exists() {
                return Ok(home_path);
            }
        }
    }

    bail!("Workflow file not found: {}\n  Hint: run `stepyard slack start` once to extract built-in workflows to ~/.stepyard/workflows/", path.display())
}

#[derive(Args)]
pub struct ExecuteArgs {
    /// Path to the workflow YAML file
    pub workflow: PathBuf,

    /// Target argument (e.g., issue number, branch, directory)
    #[arg(last = true)]
    pub target: Vec<String>,

    /// Show all step outputs
    #[arg(long)]
    pub verbose: bool,

    /// Only show errors
    #[arg(long)]
    pub quiet: bool,

    /// Output results as JSON (suppresses all decorative output)
    #[arg(long)]
    pub json: bool,

    /// Show what steps would run without executing them
    #[arg(long)]
    pub dry_run: bool,

    /// Resume execution from the named step (uses most recent state file)
    #[arg(long, value_name = "STEP_NAME")]
    pub resume: Option<String>,

    /// Run inside a Docker sandbox (default: true). Use --no-sandbox to run locally.
    #[arg(long, default_value_t = true, action = clap::ArgAction::SetTrue)]
    pub sandbox: bool,

    /// Disable Docker sandbox — run directly on your machine
    #[arg(long = "no-sandbox")]
    pub no_sandbox: bool,

    /// Set workflow variable (KEY=VALUE)
    #[arg(long = "var", value_name = "KEY=VALUE")]
    pub vars: Vec<String>,

    /// Override global timeout in seconds
    #[arg(long)]
    pub timeout: Option<u64>,

    /// Override workflow git branch strategy for v2 workspace preparation.
    /// YAML uses snake_case; the CLI uses kebab-case.
    #[arg(long, value_name = "head|merge-to-head|named-branch:<name>", value_parser = parse_cli_branch_strategy)]
    pub branch_strategy: Option<CliBranchStrategy>,

    /// GitHub repository (OWNER/REPO) to clone inside the Docker sandbox.
    /// When set, the sandbox clones this repo instead of copying the host CWD.
    /// Example: --repo allanbrunobr/stepyard
    #[arg(long, value_name = "OWNER/REPO")]
    pub repo: Option<String>,

    /// Engine implementation to drive the workflow:
    /// `v1` (default) — the legacy monolithic `Engine::run()` with all 9
    /// step types (cmd, agent, chat, gate, repeat, map, parallel, call,
    /// template, script).
    /// `v2` — the new `stepyard_harness::Engine::resume()` step loop. Only
    /// supports cmd steps in this release (Story 2.4); non-cmd workflows
    /// exit non-zero with a clear error.
    #[arg(long, value_name = "v1|v2", default_value = "v1")]
    pub engine: String,
}

#[derive(Args)]
pub struct ValidateArgs {
    /// Path to the workflow YAML file
    pub workflow: PathBuf,
}

#[derive(Args)]
pub struct InitArgs {
    /// Name for the new workflow (also used as filename)
    pub name: String,

    /// Template to use: blank, fix-issue, code-review, security-audit
    #[arg(long, short, default_value = "blank")]
    pub template: String,

    /// Output directory (default: current directory)
    #[arg(long, short)]
    pub output: Option<PathBuf>,
}

#[derive(Args)]
pub struct InspectArgs {
    /// Path to the workflow YAML file
    pub workflow: PathBuf,
}

/// CLI-layer session status filter (Story 2.5, FR24).
///
/// Separate from `stepyard_session::SessionStatus` so clap's `ValueEnum` derive
/// does not leak into the session crate; the two are kept in sync through
/// [`SessionStatus::as_db_str`]. Variant names match the DB check constraint
/// exactly (`running | completed | failed | cancelled`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "snake_case")]
pub enum SessionStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl SessionStatus {
    /// Label matching the `sessions.status` DB check constraint.
    fn as_db_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Args)]
pub struct SessionListArgs {
    /// Lifecycle status to filter by (required).
    #[arg(long, value_enum)]
    pub status: SessionStatus,

    /// Optional time window — include only sessions started within the given
    /// duration from now. Accepts human-friendly strings: `24h`, `7d`, `30m`.
    #[arg(long, value_name = "DURATION")]
    pub since: Option<humantime::Duration>,
}

/// `--engine v2` path — drives the workflow through
/// [`stepyard_harness::Engine`] step-by-step, printing the same
/// `▶ name` / `✓ name (…s)` lines the v1 display emits.
///
/// Returns `Ok(())` on successful completion. Non-zero exit codes for failed
/// or cancelled workflows are produced by `std::process::exit` so the caller
/// does not need to translate them.
async fn execute_v2(
    args: ExecuteArgs,
    workflow: crate::workflow::schema::WorkflowDef,
    sandbox_mode: SandboxMode,
    shutdown_tx: Arc<broadcast::Sender<()>>,
    shutdown_signal: Arc<OnceLock<String>>,
) -> anyhow::Result<()> {
    use stepyard_harness::{
        Defaults as HarnessDefaults, Engine as HarnessEngine, EngineError, HarnessConfig,
        RunContext as HarnessRunContext, StepOutcome, TerminationReason,
    };
    use stepyard_sandbox_orchestrator::{
        DockerLifecycle, GitWorktreeManager, LocalShellLifecycle, SandboxLifecycle,
    };

    let mut harness_workflow = harness_adapter::adapt(&workflow)
        .map_err(|e| anyhow::anyhow!("cannot run workflow on --engine v2: {e}"))?;
    apply_cli_override(&mut harness_workflow, args.branch_strategy.clone());
    // Round 3 Story 3 — env key/value validation at the workflow boundary.
    // `Workflow::try_from_yaml` runs `validate()` automatically, but the v1→v2
    // adapter builds the workflow programmatically (`Workflow::new` + field
    // assignment), bypassing that path. The explicit call here is the
    // contract every programmatic caller honors before handing the workflow
    // to the engine.
    harness_workflow.validate()?;
    let branch_strategy = harness_workflow.resolve_branch_strategy()?;

    let pool = connect_pg(args.json).await?;
    let repo_root = std::env::current_dir().context("failed to resolve current directory")?;
    let workspace_manager = Arc::new(GitWorktreeManager::new(
        repo_root.clone(),
        repo_root.join(".stepyard").join("workspaces"),
        24,
    ));

    // D8 startup reconcile (Story 2.4). Runs unconditionally before any
    // engine is constructed. Uses a dedicated `DockerLifecycle::default()`
    // because the runtime lifecycle may be `LocalShellLifecycle` when
    // `--no-sandbox` is set, and Phase 2 still has to scan real Docker.
    // When Docker is unreachable, the reconcile logs a warning via
    // `tracing::warn!` and Phase 1 (PG session cleanup) still runs.
    let reconcile_lifecycle = DockerLifecycle::default();
    let reconcile_report: crate::startup::ReconcileReport =
        crate::startup::reconcile_with_workspace_manager(
            &pool,
            &reconcile_lifecycle,
            Some(workspace_manager.as_ref()),
        )
            .await
            .context("startup reconcile failed")?;
    tracing::debug!(?reconcile_report, "startup reconcile report");

    let session = open_session_with_pool(&pool, &workflow.name).await?;
    let session_id = session.id();

    let lifecycle: Arc<dyn SandboxLifecycle> = if sandbox_mode == SandboxMode::Disabled {
        Arc::new(LocalShellLifecycle::new())
    } else {
        Arc::new(DockerLifecycle::default())
    };

    let config = HarnessConfig {
        tenant_id: std::env::var("STEPYARD_TENANT").unwrap_or_else(|_| "default".into()),
        shutdown_tx,
        shutdown_signal,
        // Production chat dispatch (PR 5c commit 3b of #31). The harness'
        // `Engine::run_chat_step` falls back to a typed `StepFailed`
        // (`ChatExecError::NoClientConfigured`) when this is `None`; CLI
        // runs MUST wire the rig-backed default so top-level `type: chat`
        // workflows actually reach a provider.
        chat_client: Some(crate::chat_client::default_chat_client()),
        ..HarnessConfig::default()
    };

    // Load `.stepyard/defaults.yaml` env layer. Missing file → empty defaults
    // (the loader is explicit about that). This is the weakest layer of the
    // cascade; step.env and workflow.env still override.
    let defaults_path = Path::new(".stepyard/defaults.yaml");
    let env_defaults = crate::config::load_env_defaults(defaults_path)
        .with_context(|| format!("failed to load {}", defaults_path.display()))?;
    // Round 3 Story 3 — defaults boundary check. The loader itself (Story
    // 3.3) accepts any well-formed YAML mapping; the env key/value grammar
    // is enforced here so a bad key in `.stepyard/defaults.yaml` surfaces
    // as `EngineError::InvalidWorkflowField` with the same constants the
    // workflow path uses. Path prefix carries the file location so the
    // operator sees where the offender lives, not just `env.<KEY>`.
    stepyard_core::env::validate_env_map(".stepyard/defaults.yaml:env", &env_defaults.env)?;
    let harness_defaults = HarnessDefaults::with_env(env_defaults.env);

    // PR 2 of Task #31: thread the CLI's `--target` and `--var k=v` into the
    // harness so gate conditions can reference `{{ target }}` / `{{ vars.X }}`.
    // Mirrors the v1 path's args.target.first() / args.vars parsing (see line
    // ~353) without reaching into the legacy engine's Context.
    let mut run_vars = std::collections::HashMap::new();
    for kv in &args.vars {
        if let Some((k, v)) = kv.split_once('=') {
            run_vars.insert(k.to_string(), v.to_string());
        }
    }
    let run_context = HarnessRunContext {
        target: args.target.first().cloned().unwrap_or_default(),
        vars: run_vars,
    };
    let planned_workspace_path = workspace_manager.workspace_path(&session_id);

    let mut engine = HarnessEngine::new(config, session, harness_workflow.clone(), lifecycle)
        .with_defaults(harness_defaults)
        .with_run_context(run_context)
        .with_workspace_manager(workspace_manager, branch_strategy, planned_workspace_path);

    // Signal interception lives in `src/signal.rs` (Story 2.2). `main()` races
    // that future against `cli.run(..)` via `tokio::select!`; no per-command
    // spawn. Story 2.3 wires the broadcast receiver arm inside `Engine::step`.

    display::workflow_start(&workflow.name);
    let wf_start = Instant::now();

    // Drive the harness one step at a time so we can print start/finish lines
    // between calls. Each `engine.step()` writes StepStarted + StepCompleted |
    // StepFailed to the session log and returns the outcome.
    let mut step_index = 0usize;
    loop {
        // Peek the upcoming step name from the adapted workflow — the log
        // has not been written yet for it. If we are past the end, the next
        // call to step() will just emit WorkflowCompleted.
        let upcoming = harness_workflow.steps.get(step_index);

        let pb = upcoming.map(|s| display::step_start(&s.name, &s.kind.to_string()));
        let step_start_time = Instant::now();

        // Story 2.3: intercept `SignalReceived` **before** the `?` converts
        // it into an anyhow error. If we let the error propagate, `cli.run`
        // resolves fast and wins `main()`'s `tokio::select!` → exit code 1
        // instead of the canonical 130/143 from `install_handlers`. Render
        // the signal line, drop the engine (releases its broadcast
        // receiver so the grace-loop's `receiver_count()` drops to zero),
        // then park this future forever so the signal-handlers arm wins.
        let outcome = match engine.step().await {
            Ok(o) => o,
            Err(EngineError::StepFailed {
                reason: TerminationReason::SignalReceived(signal),
                ..
            }) => {
                if let Some(pb) = pb {
                    pb.finish_and_clear();
                }
                display::signal_received(&signal);
                drop(engine);
                std::future::pending::<()>().await;
                unreachable!("pending never resolves — install_handlers wins main's select!")
            }
            Err(e) => {
                return Err(anyhow::Error::new(e)
                    .context("stepyard_harness::Engine::step failed"));
            }
        };

        match outcome {
            StepOutcome::StepCompleted { step_name } => {
                if let Some(pb) = pb {
                    display::step_ok(&pb, &step_name, step_start_time.elapsed());
                }
                step_index += 1;
            }
            StepOutcome::StepFailed { step_name, error } => {
                if let Some(pb) = pb {
                    display::step_fail(&pb, &step_name, &error);
                }
                eprintln!("step `{step_name}` failed: {error}");
                std::process::exit(1);
            }
            StepOutcome::WorkflowCompleted => {
                if let Some(pb) = pb {
                    pb.finish_and_clear();
                }
                display::workflow_done(wf_start.elapsed(), step_index);
                return Ok(());
            }
            StepOutcome::Cancelled => {
                if let Some(pb) = pb {
                    pb.finish_and_clear();
                }
                eprintln!("workflow cancelled");
                // 130 is the conventional exit code for SIGINT; SIGTERM maps
                // to 143 but we collapse both here to keep the UX simple.
                std::process::exit(130);
            }
            _ => {
                if let Some(pb) = pb {
                    pb.finish_and_clear();
                }
                eprintln!("unexpected outcome from harness");
                std::process::exit(1);
            }
        }
    }
}

pub async fn execute(
    args: ExecuteArgs,
    shutdown_tx: Arc<broadcast::Sender<()>>,
    shutdown_signal: Arc<OnceLock<String>>,
) -> anyhow::Result<()> {
    let workflow_path = resolve_workflow_path(&args.workflow)?;

    let mut workflow = parser::parse_file(&workflow_path)
        .with_context(|| format!("Failed to parse {}", workflow_path.display()))?;

    // Apply centralized defaults (~/.stepyard/defaults.yaml, .stepyard/config.yaml)
    // Defaults are lowest priority — workflow config overrides them.
    workflow.config = crate::config::apply_defaults(&workflow.config);

    let errors = validator::validate(&workflow);
    if !errors.is_empty() {
        if args.json {
            let json = serde_json::json!({
                "error": format!("{} validation error(s)", errors.len()),
                "details": errors,
                "type": "ValidationError"
            });
            println!("{}", serde_json::to_string_pretty(&json)?);
            std::process::exit(1);
        }
        eprintln!("Validation errors:");
        for e in &errors {
            eprintln!("  - {e}");
        }
        bail!("{} validation error(s) found", errors.len());
    }

    let target = args.target.first().cloned().unwrap_or_default();

    let mut vars = std::collections::HashMap::new();
    for kv in &args.vars {
        if let Some((k, v)) = kv.split_once('=') {
            vars.insert(k.to_string(), serde_json::Value::String(v.to_string()));
        }
    }

    // Resolve sandbox mode: sandbox is ON by default, --no-sandbox disables it
    let sandbox_flag = args.sandbox && !args.no_sandbox;
    let sandbox_mode = sandbox::resolve_mode(
        sandbox_flag,
        &workflow.config.global,
        &workflow.config.agent,
    );

    // Validate Docker availability if sandbox mode is active
    if sandbox_mode != SandboxMode::Disabled {
        if let Err(e) = sandbox::require_docker().await {
            if args.json {
                let json = serde_json::json!({
                    "error": e.to_string(),
                    "type": "SandboxUnavailable"
                });
                println!("{}", serde_json::to_string_pretty(&json)?);
                std::process::exit(1);
            }
            return Err(e);
        }
    }

    // ── Pre-flight: validate required environment variables ──────────────
    // Give clear, actionable errors before starting the workflow so that
    // developers installing via `cargo install` know exactly what's missing.
    validate_environment(&workflow, sandbox_mode != SandboxMode::Disabled, args.json).await?;

    // ── Engine selection (Epic 2 Story 2.4) ──────────────────────────────
    // `--engine v2` drives the workflow through `stepyard_harness::Engine` — a
    // thin step/resume loop that delegates persistence to the Session log and
    // execution to a `SandboxLifecycle`. v2 only supports cmd steps today;
    // the remaining step types stay on the v1 code path below.
    match args.engine.as_str() {
        "v1" => {} // fall through to legacy path
        "v2" => {
            if args.dry_run {
                bail!("--engine v2 does not support --dry-run yet (Story 2.4)");
            }
            return execute_v2(args, workflow, sandbox_mode, shutdown_tx, shutdown_signal).await;
        }
        other => bail!("unknown --engine value `{other}` (expected `v1` or `v2`)"),
    }

    // ── Session setup (Epic 1 Story 1.4) ─────────────────────────────────
    // Dry-run skips DB entirely — it is pure introspection. Any other mode
    // requires a reachable PostgreSQL as documented in ARCHITECTURE.md
    // (anti-invariant "no cold-start without PostgreSQL").
    let session = if args.dry_run {
        None
    } else {
        Some(open_session(&workflow.name, args.json).await?)
    };

    let opts = EngineOptions {
        verbose: args.verbose,
        quiet: args.quiet,
        json: args.json,
        dry_run: args.dry_run,
        resume_from: args.resume.clone(),
        sandbox_mode,
        repo: args.repo.clone(),
        session,
    };

    let mut engine = Engine::with_options(workflow.clone(), target, vars, opts).await;

    // ── Dry-run mode ──────────────────────────────────────────────────────────
    if args.dry_run {
        engine.dry_run();
        return Ok(());
    }

    // ── Execute ───────────────────────────────────────────────────────────────
    let start = Instant::now();

    let run_result = engine.run().await;
    let elapsed = start.elapsed();

    match run_result {
        Ok(output) => {
            if args.json {
                let json_out = engine.json_output("success", elapsed);
                println!("{}", serde_json::to_string_pretty(&json_out)?);
            } else if !args.quiet {
                let text = output.text();
                if !text.is_empty() {
                    println!("\n{text}");
                }
            }
            Ok(())
        }
        Err(e) => {
            if args.json {
                // Determine which step failed from the error message
                let error_str = e.to_string();
                let step_name = extract_failed_step(&error_str);
                let json = serde_json::json!({
                    "error": error_str,
                    "step": step_name,
                    "type": "Fail",
                    "workflow_name": workflow.name,
                    "steps_completed": engine.step_records().len(),
                    "partial_steps": engine.step_records(),
                });
                println!("{}", serde_json::to_string_pretty(&json)?);
                std::process::exit(1);
            }
            Err(e)
        }
    }
}

/// Pre-flight validation: check that required env vars and tools are available.
///
/// This runs **before** any Docker containers or API calls, so the developer
/// gets an instant, actionable error message instead of a cryptic failure
/// mid-workflow.
async fn validate_environment(
    workflow: &crate::workflow::schema::WorkflowDef,
    sandbox_enabled: bool,
    json_mode: bool,
) -> anyhow::Result<()> {
    let mut warnings: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    // ── Check ANTHROPIC_API_KEY (required for chat/map steps) ────────────
    let has_chat_steps = workflow.steps.iter().any(|s| {
        matches!(
            s.step_type,
            crate::workflow::schema::StepType::Chat | crate::workflow::schema::StepType::Map
        )
    });
    if has_chat_steps && std::env::var("ANTHROPIC_API_KEY").is_err() {
        errors.push(
            "ANTHROPIC_API_KEY is not set.\n\
             This workflow uses AI steps (chat/map) that require the Anthropic API.\n\
             \n\
             Set it in your shell profile (~/.zshrc, ~/.bashrc, etc.):\n\
               export ANTHROPIC_API_KEY=\"sk-ant-...\"\n\
             \n\
             Or pass it inline:\n\
               ANTHROPIC_API_KEY=\"sk-ant-...\" stepyard execute <workflow> -- <target>"
                .to_string(),
        );
    }

    // ── Check GH_TOKEN / gh CLI (if workflow uses gh commands) ───────────
    let uses_gh = workflow
        .steps
        .iter()
        .any(|s| s.run.as_deref().is_some_and(|r| r.contains("gh ")));
    if uses_gh && std::env::var("GH_TOKEN").is_err() && std::env::var("GITHUB_TOKEN").is_err() {
        // Check if gh CLI can produce a token (will be auto-detected later).
        // We use `gh auth token` instead of `gh auth status` because the
        // latter returns exit 1 if *any* configured account has a stale
        // token, even when the active account is fine.
        let gh_ok = std::process::Command::new("gh")
            .args(["auth", "token"])
            .stderr(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if !gh_ok {
            errors.push(
                "GitHub authentication is not configured.\n\
                 This workflow uses `gh` CLI commands that require authentication.\n\
                 \n\
                 Option 1 — Authenticate the gh CLI (recommended):\n\
                   gh auth login\n\
                 \n\
                 Option 2 — Set a token manually:\n\
                   export GH_TOKEN=\"ghp_...\"\n\
                 \n\
                 The token needs 'repo' scope for private repositories."
                    .to_string(),
            );
        } else {
            // gh is authenticated — token will be auto-detected in sandbox setup
            warnings.push(
                "GH_TOKEN not in environment — will auto-detect from `gh auth token`.".to_string(),
            );
        }
    }

    // ── Check Docker image exists; auto-build if missing ──────────────
    if sandbox_enabled {
        let image = {
            let cfg = crate::sandbox::SandboxConfig::from_global_config(&workflow.config.global);
            cfg.image().to_string()
        };

        if !crate::sandbox::DockerSandbox::image_exists(&image).await {
            // Auto-build the default image from the embedded Dockerfile
            if image == crate::sandbox::SandboxConfig::DEFAULT_IMAGE {
                if let Err(e) = crate::sandbox::DockerSandbox::auto_build_image(&image).await {
                    errors.push(format!(
                        "Docker image '{image}' not found and auto-build failed:\n  {e}"
                    ));
                }
            } else {
                // Custom image — we can't auto-build it
                errors.push(format!(
                    "Docker image '{image}' not found.\n\
                     \n\
                     Build or pull the image first, or use the default image:\n\
                       config:\n\
                         global:\n\
                           sandbox:\n\
                             image: \"minion-sandbox:latest\""
                ));
            }
        }
    }

    // ── Check stack context variables and prompt files ───────────────────
    {
        let uses_stack_vars = workflow_references_stack_vars(workflow);
        let uses_prompt_vars = workflow_references_prompt_vars(workflow);

        if uses_stack_vars || uses_prompt_vars {
            let registry_path = std::path::Path::new("prompts/registry.yaml");
            if !registry_path.exists() {
                if uses_stack_vars {
                    warnings.push(
                        "Workflow references {{ stack.* }} variables but 'prompts/registry.yaml' \
                         was not found. Stack variables will be empty at runtime.\n\
                         Create prompts/registry.yaml to enable automatic stack detection."
                            .to_string(),
                    );
                }
                if uses_prompt_vars {
                    errors.push(
                        "Workflow references {{ prompts.* }} but 'prompts/registry.yaml' was not found.\n\
                         \n\
                         Create prompts/registry.yaml and add prompt template files, e.g.:\n\
                           prompts/<function>/_default.md.tera"
                            .to_string(),
                    );
                }
            } else {
                // Registry exists — parse it
                match crate::prompts::registry::Registry::from_file(registry_path).await {
                    Err(e) => {
                        errors.push(format!(
                            "Failed to parse 'prompts/registry.yaml': {e}\n\
                             Fix the YAML syntax before running the workflow."
                        ));
                    }
                    Ok(registry) => {
                        // If workflow uses prompts.*, verify prompt files exist after detecting stack
                        if uses_prompt_vars {
                            let workspace = std::path::Path::new(".");
                            match crate::prompts::detector::StackDetector::detect(
                                &registry, workspace,
                            )
                            .await
                            {
                                Err(e) => {
                                    warnings.push(format!(
                                        "Stack detection failed: {e}\n\
                                         Prompt files cannot be validated. Ensure a supported \
                                         project file (e.g. Cargo.toml, package.json) is present."
                                    ));
                                }
                                Ok(stack_info) => {
                                    let prompts_dir =
                                        workflow.prompts_dir.as_deref().unwrap_or("prompts");
                                    let prompts_path = std::path::Path::new(prompts_dir);
                                    let missing = collect_missing_prompts(
                                        workflow,
                                        &stack_info,
                                        prompts_path,
                                    )
                                    .await;
                                    for m in missing {
                                        errors.push(m);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // ── Report results ──────────────────────────────────────────────────
    if !errors.is_empty() {
        if json_mode {
            let json = serde_json::json!({
                "error": "Pre-flight checks failed",
                "type": "EnvironmentError",
                "details": errors,
            });
            println!("{}", serde_json::to_string_pretty(&json)?);
            std::process::exit(1);
        }
        eprintln!("\x1b[31mPre-flight checks failed:\x1b[0m\n");
        for (i, err) in errors.iter().enumerate() {
            if i > 0 {
                eprintln!();
            }
            eprintln!("  \x1b[31m✗\x1b[0m {err}");
        }
        eprintln!();
        bail!("{} pre-flight check(s) failed", errors.len());
    }

    // Print warnings (non-blocking)
    for w in &warnings {
        tracing::info!("{w}");
    }

    Ok(())
}

/// Extract the step name from an error message like "Step 'foo' failed: ..."
fn extract_failed_step(msg: &str) -> Option<&str> {
    let start = msg.find("Step '")?;
    let rest = &msg[start + 6..];
    let end = rest.find('\'')?;
    Some(&rest[..end])
}

/// Return true if any step in the workflow (including scopes) references `{{ stack.` variables.
fn workflow_references_stack_vars(workflow: &crate::workflow::schema::WorkflowDef) -> bool {
    let contains_stack = |s: &str| s.contains("{{ stack.") || s.contains("{{stack.");
    let step_has_stack = |step: &crate::workflow::schema::StepDef| {
        step.run.as_deref().is_some_and(contains_stack)
            || step.prompt.as_deref().is_some_and(contains_stack)
            || step.condition.as_deref().is_some_and(contains_stack)
    };

    if workflow.steps.iter().any(step_has_stack) {
        return true;
    }
    workflow
        .scopes
        .values()
        .any(|scope| scope.steps.iter().any(step_has_stack))
}

/// Return true if any step in the workflow (including scopes) references `{{ prompts.` variables.
fn workflow_references_prompt_vars(workflow: &crate::workflow::schema::WorkflowDef) -> bool {
    let contains_prompts = |s: &str| s.contains("{{ prompts.") || s.contains("{{prompts.");
    let step_has_prompts = |step: &crate::workflow::schema::StepDef| {
        step.run.as_deref().is_some_and(contains_prompts)
            || step.prompt.as_deref().is_some_and(contains_prompts)
            || step.condition.as_deref().is_some_and(contains_prompts)
    };

    if workflow.steps.iter().any(step_has_prompts) {
        return true;
    }
    workflow
        .scopes
        .values()
        .any(|scope| scope.steps.iter().any(step_has_prompts))
}

/// Scan the workflow for `{{ prompts.X }}` references and verify the prompt files exist.
/// Returns a list of error messages for any missing prompt files.
async fn collect_missing_prompts(
    workflow: &crate::workflow::schema::WorkflowDef,
    stack_info: &crate::prompts::detector::StackInfo,
    prompts_dir: &std::path::Path,
) -> Vec<String> {
    let mut missing = Vec::new();
    let mut checked = std::collections::HashSet::new();

    // Collect all prompt function names referenced in a text string
    fn extract_prompt_names(text: &str, checked: &mut std::collections::HashSet<String>) -> Vec<String> {
        let mut names = Vec::new();
        let mut s = text;
        while let Some(pos) = s.find("prompts.") {
            let after = &s[pos + 8..];
            let end = after
                .find(|c: char| c.is_whitespace() || c == '}' || c == '|' || c == '?' || c == '!')
                .unwrap_or(after.len());
            let fn_name = after[..end].trim();
            if !fn_name.is_empty() && checked.insert(fn_name.to_string()) {
                names.push(fn_name.to_string());
            }
            s = &s[pos + 8 + end.min(after.len())..];
        }
        names
    }

    // Collect all prompt function names from all steps
    let mut all_names = Vec::new();
    for step in &workflow.steps {
        if let Some(ref run) = step.run {
            all_names.extend(extract_prompt_names(run, &mut checked));
        }
        if let Some(ref prompt) = step.prompt {
            all_names.extend(extract_prompt_names(prompt, &mut checked));
        }
    }
    for scope in workflow.scopes.values() {
        for step in &scope.steps {
            if let Some(ref run) = step.run {
                all_names.extend(extract_prompt_names(run, &mut checked));
            }
            if let Some(ref prompt) = step.prompt {
                all_names.extend(extract_prompt_names(prompt, &mut checked));
            }
        }
    }

    // Resolve each prompt name asynchronously
    for fn_name in &all_names {
        match crate::prompts::resolver::PromptResolver::resolve(
            fn_name,
            stack_info,
            prompts_dir,
        )
        .await
        {
            Ok(_) => {}
            Err(e) => missing.push(format!("{e}")),
        }
    }

    missing
}

pub async fn validate(args: ValidateArgs) -> anyhow::Result<()> {
    let workflow_path = resolve_workflow_path(&args.workflow)?;

    let workflow = parser::parse_file(&workflow_path)
        .with_context(|| format!("Failed to parse {}", workflow_path.display()))?;

    let errors = validator::validate(&workflow);
    if errors.is_empty() {
        println!("\x1b[32m✓\x1b[0m Workflow is valid: {}", workflow.name);
        Ok(())
    } else {
        eprintln!("Validation errors:");
        for e in &errors {
            eprintln!("  - {e}");
        }
        bail!("{} validation error(s)", errors.len());
    }
}

pub async fn list() -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let mut found = Vec::new();

    // Scan current directory, workflows/ subdirectory, and ~/.stepyard/workflows/
    let mut dirs_to_scan = vec![cwd.clone(), cwd.join("workflows")];
    if let Some(home) = dirs::home_dir() {
        dirs_to_scan.push(home.join(".stepyard").join("workflows"));
    }

    for dir in &dirs_to_scan {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "yaml" || e == "yml")
                    && !found.contains(&path)
                {
                    found.push(path);
                }
            }
        }
    }

    if found.is_empty() {
        println!("No workflow files found.");
        println!(
            "Tip: run `stepyard init <name>` to create a new workflow, or place .yaml files in:"
        );
        println!("  • {} (current directory)", cwd.display());
        println!("  • {}/workflows/", cwd.display());
        if let Some(home) = dirs::home_dir() {
            println!("  • {}/.stepyard/workflows/", home.display());
        }
    } else {
        println!("Available workflows:");
        for path in &found {
            if let Ok(wf) = parser::parse_file(path) {
                println!(
                    "  {} — {} ({} steps)",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    wf.description.as_deref().unwrap_or("no description"),
                    wf.steps.len()
                );
            } else {
                println!(
                    "  {} (parse error)",
                    path.file_name().unwrap_or_default().to_string_lossy()
                );
            }
        }
    }

    Ok(())
}

pub async fn init(args: InitArgs) -> anyhow::Result<()> {
    let available = init_templates::names();
    let template = init_templates::get(&args.template).ok_or_else(|| {
        anyhow::anyhow!(
            "Unknown template '{}'. Available: {}",
            args.template,
            available.join(", ")
        )
    })?;

    let filename = if args.name.ends_with(".yaml") || args.name.ends_with(".yml") {
        args.name.clone()
    } else {
        format!("{}.yaml", args.name)
    };

    let out_dir = args
        .output
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let out_path = out_dir.join(&filename);

    if out_path.exists() {
        bail!("File already exists: {}", out_path.display());
    }

    let content = template.content.replace("{name}", &args.name);
    std::fs::write(&out_path, &content)
        .with_context(|| format!("Failed to write {}", out_path.display()))?;

    println!(
        "\x1b[32m✓\x1b[0m Created workflow '{}' from template '{}'",
        out_path.display(),
        template.name
    );
    println!("  Template: {}", template.description);
    println!("\nEdit the file and run:");
    println!("  stepyard validate {}", out_path.display());
    println!("  stepyard execute {} -- <target>", out_path.display());

    Ok(())
}

pub async fn inspect(args: InspectArgs) -> anyhow::Result<()> {
    let workflow_path = resolve_workflow_path(&args.workflow)?;

    let workflow = parser::parse_file(&workflow_path)
        .with_context(|| format!("Failed to parse {}", workflow_path.display()))?;

    // ── Header ──────────────────────────────────────────────────────────────
    println!("\x1b[1m=== Workflow: {} ===\x1b[0m", workflow.name);
    if let Some(desc) = &workflow.description {
        println!("Description: {desc}");
    }
    if workflow.version > 0 {
        println!("Version: {}", workflow.version);
    }
    println!();

    // ── Validation ──────────────────────────────────────────────────────────
    let errors = validator::validate(&workflow);
    if errors.is_empty() {
        println!("\x1b[32m✓ Validation passed\x1b[0m");
    } else {
        println!("\x1b[31m✗ Validation errors:\x1b[0m");
        for e in &errors {
            println!("  - {e}");
        }
    }
    println!();

    // ── Config (resolved global) ─────────────────────────────────────────────
    let cfg = &workflow.config;
    let has_config = !cfg.global.is_empty()
        || !cfg.agent.is_empty()
        || !cfg.cmd.is_empty()
        || !cfg.chat.is_empty()
        || !cfg.gate.is_empty()
        || !cfg.patterns.is_empty();

    if has_config {
        println!("\x1b[1mConfig layers:\x1b[0m");
        if !cfg.global.is_empty() {
            println!("  global:");
            for (k, v) in &cfg.global {
                println!("    {k}: {v:?}");
            }
        }
        if !cfg.agent.is_empty() {
            println!("  agent:");
            for (k, v) in &cfg.agent {
                println!("    {k}: {v:?}");
            }
        }
        if !cfg.cmd.is_empty() {
            println!("  cmd:");
            for (k, v) in &cfg.cmd {
                println!("    {k}: {v:?}");
            }
        }
        if !cfg.patterns.is_empty() {
            println!("  patterns: {} pattern(s)", cfg.patterns.len());
        }
        println!();
    }

    // ── Scopes ───────────────────────────────────────────────────────────────
    if !workflow.scopes.is_empty() {
        println!("\x1b[1mScopes ({}):\x1b[0m", workflow.scopes.len());
        for (name, scope) in &workflow.scopes {
            println!(
                "  {name}: {} step(s){}",
                scope.steps.len(),
                if scope.outputs.is_some() {
                    " [has outputs]"
                } else {
                    ""
                }
            );
        }
        println!();
    }

    // ── Dependency graph (step order + scope references) ─────────────────────
    println!("\x1b[1mStep dependency graph:\x1b[0m");
    for (i, step) in workflow.steps.iter().enumerate() {
        let connector = if i + 1 < workflow.steps.len() {
            "├──"
        } else {
            "└──"
        };
        let type_label = match step.scope.as_deref() {
            Some(scope) => format!("{} → scope:{}", step.step_type, scope),
            None => step.step_type.to_string(),
        };
        println!("  {connector} [{}] {} ({})", i + 1, step.name, type_label);
    }
    println!();

    // ── Dry-run summary ──────────────────────────────────────────────────────
    println!("\x1b[1mDry-run summary:\x1b[0m");
    println!("  Total steps : {}", workflow.steps.len());

    let type_counts = {
        let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for step in &workflow.steps {
            *counts.entry(step.step_type.to_string()).or_insert(0) += 1;
        }
        counts
    };
    let mut type_list: Vec<_> = type_counts.iter().collect();
    type_list.sort_by_key(|(k, _)| k.as_str());
    for (t, n) in &type_list {
        println!("    {t}: {n}");
    }
    println!("  Scopes      : {}", workflow.scopes.len());
    if !errors.is_empty() {
        println!(
            "  \x1b[31mValidation  : {} error(s) — fix before running\x1b[0m",
            errors.len()
        );
    } else {
        println!("  Validation  : \x1b[32mok\x1b[0m");
    }

    Ok(())
}

// ── Config subcommands ───────────────────────────────────────────────────────

/// Default content for a new ~/.stepyard/defaults.yaml
const USER_DEFAULTS_TEMPLATE: &str = r#"# ============================================================
# Stepyard — User Default Configuration
# ============================================================
#
# This file overrides the built-in defaults for ALL workflows.
# Edit the values below to customize your setup.
#
# Priority (lowest → highest):
#   Built-in defaults (compiled in binary)
#   This file (~/.stepyard/defaults.yaml)     ← you are here
#   .stepyard/config.yaml (project-level)
#   workflow.yaml config:
#   step inline config:
# ============================================================

# Global settings
global:
  timeout: 300s

# AI Agent (Claude Code CLI) settings
agent:
  command: claude
  model: claude-sonnet-4-20250514
  flags:
    - "-p"
    - "--output-format"
    - "stream-json"
  permissions: skip

# Chat API (Anthropic) settings
chat:
  provider: anthropic
  model: claude-sonnet-4-20250514
  api_key_env: ANTHROPIC_API_KEY
  temperature: 0.2
  max_tokens: 4096

# Shell command settings
cmd:
  fail_on_error: true
  timeout: 60s
"#;

pub async fn config_show() -> anyhow::Result<()> {
    let defaults = crate::config::defaults::load_defaults();

    println!("\x1b[1m=== Effective Configuration ===\x1b[0m");
    println!("(merged: embedded + user + project)\n");

    let print_section = |name: &str, map: &std::collections::HashMap<String, serde_yaml::Value>| {
        if !map.is_empty() {
            println!("\x1b[1m{name}:\x1b[0m");
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort();
            for k in keys {
                let v = &map[k];
                // Format value nicely
                let display = match v {
                    serde_yaml::Value::String(s) => s.clone(),
                    serde_yaml::Value::Bool(b) => b.to_string(),
                    serde_yaml::Value::Number(n) => n.to_string(),
                    other => format!("{other:?}"),
                };
                println!("  {k}: {display}");
            }
            println!();
        }
    };

    print_section("global", &defaults.global);
    print_section("agent", &defaults.agent);
    print_section("chat", &defaults.chat);
    print_section("cmd", &defaults.cmd);
    print_section("gate", &defaults.gate);

    if !defaults.patterns.is_empty() {
        println!("\x1b[1mpatterns:\x1b[0m");
        for (pattern, values) in &defaults.patterns {
            println!("  {pattern}:");
            for (k, v) in values {
                println!("    {k}: {v:?}");
            }
        }
        println!();
    }

    Ok(())
}

pub async fn config_path() -> anyhow::Result<()> {
    println!("\x1b[1m=== Configuration File Locations ===\x1b[0m\n");

    // 1. Embedded
    println!("  \x1b[32m✓\x1b[0m Built-in defaults (compiled in binary) — always active");

    // 2. User-level
    if let Some(home) = dirs::home_dir() {
        let path = home.join(".stepyard").join("defaults.yaml");
        if path.exists() {
            println!("  \x1b[32m✓\x1b[0m {} — active", path.display());
        } else {
            println!("  \x1b[90m○\x1b[0m {} — not created", path.display());
            println!("    Run `stepyard config init` to create it");
        }
    }

    // 3. Project-level
    if let Ok(cwd) = std::env::current_dir() {
        let path = cwd.join(".stepyard").join("config.yaml");
        if path.exists() {
            println!("  \x1b[32m✓\x1b[0m {} — active", path.display());
        } else {
            println!("  \x1b[90m○\x1b[0m {} — not created", path.display());
        }
    }

    println!();
    println!("\x1b[1mPriority order\x1b[0m (lowest → highest):");
    println!("  embedded → ~/.stepyard/defaults.yaml → .stepyard/config.yaml → workflow YAML → step inline");

    Ok(())
}

pub async fn config_init() -> anyhow::Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    let dir = home.join(".stepyard");
    let path = dir.join("defaults.yaml");

    if path.exists() {
        println!("\x1b[33m!\x1b[0m Config file already exists: {}", path.display());
        println!("  Edit it directly: {}", path.display());
        println!("  Or delete it and run `stepyard config init` again.");
        return Ok(());
    }

    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create {}", dir.display()))?;

    std::fs::write(&path, USER_DEFAULTS_TEMPLATE)
        .with_context(|| format!("Failed to write {}", path.display()))?;

    println!("\x1b[32m✓\x1b[0m Created: {}", path.display());
    println!();
    println!("Edit this file to change defaults for all workflows.");
    println!("For example, to switch to Claude Opus:");
    println!();
    println!("  chat:");
    println!("    model: claude-opus-4-20250514");
    println!("  agent:");
    println!("    model: claude-opus-4-20250514");

    Ok(())
}

pub async fn config_set(key: &str, value: &str) -> anyhow::Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    let dir = home.join(".stepyard");
    let path = dir.join("defaults.yaml");

    // Parse key: "chat.model" → section="chat", field="model"
    let parts: Vec<&str> = key.splitn(2, '.').collect();
    if parts.len() != 2 {
        bail!(
            "Invalid key format: '{}'\n\
             Expected: section.field (e.g., chat.model, agent.model, global.timeout)\n\
             \n\
             Valid sections: global, agent, chat, cmd, gate",
            key
        );
    }
    let (section, field) = (parts[0], parts[1]);

    // Validate section
    let valid_sections = ["global", "agent", "chat", "cmd", "gate"];
    if !valid_sections.contains(&section) {
        bail!(
            "Unknown section: '{}'\nValid sections: {}",
            section,
            valid_sections.join(", ")
        );
    }

    // Load existing or create new
    let mut config: serde_yaml::Value = if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        serde_yaml::from_str(&content)?
    } else {
        std::fs::create_dir_all(&dir)?;
        serde_yaml::Value::Mapping(serde_yaml::Mapping::new())
    };

    // Set the value
    let mapping = config
        .as_mapping_mut()
        .ok_or_else(|| anyhow::anyhow!("Config file is not a YAML mapping"))?;

    let section_key = serde_yaml::Value::String(section.to_string());
    let section_map = mapping
        .entry(section_key)
        .or_insert_with(|| serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));

    let section_mapping = section_map
        .as_mapping_mut()
        .ok_or_else(|| anyhow::anyhow!("Section '{}' is not a YAML mapping", section))?;

    section_mapping.insert(
        serde_yaml::Value::String(field.to_string()),
        serde_yaml::Value::String(value.to_string()),
    );

    // Write back
    let yaml_str = serde_yaml::to_string(&config)?;
    std::fs::write(&path, &yaml_str)?;

    println!("\x1b[32m✓\x1b[0m Set {}.{} = {} in {}", section, field, value, path.display());

    Ok(())
}

/// One row of `session list` output: `(id, status, started_at, ended_at)`.
type SessionListRow = (uuid::Uuid, String, DateTime<Utc>, Option<DateTime<Utc>>);

/// `stepyard session list --status <status> [--since <duration>]` — FR24 / Story 2.5.
///
/// Queries PG directly for the requested status filter, printing one
/// whitespace-separated row per session:
/// `<id>  <status>  <started_at ISO-8601 UTC>  <ended_at ISO-8601 UTC or '-'>`.
///
/// `--since` binds a `"<seconds> seconds"` string cast to `INTERVAL` via the
/// `$2::INTERVAL` cast — PG's `INTERVAL` parser accepts that literal form
/// (`humantime` emits `24h`, which PG would reject).
pub async fn session_list(args: SessionListArgs) -> anyhow::Result<()> {
    // session-log-as-truth (D1): query PG, never an in-memory registry
    let pool = connect_pg(false).await?;

    let status_str = args.status.as_db_str();

    let rows: Vec<SessionListRow> = if let Some(since) = args.since {
        let interval_str = format!("{} seconds", std::time::Duration::from(since).as_secs());
        sqlx::query_as(
            r#"
            SELECT id, status, started_at, ended_at
            FROM sessions
            WHERE status = $1 AND started_at > NOW() - $2::INTERVAL
            ORDER BY started_at DESC
            "#,
        )
        .bind(status_str)
        .bind(interval_str)
        .fetch_all(&pool)
        .await
        .context("failed to list sessions")?
    } else {
        sqlx::query_as(
            r#"
            SELECT id, status, started_at, ended_at
            FROM sessions
            WHERE status = $1
            ORDER BY started_at DESC
            "#,
        )
        .bind(status_str)
        .fetch_all(&pool)
        .await
        .context("failed to list sessions")?
    };

    for (id, status, started_at, ended_at) in rows {
        let ended = ended_at
            .map(|t| t.to_rfc3339())
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{}  {}  {}  {}",
            id,
            status,
            started_at.to_rfc3339(),
            ended,
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_failed_step_parses_correctly() {
        let msg = "Step 'lint' failed: exit code 1";
        assert_eq!(extract_failed_step(msg), Some("lint"));
    }

    #[test]
    fn extract_failed_step_returns_none_on_no_match() {
        assert_eq!(extract_failed_step("some other error"), None);
    }

    #[test]
    fn session_status_as_db_str_matches_db_constraint() {
        assert_eq!(SessionStatus::Running.as_db_str(), "running");
        assert_eq!(SessionStatus::Completed.as_db_str(), "completed");
        assert_eq!(SessionStatus::Failed.as_db_str(), "failed");
        assert_eq!(SessionStatus::Cancelled.as_db_str(), "cancelled");
    }
}
