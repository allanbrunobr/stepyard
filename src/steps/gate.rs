use async_trait::async_trait;

use crate::config::StepConfig;
use crate::control_flow::ControlFlow;
use crate::engine::context::Context;
use crate::error::StepError;
use crate::workflow::schema::StepDef;

use super::{GateOutput, StepExecutor, StepOutput};

pub struct GateExecutor;

#[async_trait]
impl StepExecutor for GateExecutor {
    async fn execute(
        &self,
        step: &StepDef,
        _config: &StepConfig,
        ctx: &Context,
    ) -> Result<StepOutput, StepError> {
        let condition_template = step
            .condition
            .as_ref()
            .ok_or_else(|| StepError::Fail("gate step missing 'condition' field".into()))?;

        let rendered = ctx.render_template(condition_template)?;
        let passed = evaluate_bool(&rendered);
        let message = step.message.clone();

        let on_pass = step.on_pass.as_deref().unwrap_or("continue");
        let on_fail = step.on_fail.as_deref().unwrap_or("continue");

        let action = if passed { on_pass } else { on_fail };

        match action {
            "break" => Err(ControlFlow::Break {
                message: message.unwrap_or_else(|| "gate break".into()),
                value: None,
            }
            .into()),
            "fail" => Err(ControlFlow::Fail {
                message: message.unwrap_or_else(|| "gate failed".into()),
            }
            .into()),
            "skip" | "skip_next" => Err(ControlFlow::Skip {
                message: message.unwrap_or_else(|| "gate skip".into()),
            }
            .into()),
            _ => {
                // "continue" or unknown → just return the gate output
                Ok(StepOutput::Gate(GateOutput { passed, message }))
            }
        }
    }
}

fn evaluate_bool(s: &str) -> bool {
    let trimmed = s.trim().to_lowercase();
    matches!(trimmed.as_str(), "true" | "1" | "yes" | "ok")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::StepConfig;
    use crate::engine::context::Context;
    use crate::steps::{CmdOutput, StepOutput};
    use crate::workflow::schema::StepType;
    use std::collections::HashMap;
    use std::time::Duration;

    fn gate_step(condition: &str) -> StepDef {
        StepDef {
            name: "check".to_string(),
            step_type: StepType::Gate,
            run: None,
            prompt: None,
            condition: Some(condition.to_string()),
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

    #[test]
    fn bool_evaluation() {
        assert!(evaluate_bool("true"));
        assert!(evaluate_bool("  True  "));
        assert!(evaluate_bool("1"));
        assert!(evaluate_bool("yes"));
        assert!(!evaluate_bool("false"));
        assert!(!evaluate_bool("0"));
        assert!(!evaluate_bool("no"));
        assert!(!evaluate_bool(""));
    }

    #[tokio::test]
    async fn gate_condition_references_previous_step_exit_code() {
        let mut ctx = Context::new(String::new(), HashMap::new());
        ctx.store(
            "prev_step",
            StepOutput::Cmd(CmdOutput {
                stdout: "output".to_string(),
                stderr: String::new(),
                exit_code: 0,
                duration: Duration::ZERO,
            }),
        );

        // Condition references previous step's exit_code via template
        let step = gate_step("{{ steps.prev_step.exit_code == 0 }}");
        let result = GateExecutor
            .execute(&step, &StepConfig::default(), &ctx)
            .await
            .unwrap();

        if let StepOutput::Gate(gate) = result {
            assert!(gate.passed, "Gate should pass when exit_code == 0");
        } else {
            panic!("Expected Gate output");
        }
    }

    #[tokio::test]
    async fn gate_condition_fails_when_exit_code_nonzero() {
        let mut ctx = Context::new(String::new(), HashMap::new());
        ctx.store(
            "cmd_step",
            StepOutput::Cmd(CmdOutput {
                stdout: String::new(),
                stderr: "error".to_string(),
                exit_code: 1,
                duration: Duration::ZERO,
            }),
        );

        let step = gate_step("{{ steps.cmd_step.exit_code == 0 }}");
        let result = GateExecutor
            .execute(&step, &StepConfig::default(), &ctx)
            .await
            .unwrap();

        if let StepOutput::Gate(gate) = result {
            assert!(!gate.passed, "Gate should fail when exit_code != 0");
        } else {
            panic!("Expected Gate output");
        }
    }
}
