use thiserror::Error;

use crate::control_flow::ControlFlow;

#[derive(Error, Debug)]
#[allow(dead_code)]
pub enum StepError {
    #[error("Step failed: {0}")]
    Fail(String),

    #[error("Control flow: {0:?}")]
    ControlFlow(ControlFlow),

    #[error("Timeout after {0:?}")]
    Timeout(std::time::Duration),

    #[error("Template error: {0}")]
    Template(String),

    #[error("Sandbox error: {message} (image: {image})")]
    Sandbox { message: String, image: String },

    #[error("Config error in '{field}': {message}")]
    Config { field: String, message: String },

    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

#[allow(dead_code)]
impl StepError {
    /// Create a sandbox error with image context
    pub fn sandbox(message: impl Into<String>, image: impl Into<String>) -> Self {
        StepError::Sandbox {
            message: message.into(),
            image: image.into(),
        }
    }

    /// Create a config validation error
    pub fn config(field: impl Into<String>, message: impl Into<String>) -> Self {
        StepError::Config {
            field: field.into(),
            message: message.into(),
        }
    }

    /// Returns true if this error is a timeout
    pub fn is_timeout(&self) -> bool {
        matches!(self, StepError::Timeout(_))
    }

    /// Returns true if this error is a control flow signal (not a real error)
    pub fn is_control_flow(&self) -> bool {
        matches!(self, StepError::ControlFlow(_))
    }

    /// Returns a human-friendly error category for logging
    pub fn category(&self) -> &'static str {
        match self {
            StepError::Fail(_) => "step_failure",
            StepError::ControlFlow(_) => "control_flow",
            StepError::Timeout(_) => "timeout",
            StepError::Template(_) => "template",
            StepError::Sandbox { .. } => "sandbox",
            StepError::Config { .. } => "config",
            StepError::Other(_) => "internal",
        }
    }
}

impl From<ControlFlow> for StepError {
    fn from(cf: ControlFlow) -> Self {
        StepError::ControlFlow(cf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_categories() {
        let err = StepError::Fail("test".into());
        assert_eq!(err.category(), "step_failure");
        assert!(!err.is_timeout());
        assert!(!err.is_control_flow());

        let err = StepError::Timeout(std::time::Duration::from_secs(30));
        assert_eq!(err.category(), "timeout");
        assert!(err.is_timeout());

        let err = StepError::sandbox("container crashed", "node:20");
        assert_eq!(err.category(), "sandbox");
        assert_eq!(
            err.to_string(),
            "Sandbox error: container crashed (image: node:20)"
        );

        let err = StepError::config("sandbox.image", "image not found");
        assert_eq!(err.category(), "config");
        assert_eq!(
            err.to_string(),
            "Config error in 'sandbox.image': image not found"
        );
    }

    #[test]
    fn test_error_display() {
        let err = StepError::Fail("connection refused".into());
        assert_eq!(err.to_string(), "Step failed: connection refused");

        let err = StepError::Template("bad syntax in {{name}}".into());
        assert_eq!(err.to_string(), "Template error: bad syntax in {{name}}");
    }
}
