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
    },
    StepFailed {
        step_name: String,
        step_type: String,
        error: String,
        duration_ms: u64,
        timestamp: DateTime<Utc>,
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
