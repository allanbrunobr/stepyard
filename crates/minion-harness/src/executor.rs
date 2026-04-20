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

    /// Run `step` for `session_id` with a resolved env map. Required — no
    /// default impl, because a default that delegates to [`execute`]
    /// silently drops `env`, which masks env-routing bugs. Every executor
    /// (prod or test) decides explicitly how to treat env pairs.
    async fn execute_with_env(
        &self,
        session_id: Uuid,
        step: &Step,
        env: &HashMap<String, String>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use minion_sandbox_orchestrator::MockLifecycle;
    use tokio::sync::Mutex;

    /// Recording executor that captures the env map it receives via
    /// `execute_with_env`. Proves the harness hands a real map through —
    /// a silent-drop impl would leave `captured` empty.
    struct RecordingExecutor {
        captured: Mutex<Option<HashMap<String, String>>>,
    }

    #[async_trait]
    impl StepExecutor for RecordingExecutor {
        async fn execute(
            &self,
            _session_id: Uuid,
            _step: &Step,
        ) -> Result<ExecOutput, SandboxError> {
            unreachable!("execute_with_env is the only entrypoint the engine uses")
        }

        async fn execute_with_env(
            &self,
            _session_id: Uuid,
            _step: &Step,
            env: &HashMap<String, String>,
        ) -> Result<ExecOutput, SandboxError> {
            *self.captured.lock().await = Some(env.clone());
            Ok(ExecOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
            })
        }
    }

    #[tokio::test]
    async fn execute_with_env_receives_full_env_map() {
        let executor = RecordingExecutor {
            captured: Mutex::new(None),
        };
        let mut env = HashMap::new();
        env.insert("FOO".to_string(), "bar".to_string());
        env.insert("TOKEN".to_string(), "xyz".to_string());

        let step = Step::cmd("probe".to_string(), "true".to_string());
        executor
            .execute_with_env(Uuid::new_v4(), &step, &env)
            .await
            .expect("execute_with_env");

        let got = executor
            .captured
            .lock()
            .await
            .clone()
            .expect("env captured");
        assert_eq!(got, env);
    }

    #[tokio::test]
    async fn sandbox_step_executor_threads_env_through_lifecycle() {
        // SandboxStepExecutor uses lifecycle.exec_with_env, which for
        // MockLifecycle records the exact env map. If SandboxStepExecutor
        // ever drops env silently, MockCall::ExecWithEnv.env will diverge.
        use minion_sandbox_orchestrator::MockCall;

        let lifecycle: Arc<MockLifecycle> = Arc::new(MockLifecycle::new());
        let executor = SandboxStepExecutor::new(lifecycle.clone());

        let mut env = HashMap::new();
        env.insert("KEY".to_string(), "value".to_string());
        let step = Step::cmd("s".to_string(), "true".to_string());

        executor
            .execute_with_env(Uuid::new_v4(), &step, &env)
            .await
            .expect("execute_with_env");

        let calls = lifecycle.calls().await;
        let recorded_env = calls
            .iter()
            .find_map(|c| match c {
                MockCall::ExecWithEnv { env, .. } => Some(env.clone()),
                _ => None,
            })
            .expect("ExecWithEnv recorded");
        assert_eq!(recorded_env, env);
    }
}
