use std::path::PathBuf;

use anyhow::{Context, bail};
use clap::Args;

use crate::engine::Engine;
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

    /// Output results as JSON
    #[arg(long)]
    pub json: bool,

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

    let mut engine = Engine::new(workflow, target, vars, args.verbose, args.quiet);
    let output = engine.run().await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else if !args.quiet {
        let text = output.text();
        if !text.is_empty() {
            println!("\n{text}");
        }
    }

    Ok(())
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

    // Scan current directory and workflows/ subdirectory
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
