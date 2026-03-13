pub mod context;
pub mod state;
mod template;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use colored::Colorize;
use serde::Serialize;
use tokio::sync::Mutex;

use crate::cli::display;
use crate::config::{ConfigManager, StepConfig};
use crate::control_flow::ControlFlow;
use crate::error::StepError;
use crate::sandbox::config::SandboxConfig;
use crate::sandbox::docker::DockerSandbox;
use crate::sandbox::SandboxMode;
use crate::steps::*;
use crate::steps::{
    agent::AgentExecutor, cmd::CmdExecutor, gate::GateExecutor, repeat::RepeatExecutor,
};
use crate::workflow::schema::{OutputType, StepDef, StepType, WorkflowDef};
use context::Context;
use state::WorkflowState;

/// Options for configuring the Engine
#[derive(Debug, Default)]
pub struct EngineOptions {
    pub verbose: bool,
    pub quiet: bool,
    /// Suppress display and emit JSON summary at end
    pub json: bool,
    /// Skip execution and show step tree
    pub dry_run: bool,
    /// Resume from this step name (requires a state file)
    pub resume_from: Option<String>,
    /// Sandbox mode resolved from CLI + config
    pub sandbox_mode: SandboxMode,
}

/// Per-step execution record collected for JSON output
#[derive(Debug, Clone, Serialize)]
pub struct StepRecord {
    pub name: String,
    pub step_type: String,
    pub status: String,
    pub duration_secs: f64,
    pub output_summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// Whether this step ran inside the Docker sandbox
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub sandboxed: bool,
}

/// Full workflow JSON output (--json mode)
#[derive(Debug, Serialize)]
pub struct WorkflowJsonOutput {
    pub workflow_name: String,
    pub status: String,
    pub sandbox_mode: String,
    pub steps: Vec<StepRecord>,
    pub total_duration_secs: f64,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
}

pub struct Engine {
    pub workflow: WorkflowDef,
    pub context: Context,
    config_manager: ConfigManager,
    pub verbose: bool,
    pub quiet: bool,
    pub json: bool,
    pub dry_run: bool,
    resume_from: Option<String>,
    sandbox_mode: SandboxMode,
    /// Shared Docker sandbox instance (created during run(), destroyed on completion)
    sandbox: SharedSandbox,
    step_records: Vec<StepRecord>,
    state: Option<WorkflowState>,
    state_file: Option<PathBuf>,
}

impl Engine {
    pub fn new(
        workflow: WorkflowDef,
        target: String,
        vars: HashMap<String, serde_json::Value>,
        verbose: bool,
        quiet: bool,
    ) -> Self {
        let options = EngineOptions {
            verbose,
            quiet,
            ..Default::default()
        };
        Self::with_options(workflow, target, vars, options)
    }

    pub fn with_options(
        workflow: WorkflowDef,
        target: String,
        vars: HashMap<String, serde_json::Value>,
        options: EngineOptions,
    ) -> Self {
        let context = Context::new(target, vars);
        let config_manager = ConfigManager::new(workflow.config.clone());
        // JSON mode implies quiet (no decorative output)
        let quiet = options.quiet || options.json;
        Self {
            workflow,
            context,
            config_manager,
            verbose: options.verbose,
            quiet,
            json: options.json,
            dry_run: options.dry_run,
            resume_from: options.resume_from,
            sandbox_mode: options.sandbox_mode,
            sandbox: None,
            step_records: Vec::new(),
            state: None,
            state_file: None,
        }
    }

    /// Return collected step records (for JSON output or testing)
    pub fn step_records(&self) -> &[StepRecord] {
        &self.step_records
    }

    /// Build the JSON summary after execution
    pub fn json_output(&self, status: &str, total_duration: Duration) -> WorkflowJsonOutput {
        let total_tokens: u64 = self
            .step_records
            .iter()
            .map(|r| r.input_tokens.unwrap_or(0) + r.output_tokens.unwrap_or(0))
            .sum();
        let total_cost: f64 = self
            .step_records
            .iter()
            .filter_map(|r| r.cost_usd)
            .sum();

        WorkflowJsonOutput {
            workflow_name: self.workflow.name.clone(),
            status: status.to_string(),
            sandbox_mode: format!("{:?}", self.sandbox_mode),
            steps: self.step_records.clone(),
            total_duration_secs: total_duration.as_secs_f64(),
            total_tokens,
            total_cost_usd: total_cost,
        }
    }

