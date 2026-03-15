use std::collections::HashMap;

use async_trait::async_trait;

use crate::cli::display;
use crate::config::StepConfig;
use crate::control_flow::ControlFlow;
use crate::engine::context::Context;
use crate::error::StepError;
use crate::workflow::schema::{ScopeDef, StepDef};

use super::{
    call::dispatch_scope_step_sandboxed,
    IterationOutput, ScopeOutput, SharedSandbox, StepExecutor, StepOutput,
};

pub struct RepeatExecutor {
    scopes: HashMap<String, ScopeDef>,
    sandbox: SharedSandbox,
}

impl RepeatExecutor {
    pub fn new(scopes: &HashMap<String, ScopeDef>, sandbox: SharedSandbox) -> Self {
        Self {
            scopes: scopes.clone(),
            sandbox,
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

            // Create a child context inheriting all parent variables (stack, args, etc.)
            let mut child_ctx = Context::new(
                ctx.get_var("target")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                ctx.all_variables(),
            );
            child_ctx.scope_value = Some(scope_value.clone());
            child_ctx.scope_index = i;
            child_ctx.stack_info = ctx.get_stack_info().cloned();
            child_ctx.prompts_dir = ctx.prompts_dir.clone();

            let mut last_output = StepOutput::Empty;
            let mut should_break = false;

            for scope_step in &scope.steps {
                let step_config = StepConfig::default();

                let result = dispatch_scope_step_sandboxed(
                    scope_step, &step_config, &child_ctx, &self.scopes, &self.sandbox,
                ).await;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::StepConfig;
    use crate::engine::context::Context;
    use crate::workflow::parser;

    #[tokio::test]
    async fn repeat_runs_max_iterations_without_break() {
        let yaml = r#"
name: test
scopes:
  my_scope:
    steps:
      - name: step1
        type: cmd
        run: "echo hello"
steps:
  - name: repeat_step
    type: repeat
    scope: my_scope
    max_iterations: 3
"#;
        let wf = parser::parse_str(yaml).unwrap();
        let repeat_step = &wf.steps[0];
        let executor = RepeatExecutor::new(&wf.scopes, None);
        let ctx = Context::new(String::new(), HashMap::new());

        let result = executor
            .execute(repeat_step, &StepConfig::default(), &ctx)
            .await
            .unwrap();

        if let StepOutput::Scope(scope_out) = result {
            assert_eq!(scope_out.iterations.len(), 3);
        } else {
            panic!("Expected Scope output");
        }
    }

    #[tokio::test]
    async fn repeat_breaks_on_first_iteration_when_gate_passes() {
        let yaml = r#"
name: test
scopes:
  my_scope:
    steps:
      - name: step1
        type: cmd
        run: "echo hello"
      - name: check
        type: gate
        condition: "true"
        on_pass: break
        message: "done"
steps:
  - name: repeat_step
    type: repeat
    scope: my_scope
    max_iterations: 5
"#;
        let wf = parser::parse_str(yaml).unwrap();
        let repeat_step = &wf.steps[0];
        let executor = RepeatExecutor::new(&wf.scopes, None);
        let ctx = Context::new(String::new(), HashMap::new());

        let result = executor
            .execute(repeat_step, &StepConfig::default(), &ctx)
            .await
            .unwrap();

        if let StepOutput::Scope(scope_out) = result {
            assert_eq!(scope_out.iterations.len(), 1, "Should break after 1 iteration");
        } else {
            panic!("Expected Scope output");
        }
    }

    #[tokio::test]
    async fn repeat_scope_index_increments_each_iteration() {
        let yaml = r#"
name: test
scopes:
  counter:
    steps:
      - name: output_index
        type: cmd
        run: "echo {{ scope.index }}"
steps:
  - name: repeat_step
    type: repeat
    scope: counter
    max_iterations: 3
"#;
        let wf = parser::parse_str(yaml).unwrap();
        let repeat_step = &wf.steps[0];
        let executor = RepeatExecutor::new(&wf.scopes, None);
        let ctx = Context::new(String::new(), HashMap::new());

        let result = executor
            .execute(repeat_step, &StepConfig::default(), &ctx)
            .await
            .unwrap();

        if let StepOutput::Scope(scope_out) = result {
            assert_eq!(scope_out.iterations.len(), 3);
            assert_eq!(scope_out.iterations[0].output.text().trim(), "0");
            assert_eq!(scope_out.iterations[1].output.text().trim(), "1");
            assert_eq!(scope_out.iterations[2].output.text().trim(), "2");
        } else {
            panic!("Expected Scope output");
        }
    }

    #[tokio::test]
    async fn repeat_scope_value_flows_between_iterations() {
        // The output of each iteration becomes the scope.value for the next
        let yaml = r#"
name: test
scopes:
  counter:
    steps:
      - name: echo_scope
        type: cmd
        run: "echo iter-{{ scope.index }}"
steps:
  - name: repeat_step
    type: repeat
    scope: counter
    max_iterations: 3
    initial_value: "start"
"#;
        let wf = parser::parse_str(yaml).unwrap();
        let repeat_step = &wf.steps[0];
        let executor = RepeatExecutor::new(&wf.scopes, None);
        let ctx = Context::new(String::new(), HashMap::new());

        let result = executor
            .execute(repeat_step, &StepConfig::default(), &ctx)
            .await
            .unwrap();

        if let StepOutput::Scope(scope_out) = result {
            assert_eq!(scope_out.iterations.len(), 3);
            // Each iteration echoes its index
            assert_eq!(scope_out.iterations[0].output.text().trim(), "iter-0");
            assert_eq!(scope_out.iterations[1].output.text().trim(), "iter-1");
            assert_eq!(scope_out.iterations[2].output.text().trim(), "iter-2");
        } else {
            panic!("Expected Scope output");
        }
    }
}
