use std::collections::HashMap;

use async_trait::async_trait;
use tokio::task::JoinSet;

use crate::config::StepConfig;
use crate::engine::context::Context;
use crate::error::StepError;
use crate::workflow::schema::{ScopeDef, StepDef, StepType};

use super::{
    agent::AgentExecutor, cmd::CmdExecutor, chat::ChatExecutor, gate::GateExecutor,
    SandboxAwareExecutor, SharedSandbox, StepExecutor, StepOutput,
};

pub struct ParallelExecutor {
    scopes: HashMap<String, ScopeDef>,
    sandbox: SharedSandbox,
}

impl ParallelExecutor {
    pub fn new(scopes: &HashMap<String, ScopeDef>, sandbox: SharedSandbox) -> Self {
        Self {
            scopes: scopes.clone(),
            sandbox,
        }
    }
}

#[async_trait]
impl StepExecutor for ParallelExecutor {
    async fn execute(
        &self,
        step: &StepDef,
        _config: &StepConfig,
        ctx: &Context,
    ) -> Result<StepOutput, StepError> {
        let nested_steps = step
            .steps
            .as_ref()
            .ok_or_else(|| StepError::Fail("parallel step missing 'steps' field".into()))?;

        let mut set: JoinSet<(String, Result<StepOutput, StepError>)> = JoinSet::new();

        for sub_step in nested_steps.iter() {
            let sub = sub_step.clone();
            let scopes = self.scopes.clone();
            let child_ctx = make_child_ctx(ctx);
            let sandbox_clone = self.sandbox.clone();

            set.spawn(async move {
                let result = dispatch_step(&sub, &StepConfig::default(), &child_ctx, &scopes, &sandbox_clone).await;
                (sub.name.clone(), result)
            });
        }

        let mut outputs: HashMap<String, StepOutput> = HashMap::new();
        let mut error: Option<StepError> = None;

        while let Some(res) = set.join_next().await {
            match res {
                Ok((name, Ok(output))) => {
                    outputs.insert(name, output);
                }
                Ok((name, Err(StepError::ControlFlow(crate::control_flow::ControlFlow::Skip { .. })))) => {
                    outputs.insert(name, StepOutput::Empty);
                }
                Ok((_, Err(e))) => {
                    set.abort_all();
                    error = Some(e);
                }
                Err(e) => {
                    set.abort_all();
                    if error.is_none() {
                        error = Some(StepError::Fail(format!("Parallel task panicked: {e}")));
                    }
                }
            }
        }

        if let Some(e) = error {
            return Err(e);
        }

        // Return combined output — for now return last output or Empty
        let last_output = nested_steps
            .last()
            .and_then(|s| outputs.get(&s.name))
            .cloned()
            .unwrap_or(StepOutput::Empty);

        Ok(last_output)
    }
}

fn make_child_ctx(parent: &Context) -> Context {
    let target = parent
        .get_var("target")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Context::new(target, HashMap::new())
}

async fn dispatch_step(
    step: &StepDef,
    _config: &StepConfig,
    ctx: &Context,
    _scopes: &HashMap<String, ScopeDef>,
    sandbox: &SharedSandbox,
) -> Result<StepOutput, StepError> {
    // Build config from step's inline config (convert yaml -> json)
    let values: HashMap<String, serde_json::Value> = step
        .config
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::to_value(v).unwrap_or(serde_json::Value::Null)))
        .collect();
    let step_config = StepConfig { values };

    match step.step_type {
        StepType::Cmd => CmdExecutor.execute_sandboxed(step, &step_config, ctx, sandbox).await,
        StepType::Agent => AgentExecutor.execute_sandboxed(step, &step_config, ctx, sandbox).await,
        StepType::Gate => GateExecutor.execute(step, &step_config, ctx).await,
        StepType::Chat => ChatExecutor.execute(step, &step_config, ctx).await,
        _ => Err(StepError::Fail(format!(
            "Step type '{}' not supported in parallel",
            step.step_type
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use crate::workflow::schema::StepType;

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
            output_type: None,
            async_exec: None,
        }
    }

    fn parallel_step(name: &str, sub_steps: Vec<StepDef>) -> StepDef {
        StepDef {
            name: name.to_string(),
            step_type: StepType::Parallel,
            run: None,
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
            steps: Some(sub_steps),
            config: HashMap::new(),
            outputs: None,
            output_type: None,
            async_exec: None,
        }
    }

    #[tokio::test]
    async fn parallel_two_cmd_steps() {
        let scopes = HashMap::new();
        let step = parallel_step(
            "parallel_test",
            vec![
                cmd_step("step_a", "echo alpha"),
                cmd_step("step_b", "echo beta"),
            ],
        );
        let executor = ParallelExecutor::new(&scopes, None);
        let config = StepConfig::default();
        let ctx = Context::new(String::new(), HashMap::new());

        let result = executor.execute(&step, &config, &ctx).await;
        assert!(result.is_ok(), "Expected success: {:?}", result.err());
    }

    #[tokio::test]
    async fn parallel_one_failure_returns_error() {
        let scopes = HashMap::new();
        let step = parallel_step(
            "parallel_fail",
            vec![
                cmd_step("ok_step", "echo ok"),
                {
                    // Use an unsupported step type to force dispatch_step to return Err
                    let mut s = cmd_step("fail_step", "echo fake");
                    s.step_type = StepType::Template;
                    s
                },
            ],
        );
        let executor = ParallelExecutor::new(&scopes, None);
        let config = StepConfig::default();
        let ctx = Context::new(String::new(), HashMap::new());

        let result = executor.execute(&step, &config, &ctx).await;
        assert!(result.is_err(), "Expected error due to failing step");
    }
}
