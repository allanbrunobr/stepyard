use std::path::PathBuf;
use std::time::Instant;

use anyhow::{bail, Context};
use clap::Args;

use crate::engine::{Engine, EngineOptions};
use crate::sandbox::{self, SandboxMode};
use crate::workflow::parser;
use crate::workflow::validator;

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

    let dirs_to_scan = [cwd.clone(), cwd.join("workflows")];
    for dir in &dirs_to_scan {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "yaml" || e == "yml") {
                    found.push(path);
                }
            }
        }
    }

    if found.is_empty() {
        println!("No workflow files found in current directory.");
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
