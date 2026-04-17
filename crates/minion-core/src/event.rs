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
    /// A step hit its configured wall-clock timeout. Emitted immediately
    /// before the engine tears the sandbox down (Story 1.4 emit-before-IO
    /// ordering). `timestamp` is intentionally absent — D5's
    /// timeout-fired family carries only the structural facts the engine
    /// knows at firing time; wall-clock attribution happens via the
    /// surrounding `StepFailed` event (Story 1.4) or the session log.
    StepTimeoutFired {
        step_index: u32,
        configured_ms: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_timeout_fired_serializes_with_event_tag_and_snake_case_discriminator() {
        let event = Event::StepTimeoutFired {
            step_index: 2,
            configured_ms: 300_000,
        };
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "event": "step_timeout_fired",
                "step_index": 2,
                "configured_ms": 300_000,
            })
        );
    }

    #[test]
    fn step_timeout_fired_roundtrips_through_json() {
        let original = Event::StepTimeoutFired {
            step_index: 7,
            configured_ms: 5_000,
        };
        let s = serde_json::to_string(&original).unwrap();
        let back: Event = serde_json::from_str(&s).unwrap();
        match back {
            Event::StepTimeoutFired {
                step_index,
                configured_ms,
            } => {
                assert_eq!(step_index, 7);
                assert_eq!(configured_ms, 5_000);
            }
            other => panic!("roundtrip produced unexpected variant: {other:?}"),
        }
    }
}
