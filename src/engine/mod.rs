pub mod context;
mod template;

use std::collections::HashMap;
use std::time::Instant;

use anyhow::{Result, bail};

use crate::cli::display;
use crate::config::{ConfigManager, StepConfig};
use crate::control_flow::ControlFlow;
use crate::error::StepError;
use crate::steps::*;
use crate::steps::{cmd::CmdExecutor, agent::AgentExecutor, gate::GateExecutor, repeat::RepeatExecutor};
use crate::workflow::schema::{StepDef, StepType, WorkflowDef};
use context::Context;

pub struct Engine {
    pub workflow: WorkflowDef,
    pub context: Context,
    config_manager: ConfigManager,
    pub verbose: bool,
    pub quiet: bool,
}

impl Engine {
    pub fn new(
        workflow: WorkflowDef,
        target: String,
        vars: HashMap<String, serde_json::Value>,
        verbose: bool,
        quiet: bool,
    ) -> Self {
        let context = Context::new(target, vars);
        let config_manager = ConfigManager::new(workflow.config.clone());
        Self {
            workflow,
            context,
            config_manager,
            verbose,
            quiet,
        }
    }

    pub async fn run(&mut self) -> Result<StepOutput> {
        if !self.quiet {
            display::workflow_start(&self.workflow.name);
        }

        let start = Instant::now();
        let steps = self.workflow.steps.clone();
        let mut last_output = StepOutput::Empty;
        let mut step_count = 0;

        for step_def in &steps {
            match self.execute_step(step_def).await {
                Ok(output) => {
                    last_output = output;
                    step_count += 1;
                }
                Err(StepError::ControlFlow(ControlFlow::Skip { message })) => {
                    self.context.store(&step_def.name, StepOutput::Empty);
                    if !self.quiet {
                        let pb = display::step_start(&step_def.name, &step_def.step_type.to_string());
                        display::step_skip(&pb, &step_def.name, &message);
                    }
                }
                Err(StepError::ControlFlow(ControlFlow::Fail { message })) => {
                    if !self.quiet {
                        display::workflow_failed(&step_def.name, &message);
                    }
                    bail!("Step '{}' failed: {}", step_def.name, message);
                }
                Err(StepError::ControlFlow(ControlFlow::Break { .. })) => {
                    // Break at top level just stops the workflow
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

        if !self.quiet {
            display::workflow_done(start.elapsed(), step_count);
        }

        Ok(last_output)
    }

    pub async fn execute_step(&mut self, step_def: &StepDef) -> Result<StepOutput, StepError> {
        let config = self.resolve_config(step_def);

        let pb = if !self.quiet {
            Some(display::step_start(&step_def.name, &step_def.step_type.to_string()))
        } else {
            None
        };

        let start = Instant::now();

        tracing::debug!(
            step = %step_def.name,
            step_type = %step_def.step_type,
            "Executing step"
        );

        let result = match step_def.step_type {
            StepType::Cmd => CmdExecutor.execute(step_def, &config, &self.context).await,
            StepType::Agent => AgentExecutor.execute(step_def, &config, &self.context).await,
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

        match &result {
            Ok(output) => {
                tracing::info!(
                    step = %step_def.name,
                    step_type = %step_def.step_type,
                    duration_ms = elapsed.as_millis(),
                    status = "ok",
                    "Step completed"
                );
                self.context.store(&step_def.name, output.clone());
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
                if let Some(pb) = &pb {
                    display::step_fail(pb, &step_def.name, &e.to_string());
                }
            }
        }

        result
    }

    fn resolve_config(&self, step_def: &StepDef) -> StepConfig {
        self.config_manager
            .resolve(&step_def.name, &step_def.step_type, &step_def.config)
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
        // Last step output is returned
        assert_eq!(result.text().trim(), "second");
        // First step is stored in context
        assert!(engine.context.get_step("step1").is_some());
        assert_eq!(engine.context.get_step("step1").unwrap().text().trim(), "first");
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
        // consume step echoes the output of produce step
        assert!(result.text().contains("hello_world"));
    }
}
