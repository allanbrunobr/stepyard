//! Re-export of the canonical [`Event`] enum from `minion-core`.
//!
//! Story 2.1 moved `Event` to `crates/minion-core/src/event.rs` so that
//! downstream crates (sessions, harness, sandbox-orchestrator) can depend on
//! it without pulling in the engine binary. This module preserves the legacy
//! import path `crate::events::types::Event` so existing call sites keep
//! compiling unchanged.

pub use minion_core::Event;
