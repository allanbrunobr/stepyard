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

    #[error("Sandbox error: {message} (image: {image})")]
    Sandbox {
        message: String,
        image: String,
    },

    #[error("Config error in '{field}': {message}")]
    Config {
        field: String,
        message: String,
    },

    #[error("API rate limit exceeded after {attempts} retries (total duration: {duration:?})")]
    RateLimitExhausted {
        attempts: u32,
        duration: std::time::Duration,
        last_error: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("HTTP error {status_code}: {message}")]
    HttpError {
        status_code: u16,
        message: String,
        retry_after: Option<std::time::Duration>,
    },

    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

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

    /// Returns true if this error is a rate limit error (429 HTTP status)
    pub fn is_rate_limit(&self) -> bool {
        matches!(self, StepError::HttpError { status_code: 429, .. })
    }

    /// Returns true if this error should trigger a retry
    pub fn should_retry(&self) -> bool {
        match self {
            StepError::HttpError { status_code, .. } => *status_code == 429,
            _ => false,
        }
    }

    /// Returns the HTTP status code if this is an HTTP error
    pub fn status_code(&self) -> Option<u16> {
        match self {
            StepError::HttpError { status_code, .. } => Some(*status_code),
            _ => None,
        }
    }

    /// Returns the retry-after duration if available
    pub fn retry_after(&self) -> Option<std::time::Duration> {
        match self {
            StepError::HttpError { retry_after, .. } => *retry_after,
            _ => None,
        }
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
            StepError::RateLimitExhausted { .. } => "rate_limit_exhausted",
            StepError::HttpError { .. } => "http_error",
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

        let err = StepError::HttpError {
            status_code: 429,
            message: "Too Many Requests".to_string(),
            retry_after: Some(std::time::Duration::from_secs(30)),
        };
        assert_eq!(err.to_string(), "HTTP error 429: Too Many Requests");
        assert!(err.is_rate_limit());
        assert!(err.should_retry());
        assert_eq!(err.status_code(), Some(429));
        assert_eq!(err.retry_after(), Some(std::time::Duration::from_secs(30)));
        assert_eq!(err.category(), "http_error");

        let err = StepError::RateLimitExhausted {
            attempts: 3,
            duration: std::time::Duration::from_secs(10),
            last_error: Box::new(std::io::Error::new(std::io::ErrorKind::Other, "API limit")),
        };
        assert_eq!(
            err.to_string(),
            "API rate limit exceeded after 3 retries (total duration: 10s)"
        );
        assert_eq!(err.category(), "rate_limit_exhausted");
        assert!(!err.should_retry()); // Exhausted errors shouldn't trigger more retries
    }

    #[test]
    fn test_http_error_variants() {
        // Test non-retryable HTTP error
        let err = StepError::HttpError {
            status_code: 404,
            message: "Not Found".to_string(),
            retry_after: None,
        };
        assert!(!err.is_rate_limit());
        assert!(!err.should_retry());
        assert_eq!(err.status_code(), Some(404));
        assert_eq!(err.retry_after(), None);

        // Test retryable rate limit error
        let err = StepError::HttpError {
            status_code: 429,
            message: "Rate Limited".to_string(),
            retry_after: Some(std::time::Duration::from_secs(60)),
        };
        assert!(err.is_rate_limit());
        assert!(err.should_retry());
        assert_eq!(err.retry_after(), Some(std::time::Duration::from_secs(60)));
    }

    #[test]
    fn test_non_http_errors_retry_behavior() {
        // Non-HTTP errors should not be retryable by default
        let err = StepError::Fail("Generic failure".to_string());
        assert!(!err.should_retry());
        assert!(!err.is_rate_limit());
        assert_eq!(err.status_code(), None);
        assert_eq!(err.retry_after(), None);

        let err = StepError::Timeout(std::time::Duration::from_secs(30));
        assert!(!err.should_retry());
        assert!(!err.is_rate_limit());
    }
}
