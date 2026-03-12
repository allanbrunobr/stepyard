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
}