    // ── Sandbox Lifecycle ────────────────────────────────────────────────────

    /// Create and start the Docker sandbox container.
    /// Copies the current working directory into the container as /workspace.
    async fn sandbox_up(&mut self) -> Result<()> {
        let sandbox_config = SandboxConfig::from_global_config(&self.workflow.config.global);
        let workspace = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".to_string());

        let mut docker = DockerSandbox::new(sandbox_config, &workspace);

        if !self.quiet {
            println!("  {} Creating Docker sandbox container…", "🐳".cyan());
        }

        docker.create().await?;
        docker.copy_workspace(&workspace).await?;

        if !self.quiet {
            println!(
                "  {} Sandbox ready — workspace copied to container",
                "🔒".green()
            );
        }

        self.sandbox = Some(Arc::new(Mutex::new(docker)));
        Ok(())
    }

    /// Copy results from sandbox back to host, then destroy the container.
    async fn sandbox_down(&mut self) -> Result<()> {
        if let Some(sb) = self.sandbox.take() {
            let mut docker = sb.lock().await;

            let workspace = std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".to_string());

            if !self.quiet {
                println!("  {} Copying results from sandbox…", "📦".cyan());
            }

            docker.copy_results(&workspace).await?;
            docker.destroy().await?;

            if !self.quiet {
                println!("  {} Sandbox destroyed", "🗑️ ".dimmed());
            }
        }
        Ok(())
    }

    /// Determine whether a step should run inside the sandbox based on sandbox_mode
    fn should_sandbox_step(&self, step_type: &StepType) -> bool {
        match self.sandbox_mode {
            SandboxMode::Disabled => false,
            SandboxMode::FullWorkflow | SandboxMode::Devbox => {
                // ALL executable steps run inside the sandbox
                matches!(step_type, StepType::Cmd | StepType::Agent)
            }
            SandboxMode::AgentOnly => {
                // Only agent steps run inside the sandbox; cmd steps run on host
                matches!(step_type, StepType::Agent)
            }
        }
    }

    // ── Main Run Loop ────────────────────────────────────────────────────────

