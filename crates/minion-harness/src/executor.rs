//! The [`StepExecutor`] trait — abstraction for "run one step in a sandbox".
//!
//! Pulling this out lets tests swap in behavior without depending on a real
//! sandbox, while production code uses the default impl that calls
//! [`SandboxLifecycle::reuse_or_create`] + `sandbox.exec()`.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use minion_sandbox_orchestrator::{ExecOutput, SandboxError, SandboxId, SandboxLifecycle};
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

    /// Run `step` for `session_id` with a resolved env map. Default impl
    /// drops `env` and delegates to [`execute`] — preserving backward compat
    /// for mock executors that predate Story 3.4 (D3: additive extension).
    /// Production impls override this to plumb env pairs into argv-form
    /// `--env K=V` flags (see [`SandboxStepExecutor`]).
    async fn execute_with_env(
        &self,
        session_id: Uuid,
        step: &Step,
        env: &HashMap<String, String>,
    ) -> Result<ExecOutput, SandboxError> {
        let _ = env;
        self.execute(session_id, step).await
    }
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

    async fn execute_with_env(
        &self,
        session_id: Uuid,
        step: &Step,
        env: &HashMap<String, String>,
    ) -> Result<ExecOutput, SandboxError> {
        // Ensure the container exists (create-on-first-step). We discard the
        // Sandbox handle — the lifecycle::exec_with_env path uses the
        // SandboxId derived from session_id (harness convention), so we do
        // not need the handle's exec_fn here.
        let _sandbox = self.lifecycle.reuse_or_create(session_id).await?;
        let sandbox_id = SandboxId::from(session_id);
        // D7/NFR-argv: step.command is wrapped as `sh -c <command>` argv.
        // The env pairs flow through `--env K=V` argv elements in
        // lifecycle.exec_with_env — never concatenated into a shell string.
        let argv = vec!["sh".to_string(), "-c".to_string(), step.command.clone()];
        self.lifecycle.exec_with_env(&sandbox_id, &argv, env).await
    }
}
