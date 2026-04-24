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

    /// A single workflow field failed type-level validation at parse time.
    ///
    /// Carries the field path produced by `serde_path_to_error`
    /// (e.g. `steps[0].timeout`), the offending raw value as a string
    /// (e.g. `30`), and a static phrase describing the expected shape
    /// (e.g. `duration string (e.g. 30s, 500ms, 1h30m)`). Round 3 rule
    /// text: architecture.md §D9 Error Variants.
    ///
    /// `InvalidWorkflow(String)` remains the right variant for
    /// document-level failures that cannot be pinned to a single
    /// field (malformed YAML, cross-field invariant violations).
    #[error("invalid workflow field at `{path}`: got `{got}`, expected {expected}")]
    InvalidWorkflowField {
        path: String,
        got: String,
        expected: &'static str,
    },

    /// Persistence layer failure (session storage, migrations, etc).
    #[error("persistence error: {0}")]
    Persistence(String),

    /// Sandbox lifecycle failure (Docker create/destroy, exec, etc).
    #[error("sandbox error: {0}")]
    Sandbox(String),

    /// A step terminated abnormally. The `reason` carries the taxonomy
    /// (timeout, idle, cancel, signal, or arbitrary failure) per D9.
    #[error("step {step_index} failed: {reason}")]
    StepFailed {
        step_index: u32,
        reason: TerminationReason,
    },

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

/// How a step terminated — the `reason` carried by [`EngineError::StepFailed`].
///
/// Every path that ends a step (timeout, idle, cancel, signal, arbitrary
/// failure) funnels through this one taxonomy (D9), so consumers — CLI
/// status formatters, session replay, subscribers — match on the reason
/// in a single place instead of listing sibling variants on `EngineError`
/// itself.
#[non_exhaustive]
#[derive(Debug, Clone, Error)]
pub enum TerminationReason {
    /// Wall-clock deadline elapsed (the `timeout:` YAML field).
    #[error("step timeout after {configured_ms}ms")]
    StepTimeout { configured_ms: u64 },

    /// No stdout byte received within `idle_ms` — the agent stalled.
    #[error("idle timeout after {idle_ms}ms with no output")]
    IdleTimeout { idle_ms: u64 },

    /// Cancelled by the operator (Engine::cancel, CancelToken, etc).
    #[error("cancelled")]
    Cancelled,

    /// Process-level signal received (lowercase snake_case: `sigterm`,
    /// `sigint`, `crash_recovery`).
    #[error("signal received: {0}")]
    SignalReceived(String),

    /// Catch-all for anything the richer variants don't cover.
    #[error("{0}")]
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn termination_reason_display_messages_are_stable() {
        assert_eq!(
            TerminationReason::StepTimeout {
                configured_ms: 300_000
            }
            .to_string(),
            "step timeout after 300000ms"
        );
        assert_eq!(
            TerminationReason::IdleTimeout { idle_ms: 60_000 }.to_string(),
            "idle timeout after 60000ms with no output"
        );
        assert_eq!(TerminationReason::Cancelled.to_string(), "cancelled");
        assert_eq!(
            TerminationReason::SignalReceived("sigterm".into()).to_string(),
            "signal received: sigterm"
        );
        assert_eq!(
            TerminationReason::Other("disk full".into()).to_string(),
            "disk full"
        );
    }

    #[test]
    fn termination_reason_debug_is_stable() {
        // Event payloads and subscribers lean on Debug formatting; lock
        // the shape down so a future derive tweak doesn't silently shift
        // wire output.
        let reason = TerminationReason::StepTimeout {
            configured_ms: 5000,
        };
        assert_eq!(
            format!("{reason:?}"),
            "StepTimeout { configured_ms: 5000 }"
        );
    }

    #[test]
    fn step_failed_display_includes_reason() {
        let err = EngineError::StepFailed {
            step_index: 2,
            reason: TerminationReason::Cancelled,
        };
        assert_eq!(err.to_string(), "step 2 failed: cancelled");
    }

    #[test]
    fn invalid_workflow_field_display_is_stable() {
        let err = EngineError::InvalidWorkflowField {
            path: "steps[0].timeout".into(),
            got: "30".into(),
            expected: "duration string (e.g. 30s, 500ms, 1h30m)",
        };
        assert_eq!(
            err.to_string(),
            "invalid workflow field at `steps[0].timeout`: got `30`, \
             expected duration string (e.g. 30s, 500ms, 1h30m)"
        );
    }
}
