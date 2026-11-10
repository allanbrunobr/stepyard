use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::task::JoinSet;

use crate::config::manager::ConfigManager;
use crate::config::StepConfig;
use crate::engine::context::Context;
use crate::error::StepError;
use crate::workflow::schema::{ScopeDef, StepDef, StepType};

use super::{
    agent::AgentExecutor, call::resolve_scope_step_config, chat::ChatExecutor, cmd::CmdExecutor,
    gate::GateExecutor, SandboxAwareExecutor, SharedSandbox, StepExecutor, StepOutput,
};

pub struct ParallelExecutor {
    scopes: HashMap<String, ScopeDef>,
    sandbox: SharedSandbox,
    config_manager: Option<Arc<ConfigManager>>,
}

impl ParallelExecutor {
    pub fn new(scopes: &HashMap<String, ScopeDef>, sandbox: SharedSandbox) -> Self {
        Self {
            scopes: scopes.clone(),
            sandbox,
            config_manager: None,
        }
    }

    pub fn with_config_manager(mut self, cm: Option<Arc<ConfigManager>>) -> Self {
        self.config_manager = cm;
        self
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
            let cm_clone = self.config_manager.clone();

            set.spawn(async move {
                let step_config = resolve_scope_step_config(&cm_clone, &sub);
                let result =
                    dispatch_step(&sub, &step_config, &child_ctx, &scopes, &sandbox_clone).await;
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
                Ok((
                    name,
                    Err(StepError::ControlFlow(crate::control_flow::ControlFlow::Skip { .. })),
                )) => {
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
    config: &StepConfig,
    ctx: &Context,
    _scopes: &HashMap<String, ScopeDef>,
    sandbox: &SharedSandbox,
) -> Result<StepOutput, StepError> {
    // `config` is the four-layer resolved config (global < type < pattern <
    // step) computed by the caller with `resolve_scope_step_config`, as
    // map/repeat/call do. It used to be ignored in favour of the step's
    // inline config only, which lost the `cmd.fail_on_error: true` default:
    // a sub-step exiting non-zero was recorded as success (BUG-MOTOR-08).

    match step.step_type {
        StepType::Cmd => {
            CmdExecutor
                .execute_sandboxed(step, config, ctx, sandbox)
                .await
        }
        StepType::Agent => {
            AgentExecutor
                .execute_sandboxed(step, config, ctx, sandbox)
                .await
        }
        StepType::Gate => GateExecutor.execute(step, config, ctx).await,
        StepType::Chat => ChatExecutor.execute(step, config, ctx).await,
        _ => Err(StepError::Fail(format!(
            "Step type '{}' not supported in parallel",
            step.step_type
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::schema::StepType;
    use std::collections::HashMap;

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
            env: HashMap::new(),
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
            env: HashMap::new(),
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

    fn defaults_manager() -> Option<Arc<ConfigManager>> {
        let config =
            crate::config::apply_defaults(&crate::workflow::schema::WorkflowConfig::default());
        Some(Arc::new(ConfigManager::new(config)))
    }

    /// P1-8 (analysis 2026-11-09, BUG-MOTOR-08): with the embedded defaults
    /// (`cmd.fail_on_error: true`), a sub-step that exits non-zero must fail
    /// the parallel step. It used to be recorded as success.
    #[tokio::test]
    async fn parallel_cmd_exiting_non_zero_fails_with_the_resolved_defaults() {
        let scopes = HashMap::new();
        let step = parallel_step(
            "parallel_exit",
            vec![
                cmd_step("ok_step", "echo ok"),
                cmd_step("bad_step", "exit 101"),
            ],
        );
        let executor = ParallelExecutor::new(&scopes, None).with_config_manager(defaults_manager());
        let ctx = Context::new(String::new(), HashMap::new());

        let result = executor.execute(&step, &StepConfig::default(), &ctx).await;
        assert!(
            result.is_err(),
            "exit 101 must fail the parallel step: {result:?}"
        );
    }

    #[tokio::test]
    async fn parallel_cmd_with_inline_fail_on_error_false_still_tolerates_failure() {
        let scopes = HashMap::new();
        let mut tolerant = cmd_step("tolerant", "exit 3");
        tolerant
            .config
            .insert("fail_on_error".into(), serde_yaml::Value::Bool(false));
        let step = parallel_step("parallel_tolerant", vec![tolerant]);
        let executor = ParallelExecutor::new(&scopes, None).with_config_manager(defaults_manager());
        let ctx = Context::new(String::new(), HashMap::new());

        let result = executor.execute(&step, &StepConfig::default(), &ctx).await;
        assert!(
            result.is_ok(),
            "inline fail_on_error: false wins: {result:?}"
        );
    }

    #[tokio::test]
    async fn parallel_one_failure_returns_error() {
        let scopes = HashMap::new();
        let step = parallel_step(
            "parallel_fail",
            vec![cmd_step("ok_step", "echo ok"), {
                // Use an unsupported step type to force dispatch_step to return Err
                let mut s = cmd_step("fail_step", "echo fake");
                s.step_type = StepType::Template;
                s
            }],
        );
        let executor = ParallelExecutor::new(&scopes, None);
        let config = StepConfig::default();
        let ctx = Context::new(String::new(), HashMap::new());

        let result = executor.execute(&step, &config, &ctx).await;
        assert!(result.is_err(), "Expected error due to failing step");
    }
}
