//! The [`StepExecutor`] trait — abstraction for "run one step in a sandbox".
//!
//! Pulling this out lets tests swap in behavior without depending on a real
//! sandbox, while production code uses the default impl that calls
//! [`SandboxLifecycle::reuse_or_create`] + `sandbox.exec()`.

use async_trait::async_trait;
use minion_sandbox_orchestrator::{ExecOutput, SandboxError, SandboxLifecycle};
use std::sync::Arc;
use uuid::Uuid;

use crate::workflow::Step;

/// What `Engine::step` uses to actually execute one step inside a sandbox.
#[async_trait]
pub trait StepExecutor: Send + Sync {
    /// Run `step` for `session_id`. Implementations are responsible for
    /// spinning up / reusing a sandbox and translating `step.command` into
    /// a real execution.
    async fn execute(
        &self,
        session_id: Uuid,
        step: &Step,
    ) -> Result<ExecOutput, SandboxError>;
}

/// Default implementation — delegates to a [`SandboxLifecycle`].
pub struct SandboxStepExecutor {
    lifecycle: Arc<dyn SandboxLifecycle>,
}

impl SandboxStepExecutor {
    pub fn new(lifecycle: Arc<dyn SandboxLifecycle>) -> Self {
        Self { lifecycle }
    }
}

#[async_trait]
impl StepExecutor for SandboxStepExecutor {
    async fn execute(
        &self,
        session_id: Uuid,
        step: &Step,
    ) -> Result<ExecOutput, SandboxError> {
        let sandbox = self.lifecycle.reuse_or_create(session_id).await?;
        sandbox.exec(&step.command).await
    }
}