    pub async fn run(&mut self) -> Result<StepOutput> {
        // ── State / Resume setup ──────────────────────────────────────────────
        let state_file = WorkflowState::state_file_path(&self.workflow.name);
        self.state_file = Some(state_file.clone());

        let mut loaded_state: Option<WorkflowState> = None;
        if let Some(ref resume_step) = self.resume_from.clone() {
            match WorkflowState::find_latest(&self.workflow.name) {
                Some(path) => {
                    match WorkflowState::load(&path) {
                        Ok(s) => {
                            let exists = self.workflow.steps.iter().any(|s| &s.name == resume_step);
                            if !exists {
                                bail!(
                                    "Resume step '{}' not found in workflow '{}'. \
                                     Available steps: {}",
                                    resume_step,
                                    self.workflow.name,
                                    self.workflow.steps.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")
                                );
                            }
                            if !self.quiet {
                                println!(
                                    "  {} Resuming from step '{}' (state: {})",
                                    "↺".cyan(),
                                    resume_step,
                                    path.display()
                                );
                            }
                            loaded_state = Some(s);
                        }
                        Err(e) => bail!("Failed to load state file {}: {e}", path.display()),
                    }
                }
                None => {
                    bail!(
                        "No state file found for workflow '{}'. \
                         Cannot resume. Run the workflow without --resume first.",
                        self.workflow.name
                    );
                }
            }
        }

        // ── Initialize persisted state for this run ───────────────────────────
        let mut current_state = WorkflowState::new(&self.workflow.name);

        // ── Display ───────────────────────────────────────────────────────────
        if !self.quiet {
            display::workflow_start(&self.workflow.name);
            if self.sandbox_mode != SandboxMode::Disabled {
                println!("  {} Sandbox mode: {:?}", "🔒".cyan(), self.sandbox_mode);
            }
        }

        // ── Sandbox: Create container BEFORE step execution ───────────────────
        if self.sandbox_mode != SandboxMode::Disabled {
            self.sandbox_up().await?;
        }

        let start = Instant::now();
        let steps = self.workflow.steps.clone();
        let mut last_output = StepOutput::Empty;
        let mut step_count = 0;

        let resume_from = self.resume_from.clone();
        let mut resuming = resume_from.is_some();

        // ── Execute steps ─────────────────────────────────────────────────────
        let run_result: Result<(), anyhow::Error> = async {
            for step_def in &steps {
                // ── Resume: skip steps before the resume point ────────────────
                if resuming {
                    let is_resume_point = resume_from.as_deref() == Some(&step_def.name);
                    if !is_resume_point {
                        if let Some(ref ls) = loaded_state {
                            if let Some(output) = ls.steps.get(&step_def.name) {
                                self.context.store(&step_def.name, output.clone());
                                if !self.quiet {
                                    println!(
                                        "  {} {} {}",
                                        "⏭".yellow(),
                                        step_def.name,
                                        "(skipped — loaded from state)".dimmed()
                                    );
                                }
                                self.step_records.push(StepRecord {
                                    name: step_def.name.clone(),
                                    step_type: step_def.step_type.to_string(),
                                    status: "skipped_resume".to_string(),
                                    duration_secs: 0.0,
                                    output_summary: truncate(output.text(), 100),
                                    input_tokens: None,
                                    output_tokens: None,
                                    cost_usd: None,
                                    sandboxed: false,
                                });
                            }
                        }
                        continue;
                    }
                    resuming = false;
                }

                match self.execute_step(step_def).await {
                    Ok(output) => {
                        current_state.steps.insert(step_def.name.clone(), output.clone());
                        if let Some(ref p) = self.state_file {
                            let _ = current_state.save(p);
                        }
                        last_output = output;
                        step_count += 1;
                    }
                    Err(StepError::ControlFlow(ControlFlow::Skip { message })) => {
                        self.context.store(&step_def.name, StepOutput::Empty);
                        if !self.quiet {
                            let pb = display::step_start(&step_def.name, &step_def.step_type.to_string());
                            display::step_skip(&pb, &step_def.name, &message);
                        }
                        self.step_records.push(StepRecord {
                            name: step_def.name.clone(),
                            step_type: step_def.step_type.to_string(),
                            status: "skipped".to_string(),
                            duration_secs: 0.0,
                            output_summary: message.clone(),
                            input_tokens: None,
                            output_tokens: None,
                            cost_usd: None,
                            sandboxed: false,
                        });
                    }
                    Err(StepError::ControlFlow(ControlFlow::Fail { message })) => {
                        if !self.quiet {
                            display::workflow_failed(&step_def.name, &message);
                        }
                        bail!("Step '{}' failed: {}", step_def.name, message);
                    }
                    Err(StepError::ControlFlow(ControlFlow::Break { .. })) => {
                        break;
                    }
                    Err(e) => {
                        if !self.quiet {
                            display::workflow_failed(&step_def.name, &e.to_string());
                        }
                        return Err(e.into());
                    }
                }
            }
            Ok(())
        }
        .await;

        // ── Sandbox: Destroy container AFTER all steps (always, even on error) ─
        if self.sandbox_mode != SandboxMode::Disabled {
            if let Err(e) = self.sandbox_down().await {
                if !self.quiet {
                    eprintln!("  {} Sandbox cleanup warning: {e}", "⚠".yellow());
                }
            }
        }

        // Propagate any error from the step loop
        run_result?;

        if !self.quiet {
            display::workflow_done(start.elapsed(), step_count);
        }

        self.state = Some(current_state);
        Ok(last_output)
    }

