//! The harness loop of Stepyard Engine v2.
//!
//! This crate ships the [`Engine`] type — the central entry point that
//! decomposes the old monolithic `Engine::run()` into three composable
//! primitives:
//!
//! * [`Engine::step`] advances the workflow by exactly one step and writes
//!   `StepStarted` + `StepCompleted | StepFailed` to the [`Session`] log.
//! * [`Engine::resume`] is a convenience wrapper that drives `step` in a
//!   loop from wherever the session currently is until the workflow
//!   terminates.
//! * [`Engine::cancel`] signals an active session to stop. The currently
//!   executing step finishes as `StepFailed { error: Cancelled }`, the
//!   sandbox is destroyed, and the session status becomes `cancelled`.
//!
//! All state lives in the session log — the harness holds zero per-step
//! memory. A process crash between steps is fully recoverable via
//! [`Engine::resume`] (Invariante 11).
//!
//! # Scope
//!
//! The first revision (Story 2.3 of the engine v2 refactor) supports
//! **cmd-type steps only** — a step is `{ name, command }` and we run
//! `command` inside the [`SandboxLifecycle`]. Richer step types (agent,
//! chat, gate, repeat, map) live in the legacy engine binary and will be
//! ported in Story 2.4+. This deliberate scope cut keeps the contract
//! (step/resume/cancel) provable with minimal surface area.

mod agent_exec;
mod chat_exec;
mod defaults;
mod engine;
mod executor;
mod gate;
mod process_group;
mod render;
mod scope;
mod script_exec;
#[cfg(feature = "postgres")]
pub mod startup;
mod template_exec;
mod workflow;

pub use chat_exec::{ChatClient, ChatClientError, ChatCompletionRequest, ChatCompletionResponse};
pub use defaults::Defaults;
pub use engine::{
    resolve_env, CancelToken, Engine, EngineError, HarnessConfig, RunContext, StepOutcome,
};
pub use executor::StepExecutor;
pub use gate::{GateAction, GateError, GateOutcome};
pub use render::{render, RenderContext, RenderError};
pub use workflow::{
    AgentPermissions, AgentSessionMode, BranchStrategyYaml, ChatProvider, ChatTruncation, Scope,
    Step, StepKind, Workflow,
};

// Re-export so callers matching on `EngineError::StepFailed.reason` don't
// need to import minion-core directly (Story 1.4).
pub use stepyard_core::TerminationReason;

// Re-exported chat turn types (PR 5b of Task #31). Chat-producing
// callers (the upcoming `run_chat_step` + subscribers replaying
// history) need these to construct / deserialize
// [`stepyard_core::Event::ChatMessageAppended`] without pulling the
// core crate in separately.
pub use stepyard_core::{ChatMessage, ChatRole, Signal};
