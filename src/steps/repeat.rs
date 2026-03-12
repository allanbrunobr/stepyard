use std::collections::HashMap;

use async_trait::async_trait;

use crate::cli::display;
use crate::config::StepConfig;
use crate::control_flow::ControlFlow;
use crate::engine::context::Context;
use crate::error::StepError;
use crate::workflow::schema::{ScopeDef, StepDef};

use super::{
    cmd::CmdExecutor, agent::AgentExecutor, gate::GateExecutor,
    IterationOutput, ScopeOutput, StepExecutor, StepOutput,
};

pub struct RepeatExecutor {
    scopes: HashMap<String, ScopeDef>,
}

impl RepeatExecutor {
    pub fn new(scopes: &HashMap<String, ScopeDef>) -> Self {
        Self {
            scopes: scopes.clone(),
        }
    }
}

#[async_trait]
impl StepExecutor for RepeatExecutor {
    async fn execute(
        &self,
        step: &StepDef,
        _config: &StepConfig,
        ctx: &Context,
    ) -> Result<StepOutput, StepError> {
        let scope_name = step
            .scope
            .as_ref()
            .ok_or_else(|| StepError::Fail("repeat step missing 'scope' field".into()))?;

        let scope = self
            .scopes
            .get(scope_name)
            .ok_or_else(|| StepError::Fail(format!("scope '{}' not found", scope_name)))?;

        let max_iterations = step.max_iterations.unwrap_or(3);
        let mut iterations = Vec::new();
        let mut scope_value = step
            .initial_value
            .as_ref()
            .map(|v| serde_json::to_value(v).unwrap_or(serde_json::Value::Null))
            .unwrap_or(serde_json::Value::Null);

        for i in 0..max_iterations {
            display::iteration(i, max_iterations);

            // Create a temporary mutable child context for this iteration
            // We need to make the parent context available as Arc
            // For now, create a standalone context with parent's data
            let mut child_ctx = Context::new(
                ctx.get_var("target")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                HashMap::new(),
            );
            child_ctx.scope_value = Some(scope_value.clone());
            child_ctx.scope_index = i;

            // Copy parent step outputs into child
            // (simplified — proper implementation would use Arc parent)

            let mut last_output = StepOutput::Empty;
            let mut should_break = false;

            for scope_step in &scope.steps {
                let step_config = StepConfig::default();

                let result = match scope_step.step_type {
                    crate::workflow::schema::StepType::Cmd => {
                        CmdExecutor.execute(scope_step, &step_config, &child_ctx).await
                    }
                    crate::workflow::schema::StepType::Agent => {
                        AgentExecutor.execute(scope_step, &step_config, &child_ctx).await
                    }
                    crate::workflow::schema::StepType::Gate => {
                        GateExecutor.execute(scope_step, &step_config, &child_ctx).await
                    }
                    _ => Err(StepError::Fail(format!(
                        "Step type '{}' not supported in repeat scope",
                        scope_step.step_type
                    ))),
                };

                match result {
                    Ok(output) => {
                        child_ctx.store(&scope_step.name, output.clone());
                        last_output = output;
                    }
                    Err(StepError::ControlFlow(ControlFlow::Break { value, .. })) => {
                        if let Some(v) = value {
                            last_output = v;
                        }
                        should_break = true;
                        break;
                    }
                    Err(StepError::ControlFlow(ControlFlow::Skip { .. })) => {
                        child_ctx.store(&scope_step.name, StepOutput::Empty);
                    }
                    Err(StepError::ControlFlow(ControlFlow::Next { .. })) => {
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }

            // Use scope outputs if defined, otherwise last step
            let iter_output = if let Some(outputs_template) = &scope.outputs {
                match child_ctx.render_template(outputs_template) {
                    Ok(rendered) => StepOutput::Cmd(super::CmdOutput {
                        stdout: rendered,
                        stderr: String::new(),
                        exit_code: 0,
                        duration: std::time::Duration::ZERO,
                    }),
                    Err(_) => last_output,
                }
            } else {
                last_output
            };

            // Pass output as scope_value for next iteration
            scope_value =
                serde_json::Value::String(iter_output.text().to_string());

            iterations.push(IterationOutput {
                index: i,
                output: iter_output,
            });

            if should_break {
                break;
            }
        }

        if iterations.len() == max_iterations {
            tracing::warn!(
                "repeat '{}': max iterations ({}) reached without break",
                step.name,
                max_iterations
            );
        }

        let final_value = iterations.last().map(|i| Box::new(i.output.clone()));

        Ok(StepOutput::Scope(ScopeOutput {
            iterations,
            final_value,
        }))
    }
}