    pub async fn execute_step(&mut self, step_def: &StepDef) -> Result<StepOutput, StepError> {
        let config = self.resolve_config(step_def);
        let use_sandbox = self.should_sandbox_step(&step_def.step_type);

        let pb = if !self.quiet {
            let label = if use_sandbox {
                format!("{} 🐳", step_def.step_type)
            } else {
                step_def.step_type.to_string()
            };
            Some(display::step_start(&step_def.name, &label))
        } else {
            None
        };

        let start = Instant::now();

        tracing::debug!(
            step = %step_def.name,
            step_type = %step_def.step_type,
            sandboxed = use_sandbox,
            "Executing step"
        );

        // Choose sandbox-aware execution when sandbox is active for this step
        let sandbox_ref = if use_sandbox { &self.sandbox } else { &None };

        let result = match step_def.step_type {
            StepType::Cmd => {
                CmdExecutor
                    .execute_sandboxed(step_def, &config, &self.context, sandbox_ref)
                    .await
            }
            StepType::Agent => {
                AgentExecutor
                    .execute_sandboxed(step_def, &config, &self.context, sandbox_ref)
                    .await
            }
            StepType::Gate => GateExecutor.execute(step_def, &config, &self.context).await,
            StepType::Repeat => {
                RepeatExecutor::new(&self.workflow.scopes)
                    .execute(step_def, &config, &self.context)
                    .await
            }
            _ => Err(StepError::Fail(format!(
                "Step type '{}' not yet implemented",
                step_def.step_type
            ))),
        };

        let elapsed = start.elapsed();

        // ── Output Parsing Section ────────────────────────────────────────────
        // Parse step output according to output_type (if declared)
        let result = match result {
            Ok(output) => parse_step_output(output, step_def),
            err => err,
        };

        match &result {
            Ok(output) => {
                tracing::info!(
                    step = %step_def.name,
                    step_type = %step_def.step_type,
                    duration_ms = elapsed.as_millis(),
                    sandboxed = use_sandbox,
                    status = "ok",
                    "Step completed"
                );
                self.context.store(&step_def.name, output.clone());
                // Store parsed value separately if present
                if let Some(parsed) = extract_parsed_value(output, step_def) {
                    self.context.store_parsed(&step_def.name, parsed);
                }

                let (it, ot, cost) = token_stats(output);
                self.step_records.push(StepRecord {
                    name: step_def.name.clone(),
                    step_type: step_def.step_type.to_string(),
                    status: "ok".to_string(),
                    duration_secs: elapsed.as_secs_f64(),
                    output_summary: truncate(output.text(), 100),
                    input_tokens: it,
                    output_tokens: ot,
                    cost_usd: cost,
                    sandboxed: use_sandbox,
                });

                if let Some(pb) = &pb {
                    display::step_ok(pb, &step_def.name, elapsed);
                }
            }
            Err(StepError::ControlFlow(cf)) => {
                let msg = match cf {
                    ControlFlow::Skip { message } => format!("skipped: {message}"),
                    ControlFlow::Break { message, .. } => format!("break: {message}"),
                    ControlFlow::Fail { message } => format!("failed: {message}"),
                    ControlFlow::Next { message } => format!("next: {message}"),
                };
                tracing::info!(
                    step = %step_def.name,
                    step_type = %step_def.step_type,
                    duration_ms = elapsed.as_millis(),
                    status = "control_flow",
                    message = %msg,
                    "Step control flow"
                );
                if let Some(pb) = &pb {
                    display::step_skip(pb, &step_def.name, &msg);
                }
            }
            Err(e) => {
                tracing::warn!(
                    step = %step_def.name,
                    step_type = %step_def.step_type,
                    duration_ms = elapsed.as_millis(),
                    status = "error",
                    error = %e,
                    "Step failed"
                );
                self.step_records.push(StepRecord {
                    name: step_def.name.clone(),
                    step_type: step_def.step_type.to_string(),
                    status: "failed".to_string(),
                    duration_secs: elapsed.as_secs_f64(),
                    output_summary: e.to_string(),
                    input_tokens: None,
                    output_tokens: None,
                    cost_usd: None,
                    sandboxed: use_sandbox,
                });
                if let Some(pb) = &pb {
                    display::step_fail(pb, &step_def.name, &e.to_string());
                }
            }
        }

        result
    }

