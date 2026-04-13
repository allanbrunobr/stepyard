//! Workflow type contracts.
//!
//! Story 2.1 only ships the [`WorkflowVersion`] enum here so that downstream
//! crates (parser, validator) can compile against the version discriminator
//! without pulling the full `WorkflowDef` into `minion-core`. The complete
//! `WorkflowDef` migration to this crate happens in **Story 2.3** when the
//! parser/validator move out of the engine binary into `minion-harness`.
//!
//! Until then, the engine binary keeps `WorkflowDef` in `src/workflow/schema.rs`
//! and re-exports the same `WorkflowVersion` from this crate so both halves
//! agree on the type.

use serde::{Deserialize, Serialize};

/// YAML schema version, used by the parser to dispatch into the right struct.
///
/// `V1` is the original 0.7.x schema. `V2` is the post-engine-v2 schema (Story
/// 5.3 ships the parser change). 180-day backward compatibility is enforced
/// by the parser (ADR-012).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowVersion {
    V1,
    V2,
}

impl Default for WorkflowVersion {
    fn default() -> Self {
        Self::V1
    }
}
