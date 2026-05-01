//! Stable, IO-free types and traits shared across Stepyard Engine v2.
//!
//! This crate is the contract crate of the workspace. Other crates depend on
//! it; it depends on no runtime (no tokio, no sqlx, no reqwest). If you find
//! yourself wanting to add an IO dependency here, the type probably belongs
//! in a downstream crate (`stepyard-session`, `stepyard-harness`,
//! `stepyard-sandbox-orchestrator`).
//!
//! See `minion-engine/ARCHITECTURE.md` § "stepyard-core".

pub mod duration;
pub mod env;
mod error;
mod event;
pub mod signal;
mod subscriber;
mod workflow;

pub use error::{EngineError, TerminationReason};
pub use event::{
    ChatMessage, ChatRole, Event, GateOutcome, ScopeContext, StepOutputSnapshot,
    WorkspacePruneReason,
};
pub use signal::Signal;
pub use subscriber::EventSubscriber;
pub use workflow::WorkflowVersion;