    /// Dry-run: walk all steps and print a visual tree without executing anything.
    pub fn dry_run(&self) {
        use colored::Colorize;

        println!("{} {} (dry-run)", "▶".cyan().bold(), self.workflow.name.bold());
        if self.sandbox_mode != SandboxMode::Disabled {
            println!("  {} Sandbox mode: {:?}", "🔒".cyan(), self.sandbox_mode);
        }
        println!();

        let steps = &self.workflow.steps;
        let total = steps.len();
        for (i, step) in steps.iter().enumerate() {
            let is_last = i + 1 == total;
            let branch = if is_last { "└──" } else { "├──" };
            let config = self.resolve_config(step);

            let sandbox_indicator = if self.should_sandbox_step(&step.step_type) {
                " 🐳"
            } else {
                ""
            };

            println!(
                "{} {} {}{}",
                branch.dimmed(),
                step.name.bold(),
                format!("[{}]", step.step_type).cyan(),
                sandbox_indicator
            );

            let indent = if is_last { "    " } else { "│   " };
            self.print_step_details(step, &config, indent);

            if !is_last {
                println!("│");
            }
        }
    }

    fn print_step_details(&self, step: &StepDef, config: &StepConfig, indent: &str) {
        use colored::Colorize;

        match step.step_type {
            StepType::Cmd => {
                if let Some(ref run) = step.run {
                    let preview = truncate(run, 80);
                    println!("{}  run: {}", indent, preview.dimmed());
                }
            }
            StepType::Agent | StepType::Chat => {
                if let Some(ref prompt) = step.prompt {
                    let preview = truncate(&prompt.replace('\n', " "), 80);
                    println!("{}  prompt: {}", indent, preview.dimmed());
                }
                if let Some(model) = config.get_str("model") {
                    println!("{}  model: {}", indent, model.dimmed());
                }
            }
            StepType::Gate => {
                if let Some(ref cond) = step.condition {
                    println!("{}  condition: {}", indent, cond.dimmed());
                }
                println!(
                    "{}  on_pass: {} / on_fail: {}",
                    indent,
                    step.on_pass.as_deref().unwrap_or("continue").dimmed(),
                    step.on_fail.as_deref().unwrap_or("continue").dimmed()
                );
            }
            StepType::Repeat => {
                let scope_name = step.scope.as_deref().unwrap_or("<none>");
                let max_iter = step.max_iterations.unwrap_or(1);
                println!("{}  scope: {}", indent, scope_name.dimmed());
                println!("{}  max_iterations: {}", indent, max_iter.to_string().dimmed());
                self.print_scope_steps(scope_name, indent);
            }
            StepType::Map => {
                let scope_name = step.scope.as_deref().unwrap_or("<none>");
                let items = step.items.as_deref().unwrap_or("<none>");
                println!("{}  items: {}", indent, items.dimmed());
                println!("{}  scope: {}", indent, scope_name.dimmed());
                if let Some(p) = step.parallel {
                    println!("{}  parallel: {}", indent, p.to_string().dimmed());
                }
                self.print_scope_steps(scope_name, indent);
            }
            StepType::Call => {
                let scope_name = step.scope.as_deref().unwrap_or("<none>");
                println!("{}  scope: {}", indent, scope_name.dimmed());
                self.print_scope_steps(scope_name, indent);
            }
            StepType::Parallel => {
                if let Some(ref sub_steps) = step.steps {
                    println!("{}  parallel steps:", indent);
                    for sub in sub_steps {
                        println!(
                            "{}    • {} [{}]",
                            indent,
                            sub.name.bold(),
                            sub.step_type.to_string().cyan()
                        );
                    }
                }
            }
            StepType::Template => {
                if let Some(ref run) = step.run {
                    println!("{}  template: {}", indent, run.dimmed());
                }
            }
        }

        if let Some(t) = config.get_str("timeout") {
            println!("{}  timeout: {}", indent, t.dimmed());
        }
    }

    fn print_scope_steps(&self, scope_name: &str, indent: &str) {
        use colored::Colorize;
        if let Some(scope) = self.workflow.scopes.get(scope_name) {
            println!("{}  scope steps:", indent);
            for step in &scope.steps {
                println!(
                    "{}    • {} [{}]",
                    indent,
                    step.name.bold(),
                    step.step_type.to_string().cyan()
                );
            }
        }
    }

    fn resolve_config(&self, step_def: &StepDef) -> StepConfig {
        self.config_manager
            .resolve(&step_def.name, &step_def.step_type, &step_def.config)
    }
}

