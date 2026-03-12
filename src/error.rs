use thiserror::Error;

use crate::control_flow::ControlFlow;

#[derive(Error, Debug)]
pub enum StepError {
    #[error("Step failed: {0}")]
    Fail(String),

    #[error("Control flow: {0:?}")]
    ControlFlow(ControlFlow),

    #[error("Timeout after {0:?}")]
    Timeout(std::time::Duration),

    #[error("Template error: {0}")]
    Template(String),

    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

impl From<ControlFlow> for StepError {
    fn from(cf: ControlFlow) -> Self {
        StepError::ControlFlow(cf)
    }
}
