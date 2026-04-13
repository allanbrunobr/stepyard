//! The harness loop of Minion Engine v2.
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

mod engine;
mod executor;
mod workflow;

pub use engine::{CancelToken, Engine, EngineError, HarnessConfig, StepOutcome};
pub use executor::StepExecutor;
pub use workflow::{Step, Workflow};