/// Extract token stats from a step output (for JSON records)
fn token_stats(output: &StepOutput) -> (Option<u64>, Option<u64>, Option<f64>) {
    match output {
        StepOutput::Agent(o) => (
            Some(o.stats.input_tokens),
            Some(o.stats.output_tokens),
            Some(o.stats.cost_usd),
        ),
        StepOutput::Chat(o) => (Some(o.input_tokens), Some(o.output_tokens), None),
        _ => (None, None, None),
    }
}

/// Parse the raw step output according to the step's declared output_type.
/// Returns the output unchanged if no output_type is declared or it is Text.
fn parse_step_output(output: StepOutput, step_def: &StepDef) -> Result<StepOutput, StepError> {
    let output_type = match &step_def.output_type {
        Some(t) => t,
        None => return Ok(output),
    };

    if *output_type == OutputType::Text {
        return Ok(output);
    }

    let text = output.text().trim().to_string();

    match output_type {
        OutputType::Integer => {
            text.parse::<i64>()
                .map_err(|_| StepError::Fail(format!("Failed to parse '{}' as integer", text)))?;
        }
        OutputType::Json => {
            serde_json::from_str::<serde_json::Value>(&text)
                .map_err(|e| StepError::Fail(format!("Failed to parse output as JSON: {e}")))?;
        }
        OutputType::Boolean => {
            match text.to_lowercase().as_str() {
                "true" | "1" | "yes" | "false" | "0" | "no" => {}
                _ => {
                    return Err(StepError::Fail(format!(
                        "Failed to parse '{}' as boolean",
                        text
                    )));
                }
            }
        }
        OutputType::Lines | OutputType::Text => {}
    }

    Ok(output)
}

/// Extract a ParsedValue from the step output based on output_type.
/// Returns None if no output_type or it is Text.
fn extract_parsed_value(output: &StepOutput, step_def: &StepDef) -> Option<ParsedValue> {
    let output_type = step_def.output_type.as_ref()?;

    let text = output.text().trim().to_string();

    let parsed = match output_type {
        OutputType::Text => ParsedValue::Text(text),
        OutputType::Integer => ParsedValue::Integer(text.parse::<i64>().ok()?),
        OutputType::Json => {
            let val = serde_json::from_str::<serde_json::Value>(&text).ok()?;
            ParsedValue::Json(val)
        }
        OutputType::Lines => {
            let lines: Vec<String> = text
                .lines()
                .filter(|l| !l.is_empty())
                .map(|l| l.to_string())
                .collect();
            ParsedValue::Lines(lines)
        }
        OutputType::Boolean => {
            let b = match text.to_lowercase().as_str() {
                "true" | "1" | "yes" => true,
                _ => false,
            };
            ParsedValue::Boolean(b)
        }
    };

    Some(parsed)
}

