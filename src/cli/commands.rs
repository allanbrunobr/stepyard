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

    /// Run entire workflow inside a Docker sandbox
    #[arg(long)]
    pub sandbox: bool,

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

    // Resolve sandbox mode
    let sandbox_mode = sandbox::resolve_mode(
        args.sandbox,
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

/// Extract the step name from an error message like "Step 'foo' failed: ..."
fn extract_failed_step(msg: &str) -> Option<&str> {
    let start = msg.find("Step '")?;
    let rest = &msg[start + 6..];
    let end = rest.find('\'')?;
    Some(&rest[..end])
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

    let out_dir = args.output.unwrap_or_else(|| std::env::current_dir().unwrap());
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
