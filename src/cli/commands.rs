use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use clap::Args;

use crate::engine::{Engine, EngineOptions};
use crate::sandbox::{self, SandboxMode};
use crate::workflow::parser;
use crate::workflow::validator;

use super::init_templates;

/// Connect to PostgreSQL, run Session migrations, and open a Session for this
/// workflow dispatch.
///
/// Returns a clear error (not `anyhow!`) when DATABASE_URL is missing or the
/// database is unreachable — this fulfills Story 1.4 AC:
/// "DATABASE_URL pointing to a PG that is down -> exit != 0 with 'engine
/// requires PostgreSQL backend'."
async fn open_session(
    workflow_name: &str,
    json_mode: bool,
) -> anyhow::Result<minion_session::Session> {
    let db_url = std::env::var("DATABASE_URL").map_err(|_| {
        let msg = "engine requires PostgreSQL backend: DATABASE_URL env var is not set";
        if json_mode {
            let json = serde_json::json!({"error": msg, "type": "ConfigError"});
            println!("{}", serde_json::to_string_pretty(&json).unwrap_or_default());
        } else {
            eprintln!("{msg}");
            eprintln!(
                "Hint: export DATABASE_URL=postgres://user:password@host:port/database"
            );
        }
        anyhow::anyhow!("DATABASE_URL not set")
    })?;

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&db_url)
        .await
        .map_err(|e| {
            let msg = format!("engine requires PostgreSQL backend: cannot reach database: {e}");
            if json_mode {
                let json = serde_json::json!({"error": msg, "type": "DatabaseUnreachable"});
                println!("{}", serde_json::to_string_pretty(&json).unwrap_or_default());
            } else {
                eprintln!("{msg}");
            }
            anyhow::anyhow!("DATABASE_URL unreachable: {e}")
        })?;

    minion_session::migrate(&pool)
        .await
        .with_context(|| "engine requires PostgreSQL backend: migrations failed")?;

    // Workflow identifier — stable UUID derived from the workflow name so that
    // the same workflow name always maps to the same workflow_id row. A real
    // workflows table (Story 2.x) will replace this with an opaque lookup.
    let workflow_id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, workflow_name.as_bytes());
    let tenant_id = std::env::var("MINION_TENANT").unwrap_or_else(|_| "default".to_string());

    minion_session::Session::new(&pool, workflow_id, tenant_id)
        .await
        .with_context(|| "failed to create session row")
}

/// Resolve a workflow path with fallback chain:
/// 1. As-is (if the file exists — developer running from repo or absolute path)
/// 2. `~/.minion/workflows/<filename>` (cargo install users)
fn resolve_workflow_path(path: &PathBuf) -> anyhow::Result<PathBuf> {
    // If the file exists as specified, use it
    if path.exists() {
        return Ok(path.clone());
    }

    // Try ~/.minion/workflows/<filename>
    if let Some(filename) = path.file_name() {
        if let Some(home) = dirs::home_dir() {
            let home_path = home.join(".minion").join("workflows").join(filename);
            if home_path.exists() {
                return Ok(home_path);
            }
        }
    }

    bail!("Workflow file not found: {}\n  Hint: run `minion slack start` once to extract built-in workflows to ~/.minion/workflows/", path.display())
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

    /// GitHub repository (OWNER/REPO) to clone inside the Docker sandbox.
    /// When set, the sandbox clones this repo instead of copying the host CWD.
    /// Example: --repo allanbrunobr/minion-engine
    #[arg(long, value_name = "OWNER/REPO")]
    pub repo: Option<String>,
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

pub async fn execute(args: ExecuteArgs) -> anyhow::Result<()> {
    let workflow_path = resolve_workflow_path(&args.workflow)?;

    let mut workflow = parser::parse_file(&workflow_path)
        .with_context(|| format!("Failed to parse {}", workflow_path.display()))?;

    // Apply centralized defaults (~/.minion/defaults.yaml, .minion/config.yaml)
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
               ANTHROPIC_API_KEY=\"sk-ant-...\" minion execute <workflow> -- <target>"
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

    // Scan current directory, workflows/ subdirectory, and ~/.minion/workflows/
    let mut dirs_to_scan = vec![cwd.clone(), cwd.join("workflows")];
    if let Some(home) = dirs::home_dir() {
        dirs_to_scan.push(home.join(".minion").join("workflows"));
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
            "Tip: run `minion init <name>` to create a new workflow, or place .yaml files in:"
        );
        println!("  • {} (current directory)", cwd.display());
        println!("  • {}/workflows/", cwd.display());
        if let Some(home) = dirs::home_dir() {
            println!("  • {}/.minion/workflows/", home.display());
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
    println!("  minion validate {}", out_path.display());
    println!("  minion execute {} -- <target>", out_path.display());

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

/// Default content for a new ~/.minion/defaults.yaml
const USER_DEFAULTS_TEMPLATE: &str = r#"# ============================================================
# Minion Engine — User Default Configuration
# ============================================================
#
# This file overrides the built-in defaults for ALL workflows.
# Edit the values below to customize your setup.
#
# Priority (lowest → highest):
#   Built-in defaults (compiled in binary)
#   This file (~/.minion/defaults.yaml)     ← you are here
#   .minion/config.yaml (project-level)
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
        let path = home.join(".minion").join("defaults.yaml");
        if path.exists() {
            println!("  \x1b[32m✓\x1b[0m {} — active", path.display());
        } else {
            println!("  \x1b[90m○\x1b[0m {} — not created", path.display());
            println!("    Run `minion config init` to create it");
        }
    }

    // 3. Project-level
    if let Ok(cwd) = std::env::current_dir() {
        let path = cwd.join(".minion").join("config.yaml");
        if path.exists() {
            println!("  \x1b[32m✓\x1b[0m {} — active", path.display());
        } else {
            println!("  \x1b[90m○\x1b[0m {} — not created", path.display());
        }
    }

    println!();
    println!("\x1b[1mPriority order\x1b[0m (lowest → highest):");
    println!("  embedded → ~/.minion/defaults.yaml → .minion/config.yaml → workflow YAML → step inline");

    Ok(())
}

pub async fn config_init() -> anyhow::Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    let dir = home.join(".minion");
    let path = dir.join("defaults.yaml");

    if path.exists() {
        println!("\x1b[33m!\x1b[0m Config file already exists: {}", path.display());
        println!("  Edit it directly: {}", path.display());
        println!("  Or delete it and run `minion config init` again.");
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
    let dir = home.join(".minion");
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
}
