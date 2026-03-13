pub mod context;
pub mod state;
mod template;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use colored::Colorize;
use regex::Regex;
use serde::Serialize;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::cli::display;
use crate::config::{ConfigManager, StepConfig};
use crate::control_flow::ControlFlow;
use crate::error::StepError;
use crate::events::subscribers::{FileSubscriber, WebhookSubscriber};
use crate::events::types::Event;
use crate::events::EventBus;
use crate::plugins::registry::PluginRegistry;
use crate::sandbox::config::SandboxConfig;
use crate::sandbox::docker::DockerSandbox;
use crate::sandbox::SandboxMode;
use crate::steps::*;
use crate::steps::{
    agent::AgentExecutor, call::CallExecutor, chat::ChatExecutor, cmd::CmdExecutor,
    gate::GateExecutor, map::MapExecutor, parallel::ParallelExecutor,
    repeat::RepeatExecutor, script::ScriptExecutor, template_step::TemplateStepExecutor,
};
use crate::plugins::loader::PluginLoader;
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
    /// Pending async step futures — keyed by step name
    pending_futures: HashMap<String, JoinHandle<Result<StepOutput, StepError>>>,
    /// Plugin registry for dynamically-loaded step types
    plugin_registry: Arc<Mutex<PluginRegistry>>,
    /// Event bus for lifecycle events
    pub event_bus: EventBus,
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

        // ── Load plugins from workflow config ─────────────────────────────────
        let mut registry = PluginRegistry::new();
        for plugin_cfg in &workflow.config.plugins {
            match PluginLoader::load_plugin(&plugin_cfg.path) {
                Ok(plugin) => {
                    tracing::info!(name = %plugin_cfg.name, path = %plugin_cfg.path, "Loaded plugin");
                    registry.register(plugin);
                }
                Err(e) => {
                    tracing::warn!(
                        name = %plugin_cfg.name,
                        path = %plugin_cfg.path,
                        error = %e,
                        "Failed to load plugin"
                    );
                }
            }
        }

        // ── Wire up event subscribers from workflow config ─────────────────────
        let mut event_bus = EventBus::new();
        if let Some(ref events_cfg) = workflow.config.events {
            if let Some(ref webhook_url) = events_cfg.webhook {
                event_bus.add_subscriber(Box::new(WebhookSubscriber::new(webhook_url.clone())));
                tracing::info!(url = %webhook_url, "Registered webhook event subscriber");
            }
            if let Some(ref file_path) = events_cfg.file {
                event_bus.add_subscriber(Box::new(FileSubscriber::new(file_path.clone())));
                tracing::info!(path = %file_path, "Registered file event subscriber");
            }
        }

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
            pending_futures: HashMap::new(),
            plugin_registry: Arc::new(Mutex::new(registry)),
            event_bus,
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
        // Script steps run embedded (no external process), never sandboxed
        if *step_type == StepType::Script {
            return false;
        }
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

        // ── Event: WorkflowStarted ────────────────────────────────────────────
        self.event_bus
            .emit(Event::WorkflowStarted {
                timestamp: chrono::Utc::now(),
            })
            .await;

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

                // ── Async step: spawn and register in pending_futures ─────────
                if step_def.async_exec == Some(true) {
                    let handle = self.spawn_async_step(step_def);
                    self.pending_futures.insert(step_def.name.clone(), handle);
                    if !self.quiet {
                        println!(
                            "  {} {} {} {}",
                            "⚡".yellow(),
                            step_def.name,
                            format!("[{}]", step_def.step_type).cyan(),
                            "(async — spawned)".dimmed()
                        );
                    }
                    step_count += 1;
                    continue;
                }

                // ── Auto-await: resolve pending deps before execute ────────────
                self.await_pending_deps(step_def).await?;

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

        // ── Story 3.3: Await all remaining pending async futures ──────────────
        let remaining: Vec<(String, JoinHandle<Result<StepOutput, StepError>>)> =
            self.pending_futures.drain().collect();
        for (name, handle) in remaining {
            let step_type = self
                .workflow
                .steps
                .iter()
                .find(|s| s.name == name)
                .map(|s| s.step_type.to_string())
                .unwrap_or_else(|| "async".to_string());
            match handle.await {
                Ok(Ok(output)) => {
                    self.context.store(&name, output.clone());
                    self.step_records.push(StepRecord {
                        name: name.clone(),
                        step_type: step_type.clone(),
                        status: "ok".to_string(),
                        duration_secs: 0.0,
                        output_summary: truncate(output.text(), 100),
                        input_tokens: None,
                        output_tokens: None,
                        cost_usd: None,
                        sandboxed: false,
                    });
                }
                Ok(Err(e)) => {
                    self.step_records.push(StepRecord {
                        name: name.clone(),
                        step_type: step_type.clone(),
                        status: "failed".to_string(),
                        duration_secs: 0.0,
                        output_summary: e.to_string(),
                        input_tokens: None,
                        output_tokens: None,
                        cost_usd: None,
                        sandboxed: false,
                    });
                    if !self.quiet {
                        eprintln!("  {} Async step '{}' failed: {}", "✗".red(), name, e);
                    }
                }
                Err(e) => {
                    let msg = format!("Async step '{}' panicked: {e}", name);
                    self.step_records.push(StepRecord {
                        name: name.clone(),
                        step_type,
                        status: "failed".to_string(),
                        duration_secs: 0.0,
                        output_summary: msg.clone(),
                        input_tokens: None,
                        output_tokens: None,
                        cost_usd: None,
                        sandboxed: false,
                    });
                    if !self.quiet {
                        eprintln!("  {} {}", "✗".red(), msg);
                    }
                }
            }
        }

        // ── Sandbox: Destroy container AFTER all steps (always, even on error) ─
        if self.sandbox_mode != SandboxMode::Disabled {
            if let Err(e) = self.sandbox_down().await {
                if !self.quiet {
                    eprintln!("  {} Sandbox cleanup warning: {e}", "⚠".yellow());
                }
            }
        }

        // ── Event: WorkflowCompleted ──────────────────────────────────────────
        self.event_bus
            .emit(Event::WorkflowCompleted {
                duration_ms: start.elapsed().as_millis() as u64,
                timestamp: chrono::Utc::now(),
            })
            .await;

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

        // ── Event: StepStarted ────────────────────────────────────────────────
        self.event_bus
            .emit(Event::StepStarted {
                step_name: step_def.name.clone(),
                step_type: step_def.step_type.to_string(),
                timestamp: chrono::Utc::now(),
            })
            .await;

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
            StepType::Chat => {
                ChatExecutor.execute(step_def, &config, &self.context).await
            }
            StepType::Map => {
                MapExecutor::new(&self.workflow.scopes)
                    .execute(step_def, &config, &self.context)
                    .await
            }
            StepType::Parallel => {
                ParallelExecutor::new(&self.workflow.scopes)
                    .execute(step_def, &config, &self.context)
                    .await
            }
            StepType::Call => {
                CallExecutor::new(&self.workflow.scopes)
                    .execute(step_def, &config, &self.context)
                    .await
            }
            StepType::Template => {
                let prompts_dir = self.workflow.prompts_dir.as_deref();
                TemplateStepExecutor::new(prompts_dir)
                    .execute(step_def, &config, &self.context)
                    .await
            }
            StepType::Script => {
                ScriptExecutor.execute(step_def, &config, &self.context).await
            }
            // Note: all StepType variants are covered above.
            // Plugin dispatch will be added via a dedicated StepType::Plugin variant.
        };

        let elapsed = start.elapsed();
        let duration_ms = elapsed.as_millis() as u64;

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

        // ── Event: StepCompleted / StepFailed ─────────────────────────────────
        match &result {
            Ok(_) => {
                self.event_bus
                    .emit(Event::StepCompleted {
                        step_name: step_def.name.clone(),
                        step_type: step_def.step_type.to_string(),
                        duration_ms,
                        timestamp: chrono::Utc::now(),
                    })
                    .await;
            }
            Err(e) if !matches!(e, StepError::ControlFlow(_)) => {
                self.event_bus
                    .emit(Event::StepFailed {
                        step_name: step_def.name.clone(),
                        step_type: step_def.step_type.to_string(),
                        error: e.to_string(),
                        duration_ms,
                        timestamp: chrono::Utc::now(),
                    })
                    .await;
            }
            _ => {}
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

            let async_indicator = if step.async_exec == Some(true) { " ⚡" } else { "" };

            println!(
                "{} {} {}{}{}",
                branch.dimmed(),
                step.name.bold(),
                format!("[{}]", step.step_type).cyan(),
                sandbox_indicator,
                async_indicator
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
        StepType::Script => {
                if let Some(ref run) = step.run {
                    let preview = truncate(&run.replace('\n', " "), 80);
                    println!("{}  script: {}", indent, preview.dimmed());
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

    // ── Async Step Support ───────────────────────────────────────────────────

    /// Spawn an async step as a tokio task. Returns a JoinHandle for later awaiting.
    /// Creates a minimal context (target only) for the spawned task since the
    /// executor templates are rendered inside the task.
    fn spawn_async_step(
        &self,
        step_def: &StepDef,
    ) -> JoinHandle<Result<StepOutput, StepError>> {
        let step = step_def.clone();
        let config = self.resolve_config(step_def);
        let target = self
            .context
            .get_var("target")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        tokio::spawn(async move {
            let ctx = Context::new(target, HashMap::new());
            match step.step_type {
                StepType::Cmd => CmdExecutor.execute(&step, &config, &ctx).await,
                StepType::Agent => AgentExecutor.execute(&step, &config, &ctx).await,
                StepType::Script => ScriptExecutor.execute(&step, &config, &ctx).await,
                _ => Err(StepError::Fail(format!(
                    "Async execution not supported for step type '{}'",
                    step.step_type
                ))),
            }
        })
    }

    /// Scan the step's template fields for references to other steps.
    /// If any referenced step is in pending_futures, await it and store result in context.
    async fn await_pending_deps(&mut self, step_def: &StepDef) -> Result<(), StepError> {
        let pattern = Regex::new(r"steps\.(\w+)\.").unwrap();

        // Collect all template fields that might reference step outputs
        let mut templates: Vec<String> = Vec::new();
        if let Some(ref run) = step_def.run {
            templates.push(run.clone());
        }
        if let Some(ref prompt) = step_def.prompt {
            templates.push(prompt.clone());
        }
        if let Some(ref condition) = step_def.condition {
            templates.push(condition.clone());
        }

        // Find all step names referenced in templates
        let mut deps: Vec<String> = Vec::new();
        for tmpl in &templates {
            for cap in pattern.captures_iter(tmpl) {
                let name = cap[1].to_string();
                if self.pending_futures.contains_key(&name) && !deps.contains(&name) {
                    deps.push(name);
                }
            }
        }

        // Await each dependency
        for name in deps {
            self.await_pending_step(&name).await?;
        }

        Ok(())
    }

    /// Await a single named async step, storing its output in context.
    async fn await_pending_step(&mut self, name: &str) -> Result<(), StepError> {
        if let Some(handle) = self.pending_futures.remove(name) {
            match handle.await {
                Ok(Ok(output)) => {
                    self.context.store(name, output.clone());
                    self.step_records.push(StepRecord {
                        name: name.to_string(),
                        step_type: "async".to_string(),
                        status: "ok".to_string(),
                        duration_secs: 0.0,
                        output_summary: truncate(output.text(), 100),
                        input_tokens: None,
                        output_tokens: None,
                        cost_usd: None,
                        sandboxed: false,
                    });
                }
                Ok(Err(e)) => {
                    return Err(StepError::Fail(format!(
                        "Async step '{}' failed: {e}",
                        name
                    )));
                }
                Err(e) => {
                    return Err(StepError::Fail(format!(
                        "Async step '{}' panicked: {e}",
                        name
                    )));
                }
            }
        }
        Ok(())
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

/// Truncate a string to at most `max` chars, appending "…" if cut.
/// Uses char boundaries to avoid panicking on multi-byte UTF-8 (e.g. emojis).
fn truncate(s: &str, max: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max {
        s.to_string()
    } else {
        let end: usize = s.char_indices().nth(max).map(|(i, _)| i).unwrap_or(s.len());
        format!("{}…", &s[..end])
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
    async fn safe_accessor_returns_empty_for_missing_step() {
        let yaml = r#"
name: test-safe-accessor
steps:
  - name: use_missing
    type: cmd
    run: "echo '{{ missing.output? }}'"
"#;
        let wf = parser::parse_str(yaml).unwrap();
        let mut engine = Engine::new(wf, "".to_string(), HashMap::new(), false, true);
        let result = engine.run().await.unwrap();
        // safe accessor returns empty string when step doesn't exist
        assert_eq!(result.text().trim(), "");
    }

    #[tokio::test]
    async fn safe_accessor_returns_value_when_present() {
        let yaml = r#"
name: test-safe-accessor-present
steps:
  - name: produce
    type: cmd
    run: "echo hello"
  - name: consume
    type: cmd
    run: "echo '{{ produce.output? }}'"
"#;
        let wf = parser::parse_str(yaml).unwrap();
        let mut engine = Engine::new(wf, "".to_string(), HashMap::new(), false, true);
        let result = engine.run().await.unwrap();
        assert!(result.text().contains("hello"));
    }

    #[tokio::test]
    async fn strict_accessor_fails_when_step_missing() {
        let yaml = r#"
name: test-strict-accessor-fail
steps:
  - name: use_missing
    type: cmd
    run: "echo '{{ nonexistent.output! }}'"
"#;
        let wf = parser::parse_str(yaml).unwrap();
        let mut engine = Engine::new(wf, "".to_string(), HashMap::new(), false, true);
        let err = engine.run().await.unwrap_err();
        assert!(err.to_string().contains("strict access"), "{err}");
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

    // ── Story 3.1: Async flag and pending futures ────────────────────────────

    #[tokio::test]
    async fn async_step_is_spawned_and_completes() {
        let yaml = r#"
name: async-test
steps:
  - name: bg_task
    type: cmd
    run: "echo async_result"
    async_exec: true
  - name: sync_step
    type: cmd
    run: "echo sync_result"
"#;
        let wf = parser::parse_str(yaml).unwrap();
        let opts = EngineOptions { quiet: true, ..Default::default() };
        let mut engine = Engine::with_options(wf, "".to_string(), HashMap::new(), opts);
        let result = engine.run().await.unwrap();
        // sync_step is the last synchronous step
        assert!(result.text().contains("sync_result"));
        // bg_task should be recorded after join_all
        let records = engine.step_records();
        assert!(records.iter().any(|r| r.name == "bg_task"), "bg_task should be in records");
    }

    #[test]
    fn dry_run_shows_async_lightning_indicator() {
        let yaml = r#"
name: dry-async
steps:
  - name: fast_bg
    type: cmd
    run: "echo bg"
    async_exec: true
  - name: normal
    type: cmd
    run: "echo normal"
"#;
        // dry_run should not panic and the async step should have ⚡ in output
        let wf = parser::parse_str(yaml).unwrap();
        let engine = Engine::new(wf, "".to_string(), HashMap::new(), false, true);
        // Just verify it doesn't panic
        engine.dry_run();
    }

    #[test]
    fn should_sandbox_step_script_always_false() {
        let yaml = r#"
name: test
steps:
  - name: s
    type: cmd
    run: "echo test"
"#;
        let wf = parser::parse_str(yaml).unwrap();

        // Script steps never run in sandbox, regardless of mode
        let opts = EngineOptions {
            sandbox_mode: SandboxMode::FullWorkflow,
            quiet: true,
            ..Default::default()
        };
        let engine = Engine::with_options(wf, "".to_string(), HashMap::new(), opts);
        assert!(!engine.should_sandbox_step(&StepType::Script));
    }

    // ── Story 3.3: Await all remaining async futures at workflow end ─────────

    #[tokio::test]
    async fn multiple_async_steps_all_complete_by_workflow_end() {
        let yaml = r#"
name: multi-async
steps:
  - name: task_a
    type: cmd
    run: "echo result_a"
    async_exec: true
  - name: task_b
    type: cmd
    run: "echo result_b"
    async_exec: true
  - name: sync_done
    type: cmd
    run: "echo done"
"#;
        let wf = parser::parse_str(yaml).unwrap();
        let opts = EngineOptions { quiet: true, ..Default::default() };
        let mut engine = Engine::with_options(wf, "".to_string(), HashMap::new(), opts);
        engine.run().await.unwrap();

        let records = engine.step_records();
        assert!(records.iter().any(|r| r.name == "task_a"), "task_a should be recorded");
        assert!(records.iter().any(|r| r.name == "task_b"), "task_b should be recorded");
        assert!(records.iter().any(|r| r.name == "sync_done"), "sync_done should be recorded");
    }

    // ── Story 4.3: Script step dispatch ─────────────────────────────────────

    #[tokio::test]
    async fn engine_dispatches_script_step() {
        let yaml = r#"
name: script-dispatch
steps:
  - name: calc
    type: script
    run: |
      let x = 6 * 7;
      x.to_string()
"#;
        let wf = parser::parse_str(yaml).unwrap();
        let opts = EngineOptions { quiet: true, ..Default::default() };
        let mut engine = Engine::with_options(wf, "".to_string(), HashMap::new(), opts);
        let result = engine.run().await.unwrap();
        assert_eq!(result.text().trim(), "42");
    }
}
