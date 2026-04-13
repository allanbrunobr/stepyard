//! [`EngineError`] — the engine's domain error type.
//!
//! Engine APIs (Story 2.3 onward) return `Result<T, EngineError>`. This
//! type intentionally has no `anyhow::Error` variant in its public API
//! (Story 2.1 AC) — `anyhow` is for prototyping; engine surfaces use this
//! typed error so callers can `match` precisely.

use thiserror::Error;

/// Errors returned by engine-level APIs.
#[non_exhaustive]
#[derive(Debug, Error)]
pub enum EngineError {
    /// Workflow YAML failed parsing or validation.
    #[error("invalid workflow: {0}")]
    InvalidWorkflow(String),

    /// Persistence layer failure (session storage, migrations, etc).
    #[error("persistence error: {0}")]
    Persistence(String),

    /// Sandbox lifecycle failure (Docker create/destroy, exec, etc).
    #[error("sandbox error: {0}")]
    Sandbox(String),

    /// A step failed during execution.
    #[error("step `{step_name}` failed: {message}")]
    Step { step_name: String, message: String },

    /// Cancelled by the operator (SIGTERM, Engine::cancel, etc).
    #[error("cancelled")]
    Cancelled,

    /// Configuration error (missing env var, malformed value, etc).
    #[error("config error: {0}")]
    Config(String),

    /// Catch-all for unexpected internal errors. Fixing the cause is always
    /// preferable to widening this variant.
    #[error("internal error: {0}")]
    Internal(String),
}