/// Truncate a string to at most `max` chars, appending "…" if cut
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::parser;

    #[tokio::test]
    async fn engine_runs_sequential_cmd_steps() {
        let yaml = r#"
name: test
steps:
  - name: step1
    type: cmd
    run: "echo first"
  - name: step2
    type: cmd
    run: "echo second"
"#;
        let wf = parser::parse_str(yaml).unwrap();
        let mut engine = Engine::new(wf, "".to_string(), HashMap::new(), false, true);
        let result = engine.run().await.unwrap();
        assert_eq!(result.text().trim(), "second");
        assert!(engine.context.get_step("step1").is_some());
        assert_eq!(
            engine.context.get_step("step1").unwrap().text().trim(),
            "first"
        );
    }

    #[tokio::test]
    async fn engine_exposes_step_output_to_next_step() {
        let yaml = r#"
name: test
steps:
  - name: produce
    type: cmd
    run: "echo hello_world"
  - name: consume
    type: cmd
    run: "echo {{ steps.produce.stdout }}"
"#;
        let wf = parser::parse_str(yaml).unwrap();
        let mut engine = Engine::new(wf, "".to_string(), HashMap::new(), false, true);
        let result = engine.run().await.unwrap();
        assert!(result.text().contains("hello_world"));
    }

    #[tokio::test]
    async fn engine_collects_step_records_in_json_mode() {
        let yaml = r#"
name: json-test
steps:
  - name: alpha
    type: cmd
    run: "echo alpha"
  - name: beta
    type: cmd
    run: "echo beta"
"#;
        let wf = parser::parse_str(yaml).unwrap();
        let opts = EngineOptions {
            json: true,
            ..Default::default()
        };
        let mut engine = Engine::with_options(wf, "".to_string(), HashMap::new(), opts);
        engine.run().await.unwrap();

        let records = engine.step_records();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].name, "alpha");
        assert_eq!(records[0].status, "ok");
        assert!(!records[0].sandboxed);
        assert_eq!(records[1].name, "beta");
        assert_eq!(records[1].status, "ok");
    }

    #[tokio::test]
    async fn json_output_includes_sandbox_mode() {
        let yaml = r#"
name: json-output-test
steps:
  - name: greet
    type: cmd
    run: "echo hello"
"#;
        let wf = parser::parse_str(yaml).unwrap();
        let opts = EngineOptions {
            json: true,
            ..Default::default()
        };
        let mut engine = Engine::with_options(wf, "".to_string(), HashMap::new(), opts);
        let start = Instant::now();
        engine.run().await.unwrap();
        let out = engine.json_output("success", start.elapsed());

        let json = serde_json::to_string(&out).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["workflow_name"], "json-output-test");
        assert_eq!(parsed["status"], "success");
        assert_eq!(parsed["sandbox_mode"], "Disabled");
        assert!(parsed["steps"].is_array());
        assert_eq!(parsed["steps"][0]["name"], "greet");
    }

    #[test]
    fn should_sandbox_step_logic() {
        let yaml = r#"
name: test
steps:
  - name: s
    type: cmd
    run: "echo test"
"#;
        let wf = parser::parse_str(yaml).unwrap();

        // Disabled mode → nothing sandboxed
        let engine = Engine::new(wf.clone(), "".to_string(), HashMap::new(), false, true);
        assert!(!engine.should_sandbox_step(&StepType::Cmd));
        assert!(!engine.should_sandbox_step(&StepType::Agent));
        assert!(!engine.should_sandbox_step(&StepType::Gate));

        // FullWorkflow mode → cmd + agent sandboxed
        let opts = EngineOptions {
            sandbox_mode: SandboxMode::FullWorkflow,
            quiet: true,
            ..Default::default()
        };
        let engine = Engine::with_options(wf.clone(), "".to_string(), HashMap::new(), opts);
        assert!(engine.should_sandbox_step(&StepType::Cmd));
        assert!(engine.should_sandbox_step(&StepType::Agent));
        assert!(!engine.should_sandbox_step(&StepType::Gate));

        // AgentOnly mode → only agent sandboxed
        let opts = EngineOptions {
            sandbox_mode: SandboxMode::AgentOnly,
            quiet: true,
            ..Default::default()
        };
        let engine = Engine::with_options(wf.clone(), "".to_string(), HashMap::new(), opts);
        assert!(!engine.should_sandbox_step(&StepType::Cmd));
        assert!(engine.should_sandbox_step(&StepType::Agent));
        assert!(!engine.should_sandbox_step(&StepType::Gate));
    }

    #[test]
    fn dry_run_does_not_panic() {
        let yaml = r#"
name: dry-run-test
scopes:
  lint_fix:
    steps:
      - name: lint
        type: cmd
        run: "npm run lint"
      - name: fix_lint
        type: agent
        prompt: "Fix lint errors"
steps:
  - name: setup
    type: cmd
    run: "echo setup"
  - name: validate
    type: gate
    condition: "{{ steps.setup.exit_code == 0 }}"
    on_pass: continue
    on_fail: fail
  - name: lint_gate
    type: repeat
    scope: lint_fix
    max_iterations: 3
"#;
        let wf = crate::workflow::parser::parse_str(yaml).unwrap();
        let engine = Engine::new(wf, "".to_string(), HashMap::new(), false, true);
        engine.dry_run();
    }

    #[test]
    fn dry_run_all_step_types() {
        let yaml = r#"
name: all-types
steps:
  - name: c
    type: cmd
    run: "ls"
  - name: g
    type: gate
    condition: "{{ true }}"
    on_pass: continue
  - name: p
    type: parallel
    steps:
      - name: p1
        type: cmd
        run: "echo p1"
"#;
        let wf = crate::workflow::parser::parse_str(yaml).unwrap();
        let engine = Engine::new(wf, "".to_string(), HashMap::new(), false, true);
        engine.dry_run();
    }

    #[test]
    fn truncate_helper() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hello…");
    }

    #[tokio::test]
    async fn resume_fails_when_no_state_file() {
        let yaml = r#"
name: no-state-workflow-xyz-unique
steps:
  - name: step1
    type: cmd
    run: "echo 1"
"#;
        let wf = crate::workflow::parser::parse_str(yaml).unwrap();
        let opts = EngineOptions {
            resume_from: Some("step1".to_string()),
            quiet: true,
            ..Default::default()
        };
        let mut engine = Engine::with_options(wf, "".to_string(), HashMap::new(), opts);
        let err = engine.run().await.unwrap_err();
        assert!(
            err.to_string().contains("No state file found"),
            "Expected 'No state file found' but got: {err}"
        );
    }

    #[tokio::test]
    async fn resume_fails_for_unknown_step() {
        let workflow_name = "test-resume-unknown-step";
        let state = WorkflowState::new(workflow_name);
        let tmp_path = format!("/tmp/minion-{workflow_name}-20991231235959.state.json");
        let path = PathBuf::from(&tmp_path);
        state.save(&path).unwrap();

        let yaml = format!(
            r#"
name: {workflow_name}
steps:
  - name: step1
    type: cmd
    run: "echo 1"
"#
        );
        let wf = crate::workflow::parser::parse_str(&yaml).unwrap();
        let opts = EngineOptions {
            resume_from: Some("nonexistent_step".to_string()),
            quiet: true,
            ..Default::default()
        };
        let mut engine = Engine::with_options(wf, "".to_string(), HashMap::new(), opts);
        let err = engine.run().await.unwrap_err();
        assert!(
            err.to_string().contains("not found in workflow"),
            "Expected 'not found in workflow' but got: {err}"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn output_type_integer_parses_number() {
        let yaml = r#"
name: test-parse
steps:
  - name: count
    type: cmd
    run: "echo 42"
    output_type: integer
  - name: use_count
    type: cmd
    run: "echo {{ count.output }}"
"#;
        let wf = parser::parse_str(yaml).unwrap();
        let mut engine = Engine::new(wf, "".to_string(), HashMap::new(), false, true);
        let result = engine.run().await.unwrap();
        assert_eq!(result.text().trim(), "42");
    }

    #[tokio::test]
    async fn output_type_integer_fails_on_non_number() {
        let yaml = r#"
name: test-parse-fail
steps:
  - name: count
    type: cmd
    run: "echo not_a_number"
    output_type: integer
"#;
        let wf = parser::parse_str(yaml).unwrap();
        let mut engine = Engine::new(wf, "".to_string(), HashMap::new(), false, true);
        let err = engine.run().await.unwrap_err();
        assert!(err.to_string().contains("integer"), "{err}");
    }

    #[tokio::test]
    async fn output_type_json_allows_dot_access() {
        let yaml = r#"
name: test-json
steps:
  - name: scan
    type: cmd
    run: "echo '{\"count\": 5}'"
    output_type: json
  - name: use_scan
    type: cmd
    run: "echo {{ scan.output.count }}"
"#;
        let wf = parser::parse_str(yaml).unwrap();
        let mut engine = Engine::new(wf, "".to_string(), HashMap::new(), false, true);
        let result = engine.run().await.unwrap();
        assert_eq!(result.text().trim(), "5");
    }

    #[tokio::test]
    async fn output_type_lines_allows_length_filter() {
        let yaml = r#"
name: test-lines
steps:
  - name: files
    type: cmd
    run: "printf 'a.rs\nb.rs\nc.rs'"
    output_type: lines
  - name: count_files
    type: cmd
    run: "echo {{ files.output | length }}"
"#;
        let wf = parser::parse_str(yaml).unwrap();
        let mut engine = Engine::new(wf, "".to_string(), HashMap::new(), false, true);
        let result = engine.run().await.unwrap();
        assert_eq!(result.text().trim(), "3");
    }
}
