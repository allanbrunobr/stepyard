use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// All events that can be emitted by the engine
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    StepStarted {
        step_name: String,
        step_type: String,
        timestamp: DateTime<Utc>,
    },
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
    StepFailed {
        step_name: String,
        step_type: String,
        error: String,
        duration_ms: u64,
        timestamp: DateTime<Utc>,
        sandboxed: bool,
    },
    WorkflowStarted {
        timestamp: DateTime<Utc>,
    },
    WorkflowCompleted {
        duration_ms: u64,
        timestamp: DateTime<Utc>,
    },
    SandboxCreated {
        sandbox_id: String,
        timestamp: DateTime<Utc>,
    },
    SandboxDestroyed {
        sandbox_id: String,
        timestamp: DateTime<Utc>,
    },
}
