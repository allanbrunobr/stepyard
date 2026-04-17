//! Minimal workflow representation for Story 2.3.
//!
//! Full `WorkflowDef` with scopes, gates, repeats, maps, etc. lives in the
//! legacy engine binary (`src/workflow/schema.rs`). This crate needs only
//! the shape that `Engine::step` operates over: an ordered list of named
//! commands.
//!
//! Story 2.4+ will widen this once the step-type family moves out of the
//! engine binary.

use serde::{Deserialize, Serialize};

/// A complete workflow definition — currently just a name and an ordered
/// list of steps. Expanded in later stories.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    pub name: String,
    pub steps: Vec<Step>,
}

impl Workflow {
    pub fn new(name: impl Into<String>, steps: Vec<Step>) -> Self {
        Self {
            name: name.into(),
            steps,
        }
    }
}

/// One step in a workflow. For Story 2.3 the only supported kind is a
/// shell command executed inside the [`crate::Engine`]'s sandbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub name: String,
    /// Shell command to run inside the sandbox.
    pub command: String,
    /// Wall-clock step timeout in milliseconds. Absent = no timeout.
    /// YAML field name is `timeout` to match the Story 1.4 workflow schema.
    #[serde(rename = "timeout", default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

impl Step {
    pub fn cmd(name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            timeout: None,
        }
    }

    /// Builder variant that attaches a wall-clock timeout (milliseconds).
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout = Some(timeout_ms);
        self
    }
}
