use std::collections::HashMap;

use async_trait::async_trait;

use crate::config::StepConfig;
use crate::control_flow::ControlFlow;
use crate::engine::context::Context;
use crate::error::StepError;
use crate::workflow::schema::{ScopeDef, StepDef, StepType};

use super::{
    agent::AgentExecutor, cmd::CmdExecutor, gate::GateExecutor, repeat::RepeatExecutor,
    chat::ChatExecutor, CmdOutput, IterationOutput, ScopeOutput, StepExecutor, StepOutput,
};

pub struct CallExecutor {
    scopes: HashMap<String, ScopeDef>,
}

impl CallExecutor {
    pub fn new(scopes: &HashMap<String, ScopeDef>) -> Self {
        Self {
            scopes: scopes.clone(),
        }
    }
}

#[async_trait]
impl StepExecutor for CallExecutor {
    async fn execute(
        &self,
        step: &StepDef,
        _config: &StepConfig,
        ctx: &Context,
    ) -> Result<StepOutput, StepError> {
        let scope_name = step
            .scope
            .as_ref()
            .ok_or_else(|| StepError::Fail("call step missing 'scope' field".into()))?;

        let scope = self
            .scopes
            .get(scope_name)
            .ok_or_else(|| StepError::Fail(format!("scope '{}' not found", scope_name)))?
            .clone();

        let mut child_ctx = Context::new(
            ctx.get_var("target")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            HashMap::new(),
        );

        let mut last_output = StepOutput::Empty;

        for scope_step in &scope.steps {
            let step_config = StepConfig::default();
            let result =
                dispatch_scope_step(scope_step, &step_config, &child_ctx, &self.scopes).await;

            match result {
                Ok(output) => {
                    child_ctx.store(&scope_step.name, output.clone());
                    last_output = output;
                }
                Err(StepError::ControlFlow(ControlFlow::Break { value, .. })) => {
                    if let Some(v) = value {
                        last_output = v;
                    }
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

        // Use scope outputs if defined, otherwise last step output
        let final_output = if let Some(outputs_template) = &scope.outputs {
            match child_ctx.render_template(outputs_template) {
                Ok(rendered) => StepOutput::Cmd(CmdOutput {
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

        Ok(StepOutput::Scope(ScopeOutput {
            iterations: vec![IterationOutput {
                index: 0,
                output: final_output.clone(),
            }],
            final_value: Some(Box::new(final_output)),
        }))
    }
}

pub(super) async fn dispatch_scope_step(
    step: &StepDef,
    config: &StepConfig,
    ctx: &Context,
    scopes: &HashMap<String, ScopeDef>,
) -> Result<StepOutput, StepError> {
    match step.step_type {
        StepType::Cmd => CmdExecutor.execute(step, config, ctx).await,
        StepType::Agent => AgentExecutor.execute(step, config, ctx).await,
        StepType::Gate => GateExecutor.execute(step, config, ctx).await,
        StepType::Chat => ChatExecutor.execute(step, config, ctx).await,
        StepType::Repeat => RepeatExecutor::new(scopes).execute(step, config, ctx).await,
        StepType::Call => CallExecutor::new(scopes).execute(step, config, ctx).await,
        _ => Err(StepError::Fail(format!(
            "Step type '{}' not supported in scope",
            step.step_type
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use crate::workflow::schema::{ScopeDef, StepType};

    fn cmd_step(name: &str, run: &str) -> StepDef {
        StepDef {
            name: name.to_string(),
            step_type: StepType::Cmd,
            run: Some(run.to_string()),
            prompt: None,
            condition: None,
            on_pass: None,
            on_fail: None,
            message: None,
            scope: None,
            max_iterations: None,
            initial_value: None,
            items: None,
            parallel: None,
            steps: None,
            config: HashMap::new(),
            outputs: None,
        }
    }

    fn call_step(name: &str, scope: &str) -> StepDef {
        StepDef {
            name: name.to_string(),
            step_type: StepType::Call,
            run: None,
            prompt: None,
            condition: None,
            on_pass: None,
            on_fail: None,
            message: None,
            scope: Some(scope.to_string()),
            max_iterations: None,
            initial_value: None,
            items: None,
            parallel: None,
            steps: None,
            config: HashMap::new(),
            outputs: None,
        }
    }

    #[tokio::test]
    async fn call_scope_with_two_steps() {
        let scope = ScopeDef {
            steps: vec![
                cmd_step("step1", "echo first"),
                cmd_step("step2", "echo second"),
            ],
            outputs: None,
        };
        let mut scopes = HashMap::new();
        scopes.insert("my_scope".to_string(), scope);

        let step = call_step("call_test", "my_scope");
        let executor = CallExecutor::new(&scopes);
        let config = StepConfig::default();
        let ctx = Context::new(String::new(), HashMap::new());

        let result = executor.execute(&step, &config, &ctx).await.unwrap();
        // Last step output is "second\n"
        assert!(result.text().contains("second"));
    }

    #[tokio::test]
    async fn call_with_explicit_outputs() {
        let scope = ScopeDef {
            steps: vec![
                cmd_step("step1", "echo hello"),
            ],
            outputs: Some("rendered: {{ steps.step1.stdout }}".to_string()),
        };
        let mut scopes = HashMap::new();
        scopes.insert("output_scope".to_string(), scope);

        let step = call_step("call_out", "output_scope");
        let executor = CallExecutor::new(&scopes);
        let config = StepConfig::default();
        let ctx = Context::new(String::new(), HashMap::new());

        let result = executor.execute(&step, &config, &ctx).await.unwrap();
        assert!(result.text().contains("rendered:"));
        assert!(result.text().contains("hello"));
    }

    #[tokio::test]
    async fn call_missing_scope_error() {
        let scopes = HashMap::new();
        let step = call_step("call_bad", "nonexistent");
        let executor = CallExecutor::new(&scopes);
        let config = StepConfig::default();
        let ctx = Context::new(String::new(), HashMap::new());

        let result = executor.execute(&step, &config, &ctx).await;
        assert!(result.is_err());
    }
}
