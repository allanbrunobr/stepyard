//! Stable, IO-free types and traits shared across Stepyard Engine v2.
//!
//! This crate is the contract crate of the workspace. Other crates depend on
//! it; it depends on no runtime (no tokio, no sqlx, no reqwest). If you find
//! yourself wanting to add an IO dependency here, the type probably belongs
//! in a downstream crate (`stepyard-session`, `stepyard-harness`,
//! `stepyard-sandbox-orchestrator`).
//!
//! See `minion-engine/ARCHITECTURE.md` § "stepyard-core".

mod error;
mod event;
mod subscriber;
mod workflow;

pub use error::{EngineError, TerminationReason};
pub use event::Event;
pub use subscriber::EventSubscriber;
pub use workflow::WorkflowVersion;
