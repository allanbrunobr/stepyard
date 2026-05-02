//! Integration coverage for workflow-level `completion_signal`.
//!
//! These tests shell out to the existing mock Claude fixtures, so they require
//! the same Postgres gate as the other harness replay tests.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sqlx::postgres::PgPoolOptions;
use stepyard_harness::{
    Engine, EngineError, HarnessConfig, Step, StepExecutor, StepOutcome, TerminationReason, Workflow,
};
use stepyard_sandbox_orchestrator::{ExecOutput, MockLifecycle, SandboxError, SandboxLifecycle};
use stepyard_session::{migrate, Session, SessionEvent, SessionStatus};
use uuid::Uuid;

async fn pool() -> Option<sqlx::PgPool> {
    let url = std::env::var("STEPYARD_HARNESS_DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .expect("reach DB");
    migrate(&pool).await.expect("migrations ok");
    Some(pool)
}

macro_rules! db_test {
    ($pool:ident, $body:block) => {{
        let Some($pool) = pool().await else {
            eprintln!("[skip] STEPYARD_HARNESS_DATABASE_URL not set");
            return;
        };
        $body
    }};
}

#[derive(Default, Clone)]
struct UnreachableExecutor;

#[async_trait]
impl StepExecutor for UnreachableExecutor {
    async fn execute(&self, session_id: Uuid, step: &Step) -> Result<ExecOutput, SandboxError> {
        self.execute_with_env(session_id, step, &HashMap::new()).await
    }

    async fn execute_with_env(
        &self,
        _session_id: Uuid,
        step: &Step,
        _env: &HashMap<String, String>,
    ) -> Result<ExecOutput, SandboxError> {
        panic!("completion_signal test should not execute `{}`", step.name)
    }
}

fn lifecycle() -> Arc<dyn SandboxLifecycle> {
    Arc::new(MockLifecycle::new())
}

fn unreachable_executor() -> Arc<dyn StepExecutor> {
    Arc::new(UnreachableExecutor)
}

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn agent_step(name: &str, prompt: &str) -> Step {
    let mut step = Step::agent(name, prompt);
    step.agent_command = Some(
        fixture_path("mock_claude.sh")
            .to_string_lossy()
            .into_owned(),
    );
    step
}

fn hanging_agent_step(name: &str, prompt: &str, timeout: Duration) -> Step {
    let mut step = Step::agent(name, prompt);
    step.agent_command = Some(
        fixture_path("mock_claude_hang.sh")
            .to_string_lossy()
            .into_owned(),
    );
    step.timeout = Some(timeout);
    step
}

async fn events(engine: &Engine) -> Vec<SessionEvent> {
    engine.session().replay().await.expect("replay")
}

fn event_kind(ev: &SessionEvent) -> Option<&str> {
    ev.payload.get("event").and_then(|v| v.as_str())
}

#[tokio::test]
async fn completion_signal_ends_workflow_after_matching_agent_stdout() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "completion".into())
            .await
            .expect("session");
        let session_id = session.id();

        let mut workflow = Workflow::new(
            "completion",
            vec![
                agent_step("ask", "finish"),
                Step::cmd("must-not-run", "echo should-not-run"),
            ],
        );
        workflow.completion_signal = Some("Task completed".into());

        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            workflow,
            lifecycle(),
            unreachable_executor(),
        );

        let outcome = engine.resume().await.expect("resume");
        assert_eq!(outcome, StepOutcome::WorkflowCompleted);

        let evs = events(&engine).await;
        let tags: Vec<&str> = evs.iter().filter_map(event_kind).collect();
        assert_eq!(
            tags,
            vec![
                "workflow_started",
                "step_started",
                "completion_signaled",
                "workflow_completed",
            ],
            "completion signal should terminate before the second step, got {tags:?}"
        );

        let signaled = evs
            .iter()
            .find(|e| event_kind(e) == Some("completion_signaled"))
            .expect("completion_signaled event");
        assert_eq!(signaled.payload["step_index"], 0);
        assert_eq!(signaled.payload["signal"], "Task completed");
        assert!(
            !evs.iter().any(|e| {
                e.payload
                    .get("step_name")
                    .and_then(|v| v.as_str())
                    == Some("must-not-run")
            }),
            "completion signal should not start the following step"
        );

        let reloaded = Session::load(&pool, session_id).await.expect("reload");
        assert_eq!(reloaded.status(), SessionStatus::Completed);
    });
}

#[tokio::test]
async fn timeout_wins_when_agent_stdout_never_matches_completion_signal() {
    db_test!(pool, {
        let configured_ms = 100_u64;
        let session = Session::new(&pool, Uuid::new_v4(), "completion-timeout".into())
            .await
            .expect("session");
        let session_id = session.id();

        let mut workflow = Workflow::new(
            "completion-timeout",
            vec![hanging_agent_step(
                "slow-agent",
                "hang",
                Duration::from_millis(configured_ms),
            )],
        );
        workflow.completion_signal = Some("TASK_COMPLETE".into());

        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            workflow,
            lifecycle(),
            unreachable_executor(),
        );

        match engine.step().await {
            Err(EngineError::StepFailed { step_index, reason }) => {
                assert_eq!(step_index, 0);
                match reason {
                    TerminationReason::StepTimeout { configured_ms: got } => {
                        assert_eq!(got, configured_ms);
                    }
                    other => panic!("expected StepTimeout, got {other:?}"),
                }
            }
            other => panic!("expected timeout StepFailed, got {other:?}"),
        }

        let evs = events(&engine).await;
        let tags: Vec<&str> = evs.iter().filter_map(event_kind).collect();
        assert!(
            !tags.contains(&"completion_signaled"),
            "timeout path must not emit completion_signaled: {tags:?}"
        );
        assert!(tags.contains(&"step_timeout_fired"));
        assert!(tags.contains(&"step_failed"));

        let reloaded = Session::load(&pool, session_id).await.expect("reload");
        assert_eq!(reloaded.status(), SessionStatus::Failed);
    });
}
