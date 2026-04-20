//! Engine-level defaults loaded from `.minion/defaults.yaml` (Story 3.3).
//!
//! Kept as a thin wrapper over `HashMap<String, String>` so later stories
//! can extend beyond env (e.g., defaults for step-type configs) without a
//! cross-crate type rename. The binary-side loader
//! (`src/config/env_defaults.rs`) has its own `Defaults` type shaped the
//! same — they convert trivially at the binary boundary.

use std::collections::HashMap;

/// The defaults overlay used by [`Engine::prepare_step`](crate::Engine::prepare_step).
///
/// Placed below `workflow.env` and `step.env` in the cascade — defaults are
/// the weakest layer (step > workflow > defaults > host `${VAR}`).
#[derive(Debug, Clone, Default)]
pub struct Defaults {
    pub env: HashMap<String, String>,
}

impl Defaults {
    /// Convenience constructor for tests and call sites that already have
    /// an env map in hand.
    pub fn with_env(env: HashMap<String, String>) -> Self {
        Self { env }
    }
}
