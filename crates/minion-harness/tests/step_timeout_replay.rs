//! Regression: after a step times out, a reloaded engine must NOT advance
//! the workflow. `progress_from_log()` must recognise `step_failed` (emitted
//! alongside `step_timeout_fired`) as terminal — this is what closes the
//! "timed-out session advanced after reload" gap called out in the BMAD
//! review (architecture.md §D9 + NFR13).
//!
//! Skipped gracefully if `MINION_HARNESS_DATABASE_URL` is unset.

use std::sync::Arc;

use async_trait::async_trait;
use minion_harness::{
    Engine, EngineError, HarnessConfig, Step, StepExecutor, StepOutcome, Workflow,
};
use minion_sandbox_orchestrator::{ExecOutput, MockLifecycle, SandboxError, SandboxLifecycle};
use minion_session::{migrate, Session};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

async fn pool() -> Option<sqlx::PgPool> {
    let url = std::env::var("MINION_HARNESS_DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("reach DB");
    migrate(&pool).await.expect("migrations ok");
    Some(pool)
}

/// Executor whose `execute` future never resolves — forces the timeout
/// branch to be the only way out of `Engine::step`.
struct BlockingExecutor;

#[async_trait]
impl StepExecutor for BlockingExecutor {
    async fn execute(&self, _session_id: Uuid, _step: &Step) -> Result<ExecOutput, SandboxError> {
        std::future::pending::<()>().await;
        unreachable!("pending never resolves")
    }
}

#[tokio::test(flavor = "current_thread")]
async fn reloaded_timed_out_session_refuses_to_advance() {
    let Some(pool) = pool().await else {
        eprintln!("[skip] MINION_HARNESS_DATABASE_URL not set");
        return;
    };

    let tenant = format!("reload-timeout-{}", Uuid::new_v4());
    let workflow = Workflow::new(
        "reload-wf".to_string(),
        vec![Step::cmd("slow".to_string(), "sleep forever".to_string()).with_timeout(50)],
    );

    let mock: Arc<MockLifecycle> = Arc::new(MockLifecycle::new());
    let session = Session::new(&pool, Uuid::new_v4(), tenant)
        .await
        .expect("new session");
    let session_id = session.id();

    let lifecycle: Arc<dyn SandboxLifecycle> = mock.clone();
    let mut engine1 = Engine::with_executor(
        HarnessConfig::default(),
        session,
        workflow.clone(),
        lifecycle.clone(),
        Arc::new(BlockingExecutor),
    );

    // First run: timeout fires, session flipped to failed, log has both
    // step_timeout_fired and step_failed entries.
    let first = engine1.step().await;
    assert!(
        matches!(first, Err(EngineError::StepFailed { .. })),
        "first step must return StepFailed, got {first:?}"
    );

    let events_before = engine1
        .session()
        .replay()
        .await
        .expect("replay engine1")
        .len();
    drop(engine1);

    // Reload the session and build a fresh engine — same workflow, same
    // lifecycle. This models a process restart after the timeout commit.
    let reloaded = Session::load(&pool, session_id)
        .await
        .expect("reload session");
    let mut engine2 = Engine::with_executor(
        HarnessConfig::default(),
        reloaded,
        workflow,
        lifecycle,
        Arc::new(BlockingExecutor),
    );

    // The reloaded engine's step() must hit the progress.has_failure
    // branch and return StepOutcome::StepFailed without executing anything
    // new.
    let outcome = engine2.step().await.expect("reloaded step returns");
    match outcome {
        StepOutcome::StepFailed { error, .. } => {
            assert!(
                error.contains("previously failed"),
                "expected 'workflow previously failed' error, got {error:?}"
            );
        }
        other => panic!("reloaded step must be StepFailed, got {other:?}"),
    }

    // No new events were appended — log is idempotent across reloads of
    // a terminal session.
    let events_after = engine2
        .session()
        .replay()
        .await
        .expect("replay engine2")
        .len();
    assert_eq!(
        events_before, events_after,
        "reloaded step must not emit new events (before={events_before}, after={events_after})"
    );
}
