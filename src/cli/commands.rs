use std::path::PathBuf;
use std::time::Instant;

use anyhow::{bail, Context};
use clap::Args;

use crate::engine::{Engine, EngineOptions};
use crate::sandbox::{self, SandboxMode};
use crate::workflow::parser;
use crate::workflow::validator;

use super::init_templates;

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
    let workflow_path = &args.workflow;

    if !workflow_path.exists() {
        bail!("Workflow file not found: {}", workflow_path.display());
    }

    let workflow = parser::parse_file(workflow_path)
        .with_context(|| format!("Failed to parse {}", workflow_path.display()))?;

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
    validate_environment(&workflow, sandbox_mode != SandboxMode::Disabled, args.json)?;

    let opts = EngineOptions {
        verbose: args.verbose,
        quiet: args.quiet,
        json: args.json,
        dry_run: args.dry_run,
        resume_from: args.resume.clone(),
        sandbox_mode,
    };

    let mut engine = Engine::with_options(workflow.clone(), target, vars, opts);

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
fn validate_environment(
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
        .any(|s| s.run.as_deref().map_or(false, |r| r.contains("gh ")));
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

    // ── Check Docker image exists (if sandbox) ──────────────────────────
    if sandbox_enabled {
        let image = {
            let cfg = crate::sandbox::SandboxConfig::from_global_config(&workflow.config.global);
            cfg.image().to_string()
        };
        let image_exists = std::process::Command::new("docker")
            .args(["image", "inspect", &image])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if !image_exists {
            errors.push(format!(
                "Docker image '{image}' not found.\n\
                 \n\
                 Build it first:\n\
                   docker build -t {image} .\n\
                 \n\
                 Or specify a different image in your workflow config:\n\
                   config:\n\
                     global:\n\
                       sandbox:\n\
                         image: \"ubuntu:22.04\""
            ));
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
                match crate::prompts::registry::Registry::from_file(registry_path) {
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
                            ) {
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
                                    );
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
        step.run.as_deref().map_or(false, contains_stack)
            || step.prompt.as_deref().map_or(false, contains_stack)
            || step.condition.as_deref().map_or(false, contains_stack)
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
        step.run.as_deref().map_or(false, contains_prompts)
            || step.prompt.as_deref().map_or(false, contains_prompts)
            || step.condition.as_deref().map_or(false, contains_prompts)
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
fn collect_missing_prompts(
    workflow: &crate::workflow::schema::WorkflowDef,
    stack_info: &crate::prompts::detector::StackInfo,
    prompts_dir: &std::path::Path,
) -> Vec<String> {
    let mut missing = Vec::new();
    let mut checked = std::collections::HashSet::new();

    let check_text = |text: &str,
                      missing: &mut Vec<String>,
                      checked: &mut std::collections::HashSet<String>| {
        let mut s = text;
        while let Some(pos) = s.find("prompts.") {
            let after = &s[pos + 8..];
            // Extract function name: read until whitespace, `}}`, `|`, or `?` or `!`
            let end = after
                .find(|c: char| c.is_whitespace() || c == '}' || c == '|' || c == '?' || c == '!')
                .unwrap_or(after.len());
            let fn_name = after[..end].trim();
            if !fn_name.is_empty() && checked.insert(fn_name.to_string()) {
                match crate::prompts::resolver::PromptResolver::resolve(
                    fn_name,
                    stack_info,
                    prompts_dir,
                ) {
                    Ok(_) => {}
                    Err(e) => missing.push(format!("{e}")),
                }
            }
            s = &s[pos + 8 + end.min(after.len())..];
        }
    };

    let scan_step = |step: &crate::workflow::schema::StepDef,
                     missing: &mut Vec<String>,
                     checked: &mut std::collections::HashSet<String>| {
        if let Some(ref run) = step.run {
            check_text(run, missing, checked);
        }
        if let Some(ref prompt) = step.prompt {
            check_text(prompt, missing, checked);
        }
    };

    for step in &workflow.steps {
        scan_step(step, &mut missing, &mut checked);
    }
    for scope in workflow.scopes.values() {
        for step in &scope.steps {
            scan_step(step, &mut missing, &mut checked);
        }
    }

    missing
}

pub async fn validate(args: ValidateArgs) -> anyhow::Result<()> {
    if !args.workflow.exists() {
        bail!("Workflow file not found: {}", args.workflow.display());
    }

    let workflow = parser::parse_file(&args.workflow)
        .with_context(|| format!("Failed to parse {}", args.workflow.display()))?;

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
                if path.extension().is_some_and(|e| e == "yaml" || e == "yml") {
                    if !found.contains(&path) {
                        found.push(path);
                    }
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
    if !args.workflow.exists() {
        bail!("Workflow file not found: {}", args.workflow.display());
    }

    let workflow = parser::parse_file(&args.workflow)
        .with_context(|| format!("Failed to parse {}", args.workflow.display()))?;

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
