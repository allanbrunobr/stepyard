//! Step/resume/cancel contract tests for [`Engine`].
//!
//! Requires a PostgreSQL reachable via `MINION_HARNESS_DATABASE_URL`. Tests
//! skip gracefully (with a note) if the env var is not set — CI without a
//! database sidecar stays green.

use std::sync::Arc;

use minion_harness::{Engine, HarnessConfig, Step, StepOutcome, Workflow};
use minion_sandbox_orchestrator::{MockLifecycle, SandboxLifecycle};
use minion_session::{migrate, Session, SessionStatus};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

async fn pool() -> Option<sqlx::PgPool> {
    let url = std::env::var("MINION_HARNESS_DATABASE_URL").ok()?;
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
            eprintln!("[skip] MINION_HARNESS_DATABASE_URL not set");
            return;
        };
        $body
    }};
}

fn three_step_workflow() -> Workflow {
    Workflow::new(
        "three-steppers",
        vec![
            Step::cmd("s1", "echo step1"),
            Step::cmd("s2", "echo step2"),
            Step::cmd("s3", "echo step3"),
        ],
    )
}

#[tokio::test]
async fn three_calls_to_step_execute_one_step_each_then_workflow_completed() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("new session");
        let lifecycle: Arc<dyn SandboxLifecycle> = Arc::new(MockLifecycle::new());
        let mut engine = Engine::new(
            HarnessConfig::default(),
            session,
            three_step_workflow(),
            lifecycle,
        );

        let a = engine.step().await.expect("step 1");
        let b = engine.step().await.expect("step 2");
        let c = engine.step().await.expect("step 3");
        let d = engine.step().await.expect("step 4 = WorkflowCompleted");

        assert!(matches!(a, StepOutcome::StepCompleted { ref step_name } if step_name == "s1"));
        assert!(matches!(b, StepOutcome::StepCompleted { ref step_name } if step_name == "s2"));
        assert!(matches!(c, StepOutcome::StepCompleted { ref step_name } if step_name == "s3"));
        assert_eq!(d, StepOutcome::WorkflowCompleted);

        // Log shape: WorkflowStarted + 3x(StepStarted + StepCompleted) + WorkflowCompleted = 8.
        let events = engine.session().replay().await.expect("replay");
        let names: Vec<&str> = events
            .iter()
            .filter_map(|e| e.payload.get("event").and_then(|v| v.as_str()))
            .collect();
        assert_eq!(
            names,
            vec![
                "workflow_started",
                "step_started",
                "step_completed",
                "step_started",
                "step_completed",
                "step_started",
                "step_completed",
                "workflow_completed"
            ]
        );

        // Session must be terminal with `completed`.
        let reloaded = Session::load(&pool, engine.session().id()).await.unwrap();
        assert_eq!(reloaded.status(), SessionStatus::Completed);
    });
}

#[tokio::test]
async fn resume_after_process_crash_continues_from_last_completed_step() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("new session");
        let session_id = session.id();

        // Process 1 — runs the first two steps then dies.
        {
            let lifecycle: Arc<dyn SandboxLifecycle> = Arc::new(MockLifecycle::new());
            let mut e1 = Engine::new(
                HarnessConfig::default(),
                session,
                three_step_workflow(),
                lifecycle,
            );
            let _ = e1.step().await.expect("s1");
            let _ = e1.step().await.expect("s2");
            // Simulate crash: we just drop e1 without marking anything.
        }

        // Process 2 — spins up a fresh engine with the same session id.
        {
            let reloaded = Session::load(&pool, session_id).await.expect("load");
            assert_eq!(reloaded.status(), SessionStatus::Running);

            let lifecycle: Arc<dyn SandboxLifecycle> = Arc::new(MockLifecycle::new());
            let mut e2 = Engine::new(
                HarnessConfig::default(),
                reloaded,
                three_step_workflow(),
                lifecycle,
            );

            // `resume` drives the loop until completion. Only ONE step
            // should run — s3 — because the log already has s1 + s2
            // completed events.
            let final_outcome = e2.resume().await.expect("resume");
            assert_eq!(final_outcome, StepOutcome::WorkflowCompleted);

            let events = e2.session().replay().await.expect("replay");
            let started = events
                .iter()
                .filter(|e| {
                    e.payload.get("event").and_then(|v| v.as_str()) == Some("step_started")
                })
                .count();
            let completed = events
                .iter()
                .filter(|e| {
                    e.payload.get("event").and_then(|v| v.as_str()) == Some("step_completed")
                })
                .count();
            // 3 starts total across both processes; 3 completions.
            assert_eq!(started, 3);
            assert_eq!(completed, 3);
        }
    });
}

#[tokio::test]
async fn cancel_from_another_task_stops_workflow_and_marks_session_cancelled() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("new session");
        let lifecycle: Arc<dyn SandboxLifecycle> = Arc::new(MockLifecycle::new());
        let mut engine = Engine::new(
            HarnessConfig::default(),
            session,
            three_step_workflow(),
            lifecycle,
        );
        let cancel = engine.cancel_token();
        let sid = engine.session().id();

        // Cancel BEFORE the first step runs — simpler and deterministic.
        // The real cancel-mid-flight case is validated once a sleeping
        // StepExecutor is available (deferred to Story 2.5 stress test).
        cancel.cancel();

        let outcome = engine.step().await.expect("step while cancelled");
        assert_eq!(outcome, StepOutcome::Cancelled);

        let reloaded = Session::load(&pool, sid).await.unwrap();
        assert_eq!(reloaded.status(), SessionStatus::Cancelled);
    });
}

#[tokio::test]
async fn cancel_from_another_task_during_long_running_step() {
    // AC: "um step em execucao + cancel de outra task -> StepFailed ou
    // Cancelled + session.status = cancelled". We use a StepExecutor that
    // blocks briefly, and another tokio task flips the cancel token. By
    // the time step() returns, the cancel has been observed either
    // before/during/after exec — the engine must end with session status
    // Cancelled.
    db_test!(pool, {
        use async_trait::async_trait;
        use minion_harness::StepExecutor;
        use minion_sandbox_orchestrator::{ExecOutput, SandboxError};
        use std::time::Duration;

        struct SlowExec;
        #[async_trait]
        impl StepExecutor for SlowExec {
            async fn execute(
                &self,
                _sid: Uuid,
                _step: &minion_harness::Step,
            ) -> Result<ExecOutput, SandboxError> {
                tokio::time::sleep(Duration::from_millis(200)).await;
                Ok(ExecOutput {
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: 0,
                })
            }
        }

        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("new");
        let lifecycle: Arc<dyn SandboxLifecycle> = Arc::new(MockLifecycle::new());
        let exec: Arc<dyn StepExecutor> = Arc::new(SlowExec);
        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            three_step_workflow(),
            lifecycle,
            exec,
        );
        let cancel = engine.cancel_token();
        let sid = engine.session().id();

        // Another task cancels after 50ms — the first step's exec is
        // still running.
        tokio::spawn({
            let cancel = cancel.clone();
            async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                cancel.cancel();
            }
        });

        // Drive the engine. `resume` loops through step() until the
        // workflow terminates. Cancel happens mid-loop.
        let final_outcome = engine.resume().await.expect("resume");
        assert!(
            matches!(final_outcome, StepOutcome::Cancelled),
            "expected Cancelled, got {final_outcome:?}"
        );

        let reloaded = Session::load(&pool, sid).await.unwrap();
        assert_eq!(reloaded.status(), SessionStatus::Cancelled);
    });
}
