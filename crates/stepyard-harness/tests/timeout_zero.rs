//! Round 3 Story 1 AC7 — `timeout: 0s` follows the normal timeout path.
//!
//! With a zero-duration timeout, `tokio::time::sleep(0)` resolves
//! immediately. The select in `execute_cmd_with_select` therefore takes
//! the timeout arm as soon as the cmd step is dispatched, producing the
//! same `StepTimeoutFired` + `StepFailed` + `destroy_by_session` +
//! `Err(StepFailed { StepTimeout { configured_ms: 0 } })` shape that a
//! positive timeout would. The acceptance criterion is that `0s` stays
//! on the timeout code path rather than being special-cased as "no
//! timeout" (which would silently drop a deliberate zero-budget step
//! into an unbounded wait).
//!
//! Skipped gracefully if `STEPYARD_HARNESS_DATABASE_URL` is unset.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sqlx::postgres::PgPoolOptions;
use stepyard_harness::{
    Engine, EngineError, HarnessConfig, Step, StepExecutor, TerminationReason, Workflow,
};
use stepyard_sandbox_orchestrator::{
    ExecOutput, MockCall, MockLifecycle, SandboxError, SandboxLifecycle,
};
use stepyard_session::{migrate, Session};
use uuid::Uuid;

async fn pool() -> Option<sqlx::PgPool> {
    let url = std::env::var("STEPYARD_HARNESS_DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("reach DB");
    migrate(&pool).await.expect("migrations ok");
    Some(pool)
}

/// Executor whose `execute` future never resolves. Keeps the select
/// arm for `exec_fut` parked so the only resolution is the timeout
/// arm's zero-duration sleep.
struct BlockingExecutor;

#[async_trait]
impl StepExecutor for BlockingExecutor {
    async fn execute(&self, _: Uuid, _: &Step) -> Result<ExecOutput, SandboxError> {
        std::future::pending::<()>().await;
        unreachable!("pending never resolves")
    }

    async fn execute_with_env(
        &self,
        session_id: Uuid,
        step: &Step,
        _env: &HashMap<String, String>,
    ) -> Result<ExecOutput, SandboxError> {
        self.execute(session_id, step).await
    }
}

#[tokio::test(flavor = "current_thread")]
async fn zero_second_timeout_fires_step_timeout_fired_then_step_failed() {
    let Some(pool) = pool().await else {
        eprintln!("[skip] STEPYARD_HARNESS_DATABASE_URL not set");
        return;
    };

    let tenant = format!("timeout-zero-{}", Uuid::new_v4());
    let workflow = Workflow::new(
        "timeout-zero-wf".to_string(),
        vec![Step::cmd("insta".to_string(), "sleep forever".to_string())
            .with_timeout(Duration::ZERO)],
    );

    let mock: Arc<MockLifecycle> = Arc::new(MockLifecycle::new());
    let session = Session::new(&pool, Uuid::new_v4(), tenant)
        .await
        .expect("new session");
    let session_uuid = *session.id().as_uuid();

    let lifecycle: Arc<dyn SandboxLifecycle> = mock.clone();
    let mut engine = Engine::with_executor(
        HarnessConfig::default(),
        session,
        workflow,
        lifecycle,
        Arc::new(BlockingExecutor),
    );

    // Typed error carries the termination taxonomy with configured_ms=0.
    match engine.step().await {
        Err(EngineError::StepFailed { step_index, reason }) => {
            assert_eq!(step_index, 0);
            match reason {
                TerminationReason::StepTimeout { configured_ms } => {
                    assert_eq!(
                        configured_ms, 0,
                        "zero-second timeout should surface as 0ms"
                    );
                }
                other => panic!("expected StepTimeout, got {other:?}"),
            }
        }
        other => panic!("expected Err(StepFailed), got {other:?}"),
    }

    // Event order: StepTimeoutFired { configured_ms: 0 } before StepFailed,
    // same as the positive-budget path.
    let events = engine.session().replay().await.expect("replay");
    let tags: Vec<&str> = events
        .iter()
        .filter_map(|e| e.payload.get("event").and_then(|v| v.as_str()))
        .collect();
    assert_eq!(
        tags,
        vec![
            "workflow_started",
            "step_started",
            "step_timeout_fired",
            "step_failed",
        ],
        "zero-second timeout should hit the normal timeout path, got {tags:?}"
    );

    let fired = events
        .iter()
        .find(|e| e.payload.get("event").and_then(|v| v.as_str()) == Some("step_timeout_fired"))
        .expect("step_timeout_fired present");
    assert_eq!(fired.payload["step_index"], 0);
    assert_eq!(fired.payload["configured_ms"], 0);

    // Sandbox teardown happens exactly once — same invariant a positive
    // timeout upholds (architecture.md §D5 emit-before-IO).
    let calls = mock.calls().await;
    let destroys: Vec<&MockCall> = calls
        .iter()
        .filter(|c| matches!(c, MockCall::Destroy { .. }))
        .collect();
    assert_eq!(destroys.len(), 1, "expected one destroy, got {calls:?}");
    let MockCall::Destroy { id } = destroys[0] else {
        unreachable!("filtered for Destroy above")
    };
    assert_eq!(*id.as_uuid(), session_uuid);
}
