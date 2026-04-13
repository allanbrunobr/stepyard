//! The lifecycle [`Event`] enum emitted by the engine and consumed by every
//! [`EventSubscriber`](crate::EventSubscriber).
//!
//! # Forward-compatibility (NFC6)
//!
//! `Event` is `#[non_exhaustive]` and serializes via the `event` discriminator
//! tag in `snake_case`. Subscribers using `serde(other)` on their consumer
//! side can ignore unknown variants instead of failing — this is the contract
//! that keeps the Dashboard, Slack and webhook subscribers working when
//! the engine ships new variants.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Every observable lifecycle event in the engine.
///
/// Variants are stable. New variants may be added in minor versions.
/// Existing variant fields may gain `Option<T>` additions but never lose
/// fields without a major bump.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// A workflow execution started. Always the first event in a session.
    WorkflowStarted {
        timestamp: DateTime<Utc>,
    },
    /// A workflow execution finished successfully (status = `completed`).
    WorkflowCompleted {
        duration_ms: u64,
        timestamp: DateTime<Utc>,
    },
    /// A step started executing.
    StepStarted {
        step_name: String,
        step_type: String,
        timestamp: DateTime<Utc>,
    },
    /// A step finished successfully.
    StepCompleted {
        step_name: String,
        step_type: String,
        duration_ms: u64,
        timestamp: DateTime<Utc>,
        #[serde(skip_serializing_if = "Option::is_none")]
        input_tokens: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        output_tokens: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cost_usd: Option<f64>,
        sandboxed: bool,
    },
    /// A step finished with an error.
    StepFailed {
        step_name: String,
        step_type: String,
        error: String,
        duration_ms: u64,
        timestamp: DateTime<Utc>,
        sandboxed: bool,
    },
    /// A Docker sandbox container was created for this session.
    SandboxCreated {
        sandbox_id: String,
        timestamp: DateTime<Utc>,
    },
    /// A Docker sandbox container was destroyed.
    SandboxDestroyed {
        sandbox_id: String,
        timestamp: DateTime<Utc>,
    },
}
